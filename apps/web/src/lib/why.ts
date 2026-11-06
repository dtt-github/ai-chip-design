// "Why is it this value?": the assignment in the design that wrote a value, the if/case decisions
// that led to it and the values it read, from the simulator's record of what wrote each change.
// Port connections are followed through, and a memory read to the word read and what wrote it;
// the example's own inputs end the chain.

import { NO_ORIGIN, type SimOrigin, type SimResult, type SimSignal } from "../sim/protocol";
import type { Replay } from "./replay";

/** A value an explanation mentions, which can be explained in turn. */
export interface Read {
  /** As the person knows it: `count`, or `u_rx.state` inside a block. */
  name: string;
  full: string;
  /** In decimal, as examples write values. */
  value: string;
  /** The moment to explain it at; null if it isn't recorded (a memory word the run didn't). */
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

/** For each origin that reads memory words: the entries of `wordReads` it made, in order. */
const readsByOrigin = new WeakMap<SimResult, Map<number, number[]>>();

/** The memory words origin `by` read the last time it ran at or before `time`, one per entry of
 * its `words`; null if the run didn't record it. */
function wordsRead(sim: SimResult, by: number, time: number): number[] | null {
  const n = sim.origins[by]?.words.length ?? 0;
  if (!n) return null;
  let index = readsByOrigin.get(sim);
  if (!index) {
    index = new Map();
    sim.wordReads.origins.forEach((o, j) => {
      const list = index!.get(o);
      if (list) list.push(j);
      else index!.set(o, [j]);
    });
    readsByOrigin.set(sim, index);
  }
  const list = index.get(by) ?? [];
  const { times, words } = sim.wordReads;
  let lo = 0;
  let hi = list.length - 1;
  let end = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (times[list[mid]] <= time) {
      end = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  // Each run records one entry per word, so the last `n` entries are its words.
  if (end + 1 < n) return null;
  const group = list.slice(end + 1 - n, end + 1).map((j) => words[j]);
  return group.includes(NO_ORIGIN) ? null : group;
}

/** How to look values up in a run: its signals and memory words by full name, and how a person
 * names them (`count`, `u_rx.state`, `mem[2]`). */
function lookup(rep: Replay) {
  const signals = new Map([...rep.sim.signals, ...rep.sim.words].map((s) => [s.name, s]));
  const name = (f: string) => {
    const rest = f.startsWith(`${rep.top}.`) ? f.slice(rep.top.length + 1) : f;
    return rest.startsWith("dut.") ? rest.slice(4) : rest;
  };
  const read = (f: string, at: number): Read => {
    const s = signals.get(f);
    const i = s ? changeAt(s, at) : -1;
    return { name: name(f), full: f, value: s && i >= 0 ? s.values[i].toString() : "?", t: s && i >= 0 ? at : null };
  };
  /** The values origin `by`, run at `ran`, computed from (as they were at `at`); a memory it
   * read is the words it read. */
  const reads = (by: number, ran: number, at: number): Read[] => {
    const o = rep.sim.origins[by];
    const got = wordsRead(rep.sim, by, ran);
    return o.reads.flatMap((r) => {
      if (!got || !o.words.includes(r)) return [read(r, at)];
      const words = [...new Set(got.filter((_, i) => o.words[i] === r))];
      return words.map((k) => read(rep.sim.words[k].name, at));
    });
  };
  return { signals, name, read, reads };
}

/** Why `full` (a signal's or memory word's full name) has the value it has at time `t`. */
export function why(rep: Replay, full: string, t: number): Because | null {
  const { signals, name, read, reads } = lookup(rep);
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
      reads: reads(by, s.times[i], at),
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

/** A line of the design that could have given a signal the value an example expected, and what
 * happened to it the last time it could have run. */
export interface Candidate {
  file: string;
  line: number;
  code: string;
  /** `blocked`: a decision went another way; `ran`: it ran (and computed something else, or a
   * later line changed the value again); `never`: nothing had decided it yet. */
  verdict: "blocked" | "ran" | "never";
  /** The cycle that last decision is seen at. */
  cycle?: number;
  /** The decisions that went its way, outermost first, then (for `blocked`) the one that didn't. */
  passed: Because["conds"];
  blocked?: Because["conds"][number];
  /** For `ran`: the values it computed from. */
  reads: Read[];
}

export interface WhyNot {
  name: string;
  full: string;
  expected: string;
  actual: string;
  cycle: number;
  /** The lines that write the signal and could have written the expected value. */
  candidates: Candidate[];
  /** Lines that write the signal but only another constant (`valid <= 0` when 1 was expected). */
  others: number;
  /** The run made too many decisions to record them all, and this moment is past the record. */
  unrecorded: boolean;
}

/** A Verilog number (`1`, `4'hf`, `8'd12`), or null for anything else. */
function constant(s: string): bigint | null {
  const v = s.trim().replace(/_/g, "").toLowerCase();
  try {
    const m = v.match(/^\d*'s?([hbod])([0-9a-f]+)$/);
    if (m) return BigInt({ h: "0x", b: "0b", o: "0o", d: "" }[m[1] as "h"] + m[2]);
    return /^\d+$/.test(v) ? BigInt(v) : null;
  } catch {
    return null;
  }
}

/** Why `full` wasn't `expected` at a cycle's check point: each line that could have written that
 * value, and the decision that kept it from running (or that it did run). */
export function whyNot(rep: Replay, full: string, cycle: number, expected: bigint): WhyNot | null {
  const t = rep.at(cycle);
  if (t === undefined) return null;
  const { signals, name, read, reads } = lookup(rep);
  // Through port connections to the signal the design writes.
  let cur = full;
  for (let hop = 0; hop < 32; hop++) {
    const s = signals.get(cur);
    const i = s ? changeAt(s, t) : -1;
    if (!s || i < 0 || s.by[i] === NO_ORIGIN) break;
    const o = rep.sim.origins[s.by[i]];
    if (!/^(input|output) \S+ of /.test(o.text) || o.reads.length !== 1) break;
    cur = o.reads[0];
  }
  const shown = signals.get(full);
  const at = shown ? changeAt(shown, t) : -1;
  const out: WhyNot = {
    name: name(full),
    full,
    expected: expected.toString(),
    actual: shown && at >= 0 ? shown.values[at].toString() : "?",
    cycle,
    candidates: [],
    others: 0,
    unrecorded: false,
  };

  const { times, branches, decisionOf, cut } = rep.sim.decisions;
  out.unrecorded = cut && (times.length === 0 || times[times.length - 1] < t);
  // The last decision `d` made at or before `t` (or, from `from` on, at time `at` exactly).
  const lastBefore = (d: number) => {
    let lo = 0;
    let hi = times.length - 1;
    let end = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (times[mid] <= t) {
        end = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    for (let j = end; j >= 0; j--) if (decisionOf[branches[j]] === d) return j;
    return -1;
  };
  const nextAt = (d: number, from: number) => {
    for (let j = from + 1; j < times.length && times[j] === times[from]; j++) if (decisionOf[branches[j]] === d) return j;
    return -1;
  };

  const seen = new Set<string>();
  for (const [by, o] of rep.sim.origins.entries()) {
    if (o.writes !== cur || /^(input|output) \S+ of /.test(o.text) || o.text.endsWith("(its starting value)")) continue;
    const key = `${o.file}:${o.line}:${o.text}`;
    if (seen.has(key)) continue;
    seen.add(key);
    // A line that only ever writes another constant can't be the one.
    const m = o.text.match(/<?=/);
    const k = m?.index === undefined ? null : constant(o.text.slice(m.index + m[0].length));
    if (k !== null && k !== expected) {
      out.others++;
      continue;
    }
    const nb = nonblocking(o.text);
    const valuesAt = (tm: number) => (nb ? Math.max(0, tm - 1) : tm);
    const cond = (c: SimOrigin["conds"][number], tm: number) => ({ line: c.line, code: c.text, taken: c.taken, reads: c.reads.map((r) => read(r, valuesAt(tm))) });
    const cand: Candidate = { file: o.file, line: o.line, code: o.text, verdict: "ran", passed: [], reads: [] };
    if (o.conds.length) {
      let j = lastBefore(decisionOf[o.conds[0].point]);
      if (j < 0) cand.verdict = "never";
      else {
        cand.cycle = rep.cycleAt(times[j]);
        for (let n = 0; n < o.conds.length; n++) {
          const c = o.conds[n];
          if (n > 0) j = nextAt(decisionOf[c.point], j);
          if (j < 0) break;
          if (branches[j] !== c.point) {
            cand.verdict = "blocked";
            cand.blocked = cond(c, times[j]);
            break;
          }
          cand.passed.push(cond(c, times[j]));
        }
        if (cand.verdict === "ran") cand.reads = reads(by, times[j], valuesAt(times[j]));
      }
    } else cand.reads = reads(by, t, valuesAt(t));
    out.candidates.push(cand);
  }
  return out;
}

/** How a decision went the other way: "`state == STOP` was false", "`state` wasn't DONE". */
export function missed(c: Because["conds"][number]): string {
  if (c.taken === "true") return `\`${c.code}\` was false`;
  if (c.taken === "false") return `\`${c.code}\` was true`;
  if (c.taken === "default") return `\`${c.code}\` matched one of the listed values`;
  return `\`${c.code}\` wasn't ${c.taken.replace(/, /g, " or ")}`;
}

/** The answer as one paragraph, for the AI. */
export function whyNotText(w: WhyNot): string {
  const vals = (rs: Read[]) => rs.map((r) => `${r.name} = ${r.value}`).join(", ");
  const parts = w.candidates.map((c) => {
    const head = `line ${c.line} of ${c.file}, \`${c.code}\``;
    if (c.verdict === "never") return `${head}, had never run by then`;
    if (c.verdict === "blocked" && c.blocked)
      return `${head}, didn't run for cycle ${c.cycle} because ${missed(c.blocked)}${c.blocked.reads.length ? ` (${vals(c.blocked.reads)})` : ""}`;
    return `${head}, ran${c.cycle !== undefined ? ` for cycle ${c.cycle}` : ""}${c.reads.length ? ` with ${vals(c.reads)}` : ""}`;
  });
  const head = `Why ${w.name} wasn't ${w.expected} at cycle ${w.cycle} (it was ${w.actual}):`;
  return parts.length ? `${head} ${parts.join("; ")}.` : `${head} no line of the design writes ${w.expected} to it.`;
}
