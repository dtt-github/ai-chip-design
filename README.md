# SiliconCommons

Open, browser-based, physics-aware chip design platform. Design Verilog in the browser with a real
language server, keep every project in Git, run it from RTL to GDS with LibreLane on the IHP SG13G2
130 nm open PDK, and inspect the result in a WebGPU layout viewer with DRC markers and timing/area
reports.

Licensed under [Apache-2.0](LICENSE).

## Layout

```
crates/
  sc-api/      axum HTTP API (binary: siliconcommons-api), Postgres via sqlx, OIDC bearer auth
  sc-git/      per-project bare Git repositories (libgit2), commits without a worktree
  sc-jobs/     job backends: LibreLane in Docker, metrics summary, DRC/STA report parsing
  sc-gds/      GDSII reader: parse, flatten hierarchy, tessellate for the GPU (pure Rust)
  sc-gds-wasm/ WebAssembly bindings for sc-gds, used by the web viewer
apps/
  web/         React + TypeScript shell with the Monaco editor (Vite)
deploy/
  compose/     local infra: Postgres + Rauthy (Rust OIDC provider)
```

## Run it locally

Prerequisites: Rust (stable), Node 22 with pnpm (`corepack enable pnpm`), Docker, and for the
layout viewer's WebAssembly:

```bash
rustup target add wasm32-unknown-unknown
```

```bash
cargo install wasm-bindgen-cli --version 0.2.130 --locked
```

`pnpm dev:web` builds the WebAssembly on first start (`scripts/build-wasm.sh`, output in
`apps/web/src/wasm/`, not committed); rerun `pnpm --filter @siliconcommons/web wasm` after changing
`sc-gds`.

```bash
./scripts/fetch-verible.sh
```

```bash
./scripts/fetch-pdk.sh
```

`fetch-pdk.sh` pulls the LibreLane image (`ghcr.io/librelane/librelane:3.0.16`, about 7.5 GB) and
installs the IHP SG13G2 PDK version that image pins into the Docker volume `siliconcommons_pdk`.

```bash
docker compose -f deploy/compose/docker-compose.yml up -d
```

```bash
cp .env.example .env
```

```bash
cargo run -p sc-api
```

```bash
pnpm install && pnpm dev:web
```

Open http://localhost:5173 and sign in.

| Service  | URL                          | Notes                                   |
| -------- | ---------------------------- | --------------------------------------- |
| Web      | http://localhost:5173        | Vite dev server, proxies `/api` to 8090 |
| API      | http://localhost:8090        | `GET /healthz`                          |
| Rauthy   | http://localhost:8180/auth/v1 | admin UI at `/auth/v1/admin`            |
| Postgres | localhost:5442               | `sc` / `sc`, database `siliconcommons`  |

Dev accounts (bootstrapped from `deploy/compose/rauthy/bootstrap/`, **local only**):

- `dev@siliconcommons.dev` and `dev2@siliconcommons.dev`, password `SiliconCommons-dev-2026`
- Rauthy admin `admin@localhost`, password `SiliconCommons-admin-2026`

Rauthy bootstraps only into an empty database. After changing the bootstrap files, reset it with
`docker compose -f deploy/compose/docker-compose.yml down` and `docker volume rm siliconcommons_rauthy-data`.

### Get a token from the command line

The `siliconcommons-dev-cli` client allows the password grant (dev only), which is handy for
`curl`:

```bash
curl -s -X POST localhost:8180/auth/v1/oidc/token -d grant_type=password -d client_id=siliconcommons-dev-cli -d client_secret=scdevcliscdevcliscdevcliscdevcliscdevcliscdevcliscdevcliscdevcli -d username=dev@siliconcommons.dev -d password=SiliconCommons-dev-2026
```

## Tests

```bash
DATABASE_URL=postgres://sc:sc@127.0.0.1:5442/siliconcommons cargo test --workspace
```

The real LibreLane runs (about a minute) are opt-in:

```bash
cargo test -p sc-jobs --test docker -- --ignored
```

The API tests create a throwaway database per test (`sqlx::test`) and sign their own tokens, so
Rauthy does not need to be running.

## API (Sprint 1)

All `/api` routes need `Authorization: Bearer <access token>`.

| Method | Path                               | Purpose                                          |
| ------ | ---------------------------------- | ------------------------------------------------ |
| GET    | `/api/me`                          | Current user (created on first request)          |
| GET    | `/api/projects`                    | Projects you are a member of                     |
| POST   | `/api/projects`                    | `{name, slug?, description?, visibility?}`       |
| GET    | `/api/projects/{id}`               | Project, your role, head commit                  |
| GET    | `/api/projects/{id}/tree?rev=`     | Files at a revision (default: `main`)            |
| GET    | `/api/projects/{id}/files/{path}`  | File content at a revision                       |
| GET    | `/api/projects/{id}/commits`       | History of `main`                                |
| POST   | `/api/projects/{id}/commits`       | `{message, parent?, changes:[{op,path,content?}]}` |
| GET    | `/api/projects/{id}/members`       | Members and roles                                |
| POST   | `/api/projects/{id}/members`       | `{email, role}` (owner only)                     |
| GET    | `/api/projects/{id}/lsp`           | WebSocket to the Verilog language server         |
| POST   | `/api/projects/{id}/jobs`          | Queue an RTL→GDS run `{commit?, config?}`        |
| GET    | `/api/projects/{id}/jobs`          | Recent runs                                      |
| GET    | `/api/jobs/{id}`                   | Run status, error excerpt, sign-off summary      |
| GET    | `/api/jobs/{id}/log?offset=`       | Log from `offset`; poll until `done`             |
| POST   | `/api/jobs/{id}/cancel`            | Cancel a queued or running run (editor)          |
| GET    | `/api/jobs/{id}/artifacts`         | Files in the run's `final/` directory            |
| GET    | `/api/jobs/{id}/artifacts/{path}`  | Download one (GDS, DEF, netlists, render, ...)   |
| GET    | `/api/jobs/{id}/drc`               | DRC violations with geometry (KLayout, Magic, router) |
| GET    | `/api/jobs/{id}/report`            | Timing per corner, worst paths, area by cell class |

