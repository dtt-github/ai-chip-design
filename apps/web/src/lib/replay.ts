// Replays an example's steps against a simulation of its generated testbench,
// cycle by cycle, the way the testbench itself runs them (crates/sc-api/src/spec.rs):
// everything happens on the falling clock edge, after reset is held for two
// cycles (one idle tick when there is no reset). Gives what each connection
// actually did per cycle, every check with its outcome, and the first
// difference explained in plain words.

import type { Spec, SpecExample } from "../api";
import type { SimResult, SimSignal } from "../sim/protocol";

export interface Row {
  name: string;
  dir: "in" | "out";
  width: number;
  /** Value at each cycle's check point; null if the simulation ended before it. */
  values: (bigint | null)[];
}

export interface Check {
  step: number;
  op: "expect" | "expect_for" | "wait_until";
  signal: string;
  expected: bigint;
  /** First and last cycle the check covers. */
  from: number;
  to: number;
  ok: boolean;
  /** Where it failed and what was there. */
  failCycle?: number;
  actual?: bigint | null;
}

export interface Replay {
  /** Cycles shown: 0 .. cycles - 1. */
  cycles: number;
  /** Cycles the example itself covers; the rest are shown for context. */
  exampleCycles: number;
  rows: Row[];
  checks: Check[];
  /** Inputs the example sets: cycle and connection. */
  sets: { cycle: number; signal: string; value: bigint }[];
  firstFail: Check | null;
  /** Plain-language account of the first difference (or of the pass). */
  story: string;
  /** Set when the replay couldn't follow the run (e.g. it stopped early). */
  problem: string | null;
}

const MAX_CYCLES = 4000;
/** Cycles the browser run continues past the example, to see what came next. */
export const EXTRA_CYCLES = 20;
const SHOWN_AFTER = 6;

/** The generated testbench, made to run EXTRA_CYCLES longer before it finishes. */
export function extendTestbench(source: string, spec: Spec): string {
  const periodPs = spec.clock ? Math.max(1, Math.round(500_000 / spec.clock.mhz)) * 2 : 10_000;
  return source.replace(/(\n\s*)\$finish;(\s*\n\s*end\s*\nendmodule)/, `$1#${EXTRA_CYCLES * periodPs} $finish;$2`);
}

