#!/usr/bin/env bash
# Downloads the pinned Verible release (Verilog parser, linter, formatter and
# language server) into .tools/verible/. The API finds verible-verilog-ls there
# unless WAFER_VERIBLE_LS points elsewhere.
set -euo pipefail

VERSION="v0.0-4296-g0f262651"
case "$(uname -m)" in
  aarch64|arm64) ARCH=arm64;  SHA256=e4a70e9920b6a24ce35045d4c9457d7d9185efc3c534a35ff55ef0262578b4e0 ;;
  x86_64|amd64)  ARCH=x86_64; SHA256=8569defb891d2316067613ea00442af28a7a09d405d95b54c0c91f9942d26635 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/.tools/verible"
if [[ -x "$DEST/bin/verible-verilog-ls" ]] && "$DEST/bin/verible-verilog-ls" --version 2>/dev/null | grep -q "${VERSION#v}"; then
  echo "Verible $VERSION already installed in $DEST"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/chipsalliance/verible/releases/download/$VERSION/verible-$VERSION-linux-static-$ARCH.tar.gz"
echo "Downloading $URL"
curl -fsSL -o "$TMP/verible.tgz" "$URL"
echo "$SHA256  $TMP/verible.tgz" | sha256sum -c --quiet
mkdir -p "$TMP/x"
tar -xzf "$TMP/verible.tgz" -C "$TMP/x" --strip-components=1
rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
mv "$TMP/x" "$DEST"
"$DEST/bin/verible-verilog-ls" --version | head -1
