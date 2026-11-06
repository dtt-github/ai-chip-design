// An example's steps as one plain sentence, for people who don't read testbenches: "After reset:
// start is set to 1 and data to 0xA5; 1 clock cycle passes; busy must be 1; ...".

import type { Spec, SpecExample, SpecStep } from "../api";

export function narrate(spec: Spec, ex: SpecExample): string {
  const tick = (n: number) => (spec.clock ? (n === 1 ? "1 clock cycle" : `${n} clock cycles`) : n === 1 ? "1 step" : `${n} steps`);
  const note = (s: SpecStep) => (s.note.trim() ? ` (${s.note.trim()})` : "");
  const parts: string[] = [];
  const steps = ex.steps;
  for (let i = 0; i < steps.length; i++) {
    const s = steps[i];
    // A run of "set x, wait n" with the same input and n (a serial line, a bit pattern) reads as
    // one clause: "rx takes the values 0, 0, 1, ..., 4 clock cycles each (start bit to bit 7)".
    let k = i;
    while (
      steps[k]?.op === "set" &&
      steps[k + 1]?.op === "wait" &&
      steps[k].signal === s.signal &&
      steps[k + 1].cycles === steps[i + 1]?.cycles
    ) {
      k += 2;
    }
    const runs = (k - i) / 2;
    if (s.op === "set" && runs >= 3) {
      const run = steps.slice(i, k).filter((x) => x.op === "set");
      const notes = run.map((x) => x.note.trim()).filter(Boolean);
      const span = notes.length ? ` (${notes.length > 1 ? `${notes[0]} to ${notes[notes.length - 1]}` : notes[0]})` : "";
      parts.push(`${s.signal} takes the values ${run.map((x) => x.value).join(", ")}, ${tick(Math.max(1, steps[i + 1].cycles))} each${span}`);
      i = k - 1;
      continue;
    }
    if (s.op === "set") {
      // Inputs set together read as one clause: "start is set to 1 and data to 0xA5".
      const group = [s];
      while (steps[i + 1]?.op === "set") group.push(steps[++i]);
      parts.push(
        group.map((g, k) => (k === 0 ? `${g.signal} is set to ${g.value}` : `${g.signal} to ${g.value}`) + note(g)).join(" and "),
      );
    } else if (s.op === "wait") {
      parts.push(`${tick(Math.max(1, s.cycles))} ${Math.max(1, s.cycles) === 1 ? "passes" : "pass"}${note(s)}`);
    } else if (s.op === "wait_until") {
      parts.push(`within ${tick(s.cycles)}, ${s.signal} must become ${s.value}${note(s)}`);
    } else if (s.op === "expect") {
      parts.push(`${s.signal} must be ${s.value}${note(s)}`);
    } else if (s.op === "expect_for") {
      parts.push(`${s.signal} must stay ${s.value} for ${tick(s.cycles)}${note(s)}`);
    }
  }
  const start = spec.reset ? "After reset" : "From the start";
  const target = ex.target && ex.target !== spec.top ? ` (on the whole chip ${ex.target})` : "";
  return `${start}${target}: ${parts.join("; ")}.`;
}
