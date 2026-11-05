# SiliconCommons

Open, browser-based, physics-aware chip design platform. Edit Verilog together in real time with a
real language server, simulate it in the browser, keep every project in Git, run it from RTL to GDS
with LibreLane on the IHP SG13G2 130 nm open PDK, and inspect the result in a WebGPU layout viewer
with DRC markers and timing/area reports. An AI agent proposes RTL changes that can only be merged
when the simulation gate passes, with an evidence log for every proposal.

**Phase 0 gate passed:** a UART goes from an empty project to a sign-off-clean GDS in 4 min 30 s
while two people co-edit ([docs/phase0-gate.md](docs/phase0-gate.md)).

Licensed under [Apache-2.0](LICENSE).

## Layout

```
crates/
  sc-api/      axum HTTP API (binary: siliconcommons-api), Postgres via sqlx, OIDC bearer auth
  sc-git/      per-project bare Git repositories (libgit2), commits without a worktree
  sc-jobs/     job backends: LibreLane in Docker, metrics summary, DRC/STA report parsing
  sc-gds/      GDSII reader: parse, flatten hierarchy, tessellate for the GPU (pure Rust)
  sc-gds-wasm/ WebAssembly bindings for sc-gds, used by the web viewer
  sc-sim/      event-driven Verilog simulator (pure Rust), checked against Icarus Verilog
  sc-sim-wasm/ WebAssembly bindings for sc-sim, run in a Web Worker
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

## Deploying a pilot

`deploy/pilot/` runs everything on one host behind HTTPS (Caddy, the API, Postgres, Rauthy with a
production config, nightly backups with a tested `restore.sh`). Start with
`deploy/pilot/README.md`; `docs/security-review.md` lists what was reviewed, fixed and accepted.
Per-person limits: an AI budget per day (`SC_AI_DAILY_USD`), 3 active flow runs, 2 running agent
proposals, 3 language-server sessions; flow runs stop after `SC_JOB_TIMEOUT_MIN` and their
directories are deleted after `SC_JOB_RETENTION_DAYS`.

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
| GET    | `/api/projects/{id}/snapshot?rev=` | All text files at a revision (the working copy's base) |
| GET    | `/api/projects/{id}/lsp`           | WebSocket to the Verilog language server         |
| GET    | `/api/projects/{id}/collab`        | WebSocket for real-time co-editing (y-protocols) |
| POST   | `/api/projects/{id}/jobs`          | Queue an RTL→GDS run `{commit?, config?}`        |
| GET    | `/api/projects/{id}/jobs`          | Recent runs                                      |
| GET    | `/api/projects/{id}/spec`          | The spec, whether its testbenches are in sync    |
| PUT    | `/api/projects/{id}/spec`          | Save the spec and regenerate example testbenches |
| POST   | `/api/projects/{id}/spec/run`      | Run every testbench; results per example         |
| POST   | `/api/projects/{id}/spec/draft`    | Assistant drafts or revises the spec (unsaved)   |
| GET    | `/api/ip`                          | IP blocks you can see, with their versions       |
| POST   | `/api/projects/{id}/ip`            | Publish this project as a block version (editor) |
| GET    | `/api/projects/{id}/ip/check`      | Licence verdict and plan for an import           |
| POST   | `/api/projects/{id}/ip/import`     | Import a block version, pinned (editor)          |
| GET    | `/api/projects/{id}/ip/imports`    | `ip.lock.json` with licences re-checked          |
| POST   | `/api/projects/{id}/agent`         | Ask the agent for a change `{request}` (editor)  |
| GET    | `/api/projects/{id}/agent`         | Recent proposals                                 |
| GET    | `/api/agent/{id}`                  | Proposal: files, base content, gate, evidence    |
| POST   | `/api/agent/{id}/merge`            | Merge (editor; gate must pass, base must be head) |
| POST   | `/api/agent/{id}/regate`           | Re-run the gate on the recorded inputs           |
| POST   | `/api/agent/{id}/discard`          | Discard a finished proposal                      |
| GET    | `/api/jobs/{id}`                   | Run status, error excerpt, sign-off summary, gate-level sim |
| GET    | `/api/jobs/{id}/log?offset=`       | Log from `offset`; poll until `done`             |
| POST   | `/api/jobs/{id}/cancel`            | Cancel a queued or running run (editor)          |
| GET    | `/api/jobs/{id}/artifacts`         | Files in the run's `final/` directory            |
| GET    | `/api/jobs/{id}/artifacts/{path}`  | Download one (GDS, DEF, netlists, render, ...)   |
| GET    | `/api/jobs/{id}/drc`               | DRC violations with geometry (KLayout, Magic, router) |
| GET    | `/api/jobs/{id}/report`            | Timing per corner, worst paths, area by cell class |

Commits are optimistic: pass the `parent` you edited against and you get `409` if `main` moved.
Roles are `owner` > `editor` > `viewer`; public projects are readable by any signed-in user.

### Design view: describe, examples, health

Each project opens in **Design** view, for engineers who don't write Verilog; **Code** is the file
tree and editor for those who do. Both work on the same files and Git history.

- **Describe**: what the design should do in plain words. "Draft the spec with the assistant"
  (`POST /api/projects/{id}/spec/draft {instruction, spec?}`) has the model fill in connections
  (with plain-language meanings), clock and reset, settings, examples and open questions; answering
  a question or asking for a change goes through the same call. The server checks the model's spec
  and sends problems back until it is valid (strict tool schema, at most 4 turns). Nothing is saved
  until the person clicks Save.
- **Examples are the tests**: `spec.json` holds the spec; each example is a table of steps (set an
  input, wait N cycles, wait until, expect, expect for N cycles) that `sc-api/src/spec.rs` compiles,
  deterministically, into `tb/examples/tb_ex_<id>.v`. Saving (`PUT /api/projects/{id}/spec
  {spec, parent}`) commits the spec and the regenerated testbenches together and removes those of
  deleted examples (only files carrying the generated header). Because they are ordinary
  testbenches, the simulation gate, the browser simulator and gate-level simulation all run them.
  `POST .../spec/run` reports each example's result; "Ask the AI to make them pass" opens an agent
  proposal with the failures. The agent may not write `spec.json` or `tb/examples/`.
- **What actually happened**: selecting an example runs its testbench in the browser simulator
  (continuing 20 cycles past the end) and replays the steps against the result exactly as the
  testbench does (`apps/web/src/lib/replay.ts`): each connection per clock cycle, the checks laid
  over it, and the first difference in plain words, including when the expected value did appear
  ("one cycle later, so the design may be one cycle slower than the example says"). Two buttons
  answer the only question a non-expert has to decide: the design is wrong (an agent proposal with
  that explanation) or the example is wrong (the assistant re-checks and corrects it).
- **Health**: the latest default build in plain language (works, fast enough with the margin as
  "about N× faster", can be manufactured, size and power), each with "What does this mean?" and
  the expert numbers, plus what still needs an expert before a real chip run.

### IP registry

The IP tab publishes a project as a reusable block and imports blocks from other projects
(`sc-api/src/ip.rs`).

- **Publish** (`POST /api/projects/{id}/ip {version, name?, description?, commit?, top?}`, editor):
  takes the top module (`DESIGN_NAME` in `config.json`) and every module it instantiates, with
  their files, at one commit (testbenches stay out). Every published file needs an
  `SPDX-License-Identifier` header; they combine into the block's licence (`A AND B`). The
  project's simulation gate must pass at that commit and is stored with the version, together
  with the ports, parameters and each file's sha256. Versions are semver, immutable and must
  increase; a block name belongs to the project that first published it. Blocks are visible to
  whoever can see the source project.
- **Import** (`POST /api/projects/{id}/ip/import {block, version, accept_licence?}`, editor;
  `GET .../ip/check?block=&version=` shows the same plan without doing it): copies the files, after
  checking them against their published hashes, to `ip/<block>/`, adds them to `VERILOG_FILES` in
  the project's `config*.json` files, and pins block, version, source commit, licence and file
  hashes in `ip.lock.json`, in one commit by the person importing. Importing another version
  replaces the old files and pin. Module names that would clash with the project's are refused.
- **Licence check** (`sc-api/src/licence.rs`): every project declares an SPDX licence (chosen at
  creation, default Apache-2.0, written into the starter files). The block's SPDX expression is
  judged against it: permissive licences are compatible (Apache-2.0 into GPL-2.0-only is not);
  file-level copyleft (CERN-OHL-W, MPL, LGPL) is compatible with conditions; strong copyleft
  (GPL, AGPL, CERN-OHL-S) only goes into a project under a licence that satisfies it. `OR` takes
  the best option, `AND` the worst. Incompatible imports are refused; a licence the check doesn't
  know can only be imported by a project owner who accepts it explicitly (recorded in the lock
  file). The obligations (notices, keeping copyleft files under their licence) are listed before
  importing and written into the commit message. It is an automated check over common licences,
  not legal advice.

### AI agent with a simulation gate

The Agent tab sends a change request to an agent running in the API (`sc-api/src/agent.rs`). It
calls the Claude Messages API over HTTP (`claude-opus-5-5`, adaptive thinking with summarised
reasoning, effort `high`, prompt caching, server-side refusal fallback `fallbacks: "default"`) and
works on a private copy of the project at the current commit through four tools: `list_files`,
`read_file`, `write_file` and `run_simulation` (the same `sc-sim` engine as the browser). It cannot
commit. It finishes by calling `submit_proposal` with a commit title and a summary for the reviewer
(strict schema, so the title is a field, not parsed out of prose); an agent that ends with plain text
instead gets the first line of the request as its title.

When it finishes, the server runs the **simulation gate** (`sc-api/src/gate.rs`) on the tree the
proposal would produce: every testbench (top-level `tb*`, `*_tb`, `*test*` modules) must reach
`$finish`, print `PASS` and print no `FAIL`/`ERROR`/`MISMATCH`. A person with the editor role can
merge only a proposal whose gate passed and whose base is still the branch tip; the gate runs again
at merge time on the exact files committed. The commit is authored by the person who merged, is
titled with the agent's commit title (at most 72 characters), and records the agent's summary, the
proposal, model, gate result and evidence hash.

Every proposal keeps an **evidence log**: the request and requester, base commit, model requested
and served, effort, a hash of the agent's instructions, its reasoning summaries and messages, every
tool call with its input (file hashes, not contents) and result, token usage, the changed files'
hashes, and the gate report with the hash of every simulated file. A sha256 over the whole log goes
into the merge commit, and "Re-run gate" checks that the recorded result reproduces.

Set `ANTHROPIC_API_KEY` to enable it, or `OPENROUTER_API_KEY` to go through OpenRouter's
Anthropic-compatible endpoint (`anthropic/claude-opus-5.5`, bearer auth, no server-side refusal
fallback). `SC_AGENT_MODEL`, `SC_AGENT_EFFORT` and `SC_ANTHROPIC_BASE_URL` are optional; without a
key the endpoint answers 503.

### Co-editing

Everyone who opens a project edits one shared working copy, a Yjs document kept by the API
(`yrs`, `sc-api/src/collab.rs`): `files` (path → `Y.Text`), staged `deleted` paths and `meta.head`,
the commit it is based on. The WebSocket speaks the standard y-protocols sync and awareness messages
after a first `{"token": ...}` message, so Monaco gets shared editing and remote cursors through
`y-monaco`. Viewers receive updates but their edits are dropped by the server. Rooms are seeded from
the branch tip, saved to `data/collab/<project>.ydoc` and reloaded on restart; after every commit
the room is reconciled with Git (files nobody edited take the committed content, edited ones stay as
changes). The Changes panel compares the shared copy with `GET /snapshot`; a commit sends whatever
is in the shared copy, so it includes everyone's edits.

### Browser simulation

**▶ Simulate** runs the project's Verilog (the shared working copy, uncommitted edits included) in
a Web Worker with `sc-sim` compiled to WebAssembly, picks the testbench as top (modules nothing
instantiates, `tb*` names first), and shows the `$display` output and waveforms (zoom, pan,
cursor values, radix, signal picker) with a VCD download.

`sc-sim` is an event-driven simulator with Verilog's active/NBA/postponed regions. It supports
modules, parameters and hierarchy, `wire`/`reg`/`logic`/`integer`, memories, continuous and
procedural assignments, `if`/`case`/`casez`/loops/`wait`, `#` delays and `@` events,
`$display`/`$write`/`$strobe`/`$monitor`/`$finish`, `$time`, `$signed`, `$clog2`. It is 2-state
(x/z read as 0) and limited to 64-bit signals; functions, tasks and `generate` are not supported
yet and are reported with file and line. Every testbench in `crates/sc-sim/tests/conformance/`
must print exactly what Icarus Verilog prints (`scripts/gen-sim-expected.sh` regenerates the
references). Natively it runs a 1M-cycle counter testbench in about 0.5 s
(`cargo run -p sc-sim --release --example bench`); `cargo run -p sc-sim --example run -- *.v` is a
command-line runner.

