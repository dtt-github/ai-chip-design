#!/usr/bin/env bash
# Builds the GDS parser to WebAssembly into apps/web/src/wasm/ (generated, not committed).
# Needs: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version 0.2.130
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/apps/web/src/wasm"
PROFILE="${1:-release}"
cd "$ROOT"
cargo build -p sc-gds-wasm --target wasm32-unknown-unknown --profile "$PROFILE"
DIR=$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")
wasm-bindgen --target web --out-dir "$OUT" --out-name sc_gds "target/wasm32-unknown-unknown/$DIR/sc_gds_wasm.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -O3 "$OUT/sc_gds_bg.wasm" -o "$OUT/sc_gds_bg.wasm"
fi
ls -la "$OUT"
