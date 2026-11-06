# Getting started

How to run Wafersmith on your own machine for development. For a deployment other people
use, see [the pilot deployment](../deploy/pilot/README.md).

## Prerequisites

- Rust stable (the CI pins the version in `.github/workflows/ci.yml`)
- Node 22 with pnpm (`corepack enable pnpm`)
- Docker (for Postgres, Rauthy and flow runs)
- For the browser's WebAssembly modules:

```bash
rustup target add wasm32-unknown-unknown
```

```bash
cargo install wasm-bindgen-cli --version 0.2.130 --locked
```

`pnpm dev:web` builds the WebAssembly on first start (`scripts/build-wasm.sh`, output in
`apps/web/src/wasm/`, not committed); run `pnpm --filter @wafersmith/web wasm` again after
changing `wafer-sim` or `wafer-gds`.

## Tools and PDK

The Verilog language server ([Verible](https://github.com/chipsalliance/verible)), pinned and
checksummed, into `.tools/verible/`:

```bash
./scripts/fetch-verible.sh
```

The flow toolchain: pulls the LibreLane image (`ghcr.io/librelane/librelane:3.0.16`, about
7.5 GB) and installs the IHP SG13G2 PDK version that image pins into the Docker volume
`wafersmith_pdk`:

```bash
./scripts/fetch-pdk.sh
```

## Services and the app

Postgres and the sign-in server (Rauthy), local only:

```bash
docker compose -f deploy/compose/docker-compose.yml up -d
```

The API (reads `.env`; every variable is described in [configuration.md](configuration.md)):

```bash
cp .env.example .env
```

```bash
cargo run -p wafer-api
```

The web app:

```bash
pnpm install && pnpm dev:web
```

Open http://localhost:5173 and sign in.

| Service  | URL                           | Notes                                   |
| -------- | ----------------------------- | --------------------------------------- |
| Web      | http://localhost:5173         | Vite dev server, proxies `/api` to 8090 |
| API      | http://localhost:8090         | `GET /healthz`                          |
| Rauthy   | http://localhost:8180/auth/v1 | admin UI at `/auth/v1/admin`            |
| Postgres | localhost:5442                | `sc` / `sc`, database `wafersmith`  |

## Development accounts

Bootstrapped from `deploy/compose/rauthy/bootstrap/`. **These are for localhost only**; the pilot
deployment generates its own secrets and has none of these accounts.

- `dev@wafersmith.dev` and `dev2@wafersmith.dev`, password `Wafersmith-dev-2026`
- Rauthy admin `admin@localhost`, password `Wafersmith-admin-2026`

Rauthy bootstraps only into an empty database. After changing the bootstrap files, reset it:

```bash
docker compose -f deploy/compose/docker-compose.yml down
```

```bash
docker volume rm wafersmith_rauthy-data
```

### A token for `curl`

The development-only `wafersmith-dev-cli` client allows the password grant:

```bash
curl -s -X POST localhost:8180/auth/v1/oidc/token -d grant_type=password -d client_id=wafersmith-dev-cli -d client_secret=scdevcliscdevcliscdevcliscdevcliscdevcliscdevcliscdevcliscdevcli -d username=dev@wafersmith.dev -d password=Wafersmith-dev-2026
```

Send the `access_token` as `Authorization: Bearer <token>`; see [api.md](api.md).

## AI features

The assistant (spec drafting, connection review) and the agent need an API key in `.env`:
`ANTHROPIC_API_KEY`, or `OPENROUTER_API_KEY` to go through OpenRouter. Without one, those
endpoints answer 503 and the rest works. Each call is priced and counted against a per-person
daily budget (`WAFER_AI_DAILY_USD`, default $3).

## Tests

```bash
DATABASE_URL=postgres://wafer:wafer@127.0.0.1:5442/wafersmith cargo test --workspace
```

The API tests create a throwaway database per test (`sqlx::test`) and sign their own tokens, so
Rauthy does not need to be running. The language-server tests run when Verible is installed and
are skipped otherwise. The real LibreLane runs (about a minute each) are opt-in:

```bash
cargo test -p wafer-jobs --test docker -- --ignored
```

The web app:

```bash
pnpm --filter @wafersmith/web typecheck
```

See [development.md](development.md) for the checks CI runs on every pull request.
