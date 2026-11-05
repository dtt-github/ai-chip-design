# AI agent

The **Agent** panel sends a change request in words ("add an even-parity option"), and so do the
Design view's "make the examples pass" and "the design is wrong" buttons. The agent runs in the
API (`sc-api/src/agent.rs`) and changes a private copy of the project; it can't commit. A person
reviews its proposal and merges it.

## What it can do

It calls the Claude Messages API (adaptive thinking with summarised reasoning, prompt caching)
through five tools:

- `list_files` and `read_file`;
- `write_file`, for Verilog sources and docs only (`.v .sv .vh .svh .md .txt`); never `spec.json`
  or the example testbenches, which are your tests, nor flow configs or scripts;
- `run_simulation`, with the same simulator as the browser;
- `submit_proposal`, to finish with a commit title and a summary for the reviewer.

## The simulation gate

When the agent finishes, the server runs every testbench on the files the proposal would produce
(`sc-api/src/gate.rs`): each must reach `$finish`, print `PASS` and print nothing containing `FAIL`,
`ERROR` or `MISMATCH`. Only a proposal whose gate passed, and whose base is still `main`, can be
merged by an editor; the gate runs again at merge time on the exact files committed. The commit is
authored by the person who merged, titled with the agent's title, and records the summary, the
model, the gate result and the evidence hash.

## Evidence log

Every proposal keeps the request and who asked, the base commit, the model requested and served,
the effort, a hash of the agent's instructions, its reasoning summaries and messages, every tool
call (file hashes, not contents) with its result, token usage, the changed files' hashes, and the
gate report with a hash of every simulated file. A sha256 over the whole log goes into the merge
commit, and **Re-run gate** checks that the recorded result reproduces.

## Cost and limits

Each proposal's token usage is priced for the model that served it and counted against the person's daily AI budget
(`SC_AI_DAILY_USD`); each person can have 2 proposals running at once. Without an API key on the
server, the agent is unavailable ([configuration.md](../configuration.md#ai-assistant-and-agent)).
