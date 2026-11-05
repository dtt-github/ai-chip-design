# SiliconCommons pilot deployment

One Linux host runs everything with Docker Compose: Caddy (HTTPS, the web app), the API, Postgres,
Rauthy (sign-in) and a nightly backup. Flow runs start LibreLane containers on the host's Docker.

## What you need

- A Linux host (x86-64 or arm64) with Docker Engine and the Compose plugin. Suggested: 8+ cores,
  32 GB RAM, 200 GB disk (each flow run uses 4 cores and up to 8 GB; two run at once).
- A DNS name pointing at the host, with ports 80 and 443 open to the internet (Let's Encrypt).
- An OpenRouter (or Anthropic) API key for the assistant and agent.
- This repository on the host.

## Install

```bash
cd deploy/pilot
./setup.sh                # first run: creates .env with fresh secrets, then stops
$EDITOR .env              # set SC_DOMAIN, ACME_EMAIL, ADMIN_EMAIL, OPENROUTER_API_KEY
./setup.sh                # writes the sign-in config, data directories, pulls LibreLane + PDK
docker compose up -d --build
```

The first build takes a while (Rust and WebAssembly). Then:

1. Open `https://<SC_DOMAIN>/auth/v1/admin` and sign in as `ADMIN_EMAIL` with
   `RAUTHY_ADMIN_PASSWORD` from `.env`. Set a new password and MFA (required for admins).
2. **Invite people**: in the Rauthy admin UI, *Users → Add user* with their email. Without SMTP
   configured, use the user's *Password reset* link from the admin UI and send it to them yourself.
   (To have Rauthy email invites, add `SMTP_URL`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`
   to the `rauthy` service.) Self-registration is off.
3. They sign in at `https://<SC_DOMAIN>/` and start from one of the starter ideas.

`.env` and `rauthy/` hold secrets: keep them readable by root only and out of Git (both are
ignored). Don't change the generated secrets after the first start.

## Limits you can tune (`.env`)

| Variable | Default | |
|---|---|---|
| `SC_AI_DAILY_USD` | 3 | AI budget per person per day (assistant + agent), US dollars. 0 = no limit. |
| `SC_JOB_WORKERS` | 2 | Flow runs at once. |
| `SC_JOB_CPUS`, `SC_JOB_MEMORY` | 4, 8g | Per flow run. |
| `SC_JOB_TIMEOUT_MIN` | 120 | A flow run is stopped after this. |
| `SC_JOB_RETENTION_DAYS` | 30 | Run directories (layouts, logs) are deleted after this; summaries stay. |
| `BACKUP_KEEP_DAYS` | 14 | Backups kept. |

Fixed limits: 3 queued or running flow runs per person, 2 running agent proposals per person, 3
in-process simulations at once, 3 language-server sessions per person.

## Backups

The `backup` service writes one folder per night (02:00 UTC) to `SC_BACKUP_DIR`:
`postgres.dump`, `repos-collab.tar.gz` (the projects' Git repositories and co-editing state),
`rauthy.sqlite.gz` (the newest of Rauthy's nightly snapshots: users, passwords, keys) and
`SHA256SUMS`. Flow run outputs are not backed up; run the flow again. Copy `SC_BACKUP_DIR` off the
host (e.g. `rsync` to another machine or object storage): a backup on the same disk doesn't survive
losing the disk. Keep a copy of `.env` and `rauthy/` somewhere safe too.

Backup now: `docker compose run --rm --entrypoint /bin/sh backup /backup.sh`.

Rauthy makes its first snapshot at 02:30 UTC, so a backup taken before the first night has no
sign-in data (the script warns).

### Restore (tested)

On the same host or a fresh one, with the same `.env` and `rauthy/` in place:

```bash
./restore.sh /srv/siliconcommons/backups/<timestamp>
```

It checks the checksums, stops the app, restores the database, the repositories and Rauthy (which
restores its database from the snapshot on a one-off start), then starts everything. Tested by
deleting every volume and the data directory of a running install and restoring: people signed in
with their old passwords and found their projects.

## Updating

```bash
git pull
docker compose up -d --build
```

Database migrations run when the API starts. Running flow runs are marked failed if the API
restarts during them.

## Security notes

Read `docs/security-review.md`. In short, for this pilot:

- The API starts flow containers through the host's Docker socket: whoever controls the API
  process controls the host. Keep the host for SiliconCommons only, and keep invitations to
  people you know.
- Flow containers have no network, no capabilities, CPU/memory/PID and time limits.
- The AI provider receives project files as context when people use the assistant or the agent.
- Watch disk space (`df -h`, `du -sh $SC_DATA_DIR/*`); there are no per-project quotas yet.

## Dry run on your own machine

To try the stack without a public domain, use Caddy's internal certificate authority: copy the
`Caddyfile`, add `tls internal` under the site line, set `SC_DOMAIN=sc.localhost:8443`, and use a
Compose override that publishes `8443:8443` and mounts the modified Caddyfile. Browsers will warn
about the certificate.
