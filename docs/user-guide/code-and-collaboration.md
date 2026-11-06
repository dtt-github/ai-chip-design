# Code and collaboration

The Code view shows the project's files in a Verilog editor. Everything here also applies to files
the Design view generates, though those carry a "do not edit" header.

## Co-editing

Everyone who opens a project edits one shared working copy in real time, with each other's cursors
and names. Viewers see changes but can't make them. The **Changes** panel lists what differs from
the last commit; committing sends the shared copy, so it includes everyone's edits. After a
commit, files nobody was editing take the committed content; edited ones stay as changes.

Technically, the working copy is a Yjs document kept by the API (`wafer-api/src/collab.rs`, using
`yrs`): `files` (path → text), staged deletions and the commit it is based on. The WebSocket speaks
the standard y-protocols sync and awareness messages after a first `{"token": ...}` message, so
Monaco gets shared editing through `y-monaco`. Rooms are seeded from `main`, saved to
`data/collab/<project>.ydoc` and reloaded after a restart. Removing a member closes their sessions.

## Verilog language server

The editor gets diagnostics (syntax and lint), hover, completion, go to definition across files,
references, outline, formatting, quick fixes and rename from
[Verible](https://github.com/chipsalliance/verible)'s `verible-verilog-ls`.

Each editor session runs its own language server over a temporary copy of the project (with a
`verible.filelist`), bridged over a WebSocket (`wafer-api/src/lsp.rs`): a first `{"token": ...}`
message, then one JSON-RPC message per text frame. Documents are named
`file:///<project id>/<path>`, and the bridge drops any message that names another file. Each
person can have 3 sessions at once.

## Browser simulation

**▶ Simulate** runs the project's Verilog (the shared working copy, uncommitted edits included) in
a Web Worker with `wafer-sim` compiled to WebAssembly. It picks a testbench as the top (modules nothing
instantiates, `tb*` names first) and shows the `$display` output and waveforms (zoom, pan, cursor
values, radix, signal picker), with a VCD download.

`wafer-sim` is an event-driven simulator with Verilog's active, non-blocking and postponed regions. It
supports modules, parameters and hierarchy, `wire`/`reg`/`logic`/`integer`, memories, continuous
and procedural assignments, `if`/`case`/`casez`/loops/`wait`, `#` delays and `@` events,
`$display`/`$write`/`$strobe`/`$monitor`/`$finish`, `$time`, `$signed` and `$clog2`, and:

- **functions and tasks**, in Verilog-2001 and ANSI style, with local variables, loops and `return`.
  Each call is expanded where it is made; a function's variables are static (one copy per module
  instance), so `automatic` (recursive) functions are refused. Functions used in parameters and
  ranges (a hand-written `clog2`) are evaluated while the design is built.
- **generate**: `for` loops, `if` and `case` on parameters, and named blocks, with their own names
  and parameters (`g[2].w`, `%m` printing `tb.g[2].u`).

It counts how often each `if`/`else` branch and `case` arm runs, each part of an `&&`/`||`
condition is true or false when it is decided, and each side of a `?:` choice is taken
(coverage), which Health uses to show what the examples don't try.

It is 2-state (x and z read as 0) and limited to 64-bit signals; anything else it doesn't support is
reported with file and line. `%t` prints time in the timescale's unit, not its precision as Icarus
Verilog does. Every testbench in `crates/wafer-sim/tests/conformance/` must
print exactly what Icarus Verilog prints. Natively it runs a 1M-cycle counter testbench in about
0.5 s (`cargo run -p wafer-sim --release --example bench`); `cargo run -p wafer-sim --example run -- *.v`
is a command-line runner.
