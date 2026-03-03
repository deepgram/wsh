# macOS Cross-Compilation — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Cross-compile macOS binaries (x86_64 + aarch64) from Linux via `nix build`.

**Architecture:** Use cargo-zigbuild with a pinned macOS SDK inside a custom `stdenv.mkDerivation`. Vendor cargo dependencies via `fetchCargoVendor`. Separate Rust toolchain with darwin targets from rust-overlay.

**Tech Stack:** Nix flakes, cargo-zigbuild, zig 0.15, MacOSX14.5 SDK, rust-overlay, fetchCargoVendor

---

### Task 1: Pin macOS SDK as fetchurl

**Files:**
- Modify: `flake.nix:21-67` (inside `packages = let ... in`)

**Step 1: Add macOsSdk derivation**

Inside the `packages = let ... in { ... }` block, after the `webFrontend` derivation (after line 44), add:

```nix
          macOsSdk = pkgs.fetchurl {
            url = "https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz";
            hash = "sha256-0000000000000000000000000000000000000000000=";
          };
```

**Step 2: Build to get the correct hash**

Run: `nix build .#wsh-x86_64-apple-darwin 2>&1 | tail -20`

This will fail because `wsh-x86_64-apple-darwin` doesn't exist yet, but we can evaluate the `macOsSdk` directly:

Run: `nix build --expr '(builtins.getFlake (toString ./.)).packages.x86_64-linux.wsh-x86_64-apple-darwin.macOsSdk' 2>&1`

Actually, we can't access it that way. Instead, temporarily add a package output to test:

```nix
          sdk-test = macOsSdk;
```

Run: `nix build .#sdk-test 2>&1`

This will fail with a hash mismatch. Copy the `got:` hash into the `fetchurl` and rebuild.

Run: `nix build .#sdk-test 2>&1`
Expected: Succeeds. Then remove the `sdk-test` output.

**Step 3: Commit**

```bash
git add flake.nix
git commit -m "build: pin macOS 14.5 SDK for cross-compilation

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2: Add fetchCargoVendor and rustToolchainDarwin

**Files:**
- Modify: `flake.nix`

**Step 1: Add cargoVendor derivation**

Inside the `packages = let` block, after `macOsSdk`, add:

```nix
          cargoVendor = pkgs.rustPlatform.fetchCargoVendor {
            src = ./.;
            hash = "sha256-0000000000000000000000000000000000000000000=";
          };
```

**Step 2: Add rustToolchainDarwin**

Inside the outer `let` block (after line 18, alongside `rustToolchain`), add:

```nix
        rustToolchainDarwin = pkgs.rust-bin.stable.latest.default.override {
          targets = [
            "x86_64-apple-darwin"
            "aarch64-apple-darwin"
          ];
        };
```

**Step 3: Get the correct cargoVendor hash**

Temporarily add a test output:

```nix
          vendor-test = cargoVendor;
```

Run: `nix build .#vendor-test 2>&1`

This will fail with a hash mismatch. Copy the `got:` hash into `fetchCargoVendor` and rebuild.

Run: `nix build .#vendor-test 2>&1`
Expected: Succeeds. Then remove the `vendor-test` output.

**Step 4: Commit**

```bash
git add flake.nix
git commit -m "build: add cargo vendor and darwin rust toolchain for cross-compilation

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 3: Add mkWshDarwin builder and package outputs

**Files:**
- Modify: `flake.nix`

**Step 1: Add mkWshDarwin function**

Inside the `packages = let` block, after `cargoVendor`, add:

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
              cat > .cargo/config.toml <<CARGOEOF
              [source.crates-io]
              replace-with = "vendored-sources"

              [source.vendored-sources]
              directory = "${cargoVendor}"
              CARGOEOF

              # Untar macOS SDK
              mkdir -p $TMPDIR/sdk
              tar xf ${macOsSdk} -C $TMPDIR/sdk
              export SDKROOT=$TMPDIR/sdk/MacOSX14.5.sdk
              export MACOSX_DEPLOYMENT_TARGET=11.0

              # Copy pre-built web frontend
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

**Step 2: Add the two darwin package outputs**

In the `in { ... }` block, add after the musl targets:

```nix
          wsh-x86_64-apple-darwin = mkWshDarwin "x86_64-apple-darwin";
          wsh-aarch64-apple-darwin = mkWshDarwin "aarch64-apple-darwin";
```

**Step 3: Build the x86_64 darwin binary**

Run: `nix build .#wsh-x86_64-apple-darwin 2>&1 | tail -20`

This is the first real cross-compilation attempt. It may take 5-10 minutes.

Expected: Builds successfully

