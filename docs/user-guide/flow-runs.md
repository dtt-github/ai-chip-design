# Flow runs: RTL → GDS

**Run RTL → GDS** (Runs panel, or **Build** in Health) turns the last commit into a chip layout
with [LibreLane](https://github.com/librelane/librelane) on the IHP SG13G2 130 nm open PDK. Pick
the flow config next to the button: any `config*.json`, `.yaml` or `.tcl` in the project, with
`config.json` first (for example a second config with `SYNTH_PARAMETERS` for another setting).

## What a run does

Runs are queued in Postgres and picked up by workers inside the API (`WAFER_JOB_WORKERS`, default 2).
Each run:

- exports the project at the run's commit to `data/jobs/<run id>/work/`;
- runs `librelane --manual-pdk --pdk ihp-sg13g2 --run-tag wafer ./<config>` in a throwaway container
  with no network, no Linux capabilities, the PDK mounted read-only, and CPU, memory, process and
  time limits ([configuration.md](../configuration.md#flow-runs-rtl--gds));
- streams the output to `flow.log`;
- removes any link the flow left pointing outside its directory, then records the image digest,
  the commit, the exit code, the flow's error lines and a summary of `metrics.json` (area, cells,
  timing, power, IR drop, DRC/LVS/antenna) with a sign-off verdict.

Each person can have 3 runs queued or running. Run directories are deleted after the retention
period (`WAFER_JOB_RETENTION_DAYS`); the summary stays.

## Gate-level simulation

After a successful flow, the project's testbenches run again on the synthesised netlist with Icarus
Verilog and the PDK's cell models (zero-delay), in a fresh directory the server prepares
(`wafer-api/src/gls.rs`). Verdicts follow the simulation gate's rules. The netlist is built for one set
of settings (module defaults plus the config's `SYNTH_PARAMETERS`), so a testbench runs only if it
drives the built design with exactly those values; testbenches for other settings, or that test a
block rather than the chip, are listed as skipped with the reason. A failure shows as "netlist sim
fail" but doesn't change the physical sign-off verdict.

## Results

A run's panel shows the live log, a summary (sign-off, metrics, gate-level simulation), the layout,
reports and every file in `final/` for download.

### Layout viewer

- `wafer-gds` (Rust, compiled to WebAssembly) parses the GDSII, flattens the hierarchy (rotation,
  reflection, magnification) and tessellates polygons and paths per layer, in µm.
- WebGPU draws fills and outlines with the PDK's colours and stack order; without WebGPU, a Canvas
  2D renderer does the same.
- Layer names, colours, visibility and order come from the PDK's KLayout layer file (`sg13g2.lyp`),
  converted by `scripts/gen-layer-stack.sh` into `apps/web/src/pdk/ihp-sg13g2.json`.
- Drag to pan, wheel to zoom, double-click or `F` to fit; pin labels appear when zoomed in.
- The **DRC** tab lists violations from KLayout, Magic and the router, grouped by rule with the
  PDK's rule text; clicking one zooms to it.

### Reports

Setup and hold slack and TNS per corner, the worst setup and hold paths stage by stage (from
OpenSTA), and area by cell class.
