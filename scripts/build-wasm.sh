#!/usr/bin/env bash
# Builds the GDS parser and the simulator to WebAssembly into apps/web/src/wasm/
# (generated, not committed).
# Needs: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version 0.2.130
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/apps/web/src/wasm"
PROFILE="${1:-release}"
cd "$ROOT"
cargo build -p sc-gds-wasm -p sc-sim-wasm --target wasm32-unknown-unknown --profile "$PROFILE"
DIR=$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE")
for pair in sc_gds_wasm:sc_gds sc_sim_wasm:sc_sim; do
  crate="${pair%%:*}"; name="${pair##*:}"
  wasm-bindgen --target web --out-dir "$OUT" --out-name "$name" "target/wasm32-unknown-unknown/$DIR/$crate.wasm"
  if command -v wasm-opt >/dev/null; then
    wasm-opt -O3 "$OUT/${name}_bg.wasm" -o "$OUT/${name}_bg.wasm"
  fi
done
ls -la "$OUT"
