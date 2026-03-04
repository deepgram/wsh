# Releasing a New Version

This guide walks through the entire process of cutting a wsh release, from
"I have code I want to ship" to "users can install it."

## What a Release Does

When you release wsh, four things happen:

1. **Binaries are built** for all 4 platforms (Linux x86_64, Linux aarch64,
   macOS Intel, macOS Apple Silicon)
2. **A GitHub Release is created** — a page on GitHub where the binaries are
   hosted for download
3. **The install script starts working** for the new version (`curl ... | sh`)
4. **The Homebrew formula is updated** so `brew install` and `brew upgrade`
   get the new version

All of this is handled by a single script: `scripts/release.sh`.

## One-Time Setup

These steps only need to be done once, ever.

### 1. Install Nix

If you haven't already:

```bash
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh
```

### 2. Log in to the GitHub CLI

The release script uses `gh` (the GitHub CLI) to create releases. Enter the
dev shell and log in:

```bash
nix develop
gh auth login
```

It'll open a browser window. Log in with your GitHub account. Pick these
options when asked:

- **What account?** → GitHub.com
- **Preferred protocol?** → SSH
- **Upload your SSH key?** → Yes (if it offers)
- **Title for this SSH key?** → Anything (e.g. "wsh dev machine")
- **How would you like to authenticate?** → Login with a web browser

You only need to do this once. The token is saved at `~/.config/gh/hosts.yml`.

To verify it worked:

```bash
gh auth status
```

You should see something like:

```
github.com
  ✓ Logged in to github.com account yourname
```

### 3. Verify SSH access to GitHub

The release script pushes to two repos via SSH (`git@github.com:...`). Make
sure your SSH key is set up:

```bash
ssh -T git@github.com
```

You should see: `Hi yourname! You've successfully authenticated`.

If not, you need to add your SSH key to GitHub:
https://github.com/settings/keys

## Cutting a Release

### 1. Make sure your code is ready

All the changes you want in the release should be committed and pushed to
master:

```bash
git status          # nothing uncommitted
git push origin master   # up to date with GitHub
```

### 2. Pick a version number

We use [semantic versioning](https://semver.org/): `vMAJOR.MINOR.PATCH`.

- **PATCH** (v0.1.0 → v0.1.1): Bug fixes, small tweaks
- **MINOR** (v0.1.0 → v0.2.0): New features, backward-compatible changes
- **MAJOR** (v0.1.0 → v1.0.0): Breaking changes

### 3. Update the version in Cargo.toml

Edit `Cargo.toml` and change the version:

```toml
[package]
name = "wsh"
version = "0.2.0"    # ← update this (no "v" prefix here)
```

Commit it:

```bash
git add Cargo.toml
git commit -m "release: v0.2.0"
git push origin master
```

### 4. Run the release script

```bash
nix develop
./scripts/release.sh v0.2.0
```

The script will:

1. **Build all 4 binaries** — you'll see each one being compiled. This takes
   ~5 minutes on first run (Nix caches make subsequent builds faster).
   Each binary is verified with `file` so you can see it's the right format.

2. **Create a git tag** called `v0.2.0` pointing at your current commit, and
   push it to GitHub. (A tag is just a name for a specific commit.)

3. **Create a GitHub Release** at
   `github.com/deepgram/wsh/releases/tag/v0.2.0` and upload:
   - The 4 binaries
   - `checksums.txt` (SHA256 hashes for verification)
   - `install.sh` (the curl|sh installer)

4. **Update the Homebrew formula** in the `deepgram/homebrew-tap` repo with
   the new version and checksums.

When it's done, you'll see:

```
==> Done! Released wsh v0.2.0

    GitHub Release: https://github.com/deepgram/wsh/releases/tag/v0.2.0
    Install:        brew install deepgram/tap/wsh
    Or:             curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
```

### 5. Verify the release

Check the GitHub Release page — click the link the script printed, or go to:
https://github.com/deepgram/wsh/releases

You should see the release with all 6 assets listed (4 binaries + checksums +
install script).

## How Users Install After a Release

**Homebrew (macOS and Linux):**

```bash
brew install deepgram/tap/wsh    # first install
brew upgrade wsh                  # update to latest
```

**Install script (any machine with curl):**

```bash
curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
```

**Specific version:**

```bash
WSH_VERSION=v0.2.0 curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
```

**Direct download:**

```bash
# Pick the right one for your platform:
curl -LO https://github.com/deepgram/wsh/releases/latest/download/wsh-x86_64-linux-musl
curl -LO https://github.com/deepgram/wsh/releases/latest/download/wsh-aarch64-linux-musl
curl -LO https://github.com/deepgram/wsh/releases/latest/download/wsh-x86_64-apple-darwin
curl -LO https://github.com/deepgram/wsh/releases/latest/download/wsh-aarch64-apple-darwin
chmod +x wsh-*
sudo mv wsh-* /usr/local/bin/wsh
```

## Troubleshooting

### "gh: not logged in"

Run `gh auth login` inside `nix develop`. See the one-time setup section.

### "Permission denied" pushing tags

Your SSH key may not have push access to the repo. Check:

```bash
ssh -T git@github.com
```

If it doesn't show your username, add your SSH key at
https://github.com/settings/keys.

### "Tag already exists"

If you need to redo a release with the same version:

```bash
# Delete the tag locally and on GitHub
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0

# Delete the GitHub Release
gh release delete v0.2.0 --repo deepgram/wsh --yes

# Re-run the release script
./scripts/release.sh v0.2.0
```

### Build fails

If a `nix build` step fails, it's usually a Nix cache or network issue. Try:

```bash
# Clear the result symlink and retry
rm -f result
nix build .#wsh-x86_64-linux-musl
```

For macOS cross-compilation issues, see the troubleshooting section in
`docs/building.md`.

### Homebrew formula push fails

If the script can't push to `deepgram/homebrew-tap`, check that your SSH key
has write access to that repo. You can update the formula manually:

```bash
git clone git@github.com:deepgram/homebrew-tap.git
# Edit Formula/wsh.rb with the new version and SHA256 hashes
# (the script prints the checksums — copy them from the output)
cd homebrew-tap
git add Formula/wsh.rb
git commit -m "wsh v0.2.0"
git push
```

## Reference

| File | Purpose |
|------|---------|
| `scripts/release.sh` | The release script (builds, publishes, updates formula) |
| `install.sh` | The curl\|sh installer (downloaded by users) |
| `flake.nix` | Nix build definitions for all 4 targets |
| `docs/building.md` | How to build from source |
| `contrib/linux/wsh.service` | systemd service file template |
| `contrib/macos/com.deepgram.wsh.plist` | launchd service file template |
| `contrib/linux/server.env` | Example environment file for service config |
