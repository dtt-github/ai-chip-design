# Design evals

How well do the assistant and the agent do on real design requests, and what does each one cost?
The design evals answer that with a fixed task set, checks that need no judgement, a pass rate and
a cost per task. Everything lives in `evals/design/`.

## The task set

16 small designs in `evals/design/tasks/<id>/`:

| Kind | Tasks |
|---|---|
| **Clear** (the description settles every behaviour) | `blinker`, `debounce`, `pwm`, `parity8` (no clock), `updown`, `gray4`, `sipo`, `uart_tx`, `uart_rx`, `fifo4`, `seq1011` |
| **Vague** (the description leaves points open) | `traffic` (which light after reset), `watchdog` (does bark stay on), `toggle_led` (debounce; press or release), `stretch` (a new pulse while stretching; long pulses), `edges` (latency and pulse length; is the input synchronous) |

Each task has:

- `task.json`: the description a person would type (it names the module and its connections, so
  answers can be graded), the kind, tags and, for vague tasks, the open points with keywords that
  show the assistant asked about them;
- `spec.json`: the reference spec, with the decisions for the open points and 2-3 examples;
- `ref.v`: a reference design written for the simulator (no functions or `generate`);
- `hidden_tb.v`: a hidden testbench, never shown to the AI. Clocked tasks get thousands of cycles
  of pseudo-random stimulus, with a reset in the middle, checked cycle by cycle against a
  behavioural model; `parity8` tries every input; `uart_rx` sends 150 random frames, some with a
  bad stop bit.

`cargo test -p sc-api --test evals` checks the references themselves: every reference design
passes its examples and its hidden testbench, and a design that drives every output to 0 fails
both. Writing a hidden testbench that also catches off-by-one variants (a counter one short, a
FIFO that reads when empty) was checked by hand when the tasks were written.

## Two evals

### Assistant: does it write the right spec?

The task description goes to `POST /projects/{id}/spec/draft` in a fresh project, as the first
message in the Describe step. The drafted spec is saved in a grading project that holds the
reference design, and its examples run there:

