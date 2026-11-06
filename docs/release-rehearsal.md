# Release rehearsal: three starters to a clean layout, no code typed

**Result: passed on 6 Nov 2026.** The up/down counter, the UART receiver and the small FIFO each
went from the start page to "Ready for expert review" (examples passing on the synthesised gates,
every case tried, every planted mistake caught, timing met, DRC and LVS clean) in 1 to 3 minutes,
without typing any Verilog or testbench code and without AI.

This is step 2 of the plan to release 0.1.0. It is done as a person who doesn't read Verilog would
do it, in the browser, and timed like the [phase 0 gate](phase0-gate.md).

Setup: local stack (Postgres, Rauthy, API, web app) on one aarch64 machine (NVIDIA GB10, 20 cores),
LibreLane 3.0.16 with the IHP SG13G2 PDK; one user (Dev User).

## The runs

Each run: pick the starter on the start page and create the project; **Run examples**; Health →
**Plant mistakes and check**; **Find examples for everything** if anything is open, check the
examples found, **Add**, **Save spec**; **Build and check**.

| Design | Examples | Cases tried | Planted mistakes caught | Found by the search | Build | Start to ready |
| --- | --- | --- | --- | --- | --- | --- |
| Up/down counter | 4 of 4 | 14 of 14 | 12 of 12 | nothing open | 43 s | 1 min 28 s |
| UART receiver | 6 of 6 | 20 of 20 | 34 of 34 (32 before the search) | 1 example, catching both slipped mistakes | about 60 s | 3 min 50 s, of which about 1 min lost to an expired sign-in (fixed, below) |
| Small FIFO | 3 of 3 | 18 of 18 | 23 of 23 | nothing open | 41 s | 1 min 16 s |

What was built, from the Health page:

| Design | Cells | Area | Power | Clock |
| --- | --- | --- | --- | --- |
| Up/down counter | 40 | 0.0027 mm² | 0.06 mW | meets 50 MHz, about 3.2× headroom |
| UART receiver | 268 | 0.0120 mm² | 0.31 mW | meets 50 MHz, about 3.0× headroom |
| Small FIFO | 223 | 0.0125 mm² | 0.38 mW | meets 50 MHz, about 3.0× headroom |

The UART receiver's starter examples let two mistakes slip through, both counters one off
(`cnt == CLKS_PER_BIT / 2 - 1` and `cnt == CLKS_PER_BIT - 1`). The search found one example that
catches both: it sends 0xAA on `rx` and expects `data` = 170 and `valid` = 1. Read as a sentence,
it is checkable without knowing Verilog.

## What broke or confused, and what was done

- **The sign-in expired mid-task and reloaded the page.** The access token lasts 30 minutes and
  was never renewed, so the first request after that sent the whole page to the sign-in server.
  Rauthy still had a session and sent it straight back, but what was on screen (here a search in
  progress; it could have been unsaved spec edits) was lost. **Fixed:** the web app now renews
  the token with its refresh token in the last 45 seconds before it expires (Rauthy accepts a
  refresh only from 60 seconds before), without leaving the page; only if that fails does it
  sign in again.
- **Choosing a starter fills in the project name, and typing added to it** ("Up/down
  counterRehearsal counter"). **Fixed:** a name suggested from a starter is selected when you
  click into the field, so typing replaces it.
- **The page's commit marker didn't move after saving the spec** ("Built from an older version"
  although the build was of the latest). Not a product problem: the rehearsal drove a browser
  pane that was hidden, where the page's per-frame updates don't run. A page someone is looking
  at updates at once.

## Fresh-start check

Step 3 of the plan, on 6 Nov 2026: the repository cloned into an empty folder and
[Getting started](getting-started.md) followed as written, with nothing else.

| Step | Result |
| --- | --- |
| Prerequisites (Rust, Node 22 with pnpm, Docker, the wasm target, wasm-bindgen 0.2.130) | all present at the versions named |
| `./scripts/fetch-verible.sh` | Verible installed into `.tools/verible/` |
| `./scripts/fetch-pdk.sh` | PDK enabled in the `wafersmith_pdk` volume (image already pulled) |
| `docker compose ... up -d` | Postgres and Rauthy running |
| `cp .env.example .env` and `cargo run -p wafer-api` | built and listening on 8090 with the example settings unchanged; the AI off, as described, without a key |
| `pnpm install && pnpm dev:web` | the WebAssembly built on first start; the app served |
| Sign in, a project from the LED blinker starter, examples, planted mistakes, build | "Ready for expert review" |
| `cargo test --workspace` and the web typecheck | all pass |

No step was missing or wrong, so the guide is unchanged. Not checked: pulling the 7.5 GB
LibreLane image over the network (it was already present), and a machine without Rust or Node
installed.

## Not covered

- **A change made with the AI.** The starters were used as they come; changing a design in words
  uses the AI and costs money, so it was left out. Whether 0.1.0's rehearsal should include one
  is an open decision in the plan.
- More than one person at a time (covered by the phase 0 gate), and real network latency.
- A sign-in older than the refresh token's 48 hours, which still needs a fresh sign-in.
