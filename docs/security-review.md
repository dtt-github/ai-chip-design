# Pre-pilot security review (2026-11-05)

A code review before exposing SiliconCommons to a few invited engineers over the internet. Each
finding below is listed with what was done about it. Severity is for an internet-facing pilot
with admin-created accounts only.

## Fixed

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| C1 | Critical | Flow containers write the job directory with user-controlled tools; the server then wrote, appended to and read files there, following symlinks (host file write as the API user). Escaped Verilog identifiers and `DESIGN_NAME` were joined into host paths. | After the container exits, symlinks that point out of the job are deleted (`sc_jobs::remove_escaping_links`); flow outputs are read only through `sc_jobs::read_inside` (no symlink anywhere on the path, regular file, inside the job). Gate-level simulation runs in a fresh directory the server creates (`jobs/<id>/gls`), with project files taken from Git, logs in a directory no container mounts, and model paths limited to `/pdk/`. Testbench and design names must be plain identifiers. Containers drop all capabilities. |
| H1 | High | The Verilog parser recursed without limit: one crafted file aborted the API (stack overflow). | Nesting limit (512) and a limit on operator chains (1000) in the parser; every server-side parse, elaboration and simulation runs on a 256 MiB stack (`sc_sim::on_big_stack`). Tested with hostile inputs. |
| H2 | High | Viewers could trigger in-process simulations without limit; memory unbounded. | Running examples and re-gating need the editor role; at most 3 simulations at once server-wide; elaboration refuses designs over 8M storage words or 100k instances. |
| H3 | High | Flow runs had no time limit, no per-person limit, and run directories were never deleted. | Wall-clock limit (`SC_JOB_TIMEOUT_MIN`, default 120); at most 3 queued or running runs per person; run directories older than `SC_JOB_RETENTION_DAYS` (default 30) are deleted (the job record and its summary stay). |
| H4 | High | Unbounded AI spend. | Per-person daily budget in US dollars (`SC_AI_DAILY_USD`, default 3), priced from token usage and recorded per call; at most 2 agent proposals running per person. |
| M1 | Medium | The language server's URI check missed mixed-case percent-encoded `..` and non-lowercase `file:` schemes. | Segments are percent-decoded before checking; any other `file:` URI (any case) is refused. |
| M2 | Medium | One person could hold every language server slot; 32 MB co-editing messages. | At most 3 language server sessions per person; co-editing messages capped at 8 MB. |
| M3 | Medium | Cursor colours from other clients went into a stylesheet unchecked (CSS injection). | Only plain hex colours are used; client ids are coerced to numbers; a Content-Security-Policy is set on the app. |
| M4 | Medium | No way to remove someone from a project; their sessions stayed open. | `DELETE /api/projects/{id}/members/{user}` (owner only) also closes their co-editing connections. |
| M5 | Medium | The agent could write flow configs or tool scripts, which run in the flow after a merge. | The agent may only write Verilog sources and docs (`.v .sv .vh .svh .md .txt`); never `spec.json` or `tb/examples/`. |
| M6 | Medium | Development credentials and the password-grant CLI client must not reach a deployment. | The pilot (`deploy/pilot`) generates fresh secrets, uses HTTPS, secure cookies, MFA for admins, no self-registration, and only the web client (`SC_OIDC_AUDIENCES=siliconcommons-web`). |
| L1 | Low | Repository paths starting with `-` could be read as options inside the container. | Paths are passed as `./<path>`. |
| L2 | Low | Example values were copied raw into generated testbench comments and messages. | Only characters a number can contain are copied. |
| L4 | Low | No security headers. | HSTS, `nosniff`, `X-Frame-Options: DENY`, referrer and permissions policies, and a CSP on the app (Caddy). |

## Accepted for the pilot (documented, revisit before a wider launch)

- **The API can start containers on the host's Docker** (it mounts the Docker socket). Anyone who
  takes over the API process is root on the host. Mitigations: the API runs as an unprivileged
  user, job containers have no network, no capabilities and resource limits. Before a wider
  launch: run flow jobs on a separate machine or with rootless Docker / a job runner service.
- **Job containers run as the API's user (uid 10001).** With the symlink sweep and the safe
  reader this no longer reaches the host, but a dedicated job uid would add a layer.
- **No disk quota per run or per project** (repositories accept commits up to 32 MB each). Watch
  disk use during the pilot; old run directories are pruned.
- **Co-editing and language server sessions are checked when they connect**, not again when the
  access token expires (removal from a project does close co-editing sessions).
- **Prompt injection:** project files go into the AI's context. The agent can only change files in
  its own proposal copy, only Verilog and docs, and a person must merge; the assistant only
  returns a spec for a person to save. Risk accepted.
- **L3:** member emails are visible to anyone who can see a public project; block-name conflicts
  reveal that a private block name exists. Fine among invited pilot users.
- **L5:** no repository size quota (see disk above).
- **L6:** membership is granted by email claim; fine while accounts are admin-created.
- **Awareness spoofing:** a viewer can still broadcast fake cursor names and positions (now without
  CSS injection).

## Checked and fine

Every HTTP handler authorizes against the owning project (no cross-project access by job,
proposal or artifact id); roles are enforced as intended; JWT validation requires `kid`, takes the
algorithm from the JWKS and checks issuer and audience; WebSockets authenticate with a first
message (no tokens in URLs); `sc_git::normalize_path` is sound and repositories store blobs only;
artifact downloads canonicalize paths and send `nosniff`; there is no CORS layer and no SSRF;
internal errors are not shown to clients and secrets are not logged; flow containers run with
`--network none`, `no-new-privileges`, CPU, memory and PID limits and a read-only PDK.
