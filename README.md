# SiliconCommons

**AI-assisted chip design in the browser, for engineers who are not Verilog experts.**

Describe what a chip should do in plain words, give examples of how it should behave, and
SiliconCommons turns them into a specification and tests. An AI agent writes the Verilog until
every example passes. The design then goes from RTL to a sign-off-checked layout (GDS) on the
IHP SG13G2 130 nm open PDK, and the results are explained in plain language. Engineers who do
write Verilog work on the same project in a full code editor.

SiliconCommons is an initiative of **[DigitalTwin Technology GmbH](https://digitaltwin.technology)**,
which started the project and leads its development.

[![CI](https://github.com/dtt-github/ai-chip-design/actions/workflows/ci.yml/badge.svg)](https://github.com/dtt-github/ai-chip-design/actions/workflows/ci.yml)
· Licensed under [Apache-2.0](LICENSE) · [Changelog](CHANGELOG.md)

## What it does

| | |
|---|---|
| **Describe** | Say what the design does; an assistant drafts the connections, settings and examples and asks about anything left open. |
| **Examples are the tests** | Each example compiles into a testbench. A change, by a person or the AI, is only accepted when every example passes. "What actually happened" shows the design's behaviour cycle by cycle against the example. |
| **Blocks** | Build the chip from your design, imported IP and adapters on a canvas; every connection is checked, and the assistant reviews what they mean. |
| **Health** | Builds the layout and reports in plain words whether it works (also on the synthesised gates), is fast enough, can be manufactured, and its size and power. |
| **Code** | Real-time co-editing, a Verilog language server, browser simulation with waveforms, and Git history for every project. |
| **RTL → GDS** | LibreLane in containers, a WebGPU layout viewer with DRC markers, timing and area reports, gate-level simulation. |
| **AI agent** | Proposes changes that can only be merged when the simulation gate passes, with an evidence log. |
| **IP registry** | Publish blocks, import them pinned to a commit, with licence compatibility checks. |

## Quick start (local development)

You need Rust, Node 22 with pnpm, and Docker. Full instructions, including the WebAssembly
toolchain, the PDK and the development accounts, are in
[docs/getting-started.md](docs/getting-started.md).

```bash
./scripts/fetch-verible.sh && ./scripts/fetch-pdk.sh
```

```bash
docker compose -f deploy/compose/docker-compose.yml up -d
```

```bash
cp .env.example .env && cargo run -p sc-api
```

```bash
pnpm install && pnpm dev:web
```

Then open http://localhost:5173.

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | Running everything locally, development accounts, tests |
| [User guide](docs/user-guide/README.md) | Design view, Blocks, examples, Health, code editing, runs, agent, IP registry |
| [Architecture](docs/architecture.md) | Components, data flow, storage, trust boundaries, design decisions |
| [API reference](docs/api.md) | HTTP and WebSocket endpoints, roles, errors |
| [Configuration](docs/configuration.md) | Every environment variable |
| [Development](docs/development.md) | Workflow, conventions, CI, migrations, how to add a feature |
| [Design evals](docs/evals.md) | Task set, checks, pass rate and cost per task for the assistant and agent |
| [Pilot deployment](deploy/pilot/README.md) | One-host HTTPS deployment, backups and restore |
| [Security review](docs/security-review.md) | What was reviewed, fixed and accepted before the pilot |
| [Phase 0 gate](docs/phase0-gate.md) | The first end-to-end milestone |

Contributions follow [CONTRIBUTING.md](CONTRIBUTING.md); report vulnerabilities as described in
[SECURITY.md](SECURITY.md).

## Repository layout

```
crates/
  sc-api/        HTTP API and workers (binary: siliconcommons-api)
  sc-git/        per-project bare Git repositories (libgit2)
  sc-jobs/       flow-run backends: LibreLane in Docker, metrics and report parsing
  sc-sim/        event-driven Verilog simulator, checked against Icarus Verilog
  sc-sim-wasm/   the simulator for the browser (WebAssembly)
  sc-gds/        GDSII reader for the layout viewer
  sc-gds-wasm/   the GDS reader for the browser (WebAssembly)
apps/web/        React + TypeScript web app (Vite, Monaco)
deploy/compose/  local development services (Postgres, Rauthy)
deploy/pilot/    pilot deployment (Caddy, API, Postgres, Rauthy, backups)
docs/            documentation
scripts/         toolchain and PDK setup, code generation
```

## Credits

SiliconCommons was initiated by [DigitalTwin Technology GmbH](https://digitaltwin.technology).
It builds on open-source work including [LibreLane](https://github.com/librelane/librelane), the
[IHP SG13G2 open PDK](https://github.com/IHP-GmbH/IHP-Open-PDK),
[Verible](https://github.com/chipsalliance/verible), [Rauthy](https://github.com/sebadob/rauthy),
[Yjs/yrs](https://github.com/y-crdt), [Monaco](https://github.com/microsoft/monaco-editor) and
[Icarus Verilog](https://github.com/steveicarus/iverilog). See [NOTICE](NOTICE).

## Licence

Copyright 2026 DigitalTwin Technology GmbH and the SiliconCommons contributors. Licensed under the
[Apache License, Version 2.0](LICENSE).
