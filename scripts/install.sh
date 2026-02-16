#!/usr/bin/env bash
set -euo pipefail

REPO="NaritATK/CrabClaw"
BIN_NAME="crabclaw"
INSTALL_DIR="${CRABCLAW_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)

case "$os" in
  linux) os_part="unknown-linux-gnu" ;;
  darwin) os_part="apple-darwin" ;;
  *) echo "Unsupported OS: $os"; exit 1 ;;
esac

case "$arch" in
  x86_64|amd64) arch_part="x86_64" ;;
  arm64|aarch64) arch_part="aarch64" ;;
  *) echo "Unsupported arch: $arch"; exit 1 ;;
esac

target="${arch_part}-${os_part}"
asset="${BIN_NAME}-${target}.tar.gz"

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required"
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required"
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

url="https://github.com/${REPO}/releases/latest/download/${asset}"
echo "Downloading $url"
curl -fsSL "$url" -o "$tmpdir/$asset"

tar -xzf "$tmpdir/$asset" -C "$tmpdir"
install -m 0755 "$tmpdir/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $INSTALL_DIR/$BIN_NAME"
echo "Run: $BIN_NAME --version"
echo "Run: $BIN_NAME --diagnose"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add to PATH: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
