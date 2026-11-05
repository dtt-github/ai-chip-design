import { useEffect, useRef, useState } from "react";
import { api, isFinished, type Artifact, type GlsReport, type Job, type JobSummary } from "../api";
import { LayoutViewer } from "./LayoutViewer";
import { Report } from "./Report";

const POLL_MS = 1500;

function duration(job: Job): string {
  if (!job.started_at) return "";
  const end = job.finished_at ? new Date(job.finished_at) : new Date();
  const s = Math.max(0, Math.round((end.getTime() - new Date(job.started_at).getTime()) / 1000));
  return s >= 60 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${s}s`;
}

export function StatusBadge({ job }: { job: Job }) {
  const signoff =
    job.status === "succeeded" && job.summary ? (job.summary.signoff_clean ? "sign-off clean" : "violations") : null;
  const gls = job.gls?.status === "passed" ? "netlist sim pass" : job.gls?.status === "failed" ? "netlist sim fail" : null;
  return (
    <>
      <span className={`status status-${job.status}`}>{job.status}</span>
      {signoff && <span className={`status ${job.summary?.signoff_clean ? "status-succeeded" : "status-failed"}`}>{signoff}</span>}
      {gls && (
        <span className={`status ${job.gls?.status === "passed" ? "status-succeeded" : "status-failed"}`} title={job.gls?.summary}>
          {gls}
        </span>
      )}
    </>
  );
}

interface ListProps {
  projectId: string;
  head: string | null;
  /** Flow configs in the project, the default first. */
  configs: string[];
  canRun: boolean;
  dirty: boolean;
  selected: string | null;
  onSelect: (job: Job) => void;
}

/** Start runs and list recent ones; polls while any run is active. */
export function RunsList({ projectId, head, configs, canRun, dirty, selected, onSelect }: ListProps) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [picked, setPicked] = useState<string | null>(null);
  // Fall back to the default when the picked config is deleted.
  const config = picked && configs.includes(picked) ? picked : (configs[0] ?? "config.json");
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    api.listJobs(projectId).then(
      (j) => !cancelled && setJobs(j),
      (e: Error) => !cancelled && setError(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [projectId, tick]);

  const active = jobs.some((j) => !isFinished(j.status));
  useEffect(() => {
    if (!active) return;
    const t = window.setTimeout(() => setTick((n) => n + 1), POLL_MS);
    return () => window.clearTimeout(t);
  }, [active, tick]);

  async function start() {
    setStarting(true);
    setError(null);
    try {
      const job = await api.createJob(projectId, { config });
      setJobs((j) => [job, ...j]);
      onSelect(job);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setStarting(false);
    }
  }

  return (
    <div className="runs">
      {canRun && (
        <div className="run-start">
          <div className="run-start-row">
            <button className="primary" onClick={start} disabled={starting || !head || configs.length === 0}>
              {starting ? "Starting…" : "Run RTL → GDS"}
            </button>
            {configs.length > 1 && (
              <label className="run-config">
                <span className="muted small">with</span>
                <select value={config} onChange={(e) => setPicked(e.target.value)} aria-label="Flow config">
                  {configs.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              </label>
            )}
          </div>
          <p className="muted small">
            {configs.length === 0 ? (
              <span className="warn">Add a flow config (config.json) to run the flow. </span>
            ) : (
              configs.length === 1 && (
                <>
                  <code>{config}</code> ·{" "}
                </>
              )
            )}
            LibreLane on the last commit <code>{head?.slice(0, 8)}</code>
            {dirty && <span className="warn"> · uncommitted changes are not included</span>}
          </p>
        </div>
      )}
      {error && <p className="error small">{error}</p>}
      {jobs.length === 0 && <p className="muted small">No runs yet.</p>}
      <ul className="run-list">
        {jobs.map((j) => (
          <li key={j.id}>
            <button className={`run-item ${j.id === selected ? "active" : ""}`} onClick={() => onSelect(j)}>
              <span className="run-row">
                <StatusBadge job={j} />
                <span className="spacer" />
                <span className="muted small">{duration(j)}</span>
              </span>
              <span className="muted small">
                <code>{j.commit_id.slice(0, 8)}</code>
                {j.config_path !== "config.json" && (
                  <>
                    {" "}· <code>{j.config_path}</code>
                  </>
                )}{" "}
                · {new Date(j.created_at).toLocaleString()}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

type Tab = "log" | "summary" | "report" | "layout" | "files";

/** Details of one run: live log, sign-off summary, layout render and files. */
export function RunView({
  jobId,
  canCancel,
  onClose,
  maximized,
  onMaximize,
}: {
  jobId: string;
  canCancel: boolean;
  onClose: () => void;
  maximized: boolean;
  onMaximize: (max: boolean) => void;
}) {
  const [job, setJob] = useState<Job | null>(null);
  const [tab, setTab] = useState<Tab>("log");
  const [log, setLog] = useState("");
  const [error, setError] = useState<string | null>(null);
  const logRef = useRef<HTMLPreElement>(null);
  const follow = useRef(true);

  // Poll job state and log together until both are final.
  useEffect(() => {
    let stopped = false;
    let offset = 0;
    let timer: number | undefined;
    setLog("");
    setJob(null);
    setTab("log");
    const poll = async () => {
      try {
        const [j, chunk] = await Promise.all([api.getJob(jobId), api.jobLog(jobId, offset)]);
        if (stopped) return;
        setJob(j);
        offset = chunk.offset;
        if (chunk.content) setLog((l) => l + chunk.content);
        if (!chunk.done) timer = window.setTimeout(poll, chunk.content ? 300 : POLL_MS);
        else if (j.status === "succeeded") setTab((t) => (t === "log" ? "summary" : t));
      } catch (e) {
        if (!stopped) setError((e as Error).message);
      }
    };
    poll();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [jobId]);

  useEffect(() => {
    const el = logRef.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [log, tab]);

  return (
    <section className="run-view" aria-label="Run details">
      <header className="run-view-head">
        <strong>Run</strong>
        {job && <StatusBadge job={job} />}
        {job && (
          <span className="muted small">
            <code>{job.commit_id.slice(0, 8)}</code> · <code>{job.config_path}</code> · {job.pdk} · {duration(job)}
          </span>
        )}
        <span className="spacer" />
        <div className="run-tabs" role="tablist">
          {(["log", "summary", "report", "layout", "files"] as Tab[]).map((t) => (
            <button
              key={t}
              role="tab"
              aria-selected={tab === t}
              className={tab === t ? "active" : ""}
              onClick={() => {
                setTab(t);
                // The layout viewer needs room.
                if (t === "layout") onMaximize(true);
              }}
            >
              {t}
            </button>
          ))}
        </div>
        {canCancel && job && !isFinished(job.status) && (
          <button className="small" onClick={() => api.cancelJob(jobId).then(setJob)}>
            Cancel
          </button>
        )}
        <button className="ghost small" onClick={() => onMaximize(!maximized)} aria-label={maximized ? "Restore run panel" : "Maximize run panel"} title={maximized ? "Restore" : "Maximize"}>
          {maximized ? "▾" : "▴"}
        </button>
        <button className="ghost small" onClick={onClose} aria-label="Close run details">
          ×
        </button>
      </header>
      {error && <p className="error small pad-x">{error}</p>}
      {job?.error && tab === "log" && <pre className="run-error">{job.error}</pre>}
      <div className="run-body">
        {tab === "log" && (
          <pre
            ref={logRef}
            className="run-log"
            onScroll={(e) => {
              const el = e.currentTarget;
              follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
            }}
          >
            {log || (job?.status === "queued" ? "Waiting for a worker…" : "")}
          </pre>
        )}
        {tab === "summary" && <Summary job={job} />}
        {tab === "report" && job && <Report job={job} />}
        {tab === "layout" && job && <Layout job={job} />}
        {tab === "files" && job && <Files job={job} />}
      </div>
    </section>
  );
}

const fmt = (v: unknown, digits = 3) => (typeof v === "number" ? Number(v.toPrecision(digits)).toLocaleString() : "—");

function Summary({ job }: { job: Job | null }) {
  const s = job?.summary as JobSummary | null | undefined;
  if (!job || !isFinished(job.status)) return <p className="muted pad">Available when the run finishes.</p>;
  if (!s) return <p className="muted pad">No metrics: the flow did not get far enough.</p>;
  const check = (v: boolean | null) => (v === null ? <span className="muted">n/a</span> : v ? <span className="ok">pass</span> : <span className="bad">fail</span>);
  const rows: [string, React.ReactNode][] = [
    ["Die area", `${fmt(s.die_area_um2)} µm²`],
    ["Core area", `${fmt(s.core_area_um2)} µm²`],
    ["Standard cells", fmt(s.stdcells, 6)],
    ["Utilization", typeof s.utilization === "number" ? `${(s.utilization * 100).toFixed(0)} %` : "—"],
    ["Setup WNS", `${fmt(s.setup_wns_ns)} ns`],
    ["Hold WNS", `${fmt(s.hold_wns_ns)} ns`],
    ["Total power", typeof s.power_total_w === "number" ? `${fmt(s.power_total_w * 1e6)} µW` : "—"],
    ["Worst IR drop", typeof s.ir_drop_worst_v === "number" ? `${fmt(s.ir_drop_worst_v * 1e3)} mV` : "—"],
    ["Wirelength", `${fmt(s.wirelength_um, 6)} µm`],
  ];
  return (
    <div className="summary">
      <div className={`signoff ${s.signoff_clean ? "ok" : "bad"}`}>
        {s.signoff_clean ? "Sign-off clean: DRC, LVS, antenna and timing pass" : "Sign-off has violations"}
      </div>
      <div className="summary-grid">
        <table>
          <caption>Checks</caption>
          <tbody>
            <tr><th>DRC (Magic + KLayout)</th><td>{check(s.checks.drc)}</td></tr>
            <tr><th>LVS</th><td>{check(s.checks.lvs)}</td></tr>
            <tr><th>Antenna</th><td>{check(s.checks.antenna)}</td></tr>
            <tr><th>Setup timing</th><td>{check(s.checks.setup)}</td></tr>
            <tr><th>Hold timing</th><td>{check(s.checks.hold)}</td></tr>
          </tbody>
        </table>
        <table>
          <caption>Metrics</caption>
          <tbody>
            {rows.map(([k, v]) => (
              <tr key={k}>
                <th>{k}</th>
                <td>{v}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {job.gls && <GateLevel gls={job.gls} />}
      <p className="muted small">Toolchain: <code>{job.toolchain}</code></p>
    </div>
  );
}

const VERDICT: Record<string, [string, string]> = {
  pass: ["pass", "ok"],
  fail: ["fail", "bad"],
  no_verdict: ["no PASS line", "bad"],
  error: ["error", "bad"],
  skipped: ["skipped", "muted"],
};

/** The testbenches rerun on the synthesised netlist. */
function GateLevel({ gls }: { gls: GlsReport }) {
  const params = Object.entries(gls.params);
  return (
    <div className="gls">
      <h4>Gate-level simulation</h4>
      <p className={`small ${gls.status === "passed" ? "ok" : gls.status === "failed" ? "bad" : "muted"}`}>{gls.summary}</p>
      <p className="muted small">
        Netlist of <code>{gls.design}</code>
        {params.length > 0 && (
          <>
            {" "}built with {params.map(([k, v]) => <code key={k}>{`${k}=${v}`} </code>)}
          </>
        )}
        · {gls.simulator}
      </p>
      {gls.runs.length > 0 && (
        <table>
          <tbody>
            {gls.runs.map((r) => {
              const [label, cls] = VERDICT[r.verdict] ?? [r.verdict, "muted"];
              return (
                <tr key={r.top}>
                  <th>
                    <code>{r.top}</code>
                  </th>
                  <td className={cls}>{label}</td>
                  <td className="muted small">
                    {r.reason}
                    {r.log.length > 0 && (
                      <details>
                        <summary>output ({r.log.length} lines)</summary>
                        <pre className="gls-log">{r.log.join("\n")}</pre>
                      </details>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

function useArtifacts(job: Job) {
  const [list, setList] = useState<Artifact[] | null>(null);
  useEffect(() => {
    if (!isFinished(job.status)) return;
    api.artifacts(job.id).then(setList, () => setList([]));
  }, [job.id, job.status]);
  return list;
}

function Layout({ job }: { job: Job }) {
  const list = useArtifacts(job);
  if (!isFinished(job.status)) return <p className="muted pad">Available when the run finishes.</p>;
  if (!list) return <p className="muted pad">Loading…</p>;
  const gds = list.find((a) => a.path.startsWith("gds/") && a.path.endsWith(".gds"));
  if (!gds) return <p className="muted pad">This run produced no GDS.</p>;
  return <LayoutViewer job={job} gdsPath={gds.path} />;
}

function Files({ job }: { job: Job }) {
  const list = useArtifacts(job);
  async function download(a: Artifact) {
    const blob = await api.artifactBlob(job.id, a.path);
    const link = document.createElement("a");
    link.href = URL.createObjectURL(blob);
    link.download = a.path.split("/").pop() ?? "artifact";
    link.click();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
  }
  if (!isFinished(job.status)) return <p className="muted pad">Available when the run finishes.</p>;
  if (!list) return <p className="muted pad">Loading…</p>;
  if (!list.length) return <p className="muted pad">No output files.</p>;
  return (
    <table className="files">
      <tbody>
        {list.map((a) => (
          <tr key={a.path}>
            <td>
              <code>{a.path}</code>
            </td>
            <td className="muted small">{(a.size / 1024).toFixed(1)} KiB</td>
            <td>
              <button className="small" onClick={() => download(a)}>
                Download
              </button>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
