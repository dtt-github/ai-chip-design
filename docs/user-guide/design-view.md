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
than the example says"). Two buttons answer the one question to decide:

- **The design is wrong**: ask the AI to fix the design, with this explanation;
- **The example is wrong**: ask the assistant to re-check the example against the spec and
  correct it.

## Health

The latest build of the project's default flow config, in plain words, each with **What does this
mean?** and the expert numbers underneath:

- **It works**: how many examples pass in simulation, and how many ran and passed on the
  synthesised gates (examples for other settings, or that test a block rather than the chip, are
  counted separately).
- **Fast enough**: whether the clock target is met, and by how much ("could run about 4× faster").
- **Can be manufactured**: layout rule checks, layout versus schematic, antenna checks.
- **Size and power**: area, cell count, power, supply droop.

It also lists what still needs an expert before a real chip run (I/O pads, manufacturing test,
power-up behaviour, timing with real gate delays, the final go/no-go). **Build again** starts a new
run.

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
