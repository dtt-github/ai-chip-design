# Changelog

All notable changes to SiliconCommons are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions will follow
[Semantic Versioning](https://semver.org/) from the first release.

## [Unreleased]

### Added

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

### Fixed

- The simulator read a sized index with its top bit set as negative (`f[4'd8]`, or a 4-bit
  `b + 4'd1` reaching 8, gave 0); indices are now unsigned unless the expression is signed.
- Example checks look 1 ps after the falling clock edge, so combinational outputs have settled
  (an example of a design without a clock could see the value from before the last `set`). The
  "what actually happened" replay samples at the same moment.

### Changed

- AI costs are priced for the model that served each call (Claude Opus 5.5 and Gemini 4 Argon
  built in, `SC_AI_PRICES` for anything else) instead of always at Opus 5.5 prices, and recorded
  with the model. Drafts return their cost (`usd`), proposals keep it in the evidence log, and the
  eval runner uses the server's figure.

- Gate-level simulation and Health say how many testbenches test a block rather than the chip, and
  don't call a chip working when nothing tests it as a whole.

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
