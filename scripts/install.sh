#!/usr/bin/env bash
set -euo pipefail

REPO="NaritATK/CrabClaw"
BIN_NAME="crabclaw"
INSTALL_DIR="${CRABCLAW_INSTALL_DIR:-$HOME/.local/bin}"

usage() {
  cat <<'EOF'
CrabClaw installer

Usage:
  install.sh [--musl] [--target <rust-target>] [--help]

Options:
  --musl              Force Linux musl target (x86_64-unknown-linux-musl)
  --target <target>   Explicit target triple (overrides auto-detection)
  --help              Show this help

Environment:
  CRABCLAW_INSTALL_TARGET   Explicit target triple (same as --target)
  CRABCLAW_INSTALL_DIR      Install directory (default: ~/.local/bin)
EOF
}

FORCE_MUSL=0
CLI_TARGET=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --musl)
      FORCE_MUSL=1
      shift
      ;;
    --target)
      if [[ $# -lt 2 ]]; then
        echo "--target requires a value" >&2
        exit 1
      fi
      CLI_TARGET="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -n "$CLI_TARGET" ]]; then
  TARGET="$CLI_TARGET"
elif [[ -n "${CRABCLAW_INSTALL_TARGET:-}" ]]; then
  TARGET="$CRABCLAW_INSTALL_TARGET"
else
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)

  case "$os" in
    linux)
      if [[ "$FORCE_MUSL" -eq 1 ]]; then
        os_part="unknown-linux-musl"
      else
        os_part="unknown-linux-gnu"
      fi
      ;;
    darwin)
      os_part="apple-darwin"
      ;;
    *)
      echo "Unsupported OS: $os" >&2
      exit 1
      ;;
  esac

  case "$arch" in
    x86_64|amd64)
      arch_part="x86_64"
      ;;
    arm64|aarch64)
      arch_part="aarch64"
      ;;
    *)
      echo "Unsupported arch: $arch" >&2
      exit 1
      ;;
  esac

  TARGET="${arch_part}-${os_part}"
fi

ASSET="${BIN_NAME}-${TARGET}.tar.gz"

if ! command -v curl >/dev/null 2>&1; then
  echo "curl is required" >&2
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "tar is required" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

url="https://github.com/${REPO}/releases/latest/download/${ASSET}"
echo "Downloading $url"
curl -fsSL "$url" -o "$tmpdir/$ASSET"

tar -xzf "$tmpdir/$ASSET" -C "$tmpdir"
install -m 0755 "$tmpdir/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"

echo "Installed $BIN_NAME to $INSTALL_DIR/$BIN_NAME"
echo "Target: $TARGET"
echo "Run: $BIN_NAME --version"
echo "Run: $BIN_NAME --diagnose"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add to PATH: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
