# Starter designs

Sixteen small designs that already work, to start a project from or to build a chip with:

| Group | Designs |
|---|---|
| Basics | LED blinker, up/down counter, Gray-code counter, PWM output, parity checker |
| Inputs | button debouncer, push-button toggle, edge detector, pulse stretcher |
| Data | serial-to-parallel register, small FIFO, pattern detector |
| Communication | UART transmitter, UART receiver |
| Control | traffic light, watchdog timer |

Each one comes with a plain-language spec, examples that are its tests, and a design that passes
them. It is also checked against a hidden reference testbench with thousands of cycles of random
inputs, so the design does what the spec says in cases the examples don't cover.

## Starting a project from one

On the projects page, pick a design under **Start from a working design** (each card shows what
goes in and comes out, how many examples it has, how many of the cases the design tells apart they try, and
its clock), then **Create**. The new project has:

- `spec.json`: the description, connections, settings and examples, ready in the Design view;
- `src/<module>.v`: the design;
- `tb/examples/`: the example testbenches, all passing;
- `config.json`: a LibreLane flow configuration with the design's clock.

Change it from there in your own words: edit the description or add an example, and ask the AI to
make the examples pass. **Blank project** starts from the small counter template instead.

## Adding one to a project as a block

In the Blocks step, **+ Add a starter design** lists the designs the project doesn't have yet.
Adding one commits its file as `starters/<module>.v`, adds it to the flow configs' `VERILOG_FILES`,
and lists it under **Starter designs** among the blocks, ready to place and connect, for example
a debouncer in front of your design's button input, or a FIFO between a UART receiver and your
logic. A design whose module name is already used in the project is refused.

## Licence

Starter files keep their `SPDX-License-Identifier: Apache-2.0` header and a line naming the
starter. Apache-2.0 allows them in a project under any licence; publishing a block that contains
one combines the licences (`Apache-2.0 AND <yours>`), see [IP registry](ip-registry.md).

## Where they come from

They are the [design evals'](../evals.md) tasks: `evals/design/tasks/<id>/`, with a `starter`
entry in `task.json` (title, summary, group). The server builds the spec, the design and the
description in (`crates/wafer-api/build.rs`; the hidden testbenches stay out), so a starter can't
drift from what the evals and `tests/evals.rs` check.
