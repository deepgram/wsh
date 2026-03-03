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
