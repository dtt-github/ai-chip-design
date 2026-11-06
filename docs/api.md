# API reference

The web app uses this API; scripts can too. Every path below is under `/api` and needs
`Authorization: Bearer <access token>` from the OIDC provider ([getting-started.md](getting-started.md)
shows how to get one locally). `GET /healthz` (outside `/api`) needs no token.

## Conventions

- **Roles** per project: `owner` > `editor` > `viewer`. Public projects can be read by any
  signed-in user. Endpoints below say which role they need; reading needs `viewer`.
- **Not found vs forbidden**: a private project you can't see answers `404`, never `403`.
- **Optimistic writes**: commits and saves take the `parent` commit you edited against and answer
  `409` if `main` has moved since.
- **Errors** are JSON: `{"error": "<code>", "message": "<plain-language explanation>"}` with codes
  `bad_request` (400), `unauthorized` (401), `forbidden` (403), `not_found` (404), `conflict`
  (409), `payload_too_large` (413), `limit` (429: a per-person limit, e.g. the daily AI budget),
  `unavailable` (503: a feature not configured on this server), `internal` (500, details only in
  the server log).
- **Limits**: request bodies up to 32 MB; see [configuration.md](configuration.md#fixed-limits).

## Account

| Method | Path | Description |
|---|---|---|
| GET | `/me` | The current user (created on the first request). |
| GET | `/me/ai-usage` | `{today_usd, limit_usd}`: today's AI spend (UTC day) and the daily limit (`null` = none). |

## Projects, files and history

| Method | Path | Role | Description |
|---|---|---|---|
| GET | `/projects` | | Projects you are a member of. |
| POST | `/projects` | | Create `{name, slug?, description?, visibility?, licence?, starter?}`: from the starter design `starter` (`GET /starters`), or the counter template; `licence` is an SPDX id (default `Apache-2.0`). |
| GET | `/projects/{id}` | viewer | Project, your role, head commit. |
| GET | `/projects/{id}/tree?rev=` | viewer | Files at a revision (default: `main`). |
| GET | `/projects/{id}/files/{path}?rev=` | viewer | One file's content. |
| GET | `/projects/{id}/snapshot?rev=` | viewer | All text files at a revision. |
| GET | `/projects/{id}/commits` | viewer | History of `main`. |
| POST | `/projects/{id}/commits` | editor | `{message, parent?, changes: [{op: "write"\|"delete", path, content?}]}`. |
| GET | `/projects/{id}/members` | viewer | Members and roles. |
| POST | `/projects/{id}/members` | owner | `{email, role}` adds or changes a member (not the owner). |
| DELETE | `/projects/{id}/members/{user}` | owner | Removes a member and closes their co-editing sessions. |

## Design view: spec and examples

| Method | Path | Role | Description |
|---|---|---|---|
| GET | `/projects/{id}/spec` | viewer | `spec.json`, its problems, whether the example testbenches are in sync, the chip (if any), defaults for a new spec. |
| PUT | `/projects/{id}/spec` | editor | `{spec, parent, message?}`: commits the spec and the regenerated `tb/examples/` testbenches. `400` lists the problems. |
| POST | `/projects/{id}/spec/run` | editor | Runs every testbench at `main`; results per example (and the other testbenches), and `coverage`: `{covered, total, gaps: [{file, line, kind, what, labels, text, suggestion}]}`, the design's branches the examples take (`null` without examples or branches), and `machines`: the design's state machines `{module, file, state, states, values, transitions: [{from, to, when, line, covered}], reset}`. |
| POST | `/projects/{id}/spec/mistakes` | editor | Plants up to 40 small mistakes in the design, one at a time, and runs the examples against each (at most 30 s): `{caught, tried, sites, slipped: [{file, line, kind, original, mistake, text, suggestion}], elapsed_ms}`. `409` if an example already fails. |
| POST | `/projects/{id}/spec/testbench` | viewer | `{spec, example}`: the testbench the example compiles to (`{path, top, source}`), without saving; `400` lists the problems. Used by the Try it playground. |
| POST | `/projects/{id}/spec/draft` | editor | `{instruction, spec?}`: the assistant writes or revises the spec; nothing is saved. Returns `{spec, note, model, usage, usd, elapsed_ms}`; `usd` is priced for the model that served it and counts against the AI budget. |

The spec format and the example step semantics are in [user-guide/design-view.md](user-guide/design-view.md).

## Starter designs

| Method | Path | Role | Description |
|---|---|---|---|
| GET | `/starters` | | The starter designs: `{id, title, summary, category, top, ports, params, clock_mhz, clock, reset, examples, covered}`; `covered` is `[branches the examples take, all branches]`. |
| POST | `/projects/{id}/starters/{starter}` | editor | Adds the design as `starters/<top>.v` and to every flow config's `VERILOG_FILES`, in one commit. `409` if it's there or its module name is taken. |

## Blocks: the chip

| Method | Path | Role | Description |
|---|---|---|---|
| GET | `/projects/{id}/blocks` | viewer | `blocks.json`, modules that can be placed, checks, port meanings. |
| POST | `/projects/{id}/blocks/check` | viewer | `{blocks}`: checks a layout without saving; returns issues, ports and the generated top. |
| PUT | `/projects/{id}/blocks` | editor | `{blocks, parent, message?}`: commits `blocks.json`, the generated top, the adapters, `config.json` and regenerated chip examples. |
| POST | `/projects/{id}/blocks/review` | editor | `{blocks}`: the assistant's note per connection, with adapter suggestions. Counts against the AI budget. |

## Flow runs

| Method | Path | Role | Description |
|---|---|---|---|
| POST | `/projects/{id}/jobs` | editor | Queue an RTL → GDS run `{commit?, config?}` (at most 3 active per person). |
| GET | `/projects/{id}/jobs` | viewer | Recent runs. |
| GET | `/jobs/{id}` | viewer | Status, error excerpt, sign-off summary, gate-level simulation (`gls`). |
| GET | `/jobs/{id}/log?offset=` | viewer | Log from `offset`; poll until `done`. |
| POST | `/jobs/{id}/cancel` | editor | Cancel a queued or running run. |
| GET | `/jobs/{id}/artifacts` | viewer | Files in the run's `final/` directory. |
| GET | `/jobs/{id}/artifacts/{path}` | viewer | Download one (GDS, DEF, netlists, render…). |
| GET | `/jobs/{id}/drc` | viewer | DRC violations with geometry (KLayout, Magic, router). |
| GET | `/jobs/{id}/report` | viewer | Timing per corner, worst paths, area by cell class. |

## AI agent

| Method | Path | Role | Description |
|---|---|---|---|
| POST | `/projects/{id}/agent` | editor | `{request}`: start a proposal (at most 2 running per person; counts against the AI budget). |
| GET | `/projects/{id}/agent` | viewer | Recent proposals. |
| GET | `/agent/{id}` | viewer | Proposal: changed files, their base content, gate report, evidence log. |
| POST | `/agent/{id}/merge` | editor | Merge; the gate runs again and must pass, and the base must still be `main`. |
| POST | `/agent/{id}/compare` | editor | How well the design is tested at the proposal's base commit and with its changes: `{before, after}`, each `{examples, passing, coverage, machines}`. |
| POST | `/agent/{id}/regate` | editor | Re-run the gate on the recorded inputs and say whether it reproduces. |
| POST | `/agent/{id}/discard` | editor | Discard a finished proposal. |

## IP registry

| Method | Path | Role | Description |
|---|---|---|---|
| GET | `/ip` | | Blocks you can see, with their versions. |
| POST | `/projects/{id}/ip` | editor | Publish `{version, name?, description?, commit?, top?}`. |
| GET | `/projects/{id}/ip/check?block=&version=` | editor | Licence verdict and the import plan, without importing. |
| POST | `/projects/{id}/ip/import` | editor | `{block, version, accept_licence?}`: import, pinned. |
| GET | `/projects/{id}/ip/imports` | viewer | `ip.lock.json` with licences re-checked. |

## WebSockets

Both authenticate with a first text message `{"token": "<access token>"}` (no tokens in URLs),
then answer `{"type": "ready", ...}` or close with a reason.

| Path | Description |
|---|---|
| `/projects/{id}/collab` | Real-time co-editing: y-protocols sync and awareness, binary frames. Viewers' edits are dropped. Messages up to 8 MB. |
| `/projects/{id}/lsp` | Verible language server: one JSON-RPC message per text frame; documents are `file:///<project id>/<path>`. |
