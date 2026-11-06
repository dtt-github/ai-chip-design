# Design view

Four steps: **Describe** what the design should do, give **Examples** that become its tests, build
the chip from **Blocks**, and check its **Health**. Nobody has to read Verilog; the AI writes it
and every change has to pass your examples.

## Describe

Write what the design should do in your own words, then **Draft the spec with the assistant**. It
fills in:

- **Connections** with plain-language meanings, the clock and the reset;
- **Settings** (design parameters, e.g. how many clock cycles per LED step);
- **Examples** (next section);
- **Questions** about anything your description leaves open. The suggested answer comes first;
  answering (or typing your own) updates the spec and adds the examples that depended on it.

The assistant records only decisions you made: when it would have to guess, it asks instead
(including whether an outside input needs a synchronizer or debouncing), and it doesn't write
examples that depend on an open question. It always writes at least one example that holds
whatever the answers are. Nothing is saved until you click **Save
spec**; you can edit everything by hand, and **Ask for a change** sends an instruction in words.

Saving commits `spec.json` and the generated example testbenches together.

## Examples are the tests

Each example is a short table of steps, run on the falling clock edge (inputs settle before the
design samples them on the rising edge). Before the first step all inputs are 0 and reset is held
for two cycles.

| Step | Meaning |
|---|---|
| **Set input** | Drive an input to a value now (no time passes). |
| **Wait** | Let N clock cycles pass. |
| **Wait until** | Wait until a connection has a value, failing after N cycles. |
| **Expect** | A connection must have a value now. |
| **Expect for** | A connection must keep a value for the next N cycles (time moves past them). |

Values are numbers: `1`, `0x55`, `0b1010`, `8'hA7`. An example must fit in the simulator's 50 µs
(2,500 cycles at 50 MHz); for a long setting such as a 10 ms debounce, give the example a small
value for that setting. Each example is compiled, by fixed rules (not
by the AI), into a self-checking testbench `tb/examples/tb_ex_<id>.v`. Because these are ordinary
testbenches, everything runs them: the server's simulation gate, the browser simulator, and
gate-level simulation after a build. **Run examples** shows the result of each one;
**Ask the AI to make them pass** opens an agent proposal with the failures. The AI may not change
the spec or the examples.

### Try it: an example by playing with the design

**Try it** (next to **Run examples**) shows the design as a small board, simulated in your browser:
switches and **Pulse** buttons for 1-bit inputs, number boxes for wider ones, lights and values for
the outputs, and the clock, which only moves when you say (**+1**, **+5**, **+20** cycles). A strip
underneath shows what every connection did over the last 40 cycles.

When an output shows what it should, click **✓ Right** (or **✓ All outputs are right**). Everything
you did is listed as plain steps ("set en to 1", "4 cycles pass", "check led is 1"), with **Undo**
and **Start over**. **Save as an example** turns it into an example with those steps; save the
spec to keep it. Each change is simulated from reset with the testbench the example compiles to
(`POST /spec/testbench`), so the saved example checks exactly what you saw. It is a quick way to
close a gap Health shows ("Never tried: `en` = 0"): set the input, tick the outputs, save. When
the design has a state machine, its diagram is shown with the current state lit, so you can watch
it move as you play. It uses
the committed design.

