# SiliconCommons

Open, browser-based, physics-aware chip design platform. Design Verilog in the browser, keep every
project in Git, and (from Sprint 2) push it through LibreLane on the IHP SG13G2 130 nm open PDK.

Licensed under [Apache-2.0](LICENSE).

## Layout

```
crates/
  sc-api/      axum HTTP API (binary: siliconcommons-api), Postgres via sqlx, OIDC bearer auth
  sc-git/      per-project bare Git repositories (libgit2), commits without a worktree
apps/
  web/         React + TypeScript shell with the Monaco editor (Vite)
deploy/
  compose/     local infra: Postgres + Rauthy (Rust OIDC provider)
```

## Run it locally

Prerequisites: Rust (stable), Node 22 with pnpm (`corepack enable pnpm`), Docker.

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

Commits are optimistic: pass the `parent` you edited against and you get `409` if `main` moved.
Roles are `owner` > `editor` > `viewer`; public projects are readable by any signed-in user.

## Where this differs from the plan

- **Rust everywhere on the server**: axum instead of FastAPI; Rauthy instead of Keycloak.
- **Git**: bare repos on disk via libgit2, behind `sc-git`. Forgejo can come later for hosted
  Git remotes and pull requests; nothing in the API depends on it.
- **Jobs**: Sprint 2 runs LibreLane in local Docker containers behind a runner trait; Kubernetes
  is a later backend. Temporal is deferred; job state will live in Postgres.
- **Verilog language server**: Monaco has Verilog/SystemVerilog highlighting now; the Verible LSP
  bridge is still to do.