| Metric | Meaning |
|---|---|
| `spec_correct` | The spec saves, has examples, and every example passes on the reference design. |
| `discriminates` | At least one example fails on a design that does nothing (examples that check nothing don't count). |
| `asked_open_points` | Vague tasks: the share of open points the assistant asked about (a keyword in its questions) instead of deciding. |
| `example_pass_rate`, `examples` | Detail. |
| **`pass`** | `spec_correct` and `discriminates`, and for vague tasks every open point asked. |

An example that assumes an answer the assistant should have asked about fails on the reference,
which implements the reference decision.

### Agent: does it write a design that works?

A project gets the reference spec (so its examples become testbenches), then the request the
Design view sends for **Ask the AI to make them pass**. When the proposal finishes, its files go
into a grading project with the hidden testbench:

| Metric | Meaning |
|---|---|
| `examples_pass` | Every example passes. |
| `hidden_pass` | The hidden testbench passes. |
| `example_pass_rate`, `files_touched` | Detail. |
| **`pass`** | Both. |

Each row also records `cost_usd` (the server's prices, `crates/sc-api/src/spend.rs`), `turns` for
the agent, the model served and the latency.

## Running

The runner, `evals/design/run-eval.mjs` (Node 22, no dependencies), talks to a running API like
the web app does. Results go to `evals/design/results/` (not committed).

1. Start the local stack ([getting-started.md](getting-started.md)) and the API with no daily AI
   limit, since an eval run spends more than a person's day:

   ```bash
   SC_AI_DAILY_USD=0 cargo run -p sc-api
   ```

2. Check the grading without calling the model. `oracle` answers with the reference (every task
   must pass) and `null` with nothing (every task must fail):

   ```bash
   node evals/design/run-eval.mjs --eval assistant --mode oracle --flow evals/design/results/assistant-oracle --approve-harness
   ```

   ```bash
   node evals/design/run-eval.mjs --eval agent --mode null --flow evals/design/results/agent-null --approve-harness
   ```

3. Run the model (billed; see the cost below):

   ```bash
   node evals/design/run-eval.mjs --eval assistant --mode live --reps 2 --flow evals/design/results/assistant --approve-harness
   ```

4. Summarise pass rate and cost per task (Markdown):

   ```bash
   node evals/design/summary.mjs evals/design/results/assistant evals/design/results/agent
   ```

   `results.jsonl` follows the eval report schema, so the report builder from Anthropic's
   claude-api skill (`build-report-lite.mjs <flow-dir>`) also turns a flow directory into an HTML
   page with every case. To make it lead with `pass`, declare the metrics in the flow's
   `_state.json`, e.g. `"metrics": [{"id": "pass", "kind": "binary"}, {"id": "hidden_pass", "kind": "binary"}]`.

Options: `--reps N` (repeats per task; 2 shows how much results vary), `--concurrency N`
(default 4; the agent eval uses at most 2, the server's limit per person), `--timeout-s N` (per task, default 1800). `SC_EVAL_TASKS=uart_tx,fifo4` runs a subset.
A run resumes where it stopped; delete the flow directory to start over. `--approve-harness`
records a hash of the runner, and later runs refuse to start if the runner changed without a new
approval.

The runner signs in with the development stack's test account. For another server set
`SC_EVAL_API`, `SC_EVAL_TOKEN_URL`, `SC_EVAL_CLIENT_ID`, `SC_EVAL_CLIENT_SECRET`, `SC_EVAL_USER`
and `SC_EVAL_PASSWORD`. Each task creates two or three private projects named `eval-…` owned by
that account.

## Cost

A live run costs what the model is billed for the drafts or proposals, priced as the server prices
them (the runner's figures match the server's `ai_usage` records). Measure it with a small run
first (`SC_EVAL_TASKS=blinker --reps 1`) and multiply by the task count and repeats.

The first pilot (2026-11-05; Claude Opus 5.5 via OpenRouter; blinker, uart_tx and traffic, one
run each) passed all 6 runs and cost $1.32: $0.28 per spec draft (71-103 s) and $0.16 per agent
proposal (35-66 s, 4-7 turns). At that rate the whole set, 16 tasks with 2 repeats on both evals,
costs about $14; larger tasks such as `uart_rx` and `fifo4` will likely cost more.

## Baseline results

First full run, 2026-11-05: Claude Opus 5.5 through OpenRouter, 16 tasks, 2 repeats, both evals.
$12.72 in total, as the server recorded it (the four agent runs that a provider error stopped
were rerun; see below).

| Eval | Pass | Clear tasks | Vague tasks | Cost per run | Time per run |
|---|---|---|---|---|---|
| Assistant | 25 of 32 (78%) | 20 of 22 | 5 of 10 | $0.26 | 41-172 s |
| Agent | 32 of 32 (100%) | 22 of 22 | 10 of 10 | $0.13 | 28-70 s, 4-7 turns |

What the assistant got wrong:

- **Counting cycles** (uart_tx, sipo; one repeat each): an example expected busy to drop one
  cycle early, and one expected a bit to still be there after 8 shifts.
- **An example that depended on its own open question** (stretch, one repeat): it asked how long
  dout stays high after a pulse, then wrote an example that assumed an answer (a 2-cycle pulse
  becomes 4 cycles; the reference decision gives 5).
- **No examples at all** (toggle_led, both repeats): it asked all the right questions but wrote no
  example, although "a long press and release toggles the LED once" holds whatever the answers.
- **Didn't ask whether the input is synchronous** (edges, both repeats): it asked about latency,
  pulse length and the state after reset, but not about a synchronizer.

Provider errors: the OpenRouter account allows few requests per minute for a new account (429)
and the key had a spending cap (402). Four agent runs stopped on those errors before finishing.
The runner now counts such a run as a failed attempt to retry, not as a graded result.
