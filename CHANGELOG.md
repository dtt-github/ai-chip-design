# Changelog

All notable changes to Wafersmith are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions will follow
[Semantic Versioning](https://semver.org/) from the first release.

## [Unreleased]

### Added

- **Would your examples catch a mistake?** Health can plant small mistakes in a copy of the design,
  one at a time (comparisons turned round, constants one off, conditions and negations reversed,
  `&&`/`||` and `+`/`-` swapped, assignments left out), run the examples against each, and list
  those that slip through in plain words with **Add an example**. `POST /projects/{id}/spec/mistakes`
  (mutation testing on the parsed design; files untouched; 40 mistakes, 30 s at most).
- `scripts/remap-dev-users.sh`: after Rauthy starts afresh, moves the development accounts' existing
  users (and so their projects) to the subjects Rauthy gave them; without it, signing in shows an
  empty project list.
- **How it works**: state machines are found in the design's Verilog (a `case` on a register
  whose arms set it, directly or through a next-state variable) and drawn in Health: states,
  numbered moves listed with when they happen, the reset state, and which moves the examples
  take. In Try it the current state is lit as you play. `POST /spec/run` returns `machines`.
- **Try it**: in Examples, the design as a small board (switches, pulse buttons and number boxes
  for inputs, lights and values for outputs, a clock you step), simulated in the browser. What you
  do is recorded as steps and the outputs you tick as checks; **Save as an example** adds it to
  the spec. Each step is simulated with the testbench the example compiles to
  (`POST /projects/{id}/spec/testbench`, which saves nothing).
- **What the examples don't test**: the simulator counts every `if`/`else` branch and `case` arm
  it runs, and Health shows how many of the design's decisions the examples try, each gap in
  plain terms ("Never tried: `en` = 0") with **Add an example** (opens Examples with the case
  written out) and a link to the line. `POST /spec/run` returns `coverage`; starter cards show
  their figure.
- **Starter designs**: the 16 verified design-eval tasks are offered as starting points. The
  projects page has a gallery (by group, with what goes in and out); a project started from one
  has its spec, design and passing examples from the first commit. The Blocks step can add any of
  them to a project as a block (`starters/<module>.v`). `GET /starters`,
  `POST /projects/{id}/starters/{starter}`, and `starter` on `POST /projects`.
- The simulator runs **functions, tasks and generate blocks**: Verilog-2001 and ANSI functions and
  tasks with locals, loops and `return`, constant functions in parameters and ranges, and generate
  `for`/`if`/`case` and named blocks (including references such as `g[2].w`). Checked against Icarus
  Verilog by two new conformance testbenches. The agent is told it may use them.
- **Design evals** for the assistant and the agent (`evals/design/`, [docs/evals.md](docs/evals.md)):
  16 tasks (11 clear, 5 with open points), each with a reference spec, design and hidden testbench;
  a runner that drafts specs or proposes designs through the API, grades them with simulation, and
  reports pass rate and cost per task (`summary.mjs`); oracle and null modes check the grading for free. First baseline: assistant 78%, agent 100%,
  $12.72 for 64 runs.
- **Chip examples**: examples can test the whole chip built in the Blocks view through its pins
  (`"target": "<chip>"`), in simulation and on the chip's gate-level netlist. The assistant can
  draft them; saving the chip keeps them in step and refuses changes that would break one.
- Documentation set: getting started, user guide, architecture, API reference, configuration,
  development; `CONTRIBUTING.md`, `SECURITY.md`, `NOTICE`, issue and pull request templates.
- Credit to DigitalTwin Technology GmbH as the initiator of the project (README, NOTICE, package
  metadata).

### Changed

- Links to the repository (README badge, package metadata, issue template) point at its new home,
  `dtt-github/wafersmith`.
