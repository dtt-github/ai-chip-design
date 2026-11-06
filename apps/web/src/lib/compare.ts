// What a change does to the design's behaviour: every example simulated before and after it, and
// each output compared cycle by cycle (including the cycles after the example ends), described in
// plain words ("busy now changes 1 cycle earlier").

import type { Blocks, Spec } from "../api";
import type { SimClient } from "../sim/client";
import { extendTestbench, fmt, replay, targetSpec } from "./replay";

export interface ExampleChange {
  id: string;
  title: string;
  /** Whether the example passed before and after; null if it couldn't run. */
  before: boolean | null;
  after: boolean | null;
  /** One line per output that behaves differently. */
  differences: string[];
  problem: string | null;
}

/** How far a shifted copy of the same behaviour may be (cycles). */
const MAX_SHIFT = 5;

export async function compareExamples(
  client: SimClient,
  spec: Spec,
  chip: Blocks | null,
  before: Record<string, string>,
  after: Record<string, string>,
): Promise<ExampleChange[]> {
  const out: ExampleChange[] = [];
  for (const ex of spec.examples) {
    const tbPath = `tb/examples/tb_ex_${ex.id}.v`;
    const tb = before[tbPath];
    const tspec = targetSpec(spec, ex, chip);
    const change: ExampleChange = { id: ex.id, title: ex.title, before: null, after: null, differences: [], problem: null };
    if (!tb) {
      change.problem = "its testbench isn't in the project (save the spec)";
      out.push(change);
      continue;
    }
    try {
      const run = async (files: Record<string, string>) => {
        const list = Object.entries(files)
          .filter(([p]) => /\.s?v$/.test(p) && !p.startsWith("tb/"))
          .map(([name, source]) => ({ name, source }));
        list.push({ name: tbPath, source: extendTestbench(tb, tspec) });
        return replay(tspec, ex, await client.run(list, `tb_ex_${ex.id}`));
      };
      const b = await run(before);
      const a = await run(after);
      change.before = !b.firstFail && !b.problem;
      change.after = !a.firstFail && !a.problem;
      for (const rb of b.rows.filter((r) => r.dir === "out")) {
        const ra = a.rows.find((r) => r.name === rb.name);
        if (!ra) continue;
        const d = describe(rb.name, rb.width, rb.values, ra.values);
        if (d) change.differences.push(d);
      }
    } catch (e) {
      change.problem = (e as Error).message;
    }
    out.push(change);
  }
  return out;
}

/** How `after` differs from `before` for one output, or null if it doesn't. */
export function describe(name: string, width: number, before: (bigint | null)[], after: (bigint | null)[]): string | null {
  const n = Math.min(before.length, after.length);
  const differ: number[] = [];
  for (let c = 0; c < n; c++) if (before[c] !== null && after[c] !== null && before[c] !== after[c]) differ.push(c);
  if (!differ.length) return null;
  // The same behaviour moved in time: after[c] == before[c - k] wherever both are known.
  for (let k = 1; k <= MAX_SHIFT; k++) {
    for (const dir of [1, -1]) {
      const s = k * dir;
      let ok = true;
      let compared = 0;
      for (let c = Math.max(0, s); c < n && c - s < n; c++) {
        const x = after[c];
        const y = before[c - s];
        if (x === null || y === null) continue;
        compared++;
        if (x !== y) {
          ok = false;
          break;
        }
      }
      if (ok && compared > 0) {
        return `\`${name}\` now changes ${k} cycle${k === 1 ? "" : "s"} ${dir > 0 ? "later" : "earlier"} (first difference at cycle ${differ[0]})`;
      }
    }
  }
  const c = differ[0];
  return (
    `\`${name}\` at cycle ${c} was ${fmt(before[c], width)}, now ${fmt(after[c], width)}` +
    (differ.length > 1 ? ` (different in ${differ.length} cycles)` : "")
  );
}
