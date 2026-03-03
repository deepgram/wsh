# Distribution & Packaging — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ship wsh via Homebrew, a curl|sh installer, shell completions, and contrib service files, backed by a GitHub Actions release workflow.

**Architecture:** Add a `wsh completions` subcommand via `clap_complete`, create contrib service files for systemd/launchd, write a POSIX install script, build a GitHub Actions workflow that cross-compiles all 4 targets via Nix and publishes to GitHub Releases, and set up a Homebrew tap in a separate repo.

**Tech Stack:** clap_complete, GitHub Actions, Nix, Homebrew, POSIX shell, systemd, launchd

---

### Task 1: Add `wsh completions` subcommand

**Files:**
- Modify: `Cargo.toml:20` (add `clap_complete` dependency)
- Modify: `src/main.rs` (add `Completions` variant to `Commands` enum and handler)

**Step 1: Add clap_complete dependency**

In `Cargo.toml`, after the `clap` line (line 20), add:

```toml
clap_complete = "4"
```

**Step 2: Add the Completions subcommand variant**

In `src/main.rs`, add this import alongside the existing clap imports (line 15):

```rust
use clap::{Parser as ClapParser, Subcommand, CommandFactory};
use clap_complete::{generate, Shell};
```

In the `Commands` enum (after `Mcp` variant, around line 238), add:

```rust
    /// Generate shell completions for bash, zsh, or fish
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
```

**Step 3: Add the handler in the match block**

In the `match cli.command` block (around line 351), add before `None =>`:

```rust
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "wsh", &mut std::io::stdout());
            Ok(())
        }
```

**Step 4: Verify it compiles and produces output**

Run: `nix develop -c sh -c "cargo build 2>&1" | tail -5`
Expected: Compiles successfully

Run: `nix develop -c sh -c "cargo run -- completions bash 2>/dev/null" | head -5`
Expected: Prints bash completion script starting with `_wsh`

Run: `nix develop -c sh -c "cargo run -- completions zsh 2>/dev/null" | head -5`
Expected: Prints zsh completion script starting with `#compdef wsh`

Run: `nix develop -c sh -c "cargo run -- completions fish 2>/dev/null" | head -5`
Expected: Prints fish completion script with `complete -c wsh`

**Step 5: Run existing tests to make sure nothing broke**

Run: `nix develop -c sh -c "cargo test 2>&1" | tail -20`
Expected: All tests pass

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "feat: add wsh completions subcommand for bash, zsh, fish

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 2: Create contrib service files

**Files:**
- Create: `contrib/linux/wsh.service`
- Create: `contrib/linux/server.env`
- Create: `contrib/macos/com.deepgram.wsh.plist`

**Step 1: Create the systemd user unit**

Create `contrib/linux/wsh.service`:

```ini
[Unit]
Description=wsh terminal API server
Documentation=https://github.com/deepgram/wsh
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/wsh server
Restart=on-failure
RestartSec=5
EnvironmentFile=-%h/.config/wsh/server.env

[Install]
WantedBy=default.target
```

**Step 2: Create the example environment file**

Create `contrib/linux/server.env`:

```bash
# wsh server environment — used by the systemd unit and launchd agent.
# Uncomment and edit to customize. The server runs with defaults
# (localhost:8080, no auth, no TLS) when this file is absent or empty.

# WSH_SERVER_NAME=default
# WSH_TOKEN=your-secret-token
# WSH_TLS_CERT=/path/to/cert.pem
# WSH_TLS_KEY=/path/to/key.pem
# WSH_HOSTNAME=my-host.example.com
# WSH_BASE_PREFIX=/wsh
```

**Step 3: Create the launchd plist**

Create `contrib/macos/com.deepgram.wsh.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.deepgram.wsh</string>

    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/wsh</string>
        <string>server</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/tmp/wsh.stdout.log</string>

    <key>StandardErrorPath</key>
    <string>/tmp/wsh.stderr.log</string>

    <!--
    Uncomment and edit to customize:

    <key>EnvironmentVariables</key>
    <dict>
        <key>WSH_SERVER_NAME</key>
        <string>default</string>

        <key>WSH_TOKEN</key>
        <string>your-secret-token</string>

        <key>WSH_TLS_CERT</key>
        <string>/path/to/cert.pem</string>

        <key>WSH_TLS_KEY</key>
        <string>/path/to/key.pem</string>

        <key>WSH_HOSTNAME</key>
        <string>my-host.example.com</string>

        <key>WSH_BASE_PREFIX</key>
        <string>/wsh</string>
    </dict>
    -->
</dict>
</plist>
```

**Step 4: Commit**