Commits are optimistic: pass the `parent` you edited against and you get `409` if `main` moved.
Roles are `owner` > `editor` > `viewer`; public projects are readable by any signed-in user.

### Flow runs

"Run RTL → GDS" queues a job for the last commit. Jobs are rows in Postgres, claimed by workers
inside the API (`FOR UPDATE SKIP LOCKED`, `SC_JOB_WORKERS`, default 2). Each run:

- exports the project at the job's commit to `data/jobs/<job id>/work/`;
- runs `librelane --manual-pdk --pdk ihp-sg13g2 --run-tag sc config.json` in a throwaway
  container with no network, the PDK volume mounted read-only, CPU/memory/PID limits
  (`SC_JOB_CPUS`, `SC_JOB_MEMORY`) and the API's own uid;
- streams output to `data/jobs/<job id>/flow.log`;
- records the image digest, the commit, the exit code, the flow's ERROR lines and a summary of
  `metrics.json` (area, cells, timing, power, IR drop, DRC/LVS/antenna) with a sign-off verdict.

The web app shows the live log, the summary, the KLayout render and downloads for every file in
`final/`. On startup the API marks runs interrupted by a restart as failed and kills their
containers. Artifacts stay on local disk for now; object storage is a later backend.

### Layout viewer

The Layout tab of a run opens the run's GDS in the browser:

- `sc-gds` (Rust, compiled to WebAssembly) parses GDSII, flattens SREF/AREF hierarchy (rotation,
  reflection, magnification), tessellates polygons and paths once per cell, and returns one triangle
  buffer and one outline buffer per layer in µm. The UART example (78 instances, 20k triangles)
  parses in a few milliseconds.
- WebGPU draws fills and outlines per layer with the PDK's colours and stack order; browsers
  without a WebGPU adapter get a Canvas 2D renderer with the same features.
- Layer names, colours, default visibility and order come from the PDK's own KLayout layer file
  (`sg13g2.lyp`, Apache-2.0), converted by `scripts/gen-layer-stack.sh` into
  `apps/web/src/pdk/ihp-sg13g2.json`. Hollow KLayout layers (boundaries) are drawn as outlines.
- Pan by dragging, zoom with the wheel, double-click or `F` to fit; pin labels appear when zoomed
  in.
- The DRC tab lists violations from the KLayout, Magic and router DRC reports (all in KLayout's
  report-database format, with sub-cell items mapped to the top cell), grouped by rule with the PDK's
  rule text. Clicking one zooms to it.

The Report tab shows setup/hold slack and TNS per corner, the worst setup and hold paths
stage by stage (from OpenSTA's reports), and area by cell class.

### Verilog language server

The editor gets diagnostics (syntax and lint), hover, completion, go to definition across project
files, references, outline, formatting, quick fixes and rename from
[Verible](https://github.com/chipsalliance/verible)'s `verible-verilog-ls`.

`/api/projects/{id}/lsp` is a WebSocket. The client first sends `{"token": "<access token>"}`, gets
`{"type":"ready"}`, then exchanges one JSON-RPC message per text frame. Each session runs its own
language server over a temporary copy of the project (with a `verible.filelist`). Documents are
named `file:///<project id>/<path>`, and the bridge drops any message that names another `file:`
URI. `SC_VERIBLE_LS` sets the binary (default `.tools/verible/bin/verible-verilog-ls`), and
`SC_LSP_MAX_SESSIONS` caps concurrent sessions (default 32).

## Where this differs from the plan

- **Rust everywhere on the server**: axum instead of FastAPI; Rauthy instead of Keycloak.
- **Git**: bare repos on disk via libgit2, behind `sc-git`. Forgejo can come later for hosted
  Git remotes and pull requests; nothing in the API depends on it.
- **Jobs**: LibreLane runs in local Docker containers behind the `sc_jobs::Backend` trait;
  Kubernetes is a later backend. Temporal is deferred: job state lives in Postgres.
- **Results**: on local disk under `data/jobs/`, not object storage yet.
- **Layout viewer**: flattens the whole layout in the browser, which is fine for blocks up to a few
  million triangles. Large SoCs will need tiling and per-cell instancing on the GPU.
- **Language server transport**: a Rust WebSocket bridge (`sc-api/src/lsp.rs`) and a small
  client in `apps/web/src/lsp.ts`, instead of Monaco's bundled LSP client, which cannot be torn
  down when you switch projects.
