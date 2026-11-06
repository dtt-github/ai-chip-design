#!/usr/bin/env bash
# Restores a backup made by backup.sh, e.g. onto a fresh host after losing the old one.
#   ./restore.sh /srv/wafersmith/backups/<timestamp>
# Needs the same .env and rauthy/ as the backed-up install (keep copies of them off the host).
# Replaces the database, the project repositories and sign-in with the backup's; flow run
# outputs are not in backups (run the flows again).
# Set COMPOSE to use other Compose files or a project name, e.g.
#   COMPOSE="docker compose -f docker-compose.yml -f override.yml -p myname" ./restore.sh ...
set -euo pipefail
cd "$(dirname "$0")"
[[ $# -eq 1 && -d "$1" ]] || { echo "usage: $0 <backup folder>" >&2; exit 1; }
B="$(cd "$1" && pwd)"
COMPOSE=${COMPOSE:-docker compose}
set -a
# shellcheck disable=SC1091
source .env
set +a

echo "Checking $B"
docker run --rm -v "$B:/b:ro" alpine:3 sh -c 'cd /b && sha256sum -c SHA256SUMS'
NAME=$($COMPOSE config 2>/dev/null | awk '/^name:/ {print $2; exit}')

echo "Stopping the app"
$COMPOSE stop caddy api rauthy backup >/dev/null 2>&1 || true

echo "Database"
$COMPOSE up -d --wait postgres
$COMPOSE exec -T postgres pg_restore -U wafer -d wafersmith --clean --if-exists --no-owner < "$B/postgres.dump"

echo "Project repositories and co-editing state"
mkdir -p "$WAFER_DATA_DIR"
docker run --rm -v "$WAFER_DATA_DIR:/d" -v "$B:/b:ro" alpine:3 \
    sh -c 'rm -rf /d/repos /d/collab && tar -C /d -xzf /b/repos-collab.tar.gz && chown -R 10001:10001 /d'

if [[ -f "$B/rauthy.sqlite.gz" ]]; then
    echo "Sign-in (Rauthy)"
    $COMPOSE rm -sf rauthy >/dev/null 2>&1 || true
    $COMPOSE create rauthy >/dev/null 2>&1
    docker run --rm -v "${NAME}_rauthy-data:/d" -v "$B:/b:ro" alpine:3 \
        sh -c 'mkdir -p /d/restore && gunzip -c /b/rauthy.sqlite.gz > /d/restore/rauthy.sqlite && chown -R 10001:10001 /d'
    # Rauthy restores from a file given once at start-up, then runs normally.
    $COMPOSE run -d --no-deps --name "${NAME}-rauthy-restore" \
        -e HQL_BACKUP_RESTORE=file:/app/data/restore/rauthy.sqlite rauthy >/dev/null
    for _ in $(seq 1 60); do
        if docker logs "${NAME}-rauthy-restore" 2>&1 | grep -qiE 'restore.*(finished|success)|database initialized'; then break; fi
        sleep 2
    done
    docker logs "${NAME}-rauthy-restore" 2>&1 | grep -iE 'restor' | tail -3 || true
    docker rm -f "${NAME}-rauthy-restore" >/dev/null
    docker run --rm -v "${NAME}_rauthy-data:/d" alpine:3 rm -rf /d/restore
else
    echo "warning: this backup has no Rauthy snapshot; sign-in starts empty" >&2
fi

echo "Starting everything"
$COMPOSE up -d
echo "Restored from $B"