```bash
git add contrib/
git commit -m "feat: add contrib service files for systemd and launchd

contrib/linux/wsh.service          — systemd user unit
contrib/linux/server.env           — example environment file
contrib/macos/com.deepgram.wsh.plist — launchd user agent

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 3: Create the install script

**Files:**
- Create: `install.sh`

**Step 1: Write install.sh**

Create `install.sh` in the repo root:

```bash
#!/bin/sh
set -eu

# wsh installer — downloads the correct pre-built binary for your platform.
#
# Usage:
#   curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
#
# Environment variables:
#   WSH_VERSION      — version to install (default: latest)
#   WSH_INSTALL_DIR  — installation directory (default: /usr/local/bin)

REPO="deepgram/wsh"
INSTALL_DIR="${WSH_INSTALL_DIR:-/usr/local/bin}"

main() {
    need_cmd curl
    need_cmd uname

    local os arch target version url checksum_url

    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)   target="wsh-x86_64-linux-musl" ;;
                aarch64|arm64)  target="wsh-aarch64-linux-musl" ;;
                *) err "unsupported Linux architecture: $arch" ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64|amd64)   target="wsh-x86_64-apple-darwin" ;;
                aarch64|arm64)  target="wsh-aarch64-apple-darwin" ;;
                *) err "unsupported macOS architecture: $arch" ;;
            esac
            ;;
        *) err "unsupported OS: $os" ;;
    esac

    if [ -n "${WSH_VERSION:-}" ]; then
        version="$WSH_VERSION"
    else
        say "fetching latest version..."
        version="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"//;s/".*//')"
        if [ -z "$version" ]; then
            err "could not determine latest version (GitHub API rate limit?)"
        fi
    fi

    say "installing wsh $version ($target)"

    url="https://github.com/$REPO/releases/download/$version/$target"
    checksum_url="https://github.com/$REPO/releases/download/$version/checksums.txt"

    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT

    say "downloading $url"
    curl -fsSL -o "$tmp/wsh" "$url"

    say "verifying checksum..."
    curl -fsSL -o "$tmp/checksums.txt" "$checksum_url"
    verify_checksum "$tmp/wsh" "$tmp/checksums.txt" "$target"

    chmod +x "$tmp/wsh"

    if [ -w "$INSTALL_DIR" ]; then
        mv "$tmp/wsh" "$INSTALL_DIR/wsh"
    else
        say "installing to $INSTALL_DIR (may require sudo)"
        sudo mv "$tmp/wsh" "$INSTALL_DIR/wsh"
    fi

    say "installed wsh $version to $INSTALL_DIR/wsh"
    "$INSTALL_DIR/wsh" --version
}

verify_checksum() {
    local file="$1" checksums="$2" target="$3"
    local expected actual

    expected="$(grep "$target" "$checksums" | awk '{print $1}')"
    if [ -z "$expected" ]; then
        err "no checksum found for $target in checksums.txt"
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        say "warning: no sha256sum or shasum found, skipping checksum verification"
        return 0
    fi

    if [ "$actual" != "$expected" ]; then
        err "checksum mismatch: expected $expected, got $actual"
    fi
    say "checksum verified"
}

say() {
    printf 'wsh-installer: %s\n' "$*" >&2
}

err() {
    say "error: $*"
    exit 1
}

need_cmd() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "need '$1' (command not found)"
    fi
}

main "$@"
```

**Step 2: Make it executable**

Run: `chmod +x install.sh`

**Step 3: Test the script's platform detection locally**

Run: `sh -c 'os=$(uname -s); arch=$(uname -m); echo "$os $arch"'`
Expected: `Linux x86_64` (or similar for your machine)

Run: `sh -x install.sh 2>&1 | head -20`

This will likely fail (no release exists yet), but verify it correctly detects
the platform and constructs the right URL before failing on the download.

**Step 4: Commit**

```bash
git add install.sh
git commit -m "feat: add curl|sh install script

curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh

Detects OS/arch, downloads from GitHub Releases, verifies SHA256 checksum.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 4: Create GitHub Actions release workflow

**Files:**
- Create: `.github/workflows/release.yml`

**Step 1: Create the workflow file**

Create `.github/workflows/release.yml`:

