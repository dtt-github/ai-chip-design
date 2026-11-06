#!/usr/bin/env bash
# Regenerates crates/wafer-sim/tests/conformance/*.out with Icarus Verilog (from the
# LibreLane image), the reference the browser simulator is tested against.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${WAFER_LIBRELANE_IMAGE:-ghcr.io/librelane/librelane:3.0.16}"
docker run --rm --network none --user "$(id -u):$(id -g)" -e HOME=/tmp \
  -v "$ROOT/crates/wafer-sim/tests/conformance:/c" "$IMAGE" sh -c '
  set -e
  for f in /c/*.v; do
    n=$(basename "$f" .v)
    iverilog -g2012 -o /tmp/$n.vvp "$f"
    # Drop simulator chatter: "$finish called at ..." and VCD notes.
    vvp -n /tmp/$n.vvp | grep -v -e "\$finish called at" -e "^VCD info" > /c/$n.out
    echo "$n: $(wc -l < /c/$n.out) lines"
  done'
