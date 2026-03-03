# macOS Cross-Compilation from Linux

**Date**: 2026-03-02
**Status**: Proposed
**Scope**: macOS x86_64 + aarch64 binaries built on Linux via cargo-zigbuild

## Problem

wsh has static Linux binaries (x86_64 + aarch64 musl) but no macOS binaries.
Users on Intel and Apple Silicon Macs need to build from source. We want to
cross-compile macOS binaries from Linux so a single build machine produces
artifacts for all four targets.

## Targets

| Output name | Architecture | OS | Linking |
|---|---|---|---|
| `wsh-x86_64-apple-darwin` | x86_64 | macOS | Dynamic (libSystem.dylib) |
| `wsh-aarch64-apple-darwin` | aarch64 | macOS | Dynamic (libSystem.dylib) |

macOS does not support fully static binaries — Apple requires dynamic linking
to `libSystem.dylib`. This is fine because `libSystem` is present on every Mac.
We set `MACOSX_DEPLOYMENT_TARGET=11.0` (Big Sur) to cover most Macs in active
use.

## Approach: cargo-zigbuild + pinned macOS SDK

Zig ships with macOS system library stubs, and `cargo-zigbuild` wraps cargo to
use `zig cc` as the linker/C-compiler. For crates that need darwin headers
(like `portable-pty` needing `<util.h>` for `forkpty`), we set `SDKROOT` to a
pinned macOS SDK.

This avoids building the osxcross toolchain (~30 min) and integrates cleanly
with Nix.

Build commands:
```
nix build .#wsh-x86_64-apple-darwin
nix build .#wsh-aarch64-apple-darwin
```

## Component 1: macOS SDK

Pin MacOSX 14.5 SDK as a fixed-output derivation via `fetchurl`:

```nix
macOsSdk = pkgs.fetchurl {
  url = "https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz";
  hash = "sha256-...";
};
```

The SDK is ~50 MB compressed, downloaded once, and cached in the Nix store.
Source: [joseluisq/macosx-sdks](https://github.com/joseluisq/macosx-sdks).

## Component 2: Rust toolchain with darwin targets

Create a separate Rust toolchain that includes the darwin standard library
targets. This is only used for darwin builds, not the dev shell.

```nix
rustToolchainDarwin = pkgs.rust-bin.stable.latest.default.override {
  targets = [
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
  ];
};
```

## Component 3: Cargo dependency vendoring

`stdenv.mkDerivation` (unlike `rustPlatform.buildRustPackage`) does not handle
cargo dependency fetching. We pre-vendor all crates using
`rustPlatform.fetchCargoVendor`:

```nix
cargoVendor = pkgs.rustPlatform.fetchCargoVendor {
  src = ./.;
  hash = "sha256-...";
};
```

The build phase writes a `.cargo/config.toml` pointing at the vendored
directory. The hash needs updating when `Cargo.lock` changes — same workflow as
the `webFrontend` hash.

## Component 4: mkWshDarwin builder

A custom `stdenv.mkDerivation` that runs cargo-zigbuild:

```nix
mkWshDarwin = target: pkgs.stdenv.mkDerivation {
  pname = "wsh";
  version = "0.1.0";
  src = ./.;

  nativeBuildInputs = [
    rustToolchainDarwin
    pkgs.zig
    pkgs.cargo-zigbuild
  ];

  buildPhase = ''
    export HOME=$TMPDIR

    # Vendor cargo dependencies
    mkdir -p .cargo
    cat > .cargo/config.toml <<EOF
    [source.crates-io]
    replace-with = "vendored-sources"
    [source.vendored-sources]
    directory = "${cargoVendor}"
    EOF

    # Untar and set macOS SDK
    mkdir -p $TMPDIR/sdk
    tar xf ${macOsSdk} -C $TMPDIR/sdk
    export SDKROOT=$TMPDIR/sdk/MacOSX14.5.sdk
    export MACOSX_DEPLOYMENT_TARGET=11.0

    # Build web frontend (pre-built)
    cp -r ${webFrontend} web-dist
    export WSH_SKIP_WEB_BUILD=1

    cargo zigbuild --release --target ${target}
  '';

  installPhase = ''
    mkdir -p $out/bin
    cp target/${target}/release/wsh $out/bin/wsh
  '';
};
```

## Component 5: Flake outputs

New package outputs alongside existing ones:

| Output | Builder | Tool |
|---|---|---|
| `default` | `mkWsh pkgs` | rustPlatform (glibc) |
| `wsh-x86_64-linux-musl` | `mkWsh pkgs.pkgsCross...` | rustPlatform (musl) |
| `wsh-aarch64-linux-musl` | `mkWsh pkgs.pkgsCross...` | rustPlatform (musl) |
| `wsh-x86_64-apple-darwin` | `mkWshDarwin` | cargo-zigbuild (new) |
| `wsh-aarch64-apple-darwin` | `mkWshDarwin` | cargo-zigbuild (new) |

## Component 6: Dev shell additions

Add `zig` and `cargo-zigbuild` to `devShells.default.buildInputs` for local
iteration on darwin builds without going through full `nix build`.

## Verification

macOS binaries cannot be executed on Linux (no qemu-darwin equivalent).
Verification is limited to:

1. `file result/bin/wsh` — confirm `Mach-O 64-bit executable arm64` or
   `Mach-O 64-bit executable x86_64`
2. Binary size sanity check (expect 15-30 MB)
3. Full functional testing requires macOS hardware or CI with macOS runners

## Potential Issues

**portable-pty on darwin**: Uses `forkpty`/`openpty` from `<util.h>` on macOS
(vs `<pty.h>` on Linux). Conditional compilation handles this, and the macOS
SDK provides the header. Most likely spot for cross-compilation breakage.

**ring on darwin**: Compiles C + assembly for the target platform.
`cargo-zigbuild` handles this via `zig cc`. Well-tested in the ecosystem.

**Cargo.lock sync**: The `cargoVendor` hash needs updating when `Cargo.lock`
changes. Same workflow as the existing `webFrontend` hash.

## Success Criteria

```
$ nix build .#wsh-x86_64-apple-darwin && file result/bin/wsh
result/bin/wsh: Mach-O 64-bit executable x86_64

$ nix build .#wsh-aarch64-apple-darwin && file result/bin/wsh
result/bin/wsh: Mach-O 64-bit executable arm64
```

## Out of Scope

- Universal2 (fat) binaries (can add later with `universal2-apple-darwin`)
- Code signing / notarization (requires Apple Developer account)
- GitHub Actions CI
- GitHub Releases / install script