### Flow runs

"Run RTL → GDS" queues a job for the last commit, with the flow config picked next to the button
(any `config*.json`/`.yaml`/`.tcl` in the project; `config.json` first). Jobs are rows in Postgres, claimed by workers
inside the API (`FOR UPDATE SKIP LOCKED`, `SC_JOB_WORKERS`, default 2). Each run:

- exports the project at the job's commit to `data/jobs/<job id>/work/`;
- runs `librelane --manual-pdk --pdk ihp-sg13g2 --run-tag sc <config>` in a throwaway
  container with no network, the PDK volume mounted read-only, CPU/memory/PID limits
  (`SC_JOB_CPUS`, `SC_JOB_MEMORY`) and the API's own uid;
- streams output to `data/jobs/<job id>/flow.log`;
- records the image digest, the commit, the exit code, the flow's ERROR lines and a summary of
  `metrics.json` (area, cells, timing, power, IR drop, DRC/LVS/antenna) with a sign-off verdict;
- after a successful flow, runs **gate-level simulation** (`sc-api/src/gls.rs`): the project's
  testbenches again, on the synthesised netlist, with Icarus Verilog and the PDK's cell models
  (`CELL_VERILOG_MODELS`, zero-delay) in the same container. Verdicts use the gate's rules (reach
  `$finish`, print `PASS`, no `FAIL`/`ERROR`/`MISMATCH`). The netlist is built for one parameter set
  (module defaults plus the config's `SYNTH_PARAMETERS`), so a testbench runs only if it
  instantiates the design with exactly those values; the others are listed as skipped with the
  difference. The netlist gets the build's values as dummy parameters so `#(...)` overrides
  elaborate. Results go in the job's `gls` field and the Summary tab; a failure does not change the
  job status or the physical sign-off verdict, but shows as "netlist sim fail".

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
- **Simulator**: our own Rust simulator instead of a WASM build of Icarus Verilog or Verilator
  (no trustworthy WASM builds exist, and both depend on dynamic loading). It covers RTL and common
  testbench code, not full Verilog; heavier simulations can later run as a server job with
  Icarus/Verilator from the LibreLane image.
- **Language server transport**: a Rust WebSocket bridge (`sc-api/src/lsp.rs`) and a small
  client in `apps/web/src/lsp.ts`, instead of Monaco's bundled LSP client, which cannot be torn
  down when you switch projects.
