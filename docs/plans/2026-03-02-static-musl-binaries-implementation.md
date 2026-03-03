# Static musl Binaries — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Produce fully static, musl-linked single-file binaries for Linux x86_64 and aarch64 via `nix build`.

**Architecture:** Eliminate the cmake-dependent `aws-lc-sys` crate by switching rustls to ring-only, then add Nix cross-compilation outputs using `pkgsCross.*.pkgsStatic`.

**Tech Stack:** Nix flakes, pkgsCross, musl, rustPlatform.buildRustPackage, ring (crypto), qemu-user-static (testing)

---

### Task 1: Switch tokio-rustls from aws-lc-rs to ring

**Files:**
- Modify: `Cargo.toml:41`
- Modify: `src/tls.rs:59`

**Step 1: Update Cargo.toml dependency**

Change line 41 from:
```toml
tokio-rustls = "0.26"
```
to:
```toml
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "logging", "tls12"] }
```

**Step 2: Update CryptoProvider in src/tls.rs**

Change line 59 from:
```rust
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
```
to:
```rust
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
```

**Step 3: Regenerate Cargo.lock**

Run: `nix develop -c sh -c "cargo generate-lockfile"`

**Step 4: Verify aws-lc-sys is gone from the dependency tree**

Run: `nix develop -c sh -c "cargo tree -i aws-lc-sys 2>/dev/null"`
Expected: No output (crate not found in tree)

Run: `nix develop -c sh -c "cargo tree -i ring 2>/dev/null" | head -5`
Expected: `ring` appears, pulled in by `rustls`

**Step 5: Build and run tests**

Run: `nix develop -c sh -c "cargo build 2>&1" | tail -5`
Expected: Compiles successfully, no aws-lc-sys in build output

Run: `nix develop -c sh -c "cargo test 2>&1" | tail -20`
Expected: All tests pass, including TLS tests in `src/tls.rs` and `tests/tls_integration.rs`

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/tls.rs
git commit -m "build: switch rustls crypto provider from aws-lc-rs to ring

Drops aws-lc-sys (cmake + large C/C++ build) from the dependency tree.
ring compiles with just cc, making cross-compilation straightforward."
```

---

### Task 2: Refactor flake.nix — extract mkWsh builder and webFrontend

**Files:**
- Modify: `flake.nix:21-61`

**Step 1: Refactor flake.nix**

Replace the current `packages.default` block (lines 21–61) with a structure that hoists `webFrontend` into the outer `let` and extracts a reusable `mkWsh` function. The `packages.default` output should produce the same derivation as before.

```nix
      {
        packages = let
          webFrontend = pkgs.stdenvNoCC.mkDerivation {
            pname = "wsh-web";
            version = "0.1.0";
            src = ./.;
            nativeBuildInputs = [ pkgs.bun ];

            # FOD: allows network access; hash must be updated when web/ changes
            outputHashAlgo = "sha256";
            outputHashMode = "recursive";
            outputHash = "sha256-4AgZw+WAxDoTqJatqvEjzBMm4uRSz9bLwZL8gStblo4=";

            buildPhase = ''
              export HOME=$TMPDIR
              cd web
              bun install --frozen-lockfile
              bun run --bun node_modules/.bin/tsc
              bun run --bun node_modules/.bin/vite build
            '';

            installPhase = ''
              cp -r ../web-dist $out
            '';
          };

          mkWsh = crossPkgs: crossPkgs.rustPlatform.buildRustPackage {
            pname = "wsh";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ crossPkgs.pkg-config ];

            preBuild = ''
              cp -r ${webFrontend} web-dist
            '';

            WSH_SKIP_WEB_BUILD = "1";

            # Tests that spawn a PTY need a real shell, which isn't
            # available in the Nix build sandbox.
            doCheck = false;
          };
        in {
          default = mkWsh pkgs;
        };
```

Note: this intentionally keeps only `packages.default` for now. The cross targets are added in Task 3.

**Step 2: Verify the default package still builds**

Run: `nix build .#default 2>&1 | tail -10`
Expected: Builds successfully, produces `result/bin/wsh`

Run: `file result/bin/wsh`
Expected: `ELF 64-bit LSB` executable (dynamically linked, glibc — same as before)

**Step 3: Commit**

```bash
git add flake.nix
git commit -m "refactor: extract mkWsh builder and hoist webFrontend in flake.nix

Prepares for adding cross-compilation targets by making the Rust
package definition reusable across different rustPlatform instances."
```

---

### Task 3: Add static musl cross-compilation targets

**Files:**
- Modify: `flake.nix` (the `packages` let block from Task 2)

**Step 1: Add the two musl package outputs**

