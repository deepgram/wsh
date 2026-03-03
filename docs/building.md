# Building from Source

wsh uses [Nix](https://nixos.org/) for reproducible builds. All build commands
run on x86_64 Linux, including cross-compilation for macOS.

## Prerequisites

Install Nix with flakes enabled:

```bash
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh
```

## Development Build

Enter the dev shell and use cargo directly:

```bash
nix develop
cargo build
cargo test
```

This produces a dynamically-linked binary for your host system.

## Release Builds

### Default (dynamic, host platform)

```bash
nix build
./result/bin/wsh --version
```

### Static Linux Binaries

These produce fully static, musl-linked executables with zero runtime
dependencies. Drop the binary into `/usr/local/bin` on any Linux machine and it
just works.

**x86_64:**

```bash
nix build .#wsh-x86_64-linux-musl
```

**aarch64** (cross-compiled from x86_64, works on Raspberry Pi 4/5):

```bash
nix build .#wsh-aarch64-linux-musl
```

Both binaries are ~20MB.

### macOS Binaries

These are cross-compiled from Linux using cargo-zigbuild with a pinned macOS
SDK. The binaries link dynamically against `libSystem.dylib` (present on every
Mac).

**x86_64 (Intel Mac):**

```bash
nix build .#wsh-x86_64-apple-darwin
```

**aarch64 (Apple Silicon):**

```bash
nix build .#wsh-aarch64-apple-darwin
```

## Verifying Binaries

Check that a Linux binary is statically linked:

```bash
file result/bin/wsh
# ELF 64-bit LSB executable, ..., statically linked, ...

ldd result/bin/wsh
# not a dynamic executable
```

Test the aarch64 binary on an x86_64 machine using qemu (available in the dev
shell):

```bash
nix develop -c qemu-aarch64 result/bin/wsh --version
```

Check that macOS binaries have the correct architecture:

```bash
file result/bin/wsh
# Mach-O 64-bit executable arm64    (aarch64)
# Mach-O 64-bit executable x86_64   (Intel)
```

macOS binaries cannot be executed on Linux. Full functional testing requires
macOS hardware.

## Installing

Copy the binary to your PATH:

```bash
# Local machine (x86_64)
nix build .#wsh-x86_64-linux-musl
sudo cp result/bin/wsh /usr/local/bin/wsh

# Remote machine (e.g., Raspberry Pi)
nix build .#wsh-aarch64-linux-musl
scp result/bin/wsh pi@raspberrypi:/usr/local/bin/wsh
ssh pi@raspberrypi chmod +x /usr/local/bin/wsh
```

## Build Targets Summary

| Target | Command | Output |
|--------|---------|--------|
| Dev build | `cargo build` | Dynamic, glibc, host arch |
| Default release | `nix build` | Dynamic, glibc, host arch |
| x86_64 static | `nix build .#wsh-x86_64-linux-musl` | Static, musl, x86_64 |
| aarch64 static | `nix build .#wsh-aarch64-linux-musl` | Static, musl, aarch64 |
| x86_64 macOS | `nix build .#wsh-x86_64-apple-darwin` | Dynamic, x86_64, macOS 11+ |
| aarch64 macOS | `nix build .#wsh-aarch64-apple-darwin` | Dynamic, aarch64, macOS 11+ |

## Notes

- The web frontend is built automatically by Nix using Bun/Vite and embedded
  into the binary via `rust-embed`. No separate web build step is needed for
  `nix build`.
- For development builds with `cargo build`, the web frontend is built by the
  cargo build script. Set `WSH_SKIP_WEB_BUILD=1` to skip it if you don't need
  the web UI.
- Static builds use [ring](https://github.com/briansmith/ring) as the sole TLS
  crypto provider (no cmake dependency), which simplifies cross-compilation.
- First-time cross-compilation builds are slow (~10 minutes) because Nix
  downloads and builds the entire musl cross-toolchain. Subsequent builds use
  the Nix store cache.
- macOS binaries are cross-compiled from Linux using
  [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) with a pinned
  MacOSX 14.5 SDK. They link dynamically against `libSystem.dylib` (always
  present on macOS). The minimum deployment target is macOS 11.0 (Big Sur).
