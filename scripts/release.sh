#!/usr/bin/env bash
set -euo pipefail

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# wsh release script
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
#
# Builds all release binaries, publishes them to GitHub, and updates
# the Homebrew formula. Run this from the repo root inside `nix develop`.
#
# Usage:
#   ./scripts/release.sh v0.2.0
#
# What happens when you run this:
#
#   1. Builds 4 binaries (x86_64 Linux, aarch64 Linux, Intel Mac, Apple Silicon Mac)
#      using the Nix flake targets you already have. Takes ~5-10 min on first run,
#      much faster after that thanks to the Nix cache.
#
#   2. Creates a git tag (e.g. "v0.2.0") on your current commit and pushes it
#      to GitHub. This is how GitHub knows which commit a release corresponds to.
#
#   3. Creates a "GitHub Release" — a page on GitHub where the binaries are
#      hosted for download. If `gh` (GitHub CLI) is available, this is fully
#      automatic. If not, the script stages all the files and tells you how
#      to upload them through the GitHub website.
#
#   4. Updates the Homebrew formula in the deepgram/homebrew-tap repo with
#      the new version and binary checksums. After this step,
#      `brew install deepgram/tap/wsh` installs the new version.
#
# Prerequisites (one-time setup, see docs/releasing.md):
#   - You're inside `nix develop` (provides nix, etc.)
#   - Your SSH key can push to both repos
#   - The deepgram/homebrew-tap repo exists on GitHub
#   - Optional: `gh auth login` for fully automated releases
#
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# ── Validate the version argument ─────────────────────────────────

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    echo ""
    echo "  e.g. $0 v0.2.0"
    echo ""
    echo "Version must start with 'v' followed by major.minor.patch."
    echo "See docs/releasing.md for the full release process."
    exit 1
fi

# Enforce v1.2.3 format so we don't accidentally create a weird tag.
if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version must match v*.*.* (e.g. v0.2.0)"
    exit 1
fi

# These are the GitHub repos we'll interact with.
REPO="deepgram/wsh"               # This repo — where the release is published
TAP_REPO="deepgram/homebrew-tap"  # The Homebrew tap — where the formula lives

# The four binary targets we build. These names match the Nix flake outputs
# (defined in flake.nix) and become the filenames users download.
TARGETS=(
    wsh-x86_64-linux-musl       # Linux Intel/AMD (static, runs anywhere)
    wsh-aarch64-linux-musl      # Linux ARM (Raspberry Pi, AWS Graviton, etc.)
    wsh-x86_64-apple-darwin     # macOS Intel
    wsh-aarch64-apple-darwin    # macOS Apple Silicon (M1/M2/M3/M4)
)

# Check whether `gh` (GitHub CLI) is available. If it is, we can create
# the GitHub Release automatically. If not, we'll stage files and print
# instructions for uploading through the browser.
HAS_GH=false
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
    HAS_GH=true
fi

echo "==> Releasing wsh $VERSION"
if [ "$HAS_GH" = false ]; then
    echo "    (gh CLI not available — will use browser upload for GitHub Release)"
fi
echo ""

# ── Step 1: Build all 4 binaries ──────────────────────────────────
#
# Each `nix build .#<target>` cross-compiles a binary and places it
# at result/bin/wsh. We copy each one to a staging directory with
# the target name (e.g. "wsh-x86_64-linux-musl") so GitHub Release
# has distinct filenames for each platform.

# Use a persistent staging directory (not auto-cleaned) when doing
# manual upload, so the files survive after the script exits.
if [ "$HAS_GH" = true ]; then
    STAGING="$(mktemp -d)"
    trap 'rm -rf "$STAGING"' EXIT
else
    STAGING="release-$VERSION"
    rm -rf "$STAGING"
    mkdir -p "$STAGING"
fi

for target in "${TARGETS[@]}"; do
    echo "==> Building $target"
    nix build ".#$target" --print-build-logs
    cp result/bin/wsh "$STAGING/$target"
    chmod +x "$STAGING/$target"
    echo "    $(file "$STAGING/$target")"
done

# Generate a checksums.txt file so users (and the install script) can
# verify downloads haven't been tampered with.
echo "==> Generating checksums"
(cd "$STAGING" && sha256sum wsh-* > checksums.txt)
cat "$STAGING/checksums.txt"

# Bundle the install script so it's available as a release asset.
# Users download it with: curl ... | sh
cp install.sh "$STAGING/install.sh"

echo ""

# ── Step 2: Tag this commit and push the tag to GitHub ────────────
#
# A "tag" in git is just a named pointer to a specific commit.
# GitHub uses tags to organize releases. When you push a tag,
# it shows up at github.com/deepgram/wsh/tags.

if git rev-parse "$VERSION" >/dev/null 2>&1; then
    echo "==> Tag $VERSION already exists, skipping"
