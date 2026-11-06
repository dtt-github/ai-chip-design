# Blocks: building the chip

The Blocks step puts the chip together from your design, verified [starter designs](starters.md),
blocks imported from the IP registry and small adapters. It uses the whole window while it's open.

## Starting

**Start with <your design>** places your design as the first block, with a chip pin for each of its
connections, already wired. The chip gets its own name (`<design>_chip`).

## The canvas

- **Blocks** on the left: your project's modules, starter designs, imported IP, and the adapters.
  Click one to place it; drag blocks by their title. **+ Add a starter design** adds another
  verified design to the project.
- **Chip pins**: inputs on the left edge, outputs on the right. Add them with **+ Input** /
  **+ Output**; select one to rename it, set its width and meaning, and mark the clock and reset.
- **Connecting**: click a connection point, then another. Either order works.
- **Connections are coloured** by their checks: blue is fine, amber has a warning or a note from
  the assistant, red can't be built. Select one to see why.

Ports with the same name as the clock and reset pins are connected to them automatically.

## Checks

These run as you edit and need no judgement:

- both ends exist;
- a value flows from an output (or an input pin) to an input (or an output pin);
- the widths match ("cnt.count is 4 bits but snk.data is 8 bits");
- each input has a single source;
- unconnected inputs are held at 0, with a warning.

## The assistant's review

Whether a connection makes sense is a matter of meaning: a level going where a one-clock pulse is
expected, an active-low signal into an active-high input, a signal from outside the chip used
without a synchronizer. **Ask the assistant to review** adds a plain-language note to every
connection and, where it helps, suggests an adapter; **Add adapter** puts it into the connection.
You can also insert one yourself from the connection's menu.

| Adapter | What it does |
|---|---|
| `wafer_rise_pulse` | A level in, a one-clock pulse out when it goes from 0 to 1. |
| `wafer_sync2` | Two flip-flops that bring an outside signal safely into this clock (adds 2 cycles). |
| `wafer_invert` | Flips every bit (setting `W` = width). |
| `wafer_resize` | Changes width: pads with zeros, or keeps the lowest bits (settings `IN`, `OUT`). |

## Saving

**Save chip** commits, in one commit:

- `blocks.json` (the layout: pins, blocks with their settings and positions, connections);
- the chip's top level `src/<chip>.v` and, when adapters are used, `src/wafer_adapters.v`, both
  generated and never edited by hand;
- `config.json`, now building the chip (`DESIGN_NAME`) from exactly the files it uses;
- regenerated chip examples, if any.

From then on Health builds the chip. Add [chip examples](design-view.md#chip-examples) so the chip
as a whole is tested, also on its gates.
