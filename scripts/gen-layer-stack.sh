#!/usr/bin/env bash
# Regenerates apps/web/src/pdk/ihp-sg13g2.json from the PDK in the Docker volume.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${SC_LIBRELANE_IMAGE:-ghcr.io/librelane/librelane:3.0.16}"
VOLUME="${SC_PDK_VOLUME:-siliconcommons_pdk}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
docker run --rm --user "$(id -u):$(id -g)" -v "$VOLUME:/pdk:ro" -v "$TMP:/out" "$IMAGE" \
  cp /pdk/ihp-sg13g2/libs.tech/klayout/tech/sg13g2.lyp /out/
mkdir -p "$ROOT/apps/web/src/pdk"
python3 -I "$ROOT/scripts/gen-layer-stack.py" "$TMP/sg13g2.lyp" ihp-sg13g2 "$ROOT/apps/web/src/pdk/ihp-sg13g2.json"
