// "Why is it this value?": the assignment in the design that wrote a value, the if/case decisions
// that led to it and the values it read, from the simulator's record of what wrote each change.
// Port connections are followed through; the example's own inputs end the chain.

import { NO_ORIGIN, type SimSignal } from "../sim/protocol";
import type { Replay } from "./replay";

/** A value an explanation mentions, which can be explained in turn. */
export interface Read {
  /** As the person knows it: `count`, or `u_rx.state` inside a block. */
  name: string;
  full: string;
  /** In decimal, as examples write values. */
  value: string;
  /** The moment to explain it at; null if it isn't recorded (a memory). */
  t: number | null;
}

export interface Because {
  name: string;
  full: string;
  value: string;
  /** The first cycle that sees this value. */
  since: number;
  /** `design`: a line of the design wrote it; `input`: the example (or the testbench) set it;
   * `start`: nothing has written it. */
  kind: "design" | "input" | "start";
  /** For an input the example sets: the step. */
  step?: number;
  file?: string;
  line?: number;
  /** The assignment as written. */
  code?: string;
  /** The decisions that led to the assignment, outermost first, with the values they read. */
  conds: { line: number; code: string; taken: string; reads: Read[] }[];
  /** The values the assignment computed from. */
  reads: Read[];
}

/** The latest change of `s` at or before `t`. */
function changeAt(s: SimSignal, t: number): number {
  let lo = 0;
  let hi = s.times.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (s.times[mid] <= t) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/** A nonblocking assignment (`q <= d`), as opposed to `=` or a comparison inside one. */
const nonblocking = (code: string) => /^[^=<]*<=/.test(code);

/** Why `full` (a signal's full name) has the value it has at time `t`. */
export function why(rep: Replay, full: string, t: number): Because | null {
  const signals = new Map(rep.sim.signals.map((s) => [s.name, s]));
  const name = (f: string) => {
    const rest = f.startsWith(`${rep.top}.`) ? f.slice(rep.top.length + 1) : f;
    return rest.startsWith("dut.") ? rest.slice(4) : rest;
  };
  const read = (f: string, at: number): Read => {
    const s = signals.get(f);
    const i = s ? changeAt(s, at) : -1;
    return { name: name(f), full: f, value: s && i >= 0 ? s.values[i].toString() : "?", t: s && i >= 0 ? at : null };
  };
  const shown = full;
  let cur = full;
  let time = t;
  // Port connections pass a value on unchanged: follow them to where it was made.
  for (let hop = 0; hop < 32; hop++) {
    const s = signals.get(cur);
    if (!s) return null;
    const i = changeAt(s, time);
    if (i < 0) return null;
    const base = { name: name(shown), full: shown, value: s.values[i].toString(), since: rep.cycleAt(s.times[i]), conds: [], reads: [] };
    const by = s.by[i];
    if (by === NO_ORIGIN) return { ...base, kind: "start" };
    const o = rep.sim.origins[by];
    if (/^(input|output) \S+ of /.test(o.text) && o.reads.length === 1) {
      cur = o.reads[0];
      time = s.times[i];
      continue;
    }
    if (o.scope === rep.top) {
      const set = [...rep.sets].reverse().find((x) => x.signal === name(cur) && x.cycle <= base.since);
      return { ...base, kind: "input", step: set?.step, code: o.text };
    }
    // A nonblocking assignment computes from the values just before the clock edge that stores it.
    const at = nonblocking(o.text) ? Math.max(0, s.times[i] - 1) : s.times[i];
    return {
      ...base,
      kind: "design",
      file: o.file,
      line: o.line,
      code: o.text,
      conds: o.conds.map((c) => ({ line: c.line, code: c.text, taken: c.taken, reads: c.reads.map((r) => read(r, at)) })),
      reads: o.reads.map((r) => read(r, at)),
    };
  }
  return null;
}

/** How a decision went, in words: "`up && !down` was true", "`state` was 2'h1". */
export function decided(c: Because["conds"][number]): string {
  if (c.taken === "true" || c.taken === "false") return `\`${c.code}\` was ${c.taken}`;
  if (c.taken === "default") return `\`${c.code}\` matched none of the listed values`;
  return `\`${c.code}\` was ${c.taken.replace(/, /g, " or ")}`;
}

/** The explanation as one paragraph, for the AI: "count became 3 at cycle 7: line 13 of
 * src/updown.v, `count <= count + 1`, ran because `up && !down` was true (up = 1, down = 0); it
 * read count = 2." */
export function whyText(b: Because): string {
  const head = `${b.name} was ${b.value} from cycle ${b.since}`;
  if (b.kind === "start") return `${head}: nothing had written it.`;
  if (b.kind === "input") return `${head}: ${b.step ? `step ${b.step} of the example set it` : `the testbench set it (${b.code})`}.`;
  const vals = (rs: Read[]) => rs.map((r) => `${r.name} = ${r.value}`).join(", ");
  const conds = b.conds.map((c) => `${decided(c)}${c.reads.length ? ` (${vals(c.reads)})` : ""}`).join(", and ");
  return (
    `${head}: line ${b.line} of ${b.file}, \`${b.code}\`, wrote it` +
    (conds ? `, because ${conds}` : "") +
    (b.reads.length ? `; it read ${vals(b.reads)}` : "") +
    "."
  );
}
