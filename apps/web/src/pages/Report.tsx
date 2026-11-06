// A one-page design review report: what the design is, how it is tested (examples in words,
// coverage, planted mistakes), how it works (state machines), whether it is ready (the latest
// build), and where it came from (commits and the AI's merged changes). Printable, so it can be
// saved as a PDF and handed to a reviewer. Built from the same endpoints Health uses.

import { useEffect, useState, type ReactNode } from "react";
import { Link, useParams } from "react-router";
import {
  api,
  type CommitInfo,
  type Job,
  type MistakesReport,
  type Project,
  type ProposalListItem,
  type SpecRun,
  type SpecView,
} from "../api";
import { StateMachine } from "../components/StateMachine";
import { narrate } from "../lib/narrate";

interface Data {
  project: Project;
  view: SpecView;
  jobs: Job[];
  commits: CommitInfo[];
  proposals: ProposalListItem[];
}

export function ReportPage() {
  const { id = "" } = useParams();
  const [data, setData] = useState<Data | null>(null);
  const [run, setRun] = useState<SpecRun | null>(null);
  const [mistakes, setMistakes] = useState<MistakesReport | null>(null);
  const [notes, setNotes] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      const [project, view, jobs, commits, proposals] = await Promise.all([
        api.getProject(id),
        api.getSpec(id),
        api.listJobs(id),
        api.log(id),
        api.proposals(id),
      ]);
      setData({ project, view, jobs, commits, proposals });
      // Running the examples and planting mistakes cost server time: editors only.
      if (!view.spec?.examples.length) return;
      if (project.role === "viewer" || project.role === null) {
        setNotes(["The examples, their coverage and the planted-mistakes check need an editor of the project to run them."]);
        return;
      }
      setRun(await api.runSpec(id));
      api.plantMistakes(id).then(setMistakes, (e: Error) => setNotes((n) => [...n, `Planted mistakes: ${e.message}`]));
    })().catch((e: Error) => setError(e.message));
  }, [id]);

  if (error) return <p className="error pad">{error}</p>;
  if (!data) return <p className="muted pad">Putting the report together…</p>;
  const { project, view, jobs, commits, proposals } = data;
  const spec = view.spec;
  const head = view.head;
  const built = jobs.filter((j) => j.status === "succeeded" && j.summary);
  const job = built.find((j) => j.config_path === "config.json") ?? built[0];
  const s = job?.summary;
  const merged = proposals.filter((p) => p.status === "merged");
  const results = new Map((run?.examples ?? []).map((r) => [r.id, r]));
  const passing = run ? run.examples.filter((e) => e.verdict === "pass").length : null;

  return (
    <main className="report">
      <div className="report-actions no-print">
        <Link to={`/p/${id}`}>← Back to the project</Link>
        <span className="spacer" />
        <button className="primary" onClick={() => window.print()}>
          Print or save as PDF
        </button>
      </div>

      <header className="report-head">
        <span className="muted small">Design review report</span>
        <h1>{project.name}</h1>
        <p className="muted small">
          Commit <code>{head.slice(0, 8)}</code> · {project.pdk} · licence {project.licence} · generated{" "}
          {new Date().toLocaleString()} with Wafersmith
        </p>
        {notes.map((n) => (
          <p key={n} className="muted small">
            {n}
          </p>
        ))}
      </header>

      <Section title="1. What it is">
        {!spec ? (
          <p>This project has no spec yet: describe the design in the Design view first.</p>
        ) : (
          <>
            <p>{spec.intent}</p>
            <table className="report-table">
              <thead>
                <tr>
                  <th>Connection</th>
                  <th>Direction</th>
                  <th>Bits</th>
                  <th>What it means</th>
                </tr>
              </thead>
              <tbody>
                {spec.ports.map((p) => (
                  <tr key={p.name}>
                    <td>
                      <code>{p.name}</code>
                    </td>
                    <td>{p.dir === "in" ? "input" : "output"}</td>
                    <td>{p.width}</td>
                    <td>
                      {spec.clock?.port === p.name
                        ? `${p.meaning || "Clock"}, ${spec.clock.mhz} MHz`
                        : spec.reset?.port === p.name
                          ? p.meaning || `Reset: ${spec.reset.active_low ? "0" : "1"} resets`
                          : p.meaning}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {spec.params.length > 0 && (
              <p>
                <strong>Settings:</strong>{" "}
                {spec.params.map((p, i) => (
                  <span key={p.name}>
                    {i > 0 && "; "}
                    <code>{p.name}</code> = {p.value}
                    {p.meaning && ` (${p.meaning})`}
                  </span>
                ))}
                .
              </p>
            )}
            {spec.decisions.length > 0 && (
              <>
                <h3>Decisions</h3>
                <ul>
                  {spec.decisions.map((d) => (
                    <li key={d.question}>
                      {d.question} <strong>{d.answer}</strong>
                    </li>
                  ))}
                </ul>
              </>
            )}
            {spec.questions.length > 0 && (
              <p className="report-warn">
                <strong>Still open:</strong> {spec.questions.map((q) => q.question).join(" ")}
              </p>
            )}
          </>
        )}
      </Section>

      {spec && (
        <Section title="2. How it is tested">
          <p>
            {spec.examples.length} example{spec.examples.length === 1 ? "" : "s"}, each run as a self-checking testbench
            {passing !== null && <>: {passing} of {spec.examples.length} pass on the design at this commit</>}.
          </p>
          <ol className="report-examples">
            {spec.examples.map((e) => {
              const r = results.get(e.id);
              return (
                <li key={e.id}>
                  <strong>{e.title}</strong>
                  {r && <span className={`report-verdict ${r.verdict === "pass" ? "ok" : "bad"}`}>{r.verdict === "pass" ? "passes" : r.verdict}</span>}
                  <br />
                  <span className="small">{narrate(spec, e)}</span>
                </li>
              );
            })}
          </ol>
          {run?.coverage && (
            <>
              <h3>What the examples try</h3>
              <p>
                {run.coverage.covered} of {run.coverage.total} cases the design tells apart (each if/else branch and case choice,
                each part of a combined condition both ways, both sides of each ?: choice).
              </p>
              {run.coverage.gaps.length > 0 && (
                <ul className="small">
                  {run.coverage.gaps.map((g) => (
                    <li key={`${g.file}:${g.line}:${g.kind}:${g.what}:${g.labels ?? ""}:${g.text}`}>
                      {withCode(g.text)} ({g.file}, line {g.line})
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
          <h3>Would the examples catch a mistake?</h3>
          {mistakes ? (
            <>
              <p>
                Small mistakes were planted in a copy of the design, one at a time: {mistakes.caught} of {mistakes.tried} made at least
                one example fail
                {mistakes.tried < mistakes.sites && ` (a spread sample of ${mistakes.sites} possible places)`}.
              </p>
              {mistakes.slipped.length > 0 && (
                <ul className="small">
                  {mistakes.slipped.map((m) => (
                    <li key={`${m.file}:${m.line}:${m.kind}:${m.original}:${m.mistake}`}>{withCode(m.text)}</li>
                  ))}
                </ul>
              )}
            </>
          ) : (
            <p className="muted">{run ? "Checking…" : "Not checked."}</p>
          )}
        </Section>
      )}

      {(run?.machines.length ?? 0) > 0 && (
        <Section title="3. How it works">
          {run!.machines.map((m) => (
            <div key={`${m.module}.${m.state}`} className="report-machine">
              <h3>
                {m.module}: {m.states.length} states
              </h3>
              <StateMachine machine={m} />
            </div>
          ))}
        </Section>
      )}

      <Section title={`${(run?.machines.length ?? 0) > 0 ? 4 : 3}. Is it ready?`}>
        {!job ? (
          <p>Not built yet: the Health step's Build and check turns the design into a chip layout and checks it.</p>
        ) : (
          <>
            <p>
              From the build of <code>{job.commit_id.slice(0, 8)}</code> with <code>{job.config_path}</code>
              {job.commit_id !== head && <strong> (an older commit; the design has changed since)</strong>}.
            </p>
            <table className="report-table">
              <tbody>
                <Row label="Fast enough" ok={s?.checks.setup === true && s?.checks.hold === true}>
                  setup slack {fmt(s?.setup_wns_ns)} ns, hold slack {fmt(s?.hold_wns_ns)} ns
                  {spec?.clock && ` at ${spec.clock.mhz} MHz`}
                </Row>
                <Row label="Can be manufactured" ok={s?.checks.drc === true && s?.checks.lvs === true && s?.checks.antenna === true}>
                  layout rules {s?.checks.drc ? "clean" : "with errors"}, layout matches the design {s?.checks.lvs ? "yes" : "no"},
                  antenna {s?.checks.antenna ? "clean" : "problems"}
                </Row>
                <Row label="Works on the gates" ok={job.gls ? job.gls.status !== "failed" : null}>
                  {job.gls ? job.gls.summary : "no gate-level simulation"}
                </Row>
                <Row label="Size and power" ok={null}>
                  {typeof s?.die_area_um2 === "number" ? `${(s.die_area_um2 / 1e6).toFixed(4)} mm²` : "?"}, {fmt(s?.stdcells)} cells,{" "}
                  {typeof s?.power_total_w === "number" ? `${(s.power_total_w * 1e3).toFixed(2)} mW` : "?"}
                </Row>
              </tbody>
            </table>
          </>
        )}
        <p className="report-warn">
          <strong>Needs an expert before a real chip run:</strong> I/O pads and pin placement, manufacturing test (scan),
          behaviour at power-up, timing with real gate delays in simulation, and the final go/no-go.
        </p>
      </Section>

      <Section title={`${(run?.machines.length ?? 0) > 0 ? 5 : 4}. Where it came from`}>
        {merged.length > 0 && (
          <>
            <h3>Changes written by the AI agent and merged by a person</h3>
            <ul className="small">
              {merged.map((p) => (
                <li key={p.id}>
                  {p.title ?? p.request.slice(0, 80)}
                  {p.merged_commit && (
                    <>
                      {" "}
                      (commit <code>{p.merged_commit.slice(0, 8)}</code>)
                    </>
                  )}
                </li>
              ))}
            </ul>
          </>
        )}
        <h3>History</h3>
        <table className="report-table small">
          <tbody>
            {commits.slice(0, 30).map((c) => (
              <tr key={c.id}>
                <td>
                  <code>{c.id.slice(0, 8)}</code>
                </td>
                <td>{c.message.split("\n")[0]}</td>
                <td>{c.author_name}</td>
                <td>{new Date(c.time * 1000).toLocaleDateString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {commits.length > 30 && <p className="muted small">and {commits.length - 30} earlier commits</p>}
      </Section>
    </main>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="report-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Row({ label, ok, children }: { label: string; ok: boolean | null; children: ReactNode }) {
  return (
    <tr>
      <th>
        <span className={`report-verdict ${ok === null ? "" : ok ? "ok" : "bad"}`}>{ok === null ? "–" : ok ? "✓" : "!"}</span> {label}
      </th>
      <td>{children}</td>
    </tr>
  );
}

const fmt = (v: unknown) => (typeof v === "number" ? Number(v.toPrecision(4)).toLocaleString() : "?");

function withCode(text: string) {
  return text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>));
}