function parse(v: string): bigint | null {
  const s = v.trim().replace(/_/g, "").toLowerCase();
  try {
    if (s.startsWith("0x")) return BigInt(s);
    if (s.startsWith("0b")) return BigInt(s);
    const m = s.match(/^\d*'s?([hbod])([0-9a-f]+)$/);
    if (m) return BigInt({ h: "0x", b: "0b", o: "0o", d: "" }[m[1] as "h"] + m[2]);
    return /^\d+$/.test(s) ? BigInt(s) : null;
  } catch {
    return null;
  }
}

/** Value of `sig` just before time `t` (outputs: what the testbench sees) or at `t` (inputs, after a set). */
function valueAt(sig: SimSignal, t: number, inclusive: boolean): bigint | null {
  let lo = 0;
  let hi = sig.times.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    const tm = sig.times[mid];
    if (tm < t || (inclusive && tm === t)) {
      found = mid;
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found < 0 ? null : sig.values[found];
}

export function fmt(v: bigint | null | undefined, width: number): string {
  if (v === null || v === undefined) return "?";
  if (width === 1) return v.toString();
  return `0x${v.toString(16)} (${v.toString()})`;
}

export function replay(spec: Spec, ex: SpecExample, sim: SimResult): Replay {
  const top = `tb_ex_${ex.id}`;
  const byName = new Map(sim.signals.map((s) => [s.name, s]));
  const sig = (port: string) => byName.get(`${top}.${port}`) ?? byName.get(port);

  // Check points: the clock's falling edges, or fixed ticks without a clock.
  let tickTimes: number[] = [];
  const clk = spec.clock ? sig(spec.clock.port) : undefined;
  if (spec.clock && clk) {
    for (let i = 1; i < clk.times.length; i++) if (clk.values[i - 1] === 1n && clk.values[i] === 0n) tickTimes.push(clk.times[i]);
  } else {
    const periodPs = spec.clock ? Math.max(1, Math.round(500_000 / spec.clock.mhz)) * 2 : 10_000;
    const unit = (periodPs * 1000) / sim.timescaleFs;
    tickTimes = Array.from({ length: Math.floor(sim.time / unit) }, (_, i) => (i + 1) * unit);
  }
  const prologue = spec.reset ? 2 : 1;
  const timeOf = (c: number) => tickTimes[prologue + c - 1];

  const portOf = new Map(spec.ports.map((p) => [p.name, p]));
  const read = (port: string, c: number): bigint | null => {
    const s = sig(port);
    const t = timeOf(c);
    if (!s || t === undefined) return null;
    return valueAt(s, t, portOf.get(port)?.dir === "in");
  };

  const checks: Check[] = [];
  const sets: Replay["sets"] = [];
  let c = 0;
  let problem: string | null = null;
  ex.steps.forEach((st, i) => {
    if (problem) return;
    const n = i + 1;
    const expected = parse(st.value) ?? 0n;
    if (st.op === "set") sets.push({ cycle: c, signal: st.signal, value: expected });
    else if (st.op === "wait") c += Math.max(1, st.cycles);
    else if (st.op === "expect") {
      const actual = read(st.signal, c);
      checks.push({ step: n, op: "expect", signal: st.signal, expected, from: c, to: c, ok: actual === expected, failCycle: actual === expected ? undefined : c, actual });
    } else if (st.op === "expect_for") {
      const k0 = c;
      let fail: number | undefined;
      let actual: bigint | null = null;
      for (let k = 0; k < st.cycles; k++) {
        const v = read(st.signal, c + k);
        if (v !== expected) {
          fail = c + k;
          actual = v;
          break;
        }
      }
      checks.push({ step: n, op: "expect_for", signal: st.signal, expected, from: k0, to: k0 + st.cycles - 1, ok: fail === undefined, failCycle: fail, actual });
      c += Math.max(1, st.cycles);
    } else if (st.op === "wait_until") {
      const k0 = c;
      let k = 0;
      while (read(st.signal, c) !== expected && k < st.cycles) {
        c++;
        k++;
      }
      const v = read(st.signal, c);
      checks.push({ step: n, op: "wait_until", signal: st.signal, expected, from: k0, to: c, ok: v === expected, failCycle: v === expected ? undefined : c, actual: v });
    }
    if (c > MAX_CYCLES) problem = `the example runs longer than ${MAX_CYCLES} cycles; only the start is shown`;
  });
  if (!problem && timeOf(Math.max(0, c - 1)) === undefined) problem = "the simulation stopped before the example finished";

  const exampleCycles = Math.min(c + 1, MAX_CYCLES);
  let cycles = exampleCycles;
  while (cycles < exampleCycles + SHOWN_AFTER && timeOf(cycles) !== undefined) cycles++;
  // The clock and reset are the same in every example; leave them out unless a step uses them.
  const used = new Set(ex.steps.map((st) => st.signal));
  const shown = spec.ports.filter((p) => p.name !== spec.clock?.port && (p.name !== spec.reset?.port || used.has(p.name)));
  const rows: Row[] = shown.map((p) => ({
    name: p.name,
    dir: p.dir,
    width: p.width,
    values: Array.from({ length: cycles }, (_, k) => read(p.name, k)),
  }));
  const firstFail = checks.find((ch) => !ch.ok) ?? null;
  return { cycles, exampleCycles, rows, checks, sets, firstFail, story: explain(spec, read, firstFail, checks.length), problem };
}

function explain(spec: Spec, read: (port: string, c: number) => bigint | null, f: Check | null, total: number): string {
  if (!f) return total ? `All ${total} check${total === 1 ? "" : "s"} matched what the design did.` : "This example has no checks.";
  const port = spec.ports.find((p) => p.name === f.signal);
  const w = port?.width ?? 1;
  const name = port?.meaning ? `${f.signal} (${port.meaning.replace(/\.$/, "")})` : f.signal;
  const want = fmt(f.expected, w);
  const got = fmt(f.actual, w);
  const at = f.failCycle ?? f.from;

  if (f.op === "wait_until") {
    return `Step ${f.step} waited up to ${f.to - f.from} cycles, from cycle ${f.from}, for ${name} to become ${want}. It never did; it was still ${got} at cycle ${at}.`;
  }
  let s =
    f.op === "expect_for"
      ? `Step ${f.step} expected ${name} to stay ${want} for ${f.to - f.from + 1} cycles from cycle ${f.from}. At cycle ${at}, ${at - f.from} cycle${at - f.from === 1 ? "" : "s"} in, it was ${got}.`
      : `At cycle ${at} (step ${f.step}) the example expected ${name} to be ${want}, but the design had ${got}.`;
  // The most common cause is timing that is off by a few cycles: say when the value did appear.
  if (f.op === "expect") {
    for (let d = 1; d <= EXTRA_CYCLES; d++) {
      if (read(f.signal, at + d) === f.expected) {
        s += ` It became ${want} ${d === 1 ? "one cycle" : `${d} cycles`} later, at cycle ${at + d}, so the design may be ${d === 1 ? "one cycle" : `${d} cycles`} slower than the example says.`;
        return s;
      }
      if (at - d >= 0 && read(f.signal, at - d) === f.expected) {
        s += ` It was already ${want} ${d === 1 ? "one cycle" : `${d} cycles`} earlier, at cycle ${at - d}, so the design may be ${d === 1 ? "one cycle" : `${d} cycles`} ahead of the example.`;
        return s;
      }
    }
    s += ` It wasn't ${want} at any point within ${EXTRA_CYCLES} cycles either side.`;
  }
  return s;
}
