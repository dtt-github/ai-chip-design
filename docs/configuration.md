# Configuration

The API (`siliconcommons-api`) is configured with environment variables. In development it also
reads `.env` in the working directory (copy `.env.example`). The pilot deployment sets them in
`deploy/pilot/docker-compose.yml` from `deploy/pilot/.env`.

## Server and storage

| Variable | Default | Description |
|---|---|---|
| `SC_BIND` | `127.0.0.1:8090` | Address and port the API listens on. |
| `DATABASE_URL` | (required) | Postgres connection string. Migrations run at start-up. |
| `SC_DATA_DIR` | `./data` | Project repositories (`repos/`), co-editing state (`collab/`) and flow runs (`jobs/`). In a container that starts flow runs on the host's Docker, it must have the same absolute path inside and outside. |
| `RUST_LOG` | `info` | Log filter, e.g. `info,sc_api=debug,tower_http=info`. |

## Sign-in (OIDC)

| Variable | Default | Description |
|---|---|---|
| `SC_OIDC_ISSUER` | (required) | The provider's issuer URL, e.g. `http://localhost:8180/auth/v1/`. Tokens must carry exactly this issuer. |
| `SC_OIDC_AUDIENCES` | (required) | Comma-separated client ids accepted as token audiences. Production: `siliconcommons-web` only. |
| `SC_OIDC_INTERNAL_URL` | unset | Where the server itself reaches the provider when that differs from the public issuer (e.g. `http://rauthy:8080/auth/v1/` inside Compose). Discovery, keys and userinfo URLs are rewritten to it. |

## Flow runs (RTL → GDS)

| Variable | Default | Description |
|---|---|---|
| `SC_DOCKER` | `docker` | Docker-compatible CLI used to start flow containers. |
| `SC_LIBRELANE_IMAGE` | `ghcr.io/librelane/librelane:3.0.16` | Flow image; its digest is recorded with every run. |
| `SC_PDK_VOLUME` | `siliconcommons_pdk` | Docker volume holding the PDKs, mounted read-only at `/pdk`. |
| `SC_JOB_WORKERS` | `2` | Flow runs at once. |
| `SC_JOB_CPUS` | `4` | CPUs per flow container. |
| `SC_JOB_MEMORY` | `8g` | Memory per flow container. |
| `SC_JOB_TIMEOUT_MIN` | `120` | A flow container is stopped after this many minutes. |
| `SC_JOB_RETENTION_DAYS` | unset (keep) | Run directories of jobs finished longer ago are deleted every 6 hours; the job record and its summary stay. |

Without Docker or the image, the API starts with flow runs disabled (job creation answers 503).

## Editor

| Variable | Default | Description |
|---|---|---|
| `SC_VERIBLE_LS` | `.tools/verible/bin/verible-verilog-ls` | Verible language server binary. If missing, the editor works without language features. |
| `SC_LSP_MAX_SESSIONS` | `32` | Language-server sessions at once, server-wide (each person may hold 3). |

## AI assistant and agent

| Variable | Default | Description |
|---|---|---|
| `ANTHROPIC_API_KEY` | unset | Claude Messages API key. Takes precedence over OpenRouter. |
| `OPENROUTER_API_KEY` | unset | OpenRouter key, used through its Anthropic-compatible endpoint (model `anthropic/claude-opus-5.5`). |
| `SC_ANTHROPIC_BASE_URL` | provider default | Override the API base URL (tests point it at a mock). |
| `SC_AGENT_MODEL` | `claude-opus-5-5` (OpenRouter: `anthropic/claude-opus-5.5`) | Model for the agent, the spec assistant and the connection review. |
| `SC_AGENT_EFFORT` | `high` | Reasoning effort: `low`, `medium`, `high`, `xhigh` or `max`. |
| `SC_AI_DAILY_USD` | `3` | Per-person daily AI budget in US dollars, priced from token usage; `0` turns the limit off. |

Without a key, the AI endpoints answer 503 and everything else works.

## Fixed limits

Not configurable today: 3 queued or running flow runs per person, 2 running agent proposals per
person, 3 in-process simulations at once, simulations up to 8M storage words and 100,000
instances, parser nesting up to 512 levels.

## Web app (build time)

| Variable | Default | Description |
|---|---|---|
| `VITE_OIDC_AUTHORITY` | `http://localhost:8180/auth/v1/` | The OIDC provider the browser signs in with. |
| `VITE_OIDC_CLIENT_ID` | `siliconcommons-web` | The browser's OIDC client. |
