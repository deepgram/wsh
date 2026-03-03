# Distribution & Packaging

**Date**: 2026-03-03
**Status**: Proposed
**Scope**: Homebrew tap, GitHub Releases, install script, shell completions,
contrib service files

## Problem

wsh has cross-compiled binaries for 4 targets (x86_64/aarch64 Linux musl,
x86_64/aarch64 macOS) but no distribution story. Users must build from source
or manually download binaries. macOS users hit Gatekeeper errors on unsigned
binaries. There's no shell completion, no service management, and no install
script.

## Goals

1. `brew install wsh` works on macOS and Linux
2. `curl ... | sh` install script for environments without Homebrew
3. `wsh completions <shell>` generates shell completions for bash, zsh, fish
4. Contrib service files for systemd (Linux) and launchd (macOS)
5. GitHub Actions workflow automates builds and releases on tag push

## Component 1: GitHub Actions Release Workflow

`.github/workflows/release.yml` triggers on `v*.*.*` tag pushes.

**Steps:**

1. Install Nix (Determinate installer) on `ubuntu-latest`
2. Build all 4 targets via `nix build .#wsh-<target>`
3. Rename binaries with target triples: `wsh-x86_64-linux-musl`,
   `wsh-aarch64-linux-musl`, `wsh-x86_64-apple-darwin`,
   `wsh-aarch64-apple-darwin`
4. Generate `checksums.txt` (SHA256 of each binary)
5. Copy `install.sh` from the repo
6. Create GitHub Release from the tag with all binaries, `install.sh`, and
   `checksums.txt` as assets
7. Update the Homebrew formula in `deepgram/homebrew-tap` with new version
   and SHA256s

All builds run on `ubuntu-latest` — all targets cross-compile from x86_64
Linux via the existing Nix flake. No macOS runners needed.

## Component 2: Homebrew Tap

Separate repo: `deepgram/homebrew-tap`

Contains `Formula/wsh.rb`. Users install with:

```
brew tap deepgram/tap
brew install wsh
```

**The formula:**

- Downloads the correct pre-built binary for the user's platform from the
  GitHub Release (no compilation)
- Installs shell completions by running `wsh completions <shell>` during
  the install phase
- Includes a `service` block for `brew services start wsh` (runs
  `wsh server` as a launchd user agent on macOS, systemd user unit on Linux)

**Auto-update:** The release workflow computes SHA256s for each binary and
pushes a commit to `deepgram/homebrew-tap` updating the formula's version
and hashes.

## Component 3: Install Script

`install.sh` in the wsh repo root, also uploaded as a release asset.

```
curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
```

**The script:**

1. Detects OS (`uname -s`) and architecture (`uname -m`)
2. Maps to binary name (`Darwin` + `arm64` → `wsh-aarch64-apple-darwin`)
3. Resolves version: uses `WSH_VERSION` env var if set, otherwise queries
   the GitHub API for the latest release tag
4. Downloads the binary and `checksums.txt` from the GitHub Release
5. Verifies SHA256 checksum (`sha256sum` on Linux, `shasum -a 256` on macOS)
6. Installs to `/usr/local/bin` (or `$WSH_INSTALL_DIR` if set)
7. Prints the installed version

**Dependencies:** POSIX shell, curl or wget, sha256sum or shasum. Nothing
else.

**Scope:** The install script only drops the binary. It does not install
shell completions or service files — keep it minimal.

## Component 4: Shell Completions

Add `clap_complete` dependency and a `wsh completions <shell>` subcommand.

```bash
wsh completions bash   # prints bash completion script to stdout
wsh completions zsh    # prints zsh completion script to stdout
wsh completions fish   # prints fish completion script to stdout
```

Completions are generated at runtime from the live CLI definition — always
in sync with the current binary. No pre-generated files to maintain.

**Installation by users (non-Homebrew):**

```bash
wsh completions bash > /etc/bash_completion.d/wsh
wsh completions zsh > /usr/local/share/zsh/site-functions/_wsh
wsh completions fish > ~/.config/fish/completions/wsh.fish
```

**Installation by Homebrew:** The formula calls `wsh completions` during
install and places the output in Homebrew's completion directories
automatically.

## Component 5: Contrib Service Files

### Environment file

Both service files reference a shared environment file at
`~/.config/wsh/server.env`:

```bash
# Uncomment and edit to customize wsh server behavior.
# WSH_SERVER_NAME=default
# WSH_TOKEN=your-secret-token
# WSH_TLS_CERT=/path/to/cert.pem
# WSH_TLS_KEY=/path/to/key.pem
# WSH_HOSTNAME=my-host.example.com
# WSH_BASE_PREFIX=/wsh
```

All values are commented out by default. The server runs with defaults
(localhost:8080, no auth, no TLS) when the file is absent or empty.

Note: `--bind` does not have an env var in the current CLI. The service
files hardcode `127.0.0.1:8080` (the default). Users who need a different
bind address should edit the `ExecStart`/`ProgramArguments` directly.

### Linux: systemd user unit

`contrib/linux/wsh.service`:

```ini
[Unit]
Description=wsh terminal API server
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

Install:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/linux/wsh.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now wsh
```

The `-` prefix on `EnvironmentFile` means systemd silently ignores a
missing file. `%h` expands to the user's home directory.

### macOS: launchd user agent

`contrib/macos/com.deepgram.wsh.plist`:

A standard launchd plist with `RunAtLoad` and `KeepAlive` set to true.
Runs `wsh server` as the current user.

launchd does not support `EnvironmentFile`. The plist includes commented-out
`EnvironmentVariables` entries for common settings (`WSH_TOKEN`,
`WSH_TLS_CERT`, etc.) that users can uncomment and edit.

Install:

```bash
cp contrib/macos/com.deepgram.wsh.plist ~/Library/LaunchAgents/
launchctl load ~/Library/LaunchAgents/com.deepgram.wsh.plist
```

### Homebrew service integration

The Homebrew formula includes a `service` block that handles launchd/systemd
automatically via `brew services start wsh`. This is independent of the
contrib files — Homebrew manages its own plist/unit. The contrib files exist
for users who install via the install script or manual download.

## File Layout

```
.github/workflows/release.yml     — release automation
install.sh                         — curl|sh installer
contrib/linux/wsh.service          — systemd user unit template
contrib/linux/server.env           — example environment file
contrib/macos/com.deepgram.wsh.plist — launchd user agent template
src/main.rs                        — add completions subcommand
Cargo.toml                         — add clap_complete dependency
```

The Homebrew formula lives in the separate `deepgram/homebrew-tap` repo.

## Out of Scope

- Apple code signing and notarization (Homebrew bypasses Gatekeeper)
- Native packages (.deb, .rpm, AUR)
- Docker image
- CI for PRs (only release workflow for now)
- wsh.dev domain / hosting
