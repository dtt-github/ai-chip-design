import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  api,
  isFinished,
  type Blocks,
  type ExampleResult,
  type FindTarget,
  type Job,
  type MistakesReport,
  type SmallFix,
  type SmallFixes,
  type Spec,
  type SpecExample,
  type SpecRun,
  type SpecStep,
  type SpecView,
  type StepOp,
} from "../api";
import { extendTestbench, fmt as fmtValue, replay, targetSpec, type Replay } from "../lib/replay";
import { findFix, type Fix } from "../lib/fixexample";
import { decided, missed, why, whyNot, whyNotText, whyText, type Because, type Read, type WhyNot } from "../lib/why";
import { StateMachine } from "./StateMachine";
import { TryIt } from "./TryIt";
import { SimClient } from "../sim/client";
import { compareExamples } from "../lib/compare";
import { BlocksStep } from "./Blocks";

type Step = "describe" | "examples" | "blocks" | "health";

interface Props {
  projectId: string;
  /** Description to start a project without a spec with (from the starter ideas). */
  startIntent?: string | null;
  /** Asks the page for the full width (the Blocks canvas). */
  onWide?: (wide: boolean) => void;
  head: string | null;
  canEdit: boolean;
  /** Opens a file in the code view. */
  onOpenFile: (path: string) => void;
  onShowRun: (job: Job) => void;
  onShowProposal: (id: string) => void;
}

const OPS: { op: StepOp; label: string }[] = [
  { op: "set", label: "Set input" },
  { op: "wait", label: "Wait" },
  { op: "wait_until", label: "Wait until" },
  { op: "expect", label: "Expect" },
  { op: "expect_for", label: "Expect for" },
];

const VERDICT: Record<string, [string, string]> = {
  pass: ["Passes", "status-succeeded"],
  fail: ["Fails", "status-failed"],
  no_verdict: ["No result", "status-failed"],
  error: ["Can't run", "status-failed"],
  missing: ["Not saved yet", "status-queued"],
};

