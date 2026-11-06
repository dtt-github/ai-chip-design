#!/usr/bin/env bash
# Installs the IHP SG13G2 PDK version pinned by the LibreLane image into a
# Docker volume, which job containers mount read-only at /pdk.
set -euo pipefail

IMAGE="${WAFER_LIBRELANE_IMAGE:-ghcr.io/librelane/librelane:3.0.16}"
VOLUME="${WAFER_PDK_VOLUME:-wafersmith_pdk}"
PDK="${WAFER_PDK:-ihp-sg13g2}"

docker image inspect "$IMAGE" >/dev/null 2>&1 || docker pull "$IMAGE"
docker volume create "$VOLUME" >/dev/null
docker run --rm -v "$VOLUME:/pdk" -e PDK_ROOT=/pdk -e PDK="$PDK" "$IMAGE" sh -c '
  set -e
  dir=$(python3 -c "import librelane, os; print(os.path.dirname(librelane.__file__))")
  rev=$(python3 -c "import sys; print(dict(l.split(\": \", 1) for l in open(sys.argv[1]).read().split(\"\\n\") if l).get(sys.argv[2], \"\").strip())" "$dir/pdk_hashes.yaml" "$PDK")
  [ -n "$rev" ] || { echo "no pinned version for $PDK in this LibreLane image" >&2; exit 1; }
  ciel enable --pdk-family "$PDK" "$rev"
'
echo "PDK $PDK ready in volume $VOLUME"
