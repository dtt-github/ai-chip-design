#!/bin/sh
# Backs up everything a pilot needs to come back after losing the host:
#   - the database (projects, members, runs, proposals, IP registry, AI usage): pg_dump
#   - the project Git repositories and saved co-editing documents: data/repos, data/collab
#   - sign-in (users, clients, keys): the newest of Rauthy's own nightly database snapshots
# Flow run outputs (data/jobs) are left out: they can be rebuilt by running the flow again.
#
#   backup.sh            one backup now
#   backup.sh --daily    run forever, one backup a day at BACKUP_HOUR_UTC (the Compose service)
#
# Each backup is one folder /backups/<UTC timestamp>/ with a SHA256SUMS file; folders older
# than BACKUP_KEEP_DAYS are deleted. Restore steps: README.md in this directory.
set -eu

KEEP_DAYS="${BACKUP_KEEP_DAYS:-14}"

backup_once() {
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    dir="/backups/$stamp"
    tmp="/backups/.partial-$stamp"
    mkdir -p "$tmp"
    pg_dump --format=custom --file="$tmp/postgres.dump"
    tar -C /data -czf "$tmp/repos-collab.tar.gz" repos collab
    # Rauthy writes a consistent SQLite snapshot every night; take the newest.
    latest=$(ls -t /rauthy/state_machine/backups/*.sqlite 2>/dev/null | head -n 1 || true)
    if [ -n "$latest" ]; then
        gzip -c "$latest" > "$tmp/rauthy.sqlite.gz"
    else
        echo "warning: no Rauthy snapshot yet (Rauthy makes its first one at 02:30 UTC)" >&2
    fi
    (cd "$tmp" && sha256sum ./* > SHA256SUMS)
    mv "$tmp" "$dir"
    echo "backup written: $dir ($(du -sh "$dir" | cut -f1))"
    find /backups -mindepth 1 -maxdepth 1 -type d -name '20*' -mtime +"$KEEP_DAYS" -exec rm -rf {} +
}

if [ "${1:-}" = "--daily" ]; then
    hour="${BACKUP_HOUR_UTC:-2}"
    while true; do
        now=$(date -u +%s)
        next=$(date -u -d "@$(( (now / 86400) * 86400 + hour * 3600 ))" +%s 2>/dev/null || echo 0)
        [ "$next" -le "$now" ] && next=$((next + 86400))
        sleep $((next - now))
        backup_once || echo "backup failed" >&2
    done
else
    backup_once
fi
