# User guide

SiliconCommons has two ways into the same project:

- **Design view**, for engineers who don't write Verilog: describe the design, give examples (which
  are the tests), build the chip from blocks, and check its health in plain words. The AI writes the
  Verilog.
- **Code view**, for those who do: the files, a Verilog editor with a language server, real-time
  co-editing and simulation with waveforms.

Both work on the same files and Git history, so experts and non-experts can work together.

| Page | What it covers |
|---|---|
| [Design view](design-view.md) | Describe, Examples (and chip examples), what actually happened, Health; the spec format |
| [Blocks](blocks.md) | Building the chip from blocks, connection checks, adapters, the assistant's review |
| [Code and collaboration](code-and-collaboration.md) | The editor, co-editing, the language server, browser simulation |
| [Flow runs](flow-runs.md) | RTL → GDS, gate-level simulation, the layout viewer, reports |
| [AI agent](agent.md) | Proposals, the simulation gate, the evidence log |
| [IP registry](ip-registry.md) | Publishing and importing blocks, licence checks |

## Getting around

- The projects page starts new projects, optionally from a starter idea (LED blinker, button
  debouncer, PWM output, UART transmitter), and chooses the project's licence.
- In a project, the switch above the editor chooses **Design** or **Code**. The right-hand panel
  has Changes (commit the shared working copy), Runs, Agent and IP.
- AI features (drafting, reviews, the agent) count against a daily budget per person; Describe
  shows how much of today's budget you've used.