function blankSpec(top: string, mhz: number | null): Spec {
  return {
    version: 1,
    intent: "",
    top,
    clock: { port: "clk", mhz: mhz ?? 50 },
    reset: { port: "rst_n", active_low: true },
    ports: [
      { name: "clk", dir: "in", width: 1, meaning: "Clock" },
      { name: "rst_n", dir: "in", width: 1, meaning: "Reset: 0 resets, 1 runs" },
    ],
    params: [],
    decisions: [],
    questions: [],
    examples: [],
  };
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** Intent-first design: describe the design, give examples (they become the tests), check its health. */
export function DesignFlow({ projectId, startIntent, onWide, head, canEdit, onOpenFile, onShowRun, onShowProposal }: Props) {
  const [step, setStep] = useState<Step>("describe");
  useEffect(() => {
    onWide?.(step === "blocks");
    return () => onWide?.(false);
  }, [step, onWide]);
  const [view, setView] = useState<SpecView | null>(null);
  const [draft, setDraft] = useState<Spec | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [run, setRun] = useState<SpecRun | null>(null);
  // From Health: text for "Add an example in words", or a found example to select.
  const [exampleSeed, setExampleSeed] = useState<{ text?: string; select?: string } | null>(null);
  const [budget, setBudget] = useState<{ today_usd: number; limit_usd: number | null } | null>(null);
  const refreshBudget = useCallback(() => {
    api.aiUsage().then(setBudget, () => setBudget(null));
  }, []);
  useEffect(refreshBudget, [refreshBudget]);

  // The last spec loaded from the server: a draft that differs from it has unsaved edits to keep.
  const saved = useRef<Spec | null>(null);
  const load = useCallback(async () => {
    const v = await api.getSpec(projectId);
    setView(v);
    // A starting description from the projects page counts as the starting point, not an edit.
    const blank = blankSpec(v.design_name ?? "top", v.clock_mhz);
    const start = startIntent ? { ...blank, intent: startIntent } : blank;
    setDraft((d) => (d && saved.current && !same(d, saved.current) ? d : (v.spec ?? start)));
    saved.current = v.spec ?? start;
    if (v.spec && v.spec.examples.length) api.runSpec(projectId).then(setRun, () => setRun(null));
    else setRun(null);
    // The starting description only applies when the project is first opened.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  useEffect(() => {
    load().catch((e: Error) => setError(e.message));
  }, [load, head]);

  const dirty = !!draft && !same(draft, saved.current);

  async function ask(instruction: string) {
    if (!draft) return;
    setBusy("The assistant is working on the spec…");
    setError(null);
    try {
      const r = await api.draftSpec(projectId, { instruction, spec: view?.spec || draft.intent ? draft : null });
      setDraft(r.spec);
      setNote(`${r.note} (${Math.round(r.elapsed_ms / 1000)} s)`);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
      refreshBudget();
    }
  }

  async function save() {
    if (!draft || !view) return;
    setBusy("Saving and compiling the examples…");
    setError(null);
    try {
      await api.saveSpec(projectId, { spec: draft, parent: view.head });
      const v = await api.getSpec(projectId);
      setView(v);
      setDraft(v.spec);
      saved.current = v.spec;
      setRun(v.spec?.examples.length ? await api.runSpec(projectId) : null);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function runExamples() {
    setBusy("Running the examples…");
    setError(null);
    try {
      setRun(await api.runSpec(projectId));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function propose(request: string) {
    setBusy("Asking the agent for a proposal…");
    setError(null);
    try {
      const p = await api.propose(projectId, request);
      onShowProposal(p.id);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function makeItPass() {
    if (!run) return;
    const failing = run.examples.filter((e) => e.verdict !== "pass");
    const list = failing.map((e) => `- ${e.id} (${e.title}): ${e.reason ?? e.log.find((l) => l.includes("FAIL")) ?? e.verdict}`).join("\n");
    const request =
      `Update the design so every example in spec.json passes. The examples are compiled into tb/examples/tb_ex_*.v; ` +
      `read spec.json and those testbenches, and change the RTL (not the spec or the examples).\n` +
      (failing.length ? `Failing now:\n${list}` : "All examples pass now; check the design still matches the spec's description.");
    await propose(request);
  }

  if (!view || !draft) return <div className="center muted">{error ?? "Loading the spec…"}</div>;
  const update = (patch: Partial<Spec>) => setDraft({ ...draft, ...patch });
  const results = new Map((run?.examples ?? []).map((r) => [r.id, r]));
  const steps: [Step, string, string][] = [
    ["describe", "Describe", "What it should do"],
    ["examples", "Examples", "How you know it works"],
    ["blocks", "Blocks", "What it is built from"],
    ["health", "Health", "Is it ready"],
  ];

  return (
    <div className="design">
      <nav className="design-steps" aria-label="Design steps">
        {steps.map(([id, label, sub], i) => (
          <button key={id} className={`design-step ${step === id ? "on" : ""}`} onClick={() => setStep(id)} aria-current={step === id ? "step" : undefined}>
            <span className="design-num">{i + 1}</span>
            <span>
              <strong>{label}</strong>
              <span className="muted small">{sub}</span>
            </span>
          </button>
        ))}
      </nav>

      {(dirty || busy || error || note || view.problems.length > 0) && (
        <div className="design-bar">
          {busy && <span className="muted">{busy}</span>}
          {!busy && dirty && <span className="warn">Unsaved changes to the spec.</span>}
          {!busy && note && (
            <details className="design-note">
              <summary>
                <strong>Assistant:</strong> {note.length > 140 ? `${note.slice(0, 140).trimEnd()}…` : note}
              </summary>
              {note.length > 140 && <p>{note}</p>}
            </details>
          )}
          {error && <span className="error" style={{ whiteSpace: "pre-line" }}>{error}</span>}
          {view.problems.length > 0 && <span className="error">The saved spec has problems: {view.problems.join("; ")}</span>}
          {!busy && note && (
            <button className="ghost small" onClick={() => setNote(null)} aria-label="Dismiss the assistant's note">
              ×
            </button>
          )}
          <span className="spacer" />
          {dirty && canEdit && (
            <>
              <button className="small" onClick={() => setDraft(saved.current)} disabled={!!busy}>
                Discard
              </button>
              <button className="primary small" onClick={save} disabled={!!busy}>
                Save spec
              </button>
            </>
          )}
        </div>
      )}

      <div className="design-body">
        {step === "describe" && !view.spec && <FirstRun />}
        {step === "describe" && <Describe spec={draft} update={update} canEdit={canEdit} busy={!!busy} hasSaved={!!view.spec} ask={ask} budget={budget} />}
        {step === "examples" && (
          <Examples
            spec={draft}
            update={update}
            canEdit={canEdit}
            busy={!!busy}
            results={results}
            ran={run}
            dirty={dirty}
            ask={ask}
            runExamples={runExamples}
            makeItPass={makeItPass}
            onOpenFile={onOpenFile}
            seed={exampleSeed}
            projectId={projectId}
            head={view.head}
            chip={view.chip}
            fixDesign={(ex, story) =>
              propose(
                `Example "${ex.title}" (${ex.id}) in spec.json fails. What happened in simulation: ${story}\n` +
                  `Change the RTL so this example passes, keeping every other example passing. Don't change spec.json or tb/examples/.`,
              )
            }
            fixExample={(ex, story) =>
              ask(
                `The person thinks example "${ex.title}" (id ${ex.id}) is wrong, not the design. What happened in simulation: ${story} ` +
                  `Check the example against the description and decisions. If the example is wrong, correct its steps (keep its id) and say what you changed; ` +
                  `if the example is right and the design is wrong, leave it and say so.`,
              )
            }
          />
        )}
        {step === "blocks" && <BlocksStep projectId={projectId} head={head} canEdit={canEdit} />}
        {step === "health" && (
          <Health
            projectId={projectId}
            head={head}
            specTop={view.spec?.top ?? null}
            clockMhz={view.spec?.clock?.mhz ?? view.clock_mhz}
            run={run}
            canEdit={canEdit}
            onShowRun={onShowRun}
            onOpenFile={onOpenFile}
            onAddExample={(text) => {
              setExampleSeed({ text });
              setStep("examples");
            }}
            onFoundExample={(ex) => {
              if (!draft) return;
              // Unsaved found examples may already use the id the server picked.
              const ids = new Set(draft.examples.map((e) => e.id));
              let id = ex.id;
              for (let n = 1; ids.has(id); n++) id = `found_${n}`;
              update({ examples: [...draft.examples, { ...ex, id }] });
              setExampleSeed({ select: id });
              setStep("examples");
            }}
          />
        )}
      </div>
    </div>
  );
}

// ---- First run --------------------------------------------------------------------

const GUIDE_KEY = "wafersmith.guide.dismissed";

/** How the three steps work, until dismissed (remembered in this browser). */
function FirstRun() {
  const [hidden, setHidden] = useState(() => {
    try {
      return localStorage.getItem(GUIDE_KEY) === "1";
    } catch {
      return false;
    }
  });
  if (hidden) return null;
  const steps = [
    ["Describe", "Say what the chip should do, in your own words. The assistant turns it into a spec and asks about anything unclear. Answer its questions; nothing is saved until you click Save."],
    ["Examples", "Each example is a test: set an input, wait, expect an output. The AI writes the design until every example passes, and you can see what the design actually did, cycle by cycle."],
    ["Health", "Build it into a chip layout and check, in plain words, that it works, is fast enough, can be manufactured, and how big it is."],
  ];
  return (
    <section className="design-card first-run">
      <div className="design-row" style={{ justifyContent: "space-between" }}>
        <h3>How this works</h3>
        <button
          className="ghost small"
          onClick={() => {
            setHidden(true);
            try {
              localStorage.setItem(GUIDE_KEY, "1");
            } catch {
              /* private mode: just hide it for now */
            }
          }}
        >
          Got it
        </button>
      </div>
      <ol className="first-run-steps">
        {steps.map(([t, d]) => (
          <li key={t}>
            <strong>{t}</strong>
            <span className="small">{d}</span>
          </li>
        ))}
      </ol>
      <p className="muted small" style={{ margin: 0 }}>
        You never have to read Verilog. If you can, the Code tab shows the files, and everything the AI changes waits for a person to merge it.
      </p>
    </section>
  );
}

// ---- Describe ---------------------------------------------------------------------

function Describe({
  spec,
  update,
  canEdit,
  busy,
  hasSaved,
  ask,
  budget,
}: {
  spec: Spec;
  update: (p: Partial<Spec>) => void;
  canEdit: boolean;
  busy: boolean;
  hasSaved: boolean;
  ask: (instruction: string) => void;
  budget: { today_usd: number; limit_usd: number | null } | null;
}) {
  const [request, setRequest] = useState("");
  const [answers, setAnswers] = useState<Record<number, string>>({});
  const ro = !canEdit;
  const setPort = (i: number, patch: Partial<Spec["ports"][number]>) =>
    update({ ports: spec.ports.map((p, j) => (j === i ? { ...p, ...patch } : p)) });
  const setParam = (i: number, patch: Partial<Spec["params"][number]>) =>
    update({ params: spec.params.map((p, j) => (j === i ? { ...p, ...patch } : p)) });

  return (
    <div className="design-col">
      <section className="design-card">
        <label className="design-label" htmlFor="intent">
          What should it do?
        </label>
        <textarea
          id="intent"
          className="design-text"
          value={spec.intent}
          readOnly={ro}
          placeholder="Explain it the way you would to a colleague. For example: send bytes from my design over a serial line like a PC's COM port, with optional even parity."
          onChange={(e) => update({ intent: e.target.value })}
        />
        {canEdit && (
          <div className="design-row">
            <button className="primary" disabled={busy || !spec.intent.trim()} onClick={() => ask(hasSaved ? "Update the spec to match the description above." : "Write the first spec from the description above.")}>
              {hasSaved || spec.examples.length ? "Update spec with the assistant" : "Draft the spec with the assistant"}
            </button>
            <span className="muted small">
              The assistant fills in the connections, settings and examples, and asks about anything unclear. It takes about a minute; nothing is
              saved until you save.
              {budget?.limit_usd != null && (
                <>
                  {" "}
                  Your AI use today: ${budget.today_usd.toFixed(2)} of ${budget.limit_usd.toFixed(2)}.
                </>
              )}
            </span>
          </div>
        )}
      </section>

      {spec.questions.length > 0 && (
        <section className="design-card design-questions">
          <h3>Questions from the assistant</h3>
          <p className="muted small" style={{ margin: 0 }}>
            The description leaves these open. Examples that depend on them are added once you answer.
          </p>
          {spec.questions.map((q, i) => (
            <div key={q.question} className="design-question">
              <p>{q.question}</p>
              <div className="design-row">
                {q.options.map((o, k) => (
                  <button
                    key={o}
                    className={k === 0 ? "primary" : ""}
                    disabled={busy || ro}
                    onClick={() => ask(`Answer to the open question "${q.question}": ${o}`)}
                    title={k === 0 ? "The assistant's suggestion" : undefined}
                  >
                    {o}
                    {k === 0 ? " (suggested)" : ""}
                  </button>
                ))}
              </div>
              {canEdit && (
                <div className="design-row">
                  <input
                    aria-label={`Your own answer to: ${q.question}`}
                    placeholder="Or answer in your own words"
                    value={answers[i] ?? ""}
                    onChange={(e) => setAnswers({ ...answers, [i]: e.target.value })}
                  />
                  <button disabled={busy || !answers[i]?.trim()} onClick={() => ask(`Answer to the open question "${q.question}": ${answers[i]}`)}>
                    Answer
                  </button>
                </div>
              )}
            </div>
          ))}
        </section>
      )}

      <section className="design-card">
        <h3>Connections</h3>
        <div className="design-scroll">
          <table className="design-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Direction</th>
                <th>Bits</th>
                <th>What it means</th>
                <th aria-label="Remove" />
              </tr>
            </thead>
            <tbody>
              {spec.ports.map((p, i) => (
                <tr key={i}>
                  <td>
                    <input className="mono" aria-label="Connection name" value={p.name} readOnly={ro} onChange={(e) => setPort(i, { name: e.target.value })} />
                  </td>
                  <td>
                    <select aria-label="Direction" value={p.dir} disabled={ro} onChange={(e) => setPort(i, { dir: e.target.value as "in" | "out" })}>
                      <option value="in">in</option>
                      <option value="out">out</option>
                    </select>
                  </td>
                  <td>
                    <input aria-label="Bits" type="number" min={1} max={64} value={p.width} readOnly={ro} onChange={(e) => setPort(i, { width: Number(e.target.value) })} style={{ width: 64 }} />
                  </td>
                  <td>
                    <input aria-label="Meaning" value={p.meaning} readOnly={ro} onChange={(e) => setPort(i, { meaning: e.target.value })} />
                  </td>
                  <td>
                    {canEdit && (
                      <button className="ghost small" aria-label={`Remove ${p.name}`} onClick={() => update({ ports: spec.ports.filter((_, j) => j !== i) })}>
                        ×
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {canEdit && (
          <button className="small" onClick={() => update({ ports: [...spec.ports, { name: "", dir: "in", width: 1, meaning: "" }] })}>
            + Connection
          </button>
        )}
        <div className="design-grid">
          <label className="design-field">
            <span className="design-label">Clock</span>
            <span className="design-row">
              <select
                aria-label="Clock connection"
                value={spec.clock?.port ?? ""}
                disabled={ro}
                onChange={(e) => update({ clock: e.target.value ? { port: e.target.value, mhz: spec.clock?.mhz ?? 50 } : null })}
              >
                <option value="">No clock</option>
                {spec.ports.filter((p) => p.dir === "in").map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.name}
                  </option>
                ))}
              </select>
              {spec.clock && (
                <>
                  <input aria-label="Clock MHz" type="number" min={1} value={spec.clock.mhz} readOnly={ro} onChange={(e) => update({ clock: { ...spec.clock!, mhz: Number(e.target.value) } })} style={{ width: 80 }} />
                  <span className="muted small">MHz</span>
                </>
              )}
            </span>
          </label>
          <label className="design-field">
            <span className="design-label">Reset</span>
            <span className="design-row">
              <select
                aria-label="Reset connection"
                value={spec.reset?.port ?? ""}
                disabled={ro}
                onChange={(e) => update({ reset: e.target.value ? { port: e.target.value, active_low: spec.reset?.active_low ?? true } : null })}
              >
                <option value="">No reset</option>
                {spec.ports.filter((p) => p.dir === "in").map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.name}
                  </option>
                ))}
              </select>
              {spec.reset && (
                <select aria-label="Reset level" value={spec.reset.active_low ? "low" : "high"} disabled={ro} onChange={(e) => update({ reset: { ...spec.reset!, active_low: e.target.value === "low" } })}>
                  <option value="low">0 resets</option>
                  <option value="high">1 resets</option>
                </select>
              )}
            </span>
          </label>
        </div>
      </section>

      <section className="design-card">
        <h3>Settings</h3>
        {spec.params.length === 0 && <p className="muted small">No settings. The assistant adds them when the design has options, like parity or speed.</p>}
        {spec.params.map((p, i) => (
          <div key={i} className="design-row design-param">
            <input className="mono" aria-label="Setting name" value={p.name} readOnly={ro} onChange={(e) => setParam(i, { name: e.target.value })} style={{ width: 140 }} />
            <input className="mono" aria-label="Value" value={p.value} readOnly={ro} onChange={(e) => setParam(i, { value: e.target.value })} style={{ width: 80 }} />
            <input aria-label="Meaning" value={p.meaning} readOnly={ro} onChange={(e) => setParam(i, { meaning: e.target.value })} style={{ flex: 1 }} />
          </div>
        ))}
      </section>

      {spec.decisions.length > 0 && (
        <section className="design-card">
          <h3>Decisions</h3>
          <ul className="design-list">
            {spec.decisions.map((d) => (
              <li key={d.question}>
                <span className="muted">{d.question}</span> <strong>{d.answer}</strong>
              </li>
            ))}
          </ul>
        </section>
      )}

      {canEdit && (
        <section className="design-card">
          <label className="design-label" htmlFor="change">
            Ask for a change
          </label>
          <div className="design-row">
            <input id="change" placeholder="e.g. Make it run at 115200 baud" value={request} onChange={(e) => setRequest(e.target.value)} />
            <button
              disabled={busy || !request.trim()}
              onClick={() => {
                ask(request);
                setRequest("");
              }}
            >
              Ask
            </button>
          </div>
        </section>
      )}
    </div>
  );
}

// ---- Examples ---------------------------------------------------------------------

function Examples({
  spec,
  update,
  canEdit,
  busy,
  results,
  ran,
  dirty,
  ask,
  runExamples,
  makeItPass,
  onOpenFile,
  projectId,
  head,
  chip,
  fixDesign,
  fixExample,
  seed,
}: {
  /** Text to start "Add an example in words" with, or an example to select (a new object each time). */
  seed: { text?: string; select?: string } | null;
  projectId: string;
  head: string;
  /** The chip from the Blocks view, which examples can target. */
  chip: Blocks | null;
  fixDesign: (ex: SpecExample, story: string) => void;
  fixExample: (ex: SpecExample, story: string) => void;
  spec: Spec;
  update: (p: Partial<Spec>) => void;
  canEdit: boolean;
  busy: boolean;
  results: Map<string, ExampleResult>;
  ran: SpecRun | null;
  dirty: boolean;
  ask: (instruction: string) => void;
  runExamples: () => void;
  makeItPass: () => void;
  onOpenFile: (path: string) => void;
}) {
  const [selId, setSelId] = useState<string | null>(null);
  const [words, setWords] = useState("");
  const [trying, setTrying] = useState(false);
  const [tried, setTried] = useState<string | null>(null);
  useEffect(() => {
    if (!seed) return;
    if (seed.select) {
      setSelId(seed.select);
      return;
    }
    setWords(seed.text ?? "");
    window.setTimeout(() => {
      const box = document.getElementById("words") as HTMLTextAreaElement | null;
      box?.scrollIntoView({ block: "center" });
      box?.focus();
    }, 0);
  }, [seed]);
  const sel = spec.examples.find((e) => e.id === selId) ?? spec.examples[0];
  const passing = spec.examples.filter((e) => results.get(e.id)?.verdict === "pass").length;
  const setEx = (patch: Partial<SpecExample>) => update({ examples: spec.examples.map((e) => (e.id === sel.id ? { ...e, ...patch } : e)) });
  const setStep = (i: number, patch: Partial<SpecStep>) => setEx({ steps: sel.steps.map((s, j) => (j === i ? { ...s, ...patch } : s)) });
  const ro = !canEdit;
  // What the selected example drives: the design, or the whole chip through its pins.
  const tspec = sel ? targetSpec(spec, sel, chip) : spec;
  const chipExamples = chip ? spec.examples.filter((e) => e.target === chip.top).length : 0;

  return (
    <div className="design-col">
      <section className="design-card design-row" style={{ justifyContent: "space-between" }}>
        <div>
          <h3 style={{ margin: 0 }}>Examples are the tests</h3>
          <p className="muted small" style={{ margin: 0 }}>
            Each example is compiled into a testbench. A change, yours or the AI's, is only accepted when every example passes.
          </p>
        </div>
        <div className="design-row">
          {ran && (
            <span className={`status ${passing === spec.examples.length && spec.examples.length ? "status-succeeded" : "status-failed"}`}>
              {passing} of {spec.examples.length} passing
            </span>
          )}
          <button onClick={() => setTrying(!trying)} aria-expanded={trying} title="Play with the design and turn what you do into an example">
            Try it
          </button>
          <button onClick={runExamples} disabled={busy || dirty || spec.examples.length === 0} title={dirty ? "Save the spec first" : undefined}>
            Run examples
          </button>
          {canEdit && ran && (
            <button className="primary" onClick={makeItPass} disabled={busy || dirty}>
              {passing === spec.examples.length ? "Ask the AI to review the design" : "Ask the AI to make them pass"}
            </button>
          )}
        </div>
      </section>

      {trying && (
        <TryIt
          projectId={projectId}
          spec={spec}
          machines={ran?.machines ?? []}
          chip={chip}
          canEdit={canEdit}
          onClose={() => setTrying(false)}
          onSave={(ex) => {
            update({ examples: [...spec.examples, ex] });
            setSelId(ex.id);
            setTried(ex.title);
            setTrying(false);
          }}
        />
      )}
      {tried && dirty && (
        <p className="design-row small">
          Added “{tried}”. <strong>Save the spec</strong> to keep it; it then runs with the other examples.
        </p>
      )}

      <div className="design-split">
        <section className="design-card design-examples">
          {spec.examples.length === 0 && <p className="muted small">No examples yet. Describe one in words below, or let the assistant draft them from the description.</p>}
          {spec.examples.map((e, i) => {
            const r = results.get(e.id);
            const [label, cls] = dirty ? ["Not saved", "status-queued"] : r ? VERDICT[r.verdict] ?? [r.verdict, ""] : ["Not run", "status-queued"];
            return (
              <button key={e.id} className={`design-ex ${e.id === sel?.id ? "on" : ""}`} onClick={() => setSelId(e.id)}>
                <span className="design-num">{i + 1}</span>
                <span className="design-ex-text">
                  <strong>
                    {e.title}
                    {chip && e.target === chip.top && <span className="design-chip-tag">chip</span>}
                  </strong>
                  <span className="muted small">{e.expect || e.given}</span>
                </span>
                <span className={`status ${cls}`}>{label}</span>
              </button>
            );
          })}
          {canEdit && (
            <div className="design-add">
              <label className="design-label" htmlFor="words">
                Add an example in words
              </label>
              <textarea
                id="words"
                className="design-text"
                placeholder="e.g. If start is pressed while a byte is still going out, it is ignored."
                value={words}
                onChange={(e) => setWords(e.target.value)}
              />
              {chip && chipExamples === 0 && (
                <div className="design-chip-hint">
                  <span className="small">
                    These examples test <code>{spec.top}</code>. The chip <code>{chip.top}</code> built in Blocks has none of its own, so nothing
                    checks it as a whole.
                  </span>
                  <button
                    className="primary small"
                    disabled={busy}
                    onClick={() =>
                      ask(
                        `Add examples that test the whole chip ${chip.top} through its pins (target "${chip.top}"), covering its main behaviour end to end, including the timing of everything between the pins and ${spec.top}.`,
                      )
                    }
                  >
                    Add chip examples with the assistant
                  </button>
                </div>
              )}
              <div className="design-row">
                <button
                  disabled={busy || !words.trim()}
                  onClick={() => {
                    ask(`Add an example: ${words}`);
                    setWords("");
                  }}
                >
                  Add with the assistant
                </button>
                <button
                  disabled={busy}
                  onClick={() => {
                    const n = spec.examples.length + 1;
                    const id = `example_${n}`;
                    update({ examples: [...spec.examples, { id, title: `Example ${n}`, given: "", expect: "", params: [], steps: [] }] });
                    setSelId(id);
                  }}
                >
                  Add a blank one
                </button>
              </div>
            </div>
          )}
        </section>

        {sel && (
          <section className="design-card design-detail">
            <input className="design-title" aria-label="Example title" value={sel.title} readOnly={ro} onChange={(e) => setEx({ title: e.target.value })} />
            {chip && (
              <label className="design-row small">
                <span className="muted">Tests</span>
                <select
                  aria-label="What this example tests"
                  value={sel.target === chip.top ? chip.top : ""}
                  disabled={ro}
                  onChange={(e) => setEx({ target: e.target.value || undefined, params: e.target.value ? [] : sel.params })}
                >
                  <option value="">the design {spec.top}</option>
                  <option value={chip.top}>the whole chip {chip.top} (its pins)</option>
                </select>
              </label>
            )}
            <div className="design-ge">
              <label htmlFor="given">Given</label>
              <input id="given" value={sel.given} readOnly={ro} onChange={(e) => setEx({ given: e.target.value })} />
              <label htmlFor="expect">Expect</label>
              <input id="expect" value={sel.expect} readOnly={ro} onChange={(e) => setEx({ expect: e.target.value })} />
            </div>
            {sel.params.length > 0 && (
              <p className="small">
                Settings for this example: {sel.params.map((p) => <code key={p.name}>{`${p.name} = ${p.value} `}</code>)}
              </p>
            )}
            <Timeline spec={tspec} ex={sel} />
            <ExampleResultView r={dirty ? undefined : results.get(sel.id)} />
            {!dirty && results.get(sel.id) && results.get(sel.id)!.verdict !== "missing" && (
              <Happened
                key={`${sel.id}@${head}`}
                projectId={projectId}
                spec={tspec}
                ex={sel}
                canEdit={canEdit}
                busy={busy}
                onFixDesign={(story) => fixDesign(sel, story)}
                onFixExample={(story) => fixExample(sel, story)}
                baseSpec={spec}
                onReplaceSteps={(steps) => setEx({ steps })}
              />
            )}
            <div className="design-scroll">
              <table className="design-table design-steps-table">
                <thead>
                  <tr>
                    <th>#</th>
                    <th>Step</th>
                    <th>Connection</th>
                    <th>Value</th>
                    <th>Cycles</th>
                    <th>Note</th>
                    <th aria-label="Remove" />
                  </tr>
                </thead>
                <tbody>
                  {sel.steps.map((s, i) => (
                    <tr key={i}>
                      <td className="muted">{i + 1}</td>
                      <td>
                        <select aria-label="Step" value={s.op} disabled={ro} onChange={(e) => setStep(i, { op: e.target.value as StepOp })}>
                          {OPS.map((o) => (
                            <option key={o.op} value={o.op}>
                              {o.label}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td>
                        {s.op !== "wait" && (
                          <select aria-label="Connection" value={s.signal} disabled={ro} onChange={(e) => setStep(i, { signal: e.target.value })}>
                            <option value="">…</option>
                            {tspec.ports
                              .filter((p) => (s.op === "set" ? p.dir === "in" : true) && p.name !== tspec.clock?.port)
                              .map((p) => (
                                <option key={p.name} value={p.name}>
                                  {p.name}
                                </option>
                              ))}
                          </select>
                        )}
                      </td>
                      <td>{s.op !== "wait" && <input className="mono" aria-label="Value" value={s.value} readOnly={ro} onChange={(e) => setStep(i, { value: e.target.value })} style={{ width: 80 }} />}</td>
                      <td>
                        {(s.op === "wait" || s.op === "wait_until" || s.op === "expect_for") && (
                          <input aria-label="Cycles" type="number" min={1} value={s.cycles} readOnly={ro} onChange={(e) => setStep(i, { cycles: Number(e.target.value) })} style={{ width: 72 }} />
                        )}
                      </td>
                      <td>
                        <input aria-label="Note" value={s.note} readOnly={ro} onChange={(e) => setStep(i, { note: e.target.value })} />
                      </td>
                      <td>
                        {canEdit && (
                          <button className="ghost small" aria-label={`Remove step ${i + 1}`} onClick={() => setEx({ steps: sel.steps.filter((_, j) => j !== i) })}>
                            ×
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="design-row">
              {canEdit && (
                <>
                  <button className="small" onClick={() => setEx({ steps: [...sel.steps, { op: "expect", signal: "", value: "", cycles: 0, note: "" }] })}>
                    + Step
                  </button>
                  <button className="small" onClick={() => update({ examples: spec.examples.filter((e) => e.id !== sel.id) })}>
                    Remove example
                  </button>
                </>
              )}
              <span className="spacer" />
              <button className="link small" onClick={() => onOpenFile(`tb/examples/tb_ex_${sel.id}.v`)} disabled={dirty}>
                View the generated testbench
              </button>
            </div>
          </section>
        )}
      </div>
    </div>
  );
}

function ExampleResultView({ r }: { r: ExampleResult | undefined }) {
  if (!r || r.verdict === "pass") return null;
  const fails = r.log.filter((l) => l.startsWith("FAIL"));
  return (
    <div className="design-fail">
      <strong>{VERDICT[r.verdict]?.[0] ?? r.verdict}</strong>
      {r.reason && <span>{r.reason}</span>}
      {fails.slice(0, 5).map((l) => (
        <code key={l}>{l}</code>
      ))}
    </div>
  );
}

/** The example's testbench simulated in the browser: what each connection did, with the checks over it. */
function Happened({
  projectId,
  spec,
  ex,
  canEdit,
  busy,
  onFixDesign,
  onFixExample,
  baseSpec,
  onReplaceSteps,
}: {
  projectId: string;
  /** The spec as this example sees it (the chip's pins for a chip example). */
  spec: Spec;
  /** The project's spec, from which the server compiles a changed example. */
  baseSpec: Spec;
  ex: SpecExample;
  canEdit: boolean;
  busy: boolean;
  onFixDesign: (story: string) => void;
  onFixExample: (story: string) => void;
  onReplaceSteps: (steps: SpecExample["steps"]) => void;
}) {
  const [rep, setRep] = useState<Replay | null>(null);
  // "Why is it this value?": the values clicked and asked about, each explained, in order.
  const [trail, setTrail] = useState<Because[]>([]);
  const ask = (full: string, t: number, after: number) => {
    const b = rep && why(rep, full, t);
    if (b) setTrail((tr) => [...tr.slice(0, after), b]);
  };
  // "Why not?": why the first failing check didn't see the value the example expected.
  const [notWhy, setNotWhy] = useState<WhyNot | null>(null);
  // "Try a small fix": repairs the server found, what each does to the outputs, and the commit
  // they apply to.
  const [small, setSmall] = useState<{ result: SmallFixes; effects: string[][]; parent: string | null } | "working" | null>(null);
  const [smallError, setSmallError] = useState<string | null>(null);
  async function trySmallFix() {
    if (!rep) return;
    setSmall("working");
    setSmallError(null);
    try {
      // The lines "Why not?" and "Why?" point at are tried first.
      const f = rep.firstFail;
      const w = notWhy ?? (f && f.failCycle !== undefined ? whyNot(rep, `${rep.top}.${f.signal}`, f.failCycle, f.expected) : null);
      const focus = [
        ...(w?.candidates ?? []).flatMap((c) => [c.line, ...c.passed.map((p) => p.line), ...(c.blocked ? [c.blocked.line] : [])].map((line) => ({ file: c.file, line }))),
        ...trail.flatMap((b) => (b.file && b.line ? [b.line, ...b.conds.map((c) => c.line)].map((line) => ({ file: b.file!, line })) : [])),
      ];
      const result = await api.smallFix(projectId, focus);
      const snap = await api.snapshot(projectId);
      const client = new SimClient();
      const effects: string[][] = [];
      try {
        for (const fx of result.fixes) {
          if (!fx.edit) {
            effects.push([]);
            continue;
          }
          const changes = await compareExamples(client, baseSpec, null, snap.files, { ...snap.files, [fx.file]: fx.edit.content });
          effects.push(changes.flatMap((c) => c.differences.map((d) => `${c.title}: ${d}`)));
        }
      } finally {
        client.dispose();
      }
      setSmall({ result, effects, parent: snap.commit });
    } catch (e) {
      setSmallError((e as Error).message);
      setSmall(null);
    }
  }
  async function applySmallFix(fx: SmallFix, parent: string | null) {
    if (!fx.edit) return;
    try {
      await api.commit(projectId, {
        message: `Small fix: line ${fx.line} of ${fx.file.split("/").pop()}, ${fx.original} → ${fx.replacement}`,
        parent,
        changes: [{ op: "write", path: fx.file, content: fx.edit.content }],
      });
      setSmall(null);
    } catch (e) {
      setSmallError((e as Error).message);
    }
  }
  // "The design is right": a change to the example that makes it pass, worked out and re-checked.
  const [fix, setFix] = useState<Fix | "working" | "none" | null>(null);
  async function acceptDesign() {
    if (!rep) return;
    setFix("working");
    const client = new SimClient();
    try {
      const snap = await api.snapshot(projectId);
      const design = Object.entries(snap.files)
        .filter(([p]) => /\.s?v$/.test(p) && !p.startsWith("tb/"))
        .map(([name, source]) => ({ name, source }));
      const simulate = async (changed: SpecExample) => {
        const tb = await api.exampleTestbench(projectId, { spec: baseSpec, example: changed });
        const sim = await client.run([...design, { name: tb.path, source: extendTestbench(tb.source, spec) }], tb.top);
        return replay(spec, changed, sim);
      };
      setFix((await findFix(ex, rep, simulate)) ?? "none");
    } catch (e) {
      setError((e as Error).message);
      setFix(null);
    } finally {
      client.dispose();
    }
  }
  const [error, setError] = useState<string | null>(null);
  const [pickedCycle, setPickedCycle] = useState<number | null>(null);
  // Fit the cycles to the space available; long examples scroll.
  const box = useRef<HTMLDivElement>(null);
  const [room, setRoom] = useState(600);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setRoom(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [rep]);

  useEffect(() => {
    const client = new SimClient();
    let cancelled = false;
    (async () => {
      // The committed files: the same ones "Run examples" and the gate use.
      const snap = await api.snapshot(projectId);
      const files = Object.entries(snap.files)
        .filter(([p]) => /\.s?v$/.test(p))
        .map(([name, source]) => ({ name, source: name === `tb/examples/tb_ex_${ex.id}.v` ? extendTestbench(source, spec) : source }));
      const sim = await client.run(files, `tb_ex_${ex.id}`);
      if (!cancelled) {
        setRep(replay(spec, ex, sim));
        setTrail([]);
        setNotWhy(null);
      }
    })().catch((e: Error) => !cancelled && setError(e.message));
    return () => {
      cancelled = true;
      client.dispose();
    };
  }, [projectId, spec, ex]);

  if (error) return <p className="error small">Couldn't simulate this example: {error}</p>;
  if (!rep) return <p className="muted small">Simulating what the design does…</p>;
  const cell = Math.max(6, Math.min(56, Math.floor((room - 84) / Math.max(rep.cycles, 1))));
  const width = rep.cycles * cell;
  const every = rep.cycles > 200 ? 50 : rep.cycles > 80 ? 20 : rep.cycles > 30 ? 10 : 5;
  const failing = rep.firstFail;
  // The AI gets the explanation too, when someone followed one.
  const traced = [
    notWhy && whyNotText(notWhy),
    ...trail.map(whyText),
    small && typeof small === "object" && !small.result.fixes.length && `Small changes tried without the AI: ${small.result.message}`,
  ].filter(Boolean);
  const story = traced.length ? `${rep.story} Why, traced in the simulation: ${traced.join(" ")}` : rep.story;
  const picked = trail[0];

  return (
    <div className="happened">
      <div className="design-row">
        <strong>What actually happened</strong>
        <span className={`status ${failing ? "status-failed" : "status-succeeded"}`}>{failing ? "differs from the example" : "matches the example"}</span>
      </div>
      <p className={`small ${failing ? "" : "muted"}`} style={{ margin: 0 }}>
        {rep.story}{" "}
        {failing && failing.failCycle !== undefined && !notWhy && (
          <button
            className="link small"
            onClick={() => setNotWhy(whyNot(rep, `${rep.top}.${failing.signal}`, failing.failCycle!, failing.expected))}
          >
            Why wasn't it {failing.expected.toString()}?
          </button>
        )}
      </p>
      {notWhy && (
        <WhyNotPanel
          w={notWhy}
          onAsk={(r) => {
            if (r.t === null) return;
            setPickedCycle(null);
            ask(r.full, r.t, 0);
          }}
          onClose={() => setNotWhy(null)}
        />
      )}
      {rep.problem && <p className="warn small">Note: {rep.problem}.</p>}
      <div className="design-scroll" ref={box}>
        <div className="hp-grid" style={{ gridTemplateColumns: `72px ${width}px` }}>
          <span />
          <svg width={width} height={16} aria-hidden="true">
            {Array.from({ length: Math.floor((rep.cycles - 1) / every) + 1 }, (_, i) => i * every).map((c) => (
              <text key={c} x={c * cell + 1} y={11} className="hp-axis">
                {c}
              </text>
            ))}
          </svg>
          {rep.rows.map((r) => {
            const after = (rep.cycles - rep.exampleCycles) * cell;
            const checks = rep.checks.filter((c) => c.signal === r.name);
            return (
              <HpRow
                key={r.name}
                row={r}
                cell={cell}
                width={width}
                after={after}
                checks={checks}
                sets={rep.sets.filter((s) => s.signal === r.name).map((s) => s.cycle)}
                picked={picked?.name === r.name ? pickedCycle : null}
                onPick={(c) => {
                  const t = rep.at(c);
                  if (t === undefined) return;
                  setPickedCycle(c);
                  ask(`${rep.top}.${r.name}`, t, 0);
                }}
              />
            );
          })}
        </div>
      </div>
      <p className="muted small" style={{ margin: 0 }}>
        Cycle by cycle, from the simulator. Amber: what the example checks; red: where it differs; blue marks: inputs the example sets;
        shaded: the cycles after the example ends. <strong>Click any value to see why it is what it is.</strong>
      </p>
      {trail.length > 0 && (
        <WhyPanel
          trail={trail}
          onAsk={(r, level) => r.t !== null && ask(r.full, r.t, level + 1)}
          onClose={() => {
            setTrail([]);
            setPickedCycle(null);
          }}
        />
      )}
      {failing && canEdit && (
        <div className="design-row">
          <span className="small">Which is wrong?</span>
          <button className="primary" disabled={busy} onClick={() => onFixDesign(story)}>
            The design: ask the AI to fix it
          </button>
          <button disabled={busy || fix === "working"} onClick={acceptDesign} title="Change the example to what the design did, without the AI">
            The example: use what the design did
          </button>
          <button disabled={busy} onClick={() => onFixExample(story)}>
            The example: ask the assistant to correct it
          </button>
          <button
            disabled={busy || small === "working"}
            onClick={trySmallFix}
            title="Try small changes to the design (a comparison, a constant one off, a condition the other way), checking every example after each; no AI"
          >
            The design: try a small fix
          </button>
        </div>
      )}
      {small === "working" && <p className="muted small">Trying small changes to the design, running every example after each…</p>}
      {smallError && <p className="error small">Couldn't try small fixes: {smallError}</p>}
      {small && typeof small === "object" && (
        <div className="design-card fix-preview">
          <div className="design-row" style={{ justifyContent: "space-between" }}>
            <strong className="small">{small.result.fixes.length ? "Small changes to the design that make every example pass" : "No small fix"}</strong>
            <button className="link small" onClick={() => setSmall(null)}>
              Close
            </button>
          </div>
          <p className="muted small" style={{ margin: 0 }}>
            {small.result.message}
            {!small.result.fixes.length && " Ask the AI to fix the design; it is told what was tried."}
          </p>
          {small.result.fixes.map((fx, i) => (
            <div key={i} className="small-fix">
              <div className="small">{withCode(fx.text)}</div>
              {fx.edit && (
                <pre className="small-fix-diff mono">
                  <span className="del">- {fx.edit.before.trim()}</span>
                  {"\n"}
                  <span className="add">+ {fx.edit.after.trim()}</span>
                </pre>
              )}
              {small.effects[i]?.length > 0 && <span className="muted small">What it changes in the examples:</span>}
              {small.effects[i]?.length > 0 && (
                <ul className="small">
                  {small.effects[i].map((d) => (
                    <li key={d}>{withCode(d)}</li>
                  ))}
                </ul>
              )}
              <button
                className={i === 0 ? "primary small" : "small"}
                disabled={!fx.edit || busy}
                title={fx.edit ? undefined : "This change spans lines or is written differently; make it in the code"}
                onClick={() => applySmallFix(fx, small.parent)}
              >
                Use this change
              </button>
            </div>
          ))}
        </div>
      )}
      {fix === "working" && <p className="muted small">Working out the change and checking it…</p>}
      {fix === "none" && (
        <p className="small">
          There's no simple change (a check a few cycles off, a wait too short, a different value) that makes this example pass.
          Ask the assistant, or the AI to fix the design.
        </p>
      )}
      {fix && typeof fix === "object" && (
        <div className="design-card fix-preview">
          <strong className="small">{fix.passes ? "This change makes the example pass:" : "This change gets closer, but the example still fails:"}</strong>
          <ul className="small">
            {fix.edits.map((e) => (
              <li key={e}>{e}</li>
            ))}
          </ul>
          <div className="design-row">
            <button
              className="primary small"
              disabled={!fix.passes}
              onClick={() => {
                onReplaceSteps(fix.steps);
                setFix(null);
              }}
            >
              Change the example
            </button>
            <button className="small" onClick={() => setFix(null)}>
              Cancel
            </button>
            {fix.passes && <span className="muted small">Then save the spec to keep it.</span>}
          </div>
        </div>
      )}
    </div>
  );
}

function HpRow({
  row,
  cell,
  width,
  after,
  checks,
  sets,
  picked,
  onPick,
}: {
  row: Replay["rows"][number];
  cell: number;
  width: number;
  /** Width of the cycles past the end of the example. */
  after: number;
  checks: Replay["checks"];
  sets: number[];
  /** The cycle being explained, marked. */
  picked: number | null;
  onPick: (cycle: number) => void;
}) {
  const H = 26;
  const hi = 5;
  const lo = H - 5;
  const parts: React.ReactNode[] = [];
  for (const c of checks) {
    parts.push(<rect key={`c${c.step}`} x={c.from * cell} y={1} width={(c.to - c.from + 1) * cell} height={H - 2} className="hp-check" />);
    if (!c.ok && c.failCycle !== undefined) {
      parts.push(<rect key={`f${c.step}`} x={c.failCycle * cell} y={1} width={cell} height={H - 2} className="hp-fail" />);
    }
  }
  if (row.width === 1) {
    let d = "";
    row.values.forEach((v, i) => {
      if (v === null) return;
      const y = v ? hi : lo;
      d += `${d && row.values[i - 1] !== null ? "L" : "M"}${i * cell},${y} L${(i + 1) * cell},${y} `;
    });
    parts.push(<path key="w" d={d} className="hp-wave" />);
  } else {
    let start = 0;
    for (let i = 1; i <= row.values.length; i++) {
      if (i === row.values.length || row.values[i] !== row.values[start]) {
        const v = row.values[start];
        const x = start * cell;
        const w = (i - start) * cell;
        if (v !== null) {
          parts.push(<rect key={`b${start}`} x={x + 1} y={hi} width={Math.max(w - 2, 1)} height={lo - hi} className="hp-bus" />);
          const label = fmtValue(v, row.width).split(" ")[0];
          if (w > label.length * 6.5) {
            parts.push(
              <text key={`t${start}`} x={x + 4} y={H / 2 + 4} className="hp-val">
                {label}
              </text>,
            );
          }
        }
        start = i;
      }
    }
  }
  for (const c of sets) parts.push(<path key={`s${c}`} d={`M${c * cell},0 l4,0 l-4,5 z`} className="hp-set" />);
  if (after > 0) parts.push(<rect key="after" x={width - after} y={0} width={after} height={H} className="hp-after" />);
  if (picked !== null) parts.push(<rect key="picked" x={picked * cell} y={0} width={cell} height={H} className="hp-picked" />);
  return (
    <>
      <span className="mono small hp-label" title={row.dir === "in" ? "input" : "output"}>
        {row.name}
      </span>
      <svg
        width={width}
        height={H}
        role="img"
        className="hp-pick"
        onClick={(e) => {
          const c = Math.floor((e.clientX - e.currentTarget.getBoundingClientRect().left) / cell);
          if (c >= 0 && c < row.values.length && row.values[c] !== null) onPick(c);
        }}
        aria-label={`${row.name}: ${row.values.map((v) => fmtValue(v, row.width).split(" ")[0]).join(", ")}`}
      >
        <title>Click a value to see why it is what it is</title>
        {parts}
      </svg>
    </>
  );
}

/** "Why not?": the lines that could have written the value the example expected, and what kept
 * each from doing so; its values can be asked about like any other. */
function WhyNotPanel({ w, onAsk, onClose }: { w: WhyNot; onAsk: (r: Read) => void; onClose: () => void }) {
  const chip = (r: Read) => (
    <button key={r.full} className="link small mono" disabled={r.t === null} onClick={() => onAsk(r)} title={`Why is ${r.name} ${r.value}?`}>
      {r.name} = {r.value}
    </button>
  );
  const chips = (rs: Read[]) => rs.map((r, j) => [j > 0 && ", ", chip(r)]);
  return (
    <div className="design-card why">
      <div className="design-row" style={{ justifyContent: "space-between" }}>
        <strong className="small">
          Why wasn't <span className="mono">{w.name}</span> {w.expected} at cycle {w.cycle}?
        </strong>
        <button className="link small" onClick={onClose}>
          Close
        </button>
      </div>
      {w.unrecorded && <p className="warn small">The run is too long to have recorded every decision up to this cycle.</p>}
      {w.candidates.length === 0 ? (
        <p className="small" style={{ margin: 0 }}>
          No line of the design can write {w.expected} to <span className="mono">{w.name}</span>
          {w.others > 0 && <> (the {w.others === 1 ? "line that writes it only writes" : `${w.others} lines that write it only write`} other values)</>}. The design
          needs a line that does.
        </p>
      ) : (
        <ol className="why-chain">
          {w.candidates.map((c, k) => (
            <li key={k} className="small">
              Line {c.line} of {c.file.split("/").pop()}: <code>{c.code}</code>
              {c.verdict === "never" && <div>It hadn't run yet: nothing had reached its decisions by cycle {w.cycle}.</div>}
              {c.verdict === "blocked" && c.blocked && (
                <div>
                  Didn't run for cycle {c.cycle}
                  {c.passed.length > 0 && (
                    <>
                      {" "}
                      (
                      {c.passed.map((p, j) => (
                        <span key={j}>
                          {j > 0 && ", "}
                          {withCode(decided(p))}
                        </span>
                      ))}
                      )
                    </>
                  )}{" "}
                  because <strong>{withCode(missed(c.blocked))}</strong>
                  {c.blocked.reads.length > 0 && <> ({chips(c.blocked.reads)})</>}.
                </div>
              )}
              {c.verdict === "ran" && (
                <div>
                  It ran{c.cycle !== undefined && <> for cycle {c.cycle}</>}
                  {c.reads.length > 0 && <>, with {chips(c.reads)}</>}, and gave {w.name} another value (or a later line changed it).
                </div>
              )}
            </li>
          ))}
        </ol>
      )}
      <p className="muted small" style={{ margin: 0 }}>
        {w.candidates.length > 0 && w.others > 0 && <>Not listed: {w.others} line{w.others === 1 ? "" : "s"} writing other constants. </>}
        Click a value to see why it was that. When you ask the AI to fix something, this goes with it.
      </p>
    </div>
  );
}

/** "Why is it this value?": each value asked about, explained; the values an explanation mentions
 * can be asked about in turn, building a chain back to the cause. */
function WhyPanel({ trail, onAsk, onClose }: { trail: Because[]; onAsk: (r: Read, level: number) => void; onClose: () => void }) {
  const chip = (r: Read, level: number) => (
    <button key={r.full} className="link small mono" disabled={r.t === null} onClick={() => onAsk(r, level)} title={`Why is ${r.name} ${r.value}?`}>
      {r.name} = {r.value}
    </button>
  );
  return (
    <div className="design-card why">
      <div className="design-row" style={{ justifyContent: "space-between" }}>
        <strong className="small">Why?</strong>
        <button className="link small" onClick={onClose}>
          Close
        </button>
      </div>
      <ol className="why-chain">
        {trail.map((b, level) => (
          <li key={`${level}:${b.full}`} className="small">
            <span className="mono">
              {b.name} = {b.value}
            </span>{" "}
            from cycle {b.since}
            {b.kind === "start" && <>: nothing has written it; it has had this value from the start.</>}
            {b.kind === "input" && (
              <>
                : an input, {b.step ? <>set by step {b.step} of the example.</> : <>set by the testbench (<code>{b.code}</code>).</>}
              </>
            )}
            {b.kind === "design" && (
              <>
                : line {b.line} of {b.file?.split("/").pop()} wrote it: <code>{b.code}</code>
                {b.conds.length > 0 && (
                  <div>
                    It ran because{" "}
                    {b.conds.map((c, k) => (
                      <span key={k}>
                        {k > 0 && ", and "}
                        {withCode(decided(c))}
                        {c.reads.length > 0 && <> ({c.reads.map((r, j) => [j > 0 && ", ", chip(r, level)])})</>}
                      </span>
                    ))}
                    .
                  </div>
                )}
                {b.reads.length > 0 && <div>It used {b.reads.map((r, j) => [j > 0 && ", ", chip(r, level)])}.</div>}
              </>
            )}
          </li>
        ))}
      </ol>
      <p className="muted small" style={{ margin: 0 }}>
        Click a value to follow it back. When you ask the AI to fix something, this explanation goes with it.
      </p>
    </div>
  );
}

/** What the steps ask for, laid out over clock cycles (not a simulation). */
function Timeline({ spec, ex }: { spec: Spec; ex: SpecExample }) {
  const model = useMemo(() => {
    type Mark = { start: number; len: number; text: string; kind: "level" | "check" | "until" };
    const rows = new Map<string, Mark[]>();
    const add = (sig: string, m: Mark) => {
      if (!rows.has(sig)) rows.set(sig, []);
      rows.get(sig)!.push(m);
    };
    let t = 0;
    const levels = new Map<string, { since: number; value: string }>();
    for (const s of ex.steps) {
      if (s.op === "set") {
        const prev = levels.get(s.signal);
        if (prev && t > prev.since) add(s.signal, { start: prev.since, len: t - prev.since, text: prev.value, kind: "level" });
        levels.set(s.signal, { since: t, value: s.value });
      } else if (s.op === "wait") t += Math.max(1, s.cycles);
      else if (s.op === "expect") add(s.signal, { start: t, len: 0.6, text: s.value, kind: "check" });
      else if (s.op === "expect_for") {
        add(s.signal, { start: t, len: Math.max(1, s.cycles), text: `${s.value} × ${s.cycles}`, kind: "check" });
        t += Math.max(1, s.cycles);
      } else if (s.op === "wait_until") {
        add(s.signal, { start: t, len: 1, text: `until ${s.value}`, kind: "until" });
        t += 1;
      }
    }
    const end = Math.max(t + 1, 2);
    for (const [sig, l] of levels) add(sig, { start: l.since, len: end - l.since, text: l.value, kind: "level" });
    const order = spec.ports.map((p) => p.name).filter((n) => rows.has(n));
    return { end, rows: order.map((n) => ({ name: n, marks: rows.get(n)! })) };
  }, [spec, ex]);

  if (!model.rows.length) return null;
  return (
    <figure className="design-timeline" aria-label="Expected behaviour over clock cycles">
      {model.rows.map((r) => (
        <div key={r.name} className="design-tl-row">
          <span className="mono small">{r.name}</span>
          <div className="design-tl-track">
            {r.marks.map((m, i) => (
              <span
                key={i}
                className={`design-tl-mark ${m.kind} ${m.kind === "level" && /^(0|0x0+|0b0+)$/i.test(m.text) ? "low" : ""}`}
                style={{ left: `${(m.start / model.end) * 100}%`, width: `${Math.max((m.len / model.end) * 100, 1.2)}%` }}
                title={m.text}
              >
                {m.text}
              </span>
            ))}
          </div>
        </div>
      ))}
      <figcaption className="muted small">What this example sets and checks over {model.end - 1} clock cycles. Blue: inputs you set; amber: checks.</figcaption>
    </figure>
  );
}

// ---- Health -----------------------------------------------------------------------

function Health({
  projectId,
  head,
  clockMhz,
  specTop,
  run,
  canEdit,
  onShowRun,
  onOpenFile,
  onAddExample,
  onFoundExample,
}: {
  onOpenFile: (path: string) => void;
  /** Opens Examples with "Add an example in words" filled in. */
  onAddExample: (text: string) => void;
  /** Adds an example "Find an example" found to the spec (unsaved) and opens it. */
  onFoundExample: (ex: SpecExample) => void;
  projectId: string;
  head: string | null;
  /** From the spec, else the flow config's CLOCK_PERIOD. */
  clockMhz: number | null;
  /** The module the examples test. */
  specTop: string | null;
  run: SpecRun | null;
  canEdit: boolean;
  onShowRun: (job: Job) => void;
}) {
  const [jobs, setJobs] = useState<Job[] | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  // Planted mistakes: checked on request, for the commit it was checked at.
  const [mistakes, setMistakes] = useState<{ head: string | null; report: MistakesReport } | null>(null);
  const [planting, setPlanting] = useState(false);
  const [plantError, setPlantError] = useState<string | null>(null);
  async function plant() {
    setPlanting(true);
    setPlantError(null);
    try {
      setMistakes({ head, report: await api.plantMistakes(projectId) });
    } catch (e) {
      setPlantError((e as Error).message);
    } finally {
      setPlanting(false);
    }
  }
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    api.listJobs(projectId).then(setJobs, (e: Error) => setError(e.message));
  }, [projectId, head, tick]);
  const active = jobs?.find((j) => !isFinished(j.status));
  useEffect(() => {
    if (!active) return;
    const t = window.setTimeout(() => setTick((n) => n + 1), 3000);
    return () => window.clearTimeout(t);
  }, [active, tick]);

  async function build() {
    try {
      const j = await api.createJob(projectId);
      setJobs((js) => [j, ...(js ?? [])]);
    } catch (e) {
      setError((e as Error).message);
    }
  }

  if (!jobs) return <p className="muted pad">{error ?? "Loading…"}</p>;
  // The default build (config.json) is what the project is; other configs are variants.
  const built = jobs.filter((j) => j.status === "succeeded" && j.summary);
  const job = built.find((j) => j.config_path === "config.json") ?? built[0];
  const stale = job && head && job.commit_id !== head;
  const s = job?.summary;
  const exPass = run ? run.examples.filter((e) => e.verdict === "pass").length : 0;
  const exTotal = run?.examples.length ?? 0;
  const gls = job?.gls;
  const periodNs = clockMhz ? 1000 / clockMhz : null;
  const wns = s?.setup_wns_ns;
  const speedup = periodNs && typeof wns === "number" && wns < periodNs ? periodNs / (periodNs - wns) : null;
  const mm2 = typeof s?.die_area_um2 === "number" ? s.die_area_um2 / 1e6 : null;

  type Card = { id: string; ok: boolean | null; title: string; plain: string; explain: string; expert: string; extra?: ReactNode };
  const cards: Card[] = [];
  // Examples on the gates: some are for other settings (e.g. N = 2) and are skipped on this build.
  const gEx = (gls?.runs ?? []).filter((r) => r.top.startsWith("tb_ex_"));
  const gPass = gEx.filter((r) => r.verdict === "pass").length;
  const gSkip = gEx.filter((r) => r.verdict === "skipped").length;
  const gRan = gEx.length - gSkip;
  const gBlock = gEx.filter((r) => r.verdict === "skipped" && r.reason?.startsWith("does not instantiate")).length;
  const gatesOk = !gls || (gls.status !== "failed" && gPass === gRan);
  const works = exTotal > 0 && exPass === exTotal && gatesOk;
  // The build is a chip made of blocks and no example drives the chip itself.
  const chipUntested = !!gls && gRan === 0 && gEx.length > 0 && gEx.every((r) => r.reason?.startsWith("does not instantiate"));
  const onGates = !gls
    ? " Build the design to check them on the synthesised gates too."
    : gRan === 0
      ? gEx.length > 0 && gEx.every((r) => r.reason?.startsWith("does not instantiate"))
        ? ` They test ${gls.design === specTop ? "the design" : `the block ${specTop ?? ""}`}, but this build is the whole chip ${gls.design}, so none ran on its gates.`
        : " None of them match this build's settings, so none ran on the synthesised gates."
      : ` On the synthesised gates, ${gPass} of ${gRan} pass` +
        (gBlock ? `; ${gBlock} test${gBlock === 1 ? "s" : ""} the block ${specTop ?? ""} rather than the chip, so ran in simulation only` : "") +
        (gSkip - gBlock ? `; ${gSkip - gBlock} ${gSkip - gBlock === 1 ? "is" : "are"} for a different setting, so not part of this build` : "") +
        ".";
  cards.push({
    id: "works",
    ok: exTotal === 0 || (works && chipUntested) ? null : works,
    title:
      exTotal === 0 ? "Works: no examples yet" : !works ? "Not working yet" : chipUntested ? "The blocks work; the chip isn't tested yet" : "It works",
    plain: exTotal === 0 ? "Add examples so there's something to check the design against." : `${exPass} of ${exTotal} examples pass on your design.${onGates}`,
    explain:
      "Each example is run in simulation on the Verilog. After a build, the testbenches that match the build's settings run again on the gate-level netlist the chip would be made from.",
    expert: `${run?.engine ?? "wafer-sim"} · examples ${exPass}/${exTotal}` + (gls ? ` · gate-level: ${gls.summary}` : " · no gate-level run yet"),
  });
  const cov = run?.coverage;
  if (cov) {
    // A default arm no example reaches is often impossible by design; it doesn't hold the project back.
    const real = cov.gaps.filter((g) => g.kind !== "default");
    const maybe = cov.gaps.length - real.length;
    cards.push({
      id: "tested",
      ok: real.length === 0 ? true : null,
      title:
        cov.gaps.length === 0
          ? "The examples try every case the design tells apart"
          : real.length === 0
            ? "The examples try every case that can happen"
            : `The examples don't try ${real.length} ${real.length === 1 ? "case" : "cases"} yet`,
      plain:
        `Your examples try ${cov.covered} of ${cov.total} cases the design tells apart: each if/else branch and case choice, each part of a combined condition both ways, and both sides of each ?: choice.` +
        (real.length ? " Passing examples only show the cases they try:" : "") +
        (maybe ? ` ${maybe} ${maybe === 1 ? "choice is" : "choices are"} never reached, which may be impossible by design.` : ""),
      explain:
        "While your examples run, the simulator records which branches of the design they go through. A branch none of them takes could be wrong without any example noticing. Add an example for each case that matters.",
      expert: `coverage ${cov.covered}/${cov.total} over the example testbenches: branches (if/else, case arms; implicit defaults not counted), condition parts of &&/|| decisions both ways, ?: sides`,
      extra: cov.gaps.length ? (
        <ul className="coverage-gaps">
          {[...real, ...cov.gaps.filter((g) => g.kind === "default")].slice(0, 10).map((g) => (
            <li key={`${g.file}:${g.line}:${g.kind}:${g.labels ?? ""}`}>
              <span className="small">{withCode(g.text)}</span>
              <span className="coverage-actions">
                {canEdit && g.kind !== "default" && (
                  <>
                    <FindExample projectId={projectId} target={{ for: "gap", file: g.file, line: g.line, text: g.text }} onFound={onFoundExample} />
                    <button className="small" onClick={() => onAddExample(g.suggestion)} title="Describe an example in words for the assistant to write">
                      Add in words
                    </button>
                  </>
                )}
                <button className="link small" onClick={() => onOpenFile(g.file)} title={`${g.file}, line ${g.line}`}>
                  {g.file.split("/").pop()}:{g.line}
                </button>
              </span>
            </li>
          ))}
          {cov.gaps.length > 10 && <li className="muted small">and {cov.gaps.length - 10} more</li>}
        </ul>
      ) : undefined,
    });
  }
  if (exTotal > 0) {
    const r = mistakes && mistakes.head === head ? mistakes.report : null;
    cards.push({
      id: "mistakes",
      ok: r ? r.slipped.length === 0 : null,
      title: !r
        ? "Would your examples catch a mistake?"
        : r.tried === 0
          ? "No place to plant a mistake in this design"
          : r.slipped.length === 0
            ? `Your examples catch all ${r.tried} planted mistakes`
            : `Your examples catch ${r.caught} of ${r.tried} planted mistakes`,
      plain: !r
        ? "Wafersmith can make small mistakes in a copy of the design, one at a time (a comparison turned round, a number one off, a step left out), and see whether any example notices. It takes a few seconds."
        : r.slipped.length === 0
          ? "Every small mistake planted in a copy of the design made at least one example fail."
          : "These slipped through: every example still passed with the mistake in. An example for each one that matters makes the design safer to change. (A few planted mistakes change nothing at all; those can't be caught.)",
      explain:
        "This is mutation testing: each mistake is planted in a copy of the design (your files aren't changed), and all the examples are run against it. A mistake some example notices is caught. One that slips through shows a behaviour no example pins down.",
      expert: r
        ? `${r.caught}/${r.tried} mutants killed (${r.sites} sites${r.tried < r.sites ? ", a spread sample" : ""}) in ${r.elapsed_ms} ms; operators: relational and logical swaps, off-by-one constants, reversed conditions and negations, +/-, dropped assignments`
        : "mutation testing over the example testbenches",
      extra: (
        <div className="design-col" style={{ gap: 6 }}>
          {r && r.slipped.length > 0 && (
            <ul className="coverage-gaps">
              {r.slipped.slice(0, 12).map((g, k) => (
                <li key={k}>
                  <span className="small">{withCode(g.text)}</span>
                  <span className="coverage-actions">
                    {canEdit && (
                      <>
                        <FindExample
                          projectId={projectId}
                          target={{ for: "mistake", file: g.file, line: g.line, kind: g.kind, original: g.original, mistake: g.mistake }}
                          onFound={onFoundExample}
                        />
                        <button className="small" onClick={() => onAddExample(g.suggestion)} title="Describe an example in words for the assistant to write">
                          Add in words
                        </button>
                      </>
                    )}
                    <button className="link small" onClick={() => onOpenFile(g.file)} title={`${g.file}, line ${g.line}`}>
                      {g.file.split("/").pop()}:{g.line}
                    </button>
                  </span>
                </li>
              ))}
              {r.slipped.length > 12 && <li className="muted small">and {r.slipped.length - 12} more</li>}
            </ul>
          )}
          {canEdit && (
            <div className="design-row">
              <button className="small" onClick={plant} disabled={planting || !head}>
                {planting ? "Planting mistakes…" : r ? "Check again" : "Plant mistakes and check"}
              </button>
              {plantError && <span className="error small">{plantError}</span>}
            </div>
          )}
        </div>
      ),
    });
  }
  for (const [i, m] of (run?.machines ?? []).entries()) {
    const untested = m.transitions.filter((t) => t.covered === false).length;
    cards.push({
      id: `machine-${i}`,
      ok: null,
      title: `How it works: ${m.module} steps through ${m.states.length} states`,
      plain:
        `${m.transitions.length} ways to move between them` +
        (untested ? `; your examples never take ${untested} of them (dashed red)` : "; your examples take every one") +
        ".",
      explain:
        "The design remembers which step of its job it is on in a state register, and moves to another state when a condition holds. This diagram was found in the Verilog: each box is a state, each arrow a move, numbered and listed with when it happens.",
      expert: `register ${m.state} in ${m.file} (module ${m.module}); states ${m.states.join(", ")}${m.reset ? `; reset to ${m.reset}` : ""}`,
      extra: <StateMachine machine={m} />,
    });
  }
  if (s) {
    cards.push({
      id: "fast",
      ok: s.checks.setup === true && s.checks.hold === true,
      title: s.checks.setup && s.checks.hold ? "Fast enough" : "Too slow, or timing problems",
      plain:
        s.checks.setup && s.checks.hold
          ? `Meets the ${clockMhz ? `${Number(clockMhz.toPrecision(4))} MHz` : "target"} clock${speedup ? `, with room to spare: it could run about ${speedup.toFixed(1)}× faster` : ""}.`
          : "Some signals don't arrive in time for the next clock edge.",
      explain: "Every signal has to settle within one clock period. The margin left over is the slack; positive slack at slow, typical and fast conditions means it works at this clock.",
      expert: `setup WNS ${fmt(s.setup_wns_ns)} ns · hold WNS ${fmt(s.hold_wns_ns)} ns · setup ${s.checks.setup ? "pass" : "fail"} · hold ${s.checks.hold ? "pass" : "fail"}`,
    });
    const make = s.checks.drc === true && s.checks.lvs === true && s.checks.antenna === true;
    cards.push({
      id: "make",
      ok: make,
      title: make ? "Can be manufactured" : "Layout problems to fix",
      plain: make ? "No layout rule errors, and the layout matches the design." : "The layout breaks some of the foundry's rules, or doesn't match the design.",
      explain: "The foundry sets rules for how close and how large shapes may be. Independent checkers look for violations, and the drawn layout is compared back to the netlist.",
      expert: `DRC KLayout ${s.drc_klayout ?? "?"} · Magic ${s.drc_magic ?? "?"} · router ${s.drc_route ?? "?"} · LVS errors ${s.lvs_errors ?? "?"} · antenna nets ${s.antenna_nets ?? "?"}`,
    });
    cards.push({
      id: "size",
      ok: true,
      title: "Size and power",
      plain: `${mm2 !== null ? `${mm2.toFixed(4)} mm²` : "?"} · ${s.stdcells ?? "?"} cells · ${typeof s.power_total_w === "number" ? `${(s.power_total_w * 1e3).toFixed(2)} mW` : "?"}.`,
      explain: `About ${mm2 !== null ? Math.round(Math.sqrt(s.die_area_um2 as number)) : "?"} × ${mm2 !== null ? Math.round(Math.sqrt(s.die_area_um2 as number)) : "?"} µm. The supply voltage droops at most ${typeof s.ir_drop_worst_v === "number" ? (s.ir_drop_worst_v * 1e3).toFixed(1) : "?"} mV inside the block.`,
      expert: `die ${fmt(s.die_area_um2)} µm² · core ${fmt(s.core_area_um2)} µm² · util ${typeof s.utilization === "number" ? Math.round(s.utilization * 100) : "?"} % · IR worst ${fmt(s.ir_drop_worst_v)} V`,
    });
  }

  return (
    <div className="design-col">
      <section className="design-card design-row" style={{ justifyContent: "space-between" }}>
        <div>
          <h3 style={{ margin: 0 }}>
            {!job
              ? "Not built yet"
              : stale
                ? "Built from an older version"
                : cards.every((c) => c.ok === true)
                  ? "Ready for expert review"
                  : cards.some((c) => c.ok === false)
                    ? "Needs work"
                    : "Add examples to finish checking"}
          </h3>
          <p className="muted small" style={{ margin: 0 }}>
            {job ? (
              <>
                From the build of <code>{job.commit_id.slice(0, 8)}</code> with <code>{job.config_path}</code>
                {stale && " (the project has changed since; build again to check the latest)"}.{" "}
                <button className="link small" onClick={() => onShowRun(job)}>
                  Open the build
                </button>
              </>
            ) : (
              "Build the design to check speed, size and manufacturability."
            )}
          </p>
        </div>
        <a className="button" href={`/p/${projectId}/report`} target="_blank" rel="noreferrer" title="One page for a reviewer: what it is, how it's tested, how it works, whether it's ready">
          Review report
        </a>
        {canEdit && (
          <button className="primary" onClick={build} disabled={!!active || !head}>
            {active ? "Building…" : job ? "Build again" : "Build and check"}
          </button>
        )}
      </section>
      {error && <p className="error small">{error}</p>}
      {cards.map((c) => (
        <section key={c.id} className="design-card design-health">
          <span className={`design-dot ${c.ok === null ? "na" : c.ok ? "ok" : "bad"}`} aria-hidden="true">
            {c.ok === null ? "–" : c.ok ? "✓" : "!"}
          </span>
          <div className="design-health-text">
            <strong>{c.title}</strong>
            <span>{c.plain}</span>
            {c.extra}
            {open[c.id] && (
              <div className="design-explain">
                <span className="small">{c.explain}</span>
                <code className="small">{c.expert}</code>
              </div>
            )}
          </div>
          <button className="small" onClick={() => setOpen({ ...open, [c.id]: !open[c.id] })} aria-expanded={!!open[c.id]}>
            {open[c.id] ? "Hide" : "What does this mean?"}
          </button>
        </section>
      ))}
      <section className="design-card design-health expert">
        <span className="design-dot warn" aria-hidden="true">
          !
        </span>
        <div className="design-health-text">
          <strong>Not checked here: needs an expert before a real chip run</strong>
          <span className="small">I/O pads and pin placement, manufacturing test (scan), behaviour at power-up, timing with real gate delays in simulation, and the final go/no-go.</span>
        </div>
      </section>
    </div>
  );
}

const fmt = (v: unknown) => (typeof v === "number" ? Number(v.toPrecision(4)).toLocaleString() : "?");

/** Text with `backticked` parts shown as code. */
function withCode(text: string) {
  return text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>));
}

/** "Find an example": the simulator searches the design's inputs for an example that reaches the
 * target, which then opens in Examples as an unsaved draft. */
function FindExample({ projectId, target, onFound }: { projectId: string; target: FindTarget; onFound: (ex: SpecExample) => void }) {
  const [state, setState] = useState<"idle" | "searching" | string>("idle");
  async function search() {
    setState("searching");
    try {
      const r = await api.findExample(projectId, target);
      if (r.example) {
        setState("idle");
        onFound(r.example);
      } else {
        setState(r.message ?? "Nothing found.");
      }
    } catch (e) {
      setState((e as Error).message);
    }
  }
  return (
    <>
      <button
        className="small primary"
        onClick={search}
        disabled={state === "searching"}
        title="The simulator tries the design's inputs until it finds an example for this, which you then check"
      >
        {state === "searching" ? "Searching…" : "Find an example"}
      </button>
      {state !== "idle" && state !== "searching" && <span className="muted small find-note">{state}</span>}
    </>
  );
}