else
    echo "==> Tagging $VERSION"
    git tag "$VERSION"
fi

echo "==> Pushing tag $VERSION to GitHub"
git push origin "$VERSION"

echo ""

# ── Step 3: Create a GitHub Release ───────────────────────────────
#
# A "GitHub Release" is a page on your repo where you can attach
# downloadable files (our binaries).
#
# After this step, these URLs become live:
#   https://github.com/deepgram/wsh/releases/download/v0.2.0/wsh-x86_64-linux-musl
#   https://github.com/deepgram/wsh/releases/download/v0.2.0/install.sh
#   etc.

if [ "$HAS_GH" = true ]; then
    # `gh release create` does everything:
    #   - Creates the release page
    #   - Uploads all the files we list as "release assets"
    #   - Auto-generates release notes from commit messages
    echo "==> Creating GitHub Release (via gh)"
    gh release create "$VERSION" \
        --repo "$REPO" \
        --title "$VERSION" \
        --generate-notes \
        "$STAGING"/wsh-* \
        "$STAGING/checksums.txt" \
        "$STAGING/install.sh"
else
    echo "==> GitHub Release: manual upload required"
    echo ""
    echo "    The binaries are staged in: $(pwd)/$STAGING/"
    echo ""
    echo "    Open this URL in your browser:"
    echo "      https://github.com/$REPO/releases/new?tag=$VERSION&title=$VERSION"
    echo ""
    echo "    Then drag and drop ALL of these files onto the upload area:"
    echo ""
    for f in "$STAGING"/wsh-* "$STAGING/checksums.txt" "$STAGING/install.sh"; do
        echo "      $(basename "$f")"
    done
    echo ""
    echo "    Click 'Publish release' when done."
    echo ""
    read -rp "    Press Enter after you've published the release..."
fi

echo ""

# ── Step 4: Update the Homebrew formula ───────────────────────────
#
# Homebrew is a package manager for macOS (and Linux). Our "tap" is
# a GitHub repo (deepgram/homebrew-tap) containing a Ruby file that
# tells Homebrew where to download wsh and how to install it.
#
# We need to update the formula with:
#   - The new version number
#   - The SHA256 hash of each binary (so Homebrew can verify downloads)
#
# We do this by cloning the tap repo, overwriting the formula file,
# committing, and pushing. After this, `brew install deepgram/tap/wsh`
# and `brew upgrade wsh` will get the new version.

echo "==> Updating Homebrew formula"

# Compute SHA256 checksums for each binary. Homebrew uses these to
# verify that the downloaded file matches what we built.
declare -A HASHES
for target in "${TARGETS[@]}"; do
    HASHES[$target]="$(sha256sum "$STAGING/$target" | awk '{print $1}')"
done

# Clone the tap repo to a temp directory, update the formula, push.
TAP_DIR="$(mktemp -d)"
if [ "$HAS_GH" = true ]; then
    trap 'rm -rf "$STAGING" "$TAP_DIR"' EXIT
else
    trap 'rm -rf "$TAP_DIR"' EXIT
fi

git clone --depth 1 "git@github.com:$TAP_REPO.git" "$TAP_DIR"
mkdir -p "$TAP_DIR/Formula"

# Strip the "v" prefix for the Homebrew version (v0.2.0 → 0.2.0).
BARE_VERSION="${VERSION#v}"

# Write the formula. This is a Ruby file that Homebrew evaluates.
# It detects the user's OS and CPU, then downloads the right binary.
cat > "$TAP_DIR/Formula/wsh.rb" << EOF
class Wsh < Formula
  desc "The Web Shell — an API for your terminal"
  homepage "https://github.com/deepgram/wsh"
  version "$BARE_VERSION"
  license "ISC"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/$REPO/releases/download/$VERSION/wsh-aarch64-apple-darwin"
      sha256 "${HASHES[wsh-aarch64-apple-darwin]}"
    else
      url "https://github.com/$REPO/releases/download/$VERSION/wsh-x86_64-apple-darwin"
      sha256 "${HASHES[wsh-x86_64-apple-darwin]}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/$REPO/releases/download/$VERSION/wsh-aarch64-linux-musl"
      sha256 "${HASHES[wsh-aarch64-linux-musl]}"
    else
      url "https://github.com/$REPO/releases/download/$VERSION/wsh-x86_64-linux-musl"
      sha256 "${HASHES[wsh-x86_64-linux-musl]}"
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

# Commit and push the updated formula.
cd "$TAP_DIR"
git add Formula/wsh.rb
git commit -m "wsh $VERSION"
git push

echo ""
echo "==> Done! Released wsh $VERSION"
echo ""
echo "    GitHub Release: https://github.com/$REPO/releases/tag/$VERSION"
echo "    Install:        brew install deepgram/tap/wsh"
echo "    Or:             curl -fsSL https://github.com/$REPO/releases/latest/download/install.sh | sh"