Once the Blocks step has built a chip, **The whole chip** switches the board to the chip's pins:
you play with the chip as a whole (for example a button that goes through a synchronizer before
reaching your design), every block's state machine is shown with its current state lit, and what
you save is a [chip example](#chip-examples).

### Chip examples

Once the Blocks step has built a chip, an example can test **the whole chip** instead of the
design: choose it under **Tests**, or use **Add chip examples with the assistant**. A chip example's
steps use the chip's pins, so it checks everything between the pins and the design, such as the
two cycles a synchronizer adds. After a build, chip examples also run on the chip's gates. Saving
the chip regenerates them, and refuses a change that would break one (for example renaming a pin
an example uses).

### What actually happened

Selecting an example simulates it in your browser and replays its steps against the result, cycle
by cycle: each connection's value, the example's checks laid over it (amber), any difference in red,
and the inputs it sets. The first difference is explained in plain words, including when the
expected value did appear ("it became 1 one cycle later, so the design may be one cycle slower
than the example says").

**Click any value in the timeline to see why it is what it is.** The simulator records which line of
the design wrote every value, so the explanation says when the value appeared, the line that wrote
it ("line 20 of blinker.v wrote it: `led <= ~led`"), why that line ran (each `if` and `case`
around it and which way it went: "`cnt == N - 1` was true (cnt = 3)"), and the values it used.
Every value mentioned can be clicked in turn, so you can follow a wrong output back, one step at
a time, to the decision or the example input that caused it. Values passed through a port of a
block are followed into the block; an input the example sets ends the chain ("set by step 1 of
the example"). When you then ask the AI to fix the design or the assistant to correct the
example, the explanation you followed goes with the request.

**Why wasn't it the expected value?** When an example fails, the account of the difference ends
with a **Why wasn't it N?** link. It lists every line of the design that could have given the
signal the value the example expected, and what happened to each the last time it could have run:

- **Blocked**: a decision went the other way. "Line 20 of blinker.v: `led <= ~led`. Didn't run for
  cycle 3 (`!rst_n` was false, `!en` was false) because **`cnt == N - 1` was false** (cnt = 2)."
  The values shown can be clicked to ask why they were what they were (here: why `cnt` was only
  2), so you can follow the cause back.
- **Ran**: it ran, with the values shown, and computed something else (or a later line changed
  the value again).
- **Hadn't run yet**: nothing had reached its decisions by then.

Lines that only write other constants (`valid <= 0` when 1 was expected) are left out and counted.
If no line can write the expected value, it says so: the design needs one. This answer, too, goes
with a request to the AI.

Three buttons answer the one question to decide, **Which is wrong?**:

- **The design: ask the AI to fix it**, with this explanation;
- **The example: use what the design did**: rewrites the failing check from the simulation,
  without AI. If the expected value appears a few cycles later or earlier, the check moves
  ("Step 3 (expect led = 1): checked 1 cycle later", by adding a wait or shortening the one
  before it); a wait-until gets more cycles; a hold check covers only the cycles it held;
  otherwise the check expects the value the design gave, written the same way (`0x`, `0b` or
  decimal). The changed example is simulated again (repeating for further failing checks), and the
  changes are listed first: **Change the example** applies them only if the example then passes,
  and saving the spec keeps them. Use it when the design is right and the example's timing or
  value was a guess;
- **The example: ask the assistant to correct it**: re-checks the example against the spec in
  words and corrects it.

## Health

The latest build of the project's default flow config, in plain words, each with **What does this
mean?** and the expert numbers underneath:

- **It works**: how many examples pass in simulation, and how many ran and passed on the
  synthesised gates (examples for other settings, or that test a block rather than the chip, are
  counted separately).
- **Tested by the examples**: how many of the cases the design tells apart the examples go
  through: each `if`/`else` branch and `case` choice, each part of a combined condition both ways
  (in `wr && !full`, `full` both 0 and 1 when it is decided), and both sides of each `?:` choice.
  Each one they never try is shown in plain terms ("Never tried: `en` = 0", "Never tried: `state`
  = 2", "Never tried: `full` ≠ 0 while deciding `wr && !full`"). Passing examples only vouch for the cases they
  try. **Find an example** searches for one (see below); **Add in words** opens Examples with the
  missing case written out as a new example's description; the file and line link opens the code. A `default` choice that no example reaches
  is shown but doesn't hold the project back, since it is often impossible by design.
- **Would your examples catch a mistake?**: **Plant mistakes and check** makes small, typical
  mistakes in a copy of the design, one at a time (a comparison turned round, a number one off, a
  condition reversed, a `!` left out, `+` for `-`, an assignment left out), and runs every example
  against each. It reports how many some example caught, and each one that slipped through ("If
  `cnt == N - 1` were `cnt == N` (one off, line 15), every example would still pass"), with **Find
  an example**, **Add in words** and a link to the line. Your files aren't changed. A few planted
  mistakes change nothing at all and can't be caught. Up to 40 mistakes, spread over the design,
  in at most 30 seconds.

  **Find an example**, for a case no example tries or a mistake that slipped through, has the
  simulator search the design's inputs (no AI, nothing to pay). It tries your examples' inputs,
  as they are and with changes, and fresh sequences of values the design uses (0, 1, all ones,
  the numbers in its code), held for a cycle or many. The first sequence that reaches the case,
  or makes the mistaken copy behave differently at an output, is cut to the shortest that still
  does and stripped of every input change it doesn't need. It ends with checks of the outputs
  (for a mistake, the ones that tell the two apart), holding what the design did. The example
  opens in Examples, unsaved, titled "Tries …" or "Notices if …": **check that what it expects
  is what the design should do**. If it isn't, you have found a bug. Save the spec to keep it.
  The search takes at most about 30 seconds, and the same design and spec always give the same
  example. If nothing is found, it says why: the case may be impossible by design, or the mistake
  may change nothing that can be seen.
- **How it works**: each state machine found in the Verilog, drawn as states (boxes, with the one
  after reset marked) and numbered moves between them, listed with when they happen ("state 0 →
  state 1 when `rx` = 0"). Moves an example takes are solid; dashed red ones no example takes yet;
  dotted ones before the examples have run. A machine is a `case` on a register
  whose arms set that register, directly or through a next-state variable; named states come from
  `localparam`s.
- **Fast enough**: whether the clock target is met, and by how much ("could run about 4× faster").
- **Can be manufactured**: layout rule checks, layout versus schematic, antenna checks.
- **Size and power**: area, cell count, power, supply droop.

It also lists what still needs an expert before a real chip run (I/O pads, manufacturing test,
power-up behaviour, timing with real gate delays, the final go/no-go). **Build again** starts a new
run.

### Review report

**Review report** (in Health) opens one page to hand to a reviewer, an expert or a manager, and
**Print or save as PDF** turns it into a document. At the project's latest commit it gives:

1. **What it is**: the description, connections with their meaning, settings, decisions, and any
   questions still open.
2. **How it is tested**: every example as a sentence ("After reset: rx is set to 1; 2 clock cycles
   pass; rx takes the values 0, 0, 0, 1, 1, 1, 1, 0, 0, 4 clock cycles each (start bit to bit 7);
   ...") with whether it passes, what the examples try, and the planted-mistakes check.
3. **How it works**: the state machines.
4. **Is it ready?**: the latest build (timing, layout checks, gate-level simulation, size and power)
   and what still needs an expert.
5. **Where it came from**: the AI's merged changes and the commit history.

Running the examples and planting mistakes takes an editor; for viewers the report says so and
shows the rest.

## The spec file

`spec.json` at the project root, written by the Design view (and readable by hand):

```json
{
  "version": 1,
  "intent": "An LED blinker. While enable is on, the LED toggles every N clock cycles...",
  "top": "blinker",
  "clock": { "port": "clk", "mhz": 50 },
  "reset": { "port": "rst_n", "active_low": true },
  "ports": [{ "name": "en", "dir": "in", "width": 1, "meaning": "1 makes the LED blink" }],
  "params": [{ "name": "N", "value": "4", "meaning": "Clock cycles per LED step" }],
  "decisions": [{ "question": "...", "answer": "..." }],
  "questions": [{ "question": "...", "options": ["suggested answer", "another"] }],
  "examples": [{
    "id": "first_on", "title": "LED comes on after N cycles",
    "given": "...", "expect": "...", "params": [],
    "target": "blinker_chip",
    "steps": [
      { "op": "set", "signal": "en", "value": "1" },
      { "op": "wait_until", "signal": "led", "value": "1", "cycles": 10, "note": "first step" }
    ]
  }]
}
```

`target` is omitted for examples of the design itself. Example ids are lowercase letters, digits
and `_`, and name the testbench.
