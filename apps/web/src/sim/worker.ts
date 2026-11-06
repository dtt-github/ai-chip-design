// Runs the Rust simulator (WASM) off the main thread.
import init, { simulate, topCandidates, type Simulation } from "../wasm/wafer_sim.js";
import wasmUrl from "../wasm/wafer_sim_bg.wasm?url";
import type { SimRequest, SimResponse } from "./protocol";

const ready = init({ module_or_path: wasmUrl });
let last: Simulation | null = null;

self.onmessage = async (ev: MessageEvent<SimRequest>) => {
  await ready;
  const req = ev.data;
  const reply = (r: SimResponse, transfer: Transferable[] = []) => (self as unknown as Worker).postMessage(r, transfer);
  try {
    if (req.type === "tops") {
      reply({ type: "tops", id: req.id, tops: topCandidates(req.names, req.sources) });
    } else if (req.type === "vcd") {
      reply({ type: "vcd", id: req.id, vcd: last ? last.vcd() : "" });
    } else {
      const t0 = performance.now();
      last?.free();
      last = simulate(req.names, req.sources, req.top ?? undefined, req.maxTime);
      const ms = performance.now() - t0;
      const names = last.names;
      const widths = last.widths;
      const decisions = {
        times: last.decisionTimes(),
        branches: last.decisionBranches(),
        decisionOf: last.branchDecision(),
        cut: last.decisionsCut,
      };
      const signals = names.map((name, i) => ({ name, width: widths[i], times: last!.times(i), values: last!.values(i), by: last!.by(i) }));
      reply(
        {
          type: "result",
          id: req.id,
          result: {
            log: last.log,
            time: last.time,
            finished: last.finished,
            stopped: last.stopped ?? null,
            warnings: last.warnings,
            timescaleFs: last.timescaleFs,
            steps: last.steps,
            ms,
            signals,
            origins: JSON.parse(last.origins()),
            decisions,
          },
        },
        [...signals.flatMap((s) => [s.times.buffer, s.values.buffer, s.by.buffer]), decisions.times.buffer, decisions.branches.buffer],
      );
    }
  } catch (e) {
    reply({ type: "error", id: req.id, error: e instanceof Error ? e.message : String(e) });
  }
};
