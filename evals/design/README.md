# Design evals

Tasks, runner and results for evaluating the assistant (spec drafting) and the agent (design
proposals). See [docs/evals.md](../../docs/evals.md) for what is measured and how to run it, and
[TASKS.md](TASKS.md) for the tasks in readable form.

- `tasks/<id>/`: `task.json`, `spec.json`, `ref.v`, `hidden_tb.v`
- `run-eval.mjs`: the runner
- `list-tasks.mjs`: regenerates `TASKS.md`
- `results/`: run output (not committed)
