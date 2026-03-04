# Releasing a New Version

This guide walks through the entire process of cutting a wsh release, from
"I have code I want to ship" to "users can install it."

## What a Release Does

When you release wsh, four things happen:

1. **Binaries are built** for all 4 platforms (Linux x86_64, Linux aarch64,
   macOS Intel, macOS Apple Silicon)
2. **A GitHub Release is created** — a page on GitHub where the binaries are
   hosted for public download
3. **The install script starts working** for the new version (`curl ... | sh`)
4. **The Homebrew formula is updated** so `brew install` and `brew upgrade`
   get the new version

All of this is handled by `scripts/release.sh`. The script works in two modes:

- **With `gh` (GitHub CLI):** Fully automatic — builds, uploads, updates
  formula, done.
- **Without `gh`:** Builds the binaries and stages them locally, then opens a
  browser URL where you drag-and-drop the files to create the release. The
  Homebrew formula update is still automatic.

## One-Time Setup

### 1. Install Nix

```bash
curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | sh
```

### 2. Verify SSH access to GitHub

The release script pushes git tags and updates the Homebrew tap via SSH. Make
sure your SSH key is set up:

```bash
ssh -T git@github.com
```

You should see:

```
Hi yourname! You've successfully authenticated, but GitHub does not provide shell access.
```

If you see "Permission denied" instead, you need to add your SSH key to GitHub:

1. Check if you have a key: `ls ~/.ssh/id_*.pub`
2. If not, create one: `ssh-keygen -t ed25519` (press Enter for all defaults)
3. Copy the public key: `cat ~/.ssh/id_ed25519.pub`
4. Go to https://github.com/settings/keys
5. Click "New SSH key", paste the key, give it a name, click "Add SSH key"
6. Test again: `ssh -T git@github.com`

### 3. (Optional) Set up the GitHub CLI

This makes releases fully automatic — the script creates the GitHub Release
and uploads binaries without opening a browser. If you skip this, the script
will stage the files locally and tell you to upload them through the browser.

```bash
nix develop
gh auth login
```

When asked:

- **What account?** → GitHub.com
- **Preferred protocol?** → SSH
- **How would you like to authenticate?** → Login with a web browser

A browser window opens. Authorize the app. Done. To verify:

```bash
gh auth status
```

You should see:

```
github.com
  ✓ Logged in to github.com account yourname
```

## Cutting a Release

### 1. Make sure your code is ready

Everything you want in the release should be committed and pushed:

```bash
git status               # should show nothing uncommitted
git push origin master   # push to GitHub
```

### 2. Pick a version number

We use [semantic versioning](https://semver.org/): `vMAJOR.MINOR.PATCH`.

- **PATCH** (v0.1.0 → v0.1.1): Bug fixes, small tweaks
- **MINOR** (v0.1.0 → v0.2.0): New features, backward-compatible changes
- **MAJOR** (v0.1.0 → v1.0.0): Breaking changes

### 3. Update the version in Cargo.toml

Edit `Cargo.toml` and change the version number (no "v" prefix here):

```toml
[package]
name = "wsh"
version = "0.2.0"    # ← update this
```

Commit and push:

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

**What you'll see:**

```
==> Releasing wsh v0.2.0

==> Building wsh-x86_64-linux-musl
    result/bin/wsh: ELF 64-bit LSB executable, x86-64, ...
==> Building wsh-aarch64-linux-musl
    result/bin/wsh: ELF 64-bit LSB executable, ARM aarch64, ...
==> Building wsh-x86_64-apple-darwin
    result/bin/wsh: Mach-O 64-bit executable x86_64
==> Building wsh-aarch64-apple-darwin
    result/bin/wsh: Mach-O 64-bit executable arm64

==> Generating checksums
abc123...  wsh-aarch64-apple-darwin
def456...  wsh-aarch64-linux-musl
789abc...  wsh-x86_64-apple-darwin
012def...  wsh-x86_64-linux-musl

==> Tagging v0.2.0
==> Pushing tag v0.2.0 to GitHub
```

**If you have `gh` set up**, the script continues automatically:

```
==> Creating GitHub Release (via gh)
https://github.com/deepgram/wsh/releases/tag/v0.2.0

==> Updating Homebrew formula

==> Done! Released wsh v0.2.0
```

**If you don't have `gh`**, the script pauses and tells you to upload manually:

```
==> GitHub Release: manual upload required

    The binaries are staged in: /path/to/wsh/release-v0.2.0/

    Open this URL in your browser:
      https://github.com/deepgram/wsh/releases/new?tag=v0.2.0&title=v0.2.0

    Then drag and drop ALL of these files onto the upload area:

      wsh-aarch64-apple-darwin
      wsh-aarch64-linux-musl
      wsh-x86_64-apple-darwin
      wsh-x86_64-linux-musl
      checksums.txt
      install.sh

    Click 'Publish release' when done.

    Press Enter after you've published the release...
```

After you press Enter, the script continues with the Homebrew formula update
(which uses git over SSH, not `gh`).

### 5. Verify the release

Go to https://github.com/deepgram/wsh/releases. You should see the release
with 6 assets (4 binaries + checksums.txt + install.sh).

Test the install script:

```bash
curl -fsSL https://github.com/deepgram/wsh/releases/latest/download/install.sh | sh
wsh --version
```

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

**Direct download (fully manual):**

Go to https://github.com/deepgram/wsh/releases/latest and download the file
for your platform:

| Platform | File |
|----------|------|
| Linux x86_64 (Intel/AMD) | `wsh-x86_64-linux-musl` |
| Linux aarch64 (Raspberry Pi, Graviton) | `wsh-aarch64-linux-musl` |
| macOS Intel | `wsh-x86_64-apple-darwin` |
| macOS Apple Silicon (M1/M2/M3/M4) | `wsh-aarch64-apple-darwin` |

Then:

```bash
chmod +x wsh-*
sudo mv wsh-* /usr/local/bin/wsh
wsh --version
```

## Troubleshooting

### Build fails

If a `nix build` step fails, try clearing the result symlink and rebuilding:

```bash
rm -f result
nix build .#wsh-x86_64-linux-musl
```

For macOS cross-compilation issues, see [building.md](building.md).

### "Permission denied" pushing the tag

Your SSH key doesn't have push access. See the SSH setup section above, or
check:

```bash
ssh -T git@github.com
```

### "Tag already exists"

If you need to redo a release with the same version number:

```bash
# Delete the tag locally and remotely
git tag -d v0.2.0
git push origin :refs/tags/v0.2.0

# Delete the GitHub Release (if it exists)
# With gh:
gh release delete v0.2.0 --repo deepgram/wsh --yes
# Without gh: go to github.com/deepgram/wsh/releases, find the release,
# click the trash icon.

# Re-run
./scripts/release.sh v0.2.0
```

### Homebrew formula push fails

The script clones `deepgram/homebrew-tap` via SSH and pushes the updated
formula. If it fails, check your SSH access and do it manually:

```bash
git clone git@github.com:deepgram/homebrew-tap.git
cd homebrew-tap
# Edit Formula/wsh.rb — update the version and sha256 hashes
# (copy the hashes from the checksums.txt the script printed)
git add Formula/wsh.rb
git commit -m "wsh v0.2.0"
git push
```

### Browser upload: "tag not found"

If the browser release page says the tag doesn't exist, the tag push may have
failed. Check:

```bash
git tag -l v0.2.0          # is the tag local?
git ls-remote origin v0.2.0  # is it on GitHub?
```

If it's local but not remote: `git push origin v0.2.0`

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