Inside the `packages` `let ... in { ... }` block, add the two cross targets after `default`:

```nix
        in {
          default = mkWsh pkgs;
          wsh-x86_64-linux-musl = mkWsh pkgs.pkgsCross.musl64.pkgsStatic;
          wsh-aarch64-linux-musl = mkWsh pkgs.pkgsCross.aarch64-multiplatform-musl.pkgsStatic;
        };
```

**Step 2: Build the x86_64 musl binary**

Run: `nix build .#wsh-x86_64-linux-musl 2>&1 | tail -10`
Expected: Builds successfully

Run: `file result/bin/wsh`
Expected: `ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked`

Run: `ldd result/bin/wsh 2>&1`
Expected: `not a dynamic executable`

Run: `result/bin/wsh --version`
Expected: Prints version string

**Step 3: Build the aarch64 musl binary**

Run: `nix build .#wsh-aarch64-linux-musl 2>&1 | tail -10`
Expected: Builds successfully (cross-compilation, will take longer on first run)

Run: `file result/bin/wsh`
Expected: `ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV), statically linked`

Run: `ldd result/bin/wsh 2>&1`
Expected: `not a dynamic executable`

**Step 4: Commit**

```bash
git add flake.nix
git commit -m "feat: add static musl binary targets for x86_64 and aarch64

nix build .#wsh-x86_64-linux-musl   — static x86_64 binary
nix build .#wsh-aarch64-linux-musl  — static aarch64 binary (cross-compiled)"
```

---

### Task 4: Add qemu-user-static to dev shell and verify aarch64 binary

**Files:**
- Modify: `flake.nix` (devShells section, ~line 64)

**Step 1: Add qemu to dev shell buildInputs**

In the `devShells.default` `buildInputs` list, add `qemu` after the existing entries:

```nix
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            curl
            jq
            websocat
            bun
            qemu
          ] ++ [
            llm-agents.packages.${system}.agent-browser
          ];
```

Note: `pkgs.qemu` provides all qemu-user binaries including `qemu-aarch64`. The `-static` suffix variant is only needed when the host doesn't have matching shared libraries; Nix's qemu package works for our use case.

**Step 2: Verify qemu is available**

Run: `nix develop -c sh -c "qemu-aarch64 --version" 2>&1 | head -1`
Expected: `qemu-aarch64 version ...`

**Step 3: Test the aarch64 binary under qemu**

First build the aarch64 binary if not already built:
Run: `nix build .#wsh-aarch64-linux-musl`

Then test:
Run: `nix develop -c sh -c "qemu-aarch64 result/bin/wsh --version"`
Expected: Prints the wsh version string

**Step 4: Record binary sizes**

Run: `nix build .#wsh-x86_64-linux-musl && ls -lh result/bin/wsh`
Run: `nix build .#wsh-aarch64-linux-musl && ls -lh result/bin/wsh`

Note the sizes for future reference (expect 15–30 MB each).

**Step 5: Commit**

```bash
git add flake.nix
git commit -m "build: add qemu to dev shell for aarch64 binary testing"
```

---

### Task 5: Troubleshooting — ring cross-compilation (conditional)

This task only applies if Task 3's aarch64 build fails due to ring's C/assembly
compilation not finding the cross-compiler.

**Files:**
- Modify: `flake.nix` (mkWsh function)

**Step 1: Diagnose the error**

If `nix build .#wsh-aarch64-linux-musl` fails with errors like:
- `cc: error: unrecognized command-line option` (wrong compiler arch)
- `ring build failed` or `error running cc`
- Assembly errors about wrong target

Then ring needs explicit cross-compiler configuration.

**Step 2: Add TARGET_CC override to mkWsh**

Modify `mkWsh` to accept and forward the target CC:

```nix
          mkWsh = crossPkgs: crossPkgs.rustPlatform.buildRustPackage {
            pname = "wsh";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ crossPkgs.pkg-config ];

            preBuild = ''
              cp -r ${webFrontend} web-dist
            '';

            WSH_SKIP_WEB_BUILD = "1";

            # Ensure ring's build.rs finds the correct cross-compiler
            TARGET_CC = "${crossPkgs.stdenv.cc}/bin/${crossPkgs.stdenv.cc.targetPrefix}cc";

            doCheck = false;
          };
```

**Step 3: Rebuild and verify**

Run: `nix build .#wsh-aarch64-linux-musl 2>&1 | tail -10`
Expected: Builds successfully

Run: `file result/bin/wsh`
Expected: `ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV), statically linked`

**Step 4: Commit (if changes were needed)**

```bash
git add flake.nix
git commit -m "fix: set TARGET_CC for ring cross-compilation"
```
