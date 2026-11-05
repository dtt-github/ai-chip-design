import { useEffect, useState } from "react";
import { api, isFinished, type Job, type RunReport, type TimingPath } from "../api";

const ns = (v: number | null | undefined, d = 3) => (typeof v === "number" ? v.toFixed(d) : "—");
const slackClass = (v: number | null | undefined) => (typeof v !== "number" ? "" : v < 0 ? "bad" : "ok");
const prettyClass = (c: string) => c.replace(/_/g, " ");

/** Timing per corner, the worst setup and hold paths, and area by cell class. */
export function Report({ job }: { job: Job }) {
  const [report, setReport] = useState<RunReport | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isFinished(job.status)) return;
    setReport(null);
    api.report(job.id).then(setReport, (e: Error) => setError(e.message));
  }, [job.id, job.status]);

  if (!isFinished(job.status)) return <p className="muted pad">Available when the run finishes.</p>;
  if (error) return <p className="muted pad">{error}</p>;
  if (!report) return <p className="muted pad">Loading…</p>;

  const maxArea = Math.max(...report.classes.map((c) => c.area_um2 ?? 0), 1);
  return (
    <div className="report">
      <section>
        <h4>Timing by corner</h4>
        <table className="data">
          <thead>
            <tr>
              <th>Corner</th>
              <th>Setup WS</th>
              <th>Setup TNS</th>
              <th>Hold WS</th>
              <th>Hold TNS</th>
              <th>Violations</th>
            </tr>
          </thead>
          <tbody>
            {report.corners.map((c) => (
              <tr key={c.name}>
                <td className="mono">{c.name}</td>
                <td className={slackClass(c.setup_ws)}>{ns(c.setup_ws)} ns</td>
                <td>{ns(c.setup_tns)}</td>
                <td className={slackClass(c.hold_ws)}>{ns(c.hold_ws)} ns</td>
                <td>{ns(c.hold_tns)}</td>
                <td>{(c.setup_violations ?? 0) + (c.hold_violations ?? 0)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      {report.paths.map((p) => (
        <PathTable key={p.check} path={p} />
      ))}

      <section>
        <h4>Area</h4>
        <p className="small">
          Die {ns(report.die_area_um2, 1)} µm² · core {ns(report.core_area_um2, 1)} µm² · standard cells{" "}
          {ns(report.stdcell_area_um2, 1)} µm²
          {typeof report.utilization === "number" && <> · utilization {(report.utilization * 100).toFixed(0)} %</>}
        </p>
        <table className="data">
          <thead>
            <tr>
              <th>Cell class</th>
              <th>Count</th>
              <th>Area µm²</th>
              <th className="bar-col" aria-hidden="true" />
            </tr>
          </thead>
          <tbody>
            {report.classes.map((c) => (
              <tr key={c.class}>
                <td>{prettyClass(c.class)}</td>
                <td>{c.count ?? "—"}</td>
                <td>{ns(c.area_um2, 1)}</td>
                <td className="bar-col">
                  <span className="bar" style={{ width: `${((c.area_um2 ?? 0) / maxArea) * 100}%` }} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </div>
  );
}

function PathTable({ path }: { path: TimingPath }) {
  return (
    <section>
      <h4>
        Worst {path.check} path <span className="muted small mono">{path.corner}</span>{" "}
        <span className={slackClass(path.slack_ns)}>slack {ns(path.slack_ns)} ns</span>
      </h4>
      <p className="small muted">
        {path.startpoint} → {path.endpoint}
        {typeof path.arrival_ns === "number" && <> · arrival {ns(path.arrival_ns)} ns</>}
        {typeof path.required_ns === "number" && <> · required {ns(path.required_ns)} ns</>}
      </p>
      <details>
        <summary className="small">{path.stages.length} stages</summary>
        <table className="data">
          <thead>
            <tr>
              <th>Pin</th>
              <th>Cell</th>
              <th>Edge</th>
              <th>Delay ns</th>
              <th>Time ns</th>
            </tr>
          </thead>
          <tbody>
            {path.stages.map((s, i) => (
              <tr key={i}>
                <td className="mono">{s.pin}</td>
                <td className="mono">{s.cell ?? ""}</td>
                <td>{s.edge === "^" ? "rise" : "fall"}</td>
                <td>{ns(s.delay_ns)}</td>
                <td>{ns(s.time_ns)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </details>
    </section>
  );
}
