#!/bin/sh
# agentstack installer — downloads the latest release binary for your platform.
#   curl -fsSL https://raw.githubusercontent.com/Tarek-kharsa/agentstack/main/install.sh | sh
set -eu

# AGENTSTACK_INSTALL_REPO overrides the GitHub slug (owner/repo) for forks.
REPO="${AGENTSTACK_INSTALL_REPO:-Tarek-kharsa/agentstack}"
PREFIX="${AGENTSTACK_PREFIX:-}"
VERSION="${AGENTSTACK_VERSION:-latest}"   # "latest" or a release tag like v0.1.0

say() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) case "$arch" in
            arm64|aarch64) target="aarch64-apple-darwin" ;;
            x86_64)        target="x86_64-apple-darwin" ;;
            *) err "unsupported macOS arch: $arch" ;;
          esac ;;
  Linux)  case "$arch" in
            x86_64)        target="x86_64-unknown-linux-gnu" ;;
            aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
            *) err "unsupported Linux arch: $arch" ;;
          esac ;;
  *) err "unsupported OS: $os (on Windows, download the .zip from the releases page)" ;;
esac

# Pick an install dir we can write to.
if [ -z "$PREFIX" ]; then
  if [ -w "/usr/local/bin" ]; then PREFIX="/usr/local/bin"
  else PREFIX="$HOME/.local/bin"; fi
fi
mkdir -p "$PREFIX"

have curl || err "curl is required"
have tar || err "tar is required"

asset="agentstack-${target}.tar.gz"
if [ "$VERSION" = "latest" ]; then
  url="https://github.com/${REPO}/releases/latest/download/${asset}"
else
  url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"
fi
say "Downloading ${asset} (${VERSION}) …"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/$asset" || err "download failed: $url"
tar xzf "$tmp/$asset" -C "$tmp"
install -m 0755 "$tmp/agentstack-${target}/agentstack" "$PREFIX/agentstack"

say "Installed agentstack to $PREFIX/agentstack"
case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *) say "Add to PATH:  export PATH=\"$PREFIX:\$PATH\"" ;;
esac
say "Run 'agentstack init' to get started."
