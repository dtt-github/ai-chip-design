import { useEffect, useState, type FormEvent } from "react";
import {
  api,
  ApiError,
  type Blocks,
  type EvidenceStep,
  type GateReport,
  type Proposal,
  type ProposalListItem,
  type ProposalStatus,
  type Quality,
  type Spec,
} from "../api";
import { compareExamples, type ExampleChange } from "../lib/compare";
import { hunks, lineDiff } from "../lib/diff";
import { SimClient } from "../sim/client";

const POLL_MS = 1500;

const STATUS_LABEL: Record<ProposalStatus, string> = {
  running: "working",
  ready: "gate passed",
  gate_failed: "gate failed",
  failed: "failed",
  merged: "merged",
  discarded: "discarded",
};

const STATUS_CLASS: Record<ProposalStatus, string> = {
  running: "status-running",
  ready: "status-succeeded",
  gate_failed: "status-failed",
  failed: "status-failed",
  merged: "status-succeeded",
  discarded: "status-canceled",
};

export function ProposalBadge({ status }: { status: ProposalStatus }) {
  return <span className={`status ${STATUS_CLASS[status]}`}>{STATUS_LABEL[status]}</span>;
}

/** Ask the agent for a change; list earlier proposals. */
export function AgentList({
  projectId,
  canPropose,
  selected,
  onSelect,
}: {
  projectId: string;
  canPropose: boolean;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const [items, setItems] = useState<ProposalListItem[]>([]);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    api.proposals(projectId).then(setItems, (e: Error) => setError(e.message));
  }, [projectId, tick]);

  const active = items.some((p) => p.status === "running");
  useEffect(() => {
    if (!active) return;
    const t = window.setTimeout(() => setTick((n) => n + 1), POLL_MS);
    return () => window.clearTimeout(t);
  }, [active, tick]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!text.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const p = await api.propose(projectId, text.trim());
      setText("");
      setTick((n) => n + 1);
      onSelect(p.id);
    } catch (err) {
      setError(err instanceof ApiError && err.status === 503 ? "The agent isn't configured on this server." : (err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="agent">
      {canPropose && (
        <form className="agent-form" onSubmit={submit}>
          <textarea
            placeholder="Describe an RTL change, e.g. “Add an even-parity bit to the UART frame and test it”"
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={4}
            aria-label="Change request for the agent"
          />
          <button className="primary" disabled={busy || !text.trim()}>
            {busy ? "Starting…" : "Ask the agent"}
          </button>
          <p className="muted small">
            The agent works on a copy of the last commit. Its change can only be merged if every testbench passes the
            server's simulation gate.
          </p>
        </form>
      )}
      {error && <p className="error small">{error}</p>}
      {items.length === 0 && <p className="muted small">No proposals yet.</p>}
      <ul className="run-list">
        {items.map((p) => (
          <li key={p.id}>
            <button className={`run-item ${p.id === selected ? "active" : ""}`} onClick={() => onSelect(p.id)}>
              <span className="run-row">
                <ProposalBadge status={p.status} />
                <span className="spacer" />
                <span className="muted small">{new Date(p.created_at).toLocaleTimeString()}</span>
              </span>
              <span className="agent-request" title={p.request}>{p.title ?? p.request}</span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

type Tab = "behaviour" | "changes" | "gate" | "evidence";

/** Review a proposal: diff, gate, evidence; merge when the gate passed. */
export function AgentView({
  proposalId,
  head,
  canMerge,
  onClose,
  maximized,
  onMaximize,
}: {
  proposalId: string;
  head: string | null;
  canMerge: boolean;
  onClose: () => void;
  maximized: boolean;
  onMaximize: (m: boolean) => void;
}) {
  const [p, setP] = useState<Proposal | null>(null);
  const [tab, setTab] = useState<Tab>("evidence");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [regate, setRegate] = useState<{ report: GateReport; reproduced: boolean } | null>(null);

  useEffect(() => {
    let stopped = false;
    let timer: number | undefined;
    setP(null);
    setRegate(null);
    setError(null);
    setTab("evidence");
    const poll = async () => {
      try {
        const next = await api.proposal(proposalId);
        if (stopped) return;
        setP(next);
        if (next.status === "running") timer = window.setTimeout(poll, POLL_MS);
        // A finished proposal with changes opens on what it does, not on the code.
        else setTab((t) => (t === "evidence" && Object.keys(next.files).length ? "behaviour" : t));
      } catch (e) {
        if (!stopped) setError((e as Error).message);
      }
    };
    poll();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [proposalId]);

  async function act(f: () => Promise<Proposal>) {
    setBusy(true);
    setError(null);
    try {
      const next = await f();
      setP((cur) => (cur ? { ...cur, ...next } : next));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  const stale = !!p && !!head && p.base_commit !== head && p.status === "ready";
  const files = p ? Object.keys(p.files).sort() : [];

  return (
    <section className="run-view" aria-label="Agent proposal">
      <header className="run-view-head">
        <strong>Agent</strong>
        {p && <ProposalBadge status={p.status} />}
        {p && (
          <span className="muted small">
            on <code>{p.base_commit.slice(0, 8)}</code> · {p.model}
            {p.evidence.elapsed_ms !== undefined && <> · {(p.evidence.elapsed_ms / 1000).toFixed(0)} s</>}
          </span>
        )}
        <span className="spacer" />
        <div className="run-tabs" role="tablist">
          {(["behaviour", "changes", "gate", "evidence"] as Tab[]).map((t) => (
            <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "active" : ""} onClick={() => setTab(t)}>
              {t}
            </button>
          ))}
        </div>
        {p && canMerge && (p.status === "ready" || p.status === "gate_failed") && (
          <>
            <button
              className="small primary"
              disabled={busy || p.status !== "ready" || stale}
              title={p.status !== "ready" ? "The simulation gate did not pass" : stale ? "The project has new commits; ask again" : "Commit this change"}
              onClick={() => act(() => api.mergeProposal(p.id))}
            >
              Merge
            </button>
            <button className="small" disabled={busy} onClick={() => act(() => api.discardProposal(p.id))}>
              Discard
            </button>
          </>
        )}
        <button className="ghost small" onClick={() => onMaximize(!maximized)} aria-label={maximized ? "Restore panel" : "Maximize panel"}>
          {maximized ? "▾" : "▴"}
        </button>
        <button className="ghost small" onClick={onClose} aria-label="Close proposal">
          ×
        </button>
      </header>
      {error && <p className="error small pad-x">{error}</p>}
      {!p ? (
        <p className="muted pad">Loading…</p>
      ) : (
        <div className="run-body agent-body">
          <div className="agent-head">
            {p.title && <p className="agent-title">{p.title}</p>}
            <p className="agent-request-full">{p.request}</p>
            {p.status === "merged" && (
              <p className="ok small">
                Merged as <code>{p.merged_commit?.slice(0, 8)}</code>
              </p>
            )}
            {stale && <p className="warn small">The project has new commits since this proposal; it can't be merged. Ask the agent again.</p>}
            {p.error && <p className="error small">{p.error}</p>}
            {p.gate && (
              <p className={`small ${p.gate.passed ? "ok" : "bad"}`}>
                Simulation gate: {p.gate.passed ? "passed" : "failed"} — {p.gate.summary}
              </p>
            )}
            {p.summary && <pre className="agent-summary">{p.summary}</pre>}
          </div>
          {tab === "changes" && (
            <div className="agent-diff">
              {files.length === 0 && <p className="muted pad">{p.status === "running" ? "No changes yet." : "No changes."}</p>}
              {files.map((f) => (
                <FileDiff key={f} path={f} before={p.base_files?.[f] ?? null} after={p.files[f]} />
              ))}
            </div>
          )}
          {tab === "behaviour" && <Behaviour p={p} canRun={canMerge} />}
          {tab === "gate" && <Gate report={p.gate} regate={regate} onRegate={async () => setRegate(await api.regate(p.id))} />}
          {tab === "evidence" && <Evidence p={p} />}
        </div>
      )}
    </section>
  );
}

function FileDiff({ path, before, after }: { path: string; before: string | null; after: string }) {
  const lines = hunks(lineDiff(before ?? "", after));
  const added = lines.filter((l) => l?.kind === "add").length;
  const removed = lines.filter((l) => l?.kind === "del").length;
  return (
    <details className="file-diff" open>
      <summary>
        <code>{path}</code> {before === null && <span className="badge">new</span>} <span className="ok small">+{added}</span>{" "}
        <span className="bad small">−{removed}</span>
      </summary>
      <pre className="diff">
        {lines.map((l, i) =>
          l === null ? (
            <span key={i} className="diff-gap">
              {"⋯\n"}
            </span>
          ) : (
            <span key={i} className={`diff-${l.kind}`}>
              {l.kind === "add" ? "+ " : l.kind === "del" ? "- " : "  "}
              {l.text}
              {"\n"}
            </span>
          ),
        )}
      </pre>
    </details>
  );
}

function Gate({
  report,
  regate,
  onRegate,
}: {
  report: GateReport | null;
  regate: { report: GateReport; reproduced: boolean } | null;
  onRegate: () => Promise<void>;
}) {
  const [running, setRunning] = useState(false);
  if (!report) return <p className="muted pad">The gate runs when the agent finishes.</p>;
  return (
    <div className="gate">
      <div className="row small-gap">
        <span className="muted small">{report.engine}</span>
        <span className="spacer" />
        <button
          className="small"
          disabled={running}
          onClick={async () => {
            setRunning(true);
            try {
              await onRegate();
            } finally {
              setRunning(false);
            }
          }}
        >
          {running ? "Running…" : "Re-run gate"}
        </button>
        {regate && (
          <span className={regate.reproduced ? "ok small" : "bad small"}>
            {regate.reproduced ? "reproduced: same inputs, same results" : "did not reproduce"}
          </span>
        )}
      </div>
      {report.runs.map((r) => (
        <details key={r.top} className="gate-run" open={r.verdict !== "pass"}>
          <summary>
            <span className={`status ${r.verdict === "pass" ? "status-succeeded" : "status-failed"}`}>{r.verdict}</span>{" "}
            <code>{r.top}</code>{" "}
            <span className="muted small">
              t={r.time} · {r.steps.toLocaleString()} steps{r.reason ? ` · ${r.reason}` : ""}
            </span>
          </summary>
          <pre className="run-log">{r.log.join("\n") || "(no output)"}</pre>
        </details>
      ))}
      <details>
        <summary className="small">Input files ({Object.keys(report.inputs).length}, sha256)</summary>
        <table className="data">
          <tbody>
            {Object.entries(report.inputs).map(([f, h]) => (
              <tr key={f}>
                <td className="mono">{f}</td>
                <td className="mono muted">{h.slice(0, 16)}…</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </div>
  );
}

function stepLabel(s: EvidenceStep): string {
  if (s.kind !== "tool") return s.kind;
  const input = s.input ?? {};
  const arg = (input.path as string) ?? (input.top !== undefined ? `top=${input.top || "auto"}` : "");
  return `${s.name}${arg ? ` ${arg}` : ""}`;
}

function Evidence({ p }: { p: Proposal }) {
  const ev = p.evidence;
  const u = ev.usage ?? {};
  return (
    <div className="evidence">
      <table className="data">
        <tbody>
          <tr><th>Evidence hash</th><td className="mono">{p.evidence_hash ?? (p.status === "running" ? "(when finished)" : "—")}</td></tr>
          <tr><th>Model</th><td>{p.model}{ev.models_served && ev.models_served.join(",") !== p.model ? ` (served: ${ev.models_served.join(", ")})` : ""}, effort {ev.effort}</td></tr>
          <tr><th>Instructions</th><td className="mono">sha256 {ev.system_prompt_sha256?.slice(0, 16)}…</td></tr>
          <tr><th>Tokens</th><td>{(u.input_tokens ?? 0).toLocaleString()} in · {(u.output_tokens ?? 0).toLocaleString()} out · {(u.cache_read_input_tokens ?? 0).toLocaleString()} cached</td></tr>
          <tr><th>Simulator</th><td>{ev.engine}</td></tr>
        </tbody>
      </table>
      <ol className="steps">
        {(ev.steps ?? []).map((s, i) => (
          <li key={i} className={`step step-${s.kind}${s.is_error ? " step-error" : ""}`}>
            <div className="step-head">
              <span className="muted small">#{s.turn}</span> <strong>{stepLabel(s)}</strong>
              {s.ms !== undefined && <span className="muted small"> · {s.ms} ms</span>}
              {s.kind === "fallback" && <span className="small"> {s.from?.model} → {s.to?.model}</span>}
            </div>
            {s.text && <div className="step-text">{s.text}</div>}
            {s.output && (
              <details>
                <summary className="small muted">result</summary>
                <pre className="run-log">{s.output}</pre>
              </details>
            )}
          </li>
        ))}
        {p.status === "running" && <li className="muted small">Working…</li>}
      </ol>
    </div>
  );
}

/** What the proposal does: each example before and after it, and how well the design is tested. */
function Behaviour({ p, canRun }: { p: Proposal; canRun: boolean }) {
  const [examples, setExamples] = useState<ExampleChange[] | null>(null);
  const [quality, setQuality] = useState<{ before: Quality; after: Quality } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function compare() {
    setBusy(true);
    setError(null);
    const client = new SimClient();
    try {
      const snap = await api.snapshot(p.project_id, p.base_commit);
      const before = snap.files;
      const after = { ...before, ...p.files };
      const specText = before["spec.json"];
      if (!specText) throw new Error("the project has no spec, so there are no examples to compare");
      const spec = JSON.parse(specText) as Spec;
      const chip = before["blocks.json"] ? (JSON.parse(before["blocks.json"]) as Blocks) : null;
      const [ex, q] = await Promise.all([
        compareExamples(client, spec, chip, before, after),
        canRun ? api.compareProposal(p.id) : Promise.resolve(null),
      ]);
      setExamples(ex);
      setQuality(q);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      client.dispose();
      setBusy(false);
    }
  }

  if (p.status === "running") return <p className="muted pad">The agent is still working.</p>;
  const changed = examples?.filter((e) => e.differences.length || e.before !== e.after) ?? [];
  return (
    <div className="pad behaviour">
      <p className="small">
        Runs every example on the project as it was (<code>{p.base_commit.slice(0, 8)}</code>) and with this change, and compares
        every output cycle by cycle, including the cycles after each example ends.
      </p>
      <button className="primary small" onClick={compare} disabled={busy}>
        {busy ? "Comparing…" : examples ? "Compare again" : "Compare behaviour"}
      </button>
      {error && <p className="error small">{error}</p>}
      {examples && (
        <>
          <h4>The examples</h4>
          {changed.length === 0 ? (
            <p className="ok small">
              Every example behaves exactly as before. The change only affects cases the examples don't try, or none at all; the
              coverage below says whether that leaves something untested.
            </p>
          ) : (
            <p className="small">
              {changed.length} of {examples.length} example{examples.length === 1 ? "" : "s"} behave differently:
            </p>
          )}
          <ul className="behaviour-list">
            {examples
              .filter((e) => e.differences.length || e.before !== e.after || e.problem)
              .map((e) => (
                <li key={e.id}>
                  <strong>{e.title}</strong>{" "}
                  <span className="muted small">
                    {verdict(e.before)} → {verdict(e.after)}
                  </span>
                  {e.problem && <p className="error small">{e.problem}</p>}
                  <ul className="small">
                    {e.differences.map((d) => (
                      <li key={d}>{withCode(d)}</li>
                    ))}
                  </ul>
                </li>
              ))}
          </ul>
        </>
      )}
      {quality && (
        <>
          <h4>How well it is tested</h4>
          <table className="report-table small">
            <tbody>
              <tr>
                <th>Examples passing</th>
                <td>
                  {quality.before.passing} of {quality.before.examples} → {quality.after.passing} of {quality.after.examples}
                </td>
              </tr>
              <tr>
                <th>Cases the examples try</th>
                <td>
                  {cov(quality.before)} → {cov(quality.after)}
                </td>
              </tr>
              {moves(quality).map(([label, text]) => (
                <tr key={label}>
                  <th>{label}</th>
                  <td>{text}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {(quality.after.coverage?.gaps.length ?? 0) > (quality.before.coverage?.gaps.length ?? 0) && (
            <p className="warn small">
              With this change the examples leave more cases untried; Health lists them once it is merged.
            </p>
          )}
        </>
      )}
      {examples && !canRun && <p className="muted small">The coverage comparison needs an editor of the project.</p>}
    </div>
  );
}

const verdict = (v: boolean | null) => (v === null ? "couldn't run" : v ? "passes" : "fails");
const cov = (q: Quality) => (q.coverage ? `${q.coverage.covered} of ${q.coverage.total}` : "none");

/** State machines and their moves: what the change adds or removes. */
function moves(q: { before: Quality; after: Quality }): [string, string][] {
  const key = (m: Quality["machines"][number]) => `${m.module}.${m.state}`;
  const list = (m: Quality["machines"][number]) => m.transitions.map((t) => `${t.from} → ${t.to} when ${t.when}`);
  const out: [string, string][] = [];
  const names = new Set([...q.before.machines.map(key), ...q.after.machines.map(key)]);
  for (const name of names) {
    const b = q.before.machines.find((m) => key(m) === name);
    const a = q.after.machines.find((m) => key(m) === name);
    const bl = b ? list(b) : [];
    const al = a ? list(a) : [];
    const added = al.filter((x) => !bl.includes(x));
    const removed = bl.filter((x) => !al.includes(x));
    const label = `State machine ${name}`;
    if (!b) out.push([label, `new, with ${al.length} moves`]);
    else if (!a) out.push([label, "removed"]);
    else if (added.length || removed.length)
      out.push([label, [...added.map((x) => `+ ${x.replace(/`/g, "")}`), ...removed.map((x) => `− ${x.replace(/`/g, "")}`)].join("; ")]);
    else out.push([label, `unchanged (${al.length} moves)`]);
  }
  return out;
}

function withCode(text: string) {
  return text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>));
}