- The development-only `wafersmith-dev-cli` client secret no longer carries the old `sc` prefix.
- **Renamed to Wafersmith** (formerly SiliconCommons, a name already used by lowRISC for its
  open-silicon process). Crates are now `wafer-*` (binary `wafersmith-api`), environment
  variables `WAFER_*` instead of `SC_*`, the web package `@wafersmith/web`, the adapter modules
  `wafer_*` in `src/wafer_adapters.v`, and the Docker Compose project, volumes, database and role
  `wafersmith`/`wafer`; the OIDC clients are `wafersmith-web` and `wafersmith-dev-cli`. Existing
  projects keep working: files generated under the old name are still recognised as generated,
  the old `src/sc_adapters.v` is removed the next time the blocks are saved, and
  flow runs now use LibreLane's run tag `wafer` while earlier runs are still read from `runs/sc/`. To move a local
  setup, rename the variables in `.env`, copy the `siliconcommons_pdk` and `siliconcommons_pg-data`
  volumes to `wafersmith_*`, rename the database and role, let Rauthy bootstrap afresh, and run
  `scripts/remap-dev-users.sh` before signing in so the development accounts keep their projects.
- Conditions in messages and coverage are printed with only the parentheses precedence needs
  (`cnt == N / 2 - 1`).
- The assistant counts example timing explicitly (cycle by cycle, with the reasoning), asks whether
  outside inputs need a synchronizer or debouncing, treats an example of the situation a question is
  about as depending on it, and always writes at least one example that holds whatever the answers
  are (a draft without examples is sent back once). From the design eval's baseline failures.
- AI costs are priced for the model that served each call (Claude Opus 5.5 and Gemini 4 Argon
  built in, `WAFER_AI_PRICES` for anything else) instead of always at Opus 5.5 prices, and recorded
  with the model. Drafts return their cost (`usd`), proposals keep it in the evidence log, and the
  eval runner uses the server's figure.

- Gate-level simulation and Health say how many testbenches test a block rather than the chip, and
  don't call a chip working when nothing tests it as a whole.

### Fixed

- The Blocks step no longer throws away an unsaved chip when the project changes underneath it
  (a commit by someone else, or adding a block).
- The spec check refuses an example that runs longer than the simulator's 50 µs limit (it could
  never pass), saying how many cycles fit; the assistant is told to use small settings instead.
- Drafting no longer crashes the request when the model sends the spec as JSON text instead of an
  object: the text is read, or the problem goes back to the model.
- The simulator read a sized index with its top bit set as negative (`f[4'd8]`, or a 4-bit
  `b + 4'd1` reaching 8, gave 0); indices are now unsigned unless the expression is signed.
- Example checks look 1 ps after the falling clock edge, so combinational outputs have settled
  (an example of a design without a clock could see the value from before the last `set`). The
  "what actually happened" replay samples at the same moment.

## 0.1.0 development line (2026-11-05)

Built before this changelog was kept; the commits on `main` have the details.

### Added

- Monorepo with the Rust API (axum, Postgres, sign-in through Rauthy), one Git repository per
  project and the Monaco editor (`9cfe500`), with the Verible language server (`05fe046`).
- RTL → GDS runs with LibreLane on the IHP SG13G2 PDK in containers (`52926f0`).
- WebGPU layout viewer with DRC markers, timing and area reports (`858c971`).
- Browser simulation with waveforms and real-time co-editing; Phase 0 gate passed (`b379278`).
- AI agent proposing changes behind a simulation gate, with an evidence log (`2b2b1f1`), also
  through OpenRouter (`29b9dbb`).
- Flow-config picker, gate-level simulation after each run, agent commit titles (`f618809`).
- IP registry with pinned imports and licence checks (`39b36dc`).
- Design view: Describe, examples that become testbenches, plain-language Health (`8a852de`), and
  "what actually happened" for each example (`ff0bf30`).
- Pilot readiness: the assistant asks instead of deciding, onboarding, a per-person AI budget, the
  pilot deployment with backups and restore, and fixes from a security review (`8540ac5`).
- Continuous integration on GitHub (`89a0a74`).
- The assistant no longer writes examples that depend on unanswered questions (`feb5e1d`).
- Blocks view: build the chip from blocks with checked connections and adapters (`f22d844`).
