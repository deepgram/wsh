# Static musl Binary Distribution for Linux

**Date**: 2026-03-02
**Status**: Proposed
**Scope**: Linux x86_64 + aarch64 static binaries via Nix

## Problem

wsh currently builds dynamically-linked glibc binaries. Users on different Linux
distros (or minimal environments like Raspberry Pi OS, Alpine, containers) may
have incompatible glibc versions. We want single-file, zero-dependency
executables that can be dropped into `/usr/local/bin` and "just work" on any
Linux machine.

## Targets

| Output name | Architecture | Libc | Linking |
|---|---|---|---|
| `wsh-x86_64-linux-musl` | x86_64 | musl | static |
| `wsh-aarch64-linux-musl` | aarch64 | musl | static |

macOS targets (Intel + Apple Silicon) are deferred to a follow-up sprint. macOS
does not support fully static binaries (Apple requires dynamic linking to
`libSystem.dylib`), so that work is a cross-compilation toolchain problem
(osxcross or zig), not a libc problem.

## Approach: Nix pkgsCross + pkgsStatic

Use Nix's `pkgsCross.musl64.pkgsStatic` and
`pkgsCross.aarch64-multiplatform-musl.pkgsStatic` to get complete musl-based
cross-compilation toolchains. Each provides a `rustPlatform.buildRustPackage`
pre-configured with musl, static linking flags, and the correct cross-C-compiler.

Build commands:
```
nix build .#wsh-x86_64-linux-musl
nix build .#wsh-aarch64-linux-musl
```

## Step 1: Eliminate aws-lc-sys

The `aws-lc-sys` crate compiles a large C/C++ codebase via cmake + cc. It exists
solely because `tokio-rustls`'s default features include `aws_lc_rs`. All actual
TLS consumers (reqwest, hyper-rustls) already use `ring` as their crypto
provider.

Change in `Cargo.toml`:
```toml
# Before:
tokio-rustls = "0.26"

# After:
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "logging", "tls12"] }
```

This drops `aws-lc-sys` and `aws-lc-rs` entirely. `ring` remains as the sole
crypto provider. `ring` compiles with just `cc` (no cmake), making
cross-compilation straightforward.

## Step 2: Refactor flake.nix

Extract a shared `mkWsh` builder function so the `webFrontend` derivation and
Rust package definition are reused across all targets.

```nix
mkWsh = { rustPlatform, ... }: rustPlatform.buildRustPackage {
  pname = "wsh";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  preBuild = ''cp -r ${webFrontend} web-dist'';
  WSH_SKIP_WEB_BUILD = "1";
  doCheck = false;
};
```

New flake outputs (all under `x86_64-linux`):

| Output | rustPlatform source |
|---|---|
| `packages.default` | `pkgs` (dynamic glibc, unchanged) |
| `packages.wsh-x86_64-linux-musl` | `pkgs.pkgsCross.musl64.pkgsStatic` |
| `packages.wsh-aarch64-linux-musl` | `pkgs.pkgsCross.aarch64-multiplatform-musl.pkgsStatic` |

The `webFrontend` derivation is unchanged — it's a `stdenvNoCC` bun/vite build
that runs on the build machine and produces static HTML/JS/CSS. It's shared
across all targets via `preBuild`.

`devShells.default` is unchanged except for adding `qemu-user-static` to
`buildInputs` for aarch64 binary testing.

## Step 3: ring cross-compilation

`ring` is the one remaining crate that compiles native C + assembly. Nix's
`pkgsCross` sets up the correct cross-compiler via `stdenv.cc`, and
`rustPlatform.buildRustPackage` propagates `CC`/`TARGET_CC` automatically.

If auto-detection fails (discovered on first build), the fallback is explicit
env vars in the derivation:
```nix
env = {
  TARGET_CC = "${crossStdenv.cc}/bin/${crossStdenv.cc.targetPrefix}cc";
};
```

## Step 4: Dev shell addition

Add `qemu-user-static` to `devShells.default.buildInputs` so that
`qemu-aarch64-static` is available for testing aarch64 binaries on x86_64 build
machines.

## Verification

After each `nix build`:

1. `file result/bin/wsh` — confirm `statically linked`, correct architecture
2. `ldd result/bin/wsh` — should print `not a dynamic executable`
3. `ls -lh result/bin/wsh` — note binary size (expect 15–30 MB)
4. x86_64-musl: `result/bin/wsh --version` directly
5. aarch64-musl: `qemu-aarch64-static result/bin/wsh --version`

## Success Criteria

```
$ nix build .#wsh-x86_64-linux-musl && file result/bin/wsh
ELF 64-bit LSB executable, x86-64, ..., statically linked, ...

$ nix build .#wsh-aarch64-linux-musl && file result/bin/wsh
ELF 64-bit LSB executable, ARM aarch64, ..., statically linked, ...

$ ldd result/bin/wsh
        not a dynamic executable
```

## Out of Scope

- macOS builds (follow-up sprint)
- GitHub Actions CI (later)
- GitHub Releases / install script (later)
- Code signing (later)
