// "The design is right": when an example fails because the example is off (a check a cycle too
// early, a wait too short, a wrong value), change the example to what the design did, without the
// AI. Each fix is the smallest that matches the simulation; the fixed example is simulated again
// (with the testbench the server generates) until it passes, so what is offered is known to pass.

import type { SpecExample, SpecStep } from "../api";
import type { Replay } from "./replay";

export interface Fix {
  steps: SpecStep[];
  /** What changed, step by step, in words. */
  edits: string[];
  /** Whether the example passes with all the edits. */
  passes: boolean;
}

/** How far a value may have moved in time and still count as "a few cycles off". */
const MAX_SHIFT = 5;

const wait = (cycles: number, note = ""): SpecStep => ({ op: "wait", signal: "", value: "", cycles, note });

/** `v` written the way `like` was (0x.., 0b.. or decimal). */
function writeLike(like: string, v: bigint): string {
  const t = like.trim().toLowerCase();
  if (t.startsWith("0x") || /'h/.test(t)) return `0x${v.toString(16).toUpperCase()}`;
  if (t.startsWith("0b") || /'b/.test(t)) return `0b${v.toString(2)}`;
  return v.toString();
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** The smallest change to `steps` that makes the first failing check match the simulation. */
export function oneFix(steps: SpecStep[], rep: Replay): { steps: SpecStep[]; edit: string } | null {
  const f = rep.firstFail;
  if (!f) return null;
  const i = f.step - 1;
  const st = steps[i];
  if (!st) return null;
  const at = `Step ${f.step} (${st.op.replace("_", " ")} ${st.signal} = ${st.value})`;
  const read = (c: number) => (c >= 0 ? rep.read(f.signal, c) : null);
  const next = steps.map((s) => ({ ...s }));

  if (f.op === "expect" || (f.op === "expect_for" && f.failCycle === f.from)) {
    const n = f.op === "expect_for" ? f.to - f.from + 1 : 1;
    const holds = (c: number) => Array.from({ length: n }, (_, k) => read(c + k)).every((v) => v === f.expected);
    for (let d = 1; d <= MAX_SHIFT; d++) {
      // A little later: wait before the check.
      if (holds(f.from + d)) {
        next.splice(i, 0, wait(d, "the design is this much slower"));
        return { steps: next, edit: `${at}: checked ${plural(d, "cycle")} later` };
      }
      // A little earlier: shorten the wait just before it.
      const prev = next[i - 1];
      if (prev?.op === "wait" && prev.cycles > d && holds(f.from - d)) {
        prev.cycles -= d;
        return { steps: next, edit: `${at}: checked ${plural(d, "cycle")} earlier (the wait before it is shorter)` };
      }
    }
  }
  if (f.op === "expect_for" && f.failCycle !== undefined && f.failCycle > f.from) {
    // It holds, just not for as long: keep what holds, and keep the rest of the example's timing.
    const kept = f.failCycle - f.from;
    const total = f.to - f.from + 1;
    next[i].cycles = kept;
    next.splice(i + 1, 0, wait(total - kept));
    return { steps: next, edit: `${at}: must stay ${st.value} for ${plural(kept, "cycle")} instead of ${total}` };
  }
  if (f.op === "wait_until") {
    // It does become the value, just later than the limit.
    for (let c = f.from; c <= f.from + st.cycles + 20; c++) {
      if (read(c) === f.expected) {
        next[i].cycles = Math.max(1, c - f.from);
        return { steps: next, edit: `${at}: waits up to ${plural(next[i].cycles, "cycle")} instead of ${st.cycles}` };
      }
    }
    return null;
  }
  // Otherwise the value itself: what the design had.
  const seen = f.op === "expect_for" ? read(f.from) : f.actual;
  if (seen === null || seen === undefined) return null;
  if (f.op === "expect_for") {
    const n = f.to - f.from + 1;
    if (!Array.from({ length: n }, (_, k) => read(f.from + k)).every((v) => v === seen)) return null;
  }
  next[i].value = writeLike(st.value, seen);
  return { steps: next, edit: `${at}: expects ${st.signal} = ${next[i].value} instead of ${st.value}` };
}

/**
 * Fixes the example one failing check at a time, simulating it again after each fix, until it
 * passes or no simple fix is left (null if there wasn't even one).
 */
export async function findFix(
  ex: SpecExample,
  first: Replay,
  simulate: (ex: SpecExample) => Promise<Replay>,
  maxRounds = 6,
): Promise<Fix | null> {
  let steps = ex.steps;
  let rep = first;
  const edits: string[] = [];
  for (let round = 0; round < maxRounds; round++) {
    if (!rep.firstFail && !rep.problem) return { steps, edits, passes: true };
    const f = oneFix(steps, rep);
    if (!f) break;
    steps = f.steps;
    edits.push(f.edit);
    rep = await simulate({ ...ex, steps });
  }
  if (!edits.length) return null;
  return { steps, edits, passes: !rep.firstFail && !rep.problem };
}