Run: `file result/bin/wsh`
Expected: `Mach-O 64-bit executable x86_64`

If the build FAILS, read the error carefully:
- If it's a missing header (e.g., `util.h`, `pty.h`): the SDK path may need adjustment
- If it's a linker error (e.g., `iconv`, `CoreFoundation`): SDKROOT may not be picked up correctly
- If it's a ring/C compilation error: zig may need TARGET_CC hints
- Report the FULL error output

**Step 4: Build the aarch64 darwin binary**

Run: `nix build .#wsh-aarch64-apple-darwin 2>&1 | tail -20`
Expected: Builds successfully

Run: `file result/bin/wsh`
Expected: `Mach-O 64-bit executable arm64`

**Step 5: Commit**

```bash
git add flake.nix
git commit -m "feat: add macOS cross-compilation targets (x86_64 + aarch64)

nix build .#wsh-x86_64-apple-darwin   — Intel Mac binary
nix build .#wsh-aarch64-apple-darwin  — Apple Silicon binary

Uses cargo-zigbuild with pinned MacOSX14.5 SDK.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 4: Add zig and cargo-zigbuild to dev shell

**Files:**
- Modify: `flake.nix` (devShells section, ~line 69)

**Step 1: Add to buildInputs**

In the `devShells.default` `buildInputs` list, add `zig` and `cargo-zigbuild`:

```nix
          buildInputs = with pkgs; [
            rustToolchain
            pkg-config
            curl
            jq
            websocat
            bun
            qemu
            zig
            cargo-zigbuild
          ] ++ [
            llm-agents.packages.${system}.agent-browser
          ];
```

**Step 2: Verify tools are available**

Run: `nix develop -c sh -c "zig version"`
Expected: `0.15.2` (or similar)

Run: `nix develop -c sh -c "cargo zigbuild --version"`
Expected: `cargo-zigbuild 0.20.1` (or similar)

**Step 3: Commit**

```bash
git add flake.nix
git commit -m "build: add zig and cargo-zigbuild to dev shell

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 5: Update docs/building.md

**Files:**
- Modify: `docs/building.md`

**Step 1: Add macOS section**

After the "Static Linux Binaries" section, add a "macOS Binaries" section:

```markdown
### macOS Binaries

These are cross-compiled from Linux using cargo-zigbuild with a pinned macOS
SDK. The binaries link dynamically against `libSystem.dylib` (present on every
Mac).

**x86_64 (Intel Mac):**

\`\`\`bash
nix build .#wsh-x86_64-apple-darwin
\`\`\`

**aarch64 (Apple Silicon):**

\`\`\`bash
nix build .#wsh-aarch64-apple-darwin
\`\`\`
```

**Step 2: Update the Build Targets Summary table**

Add the two macOS rows:

```markdown
| x86_64 macOS | `nix build .#wsh-x86_64-apple-darwin` | Dynamic, x86_64, macOS 11+ |
| aarch64 macOS | `nix build .#wsh-aarch64-apple-darwin` | Dynamic, aarch64, macOS 11+ |
```

**Step 3: Update the Notes section**

Add a note about macOS cross-compilation:

```markdown
- macOS binaries are cross-compiled from Linux using
  [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) with a pinned
  MacOSX 14.5 SDK. They link dynamically against `libSystem.dylib` (always
  present on macOS). The minimum deployment target is macOS 11.0 (Big Sur).
```

**Step 4: Commit**

```bash
git add docs/building.md
git commit -m "docs: add macOS cross-compilation to building guide

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 6: Troubleshooting — cross-compilation failures (conditional)

This task only applies if Task 3 builds fail.

**Common issues and fixes:**

**Issue: Missing `util.h` or `pty.h`**

The macOS SDK may not be found. Verify SDKROOT points to the right directory:

```bash
# Check SDK structure
tar tf <macOsSdk-path> | head -20
# Adjust the path in SDKROOT if the tarball extracts to a different directory name
```

**Issue: `iconv` or `charset` linker error**

Known issue with zig 0.14.0. Verify zig version is 0.15+:
```bash
nix develop -c sh -c "zig version"
```

If stuck on an older zig, try adding to the build phase:
```bash
export LIBRARY_PATH=$SDKROOT/usr/lib
```

**Issue: ring C/assembly compilation failure**

Try adding explicit CC targeting:
```bash
export CC_x86_64_apple_darwin="zig cc -target x86_64-macos"
export CC_aarch64_apple_darwin="zig cc -target aarch64-macos"
```

**Issue: cargo-zigbuild can't find SDKROOT**

Ensure the SDK is extracted before cargo zigbuild runs. The tarball may extract
to a directory with a different name than expected. Use `ls $TMPDIR/sdk/` to
check.