```yaml
name: Release

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: write

jobs:
  build:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        target:
          - wsh-x86_64-linux-musl
          - wsh-aarch64-linux-musl
          - wsh-x86_64-apple-darwin
          - wsh-aarch64-apple-darwin
    steps:
      - uses: actions/checkout@v4

      - uses: DeterminateSystems/nix-installer-action@main

      - uses: DeterminateSystems/magic-nix-cache-action@main

      - name: Build ${{ matrix.target }}
        run: nix build .#${{ matrix.target }} --print-build-logs

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.target }}
          path: result/bin/wsh

  release:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Prepare release assets
        run: |
          mkdir -p release
          for target in wsh-x86_64-linux-musl wsh-aarch64-linux-musl wsh-x86_64-apple-darwin wsh-aarch64-apple-darwin; do
            cp "artifacts/$target/wsh" "release/$target"
            chmod +x "release/$target"
          done
          cp install.sh release/install.sh

          # Generate checksums
          cd release
          sha256sum wsh-* > checksums.txt
          cat checksums.txt

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          files: |
            release/wsh-*
            release/install.sh
            release/checksums.txt

  update-homebrew:
    needs: release
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Compute SHA256 hashes
        id: hashes
        run: |
          for target in wsh-x86_64-linux-musl wsh-aarch64-linux-musl wsh-x86_64-apple-darwin wsh-aarch64-apple-darwin; do
            hash=$(sha256sum "artifacts/$target/wsh" | awk '{print $1}')
            key=$(echo "$target" | tr '-' '_')
            echo "${key}=${hash}" >> "$GITHUB_OUTPUT"
          done

      - name: Update Homebrew formula
        uses: actions/checkout@v4
        with:
          repository: deepgram/homebrew-tap
          token: ${{ secrets.HOMEBREW_TAP_TOKEN }}
          path: homebrew-tap

      - name: Write formula
        env:
          VERSION: ${{ github.ref_name }}
        run: |
          cat > homebrew-tap/Formula/wsh.rb << 'FORMULA'
          class Wsh < Formula
            desc "The Web Shell — an API for your terminal"
            homepage "https://github.com/deepgram/wsh"
            version "${VERSION#v}"
            license "ISC"

            on_macos do
              if Hardware::CPU.arm?
                url "https://github.com/deepgram/wsh/releases/download/${VERSION}/wsh-aarch64-apple-darwin"
                sha256 "${{ steps.hashes.outputs.wsh_aarch64_apple_darwin }}"
              else
                url "https://github.com/deepgram/wsh/releases/download/${VERSION}/wsh-x86_64-apple-darwin"
                sha256 "${{ steps.hashes.outputs.wsh_x86_64_apple_darwin }}"
              end
            end

            on_linux do
              if Hardware::CPU.arm?
                url "https://github.com/deepgram/wsh/releases/download/${VERSION}/wsh-aarch64-linux-musl"
                sha256 "${{ steps.hashes.outputs.wsh_aarch64_linux_musl }}"
              else
                url "https://github.com/deepgram/wsh/releases/download/${VERSION}/wsh-x86_64-linux-musl"
                sha256 "${{ steps.hashes.outputs.wsh_x86_64_linux_musl }}"
              end
            end

            def install
              bin.install stable.url.split("/").last => "wsh"

              generate_completions_from_executable(bin/"wsh", "completions")
            end

            service do
              run [opt_bin/"wsh", "server"]
              keep_alive true
              log_path var/"log/wsh.log"
              error_log_path var/"log/wsh.log"
            end

            test do
              assert_match "wsh", shell_output("#{bin}/wsh --version")
            end
          end
          FORMULA

          # Expand version in the formula
          cd homebrew-tap
          sed -i "s|\${VERSION#v}|${VERSION#v}|g" Formula/wsh.rb
          sed -i "s|\${VERSION}|${VERSION}|g" Formula/wsh.rb

      - name: Push formula update
        working-directory: homebrew-tap
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add Formula/wsh.rb
          git commit -m "wsh ${GITHUB_REF_NAME}"
          git push
```

**Step 2: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add GitHub Actions release workflow

Triggers on v*.*.* tag push. Builds all 4 targets via Nix,
creates GitHub Release with binaries + checksums + install.sh,
and auto-updates the Homebrew formula in deepgram/homebrew-tap.

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 5: Create the Homebrew tap repo

This task happens outside the wsh repo. It creates the `deepgram/homebrew-tap`
repo with a placeholder formula that the release workflow will overwrite.

**Step 1: Create the repo via GitHub CLI**

Run: `gh repo create deepgram/homebrew-tap --public --description "Homebrew formulae for Deepgram tools" --clone=false`

If the repo already exists, skip this step.

**Step 2: Create a Personal Access Token (or use existing)**

The release workflow needs a `HOMEBREW_TAP_TOKEN` secret with `repo` scope
on `deepgram/homebrew-tap`. This must be configured manually:

1. Go to https://github.com/settings/tokens
2. Create a fine-grained token with "Contents: Read and write" permission
   on `deepgram/homebrew-tap`
