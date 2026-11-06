// "Try it": the design as a small board to play with. Inputs are switches, buttons and number
// boxes; outputs are lights and values; the clock moves when you say. Everything you do is
// recorded as example steps (set, wait, and the checks you tick), simulated from reset each time
// with the testbench the server would generate for that example, so what you see is exactly what
// the example will check once saved.

import { useEffect, useMemo, useRef, useState } from "react";
import { api, type Spec, type SpecExample, type SpecPort, type SpecStep } from "../api";
import { extendTestbench, replay, type Replay } from "../lib/replay";
import { SimClient } from "../sim/client";

/** Cycles of history shown under the board. */
const HISTORY = 40;

const step = (op: SpecStep["op"], signal = "", value = "", cycles = 0, note = ""): SpecStep => ({ op, signal, value, cycles, note });

export function TryIt({
  projectId,
  spec,
  canEdit,
  onSave,
  onClose,
}: {
  projectId: string;
  /** The spec being edited; its connections must match the committed design. */
  spec: Spec;
  canEdit: boolean;
  onSave: (ex: SpecExample) => void;
  onClose: () => void;
}) {
  // Each action is one or more steps; undo removes the last action.
  const [actions, setActions] = useState<SpecStep[][]>([]);
  const steps = useMemo(() => actions.flat(), [actions]);
  const [rep, setRep] = useState<Replay | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [title, setTitle] = useState("");
  const [buses, setBuses] = useState<Record<string, string>>({});
  const files = useRef<{ name: string; source: string }[] | null>(null);
  const client = useRef<SimClient | null>(null);
  const latest = useRef(0);

  const inputs = spec.ports.filter((p) => p.dir === "in" && p.name !== spec.clock?.port && p.name !== spec.reset?.port);
  const outputs = spec.ports.filter((p) => p.dir === "out");
  const now = steps.reduce((c, s) => c + (s.op === "wait" ? Math.max(1, s.cycles) : 0), 0);
  const current = (name: string): bigint => {
    for (let i = steps.length - 1; i >= 0; i--) if (steps[i].op === "set" && steps[i].signal === name) return parse(steps[i].value) ?? 0n;
    return 0n;
  };
  const checks = steps.filter((s) => s.op === "expect").length;

  useEffect(() => {
    client.current = new SimClient();
    return () => client.current?.dispose();
  }, []);

  // Simulate the session from reset after every action.
  useEffect(() => {
    const id = ++latest.current;
    const ex = session(steps.length ? steps : [step("wait", "", "", 1)]);
    setRunning(true);
    (async () => {
      if (!files.current) {
        // The committed design, without the project's testbenches.
        const snap = await api.snapshot(projectId);
        files.current = Object.entries(snap.files)
          .filter(([p]) => /\.s?v$/.test(p) && !p.startsWith("tb/"))
          .map(([name, source]) => ({ name, source }));
      }
      const tb = await api.exampleTestbench(projectId, { spec, example: ex });
      const sim = await client.current!.run([...files.current, { name: tb.path, source: extendTestbench(tb.source, spec) }], tb.top);
      if (id !== latest.current) return;
      setRep(replay(spec, ex, sim));
      setError(null);
    })()
      .catch((e: Error) => id === latest.current && setError(e.message))
      .finally(() => id === latest.current && setRunning(false));
  }, [projectId, spec, steps]);

  const value = (name: string, c = now): bigint | null => rep?.rows.find((r) => r.name === name)?.values[c] ?? null;
  const act = (...s: SpecStep[]) => setActions((a) => [...a, s]);
  /** Waits merge into the previous wait, so the steps stay short. */
  const wait = (n: number) =>
    setActions((a) => {
      const last = a[a.length - 1];
      if (last?.length === 1 && last[0].op === "wait") return [...a.slice(0, -1), [step("wait", "", "", last[0].cycles + n)]];
      return [...a, [step("wait", "", "", n)]];
    });
  const check = (p: SpecPort) => {
    const v = value(p.name);
    if (v !== null) act(step("expect", p.name, v.toString(), 0, "checked in Try it"));
  };

  function save() {
    const base = slug(title) || "tried";
    let id = base;
    for (let n = 2; spec.examples.some((e) => e.id === id); n++) id = `${base}_${n}`;
    onSave({ ...session(steps), id, title: title.trim() || "Tried in the playground" });
    setActions([]);
    setTitle("");
  }

  const tick = spec.clock ? "cycle" : "step";
  const from = Math.max(0, now - HISTORY + 1);
  const shown = rep ? rep.rows.filter((r) => r.name !== spec.reset?.port) : [];

  return (
    <section className="design-card tryit">
      <div className="design-row" style={{ justifyContent: "space-between" }}>
        <div>
          <h3 style={{ margin: 0 }}>Try it</h3>
          <p className="muted small" style={{ margin: 0 }}>
            Play with the design: set its inputs, move the clock, and tick the outputs that are right. What you do becomes an
            example.
          </p>
        </div>
        <button className="small" onClick={onClose}>
          Close
        </button>
      </div>

      <div className="tryit-board">
        <div className="tryit-side">
          <span className="design-label">Inputs</span>
          {inputs.map((p) =>
            p.width === 1 ? (
              <div key={p.name} className="tryit-io">
                <button
                  className={`tryit-switch ${current(p.name) ? "on" : ""}`}
                  aria-pressed={current(p.name) === 1n}
                  onClick={() => act(step("set", p.name, current(p.name) ? "0" : "1"))}
                  title={p.meaning}
                >
                  <code>{p.name}</code> {current(p.name).toString()}
                </button>
                {spec.clock && (
                  <button className="small" onClick={() => act(step("set", p.name, "1"), step("wait", "", "", 1), step("set", p.name, "0"))} title="1 for one cycle, then 0">
                    Pulse
                  </button>
                )}
              </div>
            ) : (
              <form
                key={p.name}
                className="tryit-io"
                onSubmit={(e) => {
                  e.preventDefault();
                  const v = parse(buses[p.name] ?? "");
                  if (v === null || v < 0n || v >= 1n << BigInt(p.width)) {
                    setError(`${p.name} takes a number from 0 to ${(1n << BigInt(p.width)) - 1n}`);
                    return;
                  }
                  act(step("set", p.name, buses[p.name].trim()));
                }}
              >
                <code title={p.meaning}>{p.name}</code>
                <input
                  className="mono"
                  aria-label={`Value for ${p.name}`}
                  placeholder={current(p.name).toString()}
                  value={buses[p.name] ?? ""}
                  onChange={(e) => setBuses({ ...buses, [p.name]: e.target.value })}
                  style={{ width: 90 }}
                />
                <button className="small">Set</button>
              </form>
            ),
          )}
          {inputs.length === 0 && <span className="muted small">No inputs besides the clock and reset.</span>}
        </div>

        <div className="tryit-clock">
          <span className="design-label">{spec.clock ? `Clock, ${spec.clock.mhz} MHz` : "Time"}</span>
          <strong>
            {tick} {now}
          </strong>
          <div className="design-row">
            {[1, 5, 20].map((n) => (
              <button key={n} className="small" onClick={() => wait(n)}>
                +{n}
              </button>
            ))}
          </div>
          {running && <span className="muted small">Simulating…</span>}
        </div>

        <div className="tryit-side">
          <span className="design-label">Outputs</span>
          {outputs.map((p) => {
            const v = value(p.name);
            return (
              <div key={p.name} className="tryit-io">
                {p.width === 1 ? (
                  <span className={`tryit-led ${v === 1n ? "on" : ""}`} aria-label={`${p.name} is ${v ?? "unknown"}`} />
                ) : null}
                <code title={p.meaning}>{p.name}</code>
                <span className="mono">{v === null ? "?" : p.width === 1 ? v.toString() : `${v.toString()} (0x${v.toString(16)})`}</span>
                <button className="small" onClick={() => check(p)} disabled={v === null} title={`The example will check that ${p.name} is ${v} here`}>
                  ✓ Right
                </button>
              </div>
            );
          })}
          {outputs.length > 1 && (
            <button className="small" onClick={() => outputs.forEach(check)} disabled={!rep}>
              ✓ All outputs are right
            </button>
          )}
        </div>
      </div>

      {error && <p className="error small">{error}</p>}

      {rep && (
        <div className="tryit-history" aria-label={`The last ${now - from + 1} ${tick}s`}>
          {shown.map((r) => (
            <div key={r.name} className="tryit-row">
              <code className="tryit-name">{r.name}</code>
              <div className="tryit-cells">
                {r.values.slice(from, now + 1).map((v, k) => {
                  const prev = k > 0 ? r.values[from + k - 1] : null;
                  return (
                    <span
                      key={k}
                      className={`tryit-cell ${r.width === 1 ? (v === 1n ? "hi" : "lo") : "bus"} ${from + k === now ? "now" : ""}`}
                      title={`${tick} ${from + k}: ${v ?? "?"}`}
                    >
                      {r.width > 1 && v !== prev ? (v ?? "?").toString() : ""}
                    </span>
                  );
                })}
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="tryit-steps">
        <span className="design-label">
          What you did ({steps.length} step{steps.length === 1 ? "" : "s"}, {checks} check{checks === 1 ? "" : "s"})
        </span>
        <ol className="small">
          {steps.map((s, i) => (
            <li key={i}>{describe(s, tick)}</li>
          ))}
        </ol>
        <div className="design-row">
          <button className="small" onClick={() => setActions((a) => a.slice(0, -1))} disabled={!actions.length}>
            Undo
          </button>
          <button className="small" onClick={() => setActions([])} disabled={!actions.length}>
            Start over
          </button>
        </div>
      </div>

      {canEdit && (
        <form
          className="design-row"
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <input aria-label="Example title" placeholder="What this shows, e.g. A press turns the LED on" value={title} onChange={(e) => setTitle(e.target.value)} style={{ flex: 1 }} />
          <button className="primary" disabled={!checks || !!error} title={checks ? undefined : "Tick at least one output as right first"}>
            Save as an example
          </button>
        </form>
      )}
      {canEdit && !checks && <p className="muted small">Tick ✓ Right on an output when it shows what it should; those become the example's checks.</p>}
    </section>
  );
}

function session(steps: SpecStep[]): SpecExample {
  return { id: "try_it", title: "Try it", given: "", expect: "", params: [], steps };
}

function describe(s: SpecStep, tick: string): string {
  if (s.op === "set") return `set ${s.signal} to ${s.value}`;
  if (s.op === "wait") return `${s.cycles} ${tick}${s.cycles === 1 ? "" : "s"} pass`;
  return `check ${s.signal} is ${s.value}`;
}

function parse(v: string): bigint | null {
  const s = v.trim().replace(/_/g, "").toLowerCase();
  if (!s) return null;
  try {
    if (/^0[xb][0-9a-f]+$/.test(s) || /^\d+$/.test(s)) return BigInt(s);
  } catch {
    return null;
  }
  return null;
}

function slug(t: string): string {
  return t
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 40);
}
