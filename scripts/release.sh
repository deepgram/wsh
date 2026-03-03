#!/usr/bin/env bash
set -euo pipefail

# Release script for wsh.
#
# Usage:
#   ./scripts/release.sh v0.2.0
#
# What it does:
#   1. Builds all 4 binaries via nix build
#   2. Tags the commit and pushes the tag
#   3. Creates a GitHub Release with binaries, checksums, and install.sh
#   4. Updates the Homebrew formula in deepgram/homebrew-tap
#
# Prerequisites:
#   - Run from the repo root inside `nix develop`
#   - `gh auth login` has been run at least once
#   - deepgram/homebrew-tap repo exists on GitHub

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    echo "  e.g. $0 v0.2.0"
    exit 1
fi

if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version must match v*.*.* (e.g. v0.2.0)"
    exit 1
fi

REPO="deepgram/wsh"
TAP_REPO="deepgram/homebrew-tap"
TARGETS=(
    wsh-x86_64-linux-musl
    wsh-aarch64-linux-musl
    wsh-x86_64-apple-darwin
    wsh-aarch64-apple-darwin
)

echo "==> Releasing wsh $VERSION"
echo ""

# ── Step 1: Build all targets ──────────────────────────────────────

STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

for target in "${TARGETS[@]}"; do
    echo "==> Building $target"
    nix build ".#$target" --print-build-logs
    cp result/bin/wsh "$STAGING/$target"
    chmod +x "$STAGING/$target"
    echo "    $(file "$STAGING/$target")"
done

# Checksums
echo "==> Generating checksums"
(cd "$STAGING" && sha256sum wsh-* > checksums.txt)
cat "$STAGING/checksums.txt"

# Copy install.sh
cp install.sh "$STAGING/install.sh"

echo ""

# ── Step 2: Tag and push ──────────────────────────────────────────

if git rev-parse "$VERSION" >/dev/null 2>&1; then
    echo "==> Tag $VERSION already exists, skipping"
else
    echo "==> Tagging $VERSION"
    git tag "$VERSION"
fi

echo "==> Pushing tag $VERSION"
git push origin "$VERSION"

echo ""

# ── Step 3: Create GitHub Release ─────────────────────────────────

echo "==> Creating GitHub Release"
gh release create "$VERSION" \
    --repo "$REPO" \
    --title "$VERSION" \
    --generate-notes \
    "$STAGING"/wsh-* \
    "$STAGING/checksums.txt" \
    "$STAGING/install.sh"

echo ""

# ── Step 4: Update Homebrew formula ───────────────────────────────

echo "==> Updating Homebrew formula"

# Compute SHA256 for each target
declare -A HASHES
for target in "${TARGETS[@]}"; do
    HASHES[$target]="$(sha256sum "$STAGING/$target" | awk '{print $1}')"
done

TAP_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGING" "$TAP_DIR"' EXIT

gh repo clone "$TAP_REPO" "$TAP_DIR" -- --depth 1
mkdir -p "$TAP_DIR/Formula"

BARE_VERSION="${VERSION#v}"

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