3. Add it as a secret named `HOMEBREW_TAP_TOKEN` in `deepgram/wsh` repo
   settings → Secrets and variables → Actions

**Step 3: Bootstrap the tap repo with a placeholder formula**

Run:
```bash
tmp=$(mktemp -d)
cd "$tmp"
git init homebrew-tap && cd homebrew-tap
mkdir -p Formula
cat > Formula/wsh.rb << 'EOF'
class Wsh < Formula
  desc "The Web Shell — an API for your terminal"
  homepage "https://github.com/deepgram/wsh"
  version "0.0.0"
  license "ISC"

  # This formula is auto-updated by the wsh release workflow.
  # Do not edit manually.

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/deepgram/wsh/releases/download/v0.0.0/wsh-aarch64-apple-darwin"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      url "https://github.com/deepgram/wsh/releases/download/v0.0.0/wsh-x86_64-apple-darwin"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/deepgram/wsh/releases/download/v0.0.0/wsh-aarch64-linux-musl"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    else
      url "https://github.com/deepgram/wsh/releases/download/v0.0.0/wsh-x86_64-linux-musl"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install stable.url.split("/").last => "wsh"

    generate_completions_from_executable(bin/"wsh", "completions")
  end

  service do
    run [opt_bin/"wsh", "server"]
    keep_alive true
    log_path var/"log/wsh.log"
    error_log_path var/"log/wsh.log"
  end

  test do
    assert_match "wsh", shell_output("#{bin}/wsh --version")
  end
end
EOF
git add .
git commit -m "Initial placeholder formula"
git remote add origin https://github.com/deepgram/homebrew-tap.git
git push -u origin main
```

**Step 4: Verify tap is accessible**

Run: `brew tap deepgram/tap`
Expected: Taps successfully (formula won't install yet since v0.0.0 doesn't exist)

---

### Task 6: Update README and docs

**Files:**
- Modify: `README.md` (install section)
- Modify: `docs/building.md` (add shell completions and service file sections)

**Step 1: Update README install section**

The README (lines 13-23) currently has:

```markdown
## Install

```bash
curl -fsSL https://wsh.dev/install.sh | sh
```

Or with Cargo:

```bash
cargo install wsh
```
```

Replace with:

```markdown
## Install

**Homebrew** (macOS and Linux):

```bash
brew install deepgram/tap/wsh
```

**Shell script** (Linux and macOS):

```bash
curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
```

**Cargo** (build from source):

```bash
cargo install wsh
```
```

**Step 2: Add shell completions section to docs/building.md**

After the "Build Targets Summary" section and before "Notes", add:

```markdown
## Shell Completions

Generate completions for your shell:

```bash
# Bash
wsh completions bash > /etc/bash_completion.d/wsh

# Zsh
wsh completions zsh > /usr/local/share/zsh/site-functions/_wsh

# Fish
wsh completions fish > ~/.config/fish/completions/wsh.fish
```

Homebrew installs completions automatically.

## Running as a Service

wsh auto-spawns an ephemeral server on first use, so most users don't need a
service. For persistent server operation, see the contrib templates:

- **Linux (systemd):** `contrib/linux/wsh.service`
- **macOS (launchd):** `contrib/macos/com.deepgram.wsh.plist`
- **Homebrew:** `brew services start wsh`

Configuration is via environment variables in `~/.config/wsh/server.env`.
See `contrib/linux/server.env` for available options.
```

**Step 3: Commit**

```bash
git add README.md docs/building.md
git commit -m "docs: update install instructions and add completions/service docs

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

---

### Task 7: End-to-end verification

**Step 1: Verify the completions subcommand works from a release build**

Run: `nix build && ./result/bin/wsh completions bash | head -3`
Expected: Bash completion output

Run: `./result/bin/wsh completions zsh | head -3`
Expected: Zsh completion output

Run: `./result/bin/wsh completions fish | head -3`
Expected: Fish completion output

**Step 2: Verify install.sh platform detection**

Run: `sh install.sh 2>&1 | head -10`
Expected: Detects platform correctly, then fails on download (no release yet).
Should show something like: `wsh-installer: installing wsh ... (wsh-x86_64-linux-musl)`

**Step 3: Verify all existing tests still pass**

Run: `nix develop -c sh -c "cargo test 2>&1" | tail -20`
Expected: All tests pass

**Step 4: Verify the workflow YAML is valid**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" 2>&1`
Expected: No output (valid YAML). If python3/yaml is unavailable, skip.

**Step 5: List all new files to confirm nothing is missing**

Run: `git diff --name-only HEAD~6`
Expected: Shows all files created across Tasks 1-6.
