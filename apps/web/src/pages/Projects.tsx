import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, PROJECT_LICENCES, type Project, type Starter } from "../api";

export function ProjectsPage() {
  const navigate = useNavigate();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [visibility, setVisibility] = useState<"private" | "public">("private");
  const [licence, setLicence] = useState("Apache-2.0");
  const [starters, setStarters] = useState<Starter[]>([]);
  const [starter, setStarter] = useState<Starter | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.listProjects().then(setProjects, (e: Error) => setError(e.message));
    api.starters().then(setStarters, () => setStarters([]));
  }, []);

  /** Picks a starter (or none, for a blank project) and suggests its title as the name. */
  function pick(st: Starter | null) {
    if (starter && name === starter.title) setName(st ? st.title : "");
    else if (!name.trim() && st) setName(st.title);
    setStarter(st);
  }

  async function create(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const p = await api.createProject({ name: name.trim(), visibility, licence, starter: starter?.id });
      navigate(`/p/${p.id}`);
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
    }
  }

  return (
    <main className="page">
      <section className="card">
        <h2>New project</h2>
        <form className="row" onSubmit={create}>
          <input
            aria-label="Project name"
            placeholder="Project name, e.g. UART transmitter"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoFocus
          />
          <select aria-label="Visibility" value={visibility} onChange={(e) => setVisibility(e.target.value as "private" | "public")}>
            <option value="private">Private</option>
            <option value="public">Public</option>
          </select>
          <select aria-label="Licence" value={licence} onChange={(e) => setLicence(e.target.value)} title="Licence for the project's own files">
            {PROJECT_LICENCES.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
          <button className="primary" disabled={busy || !name.trim()}>
            {busy ? "Creating…" : "Create"}
          </button>
        </form>
        {error && <p className="error">{error}</p>}
        <p className="muted small">
          {starter
            ? `Starts from the ${starter.title} design: its spec, ${starter.examples} examples that already pass, and a LibreLane config for IHP SG13G2.`
            : "Starts with a counter design and a LibreLane config for IHP SG13G2."}{" "}
          The licence goes in the project's own files and decides which IP blocks it can import (starter designs keep their
          Apache-2.0 header).
        </p>
        {starters.length > 0 && (
          <div className="starters">
            <div className="starters-head">
              <span className="small">
                <strong>Start from a working design</strong>{" "}
                <span className="muted">
                  Each one is verified by simulation; change it in your own words afterwards, or add more as blocks.
                </span>
              </span>
              <button type="button" className={`idea ${starter ? "" : "on"}`} aria-pressed={!starter} onClick={() => pick(null)}>
                Blank project
              </button>
            </div>
            {categories(starters).map(([cat, list]) => (
              <div key={cat} className="starters-group">
                <span className="design-label">{cat}</span>
                <div className="starters-grid">
                  {list.map((st) => (
                    <button
                      key={st.id}
                      type="button"
                      className={`starter-card ${starter?.id === st.id ? "on" : ""}`}
                      aria-pressed={starter?.id === st.id}
                      onClick={() => pick(starter?.id === st.id ? null : st)}
                    >
                      <span className="starter-title">{st.title}</span>
                      <span className="small">{st.summary}</span>
                      <span className="muted small">
                        <code>{connections(st)}</code> · {st.examples} examples
                        {st.covered
                          ? st.covered[0] === st.covered[1]
                            ? " trying every case"
                            : ` trying ${st.covered[0]} of ${st.covered[1]} cases`
                          : ""}
                        {st.clock_mhz ? ` · ${st.clock_mhz} MHz` : " · no clock"}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}
        
      </section>

      <section>
        <h2>Your projects</h2>
        {projects === null && !error && <p className="muted">Loading…</p>}
        {projects?.length === 0 && <p className="muted">No projects yet.</p>}
        <ul className="project-list">
          {projects?.map((p) => (
            <li key={p.id}>
              <Link to={`/p/${p.id}`} className="project-item">
                <span className="project-name">{p.name}</span>
                <span className="badge">{p.pdk}</span>
                <span className="badge">{p.visibility}</span>
                <span className="badge">{p.licence}</span>
                <span className="badge">{p.role}</span>
                <span className="spacer" />
                <span className="muted small">updated {new Date(p.updated_at).toLocaleString()}</span>
              </Link>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}

/** Starters grouped by category, in the server's order. */
function categories(list: Starter[]): [string, Starter[]][] {
  const out: [string, Starter[]][] = [];
  for (const st of list) {
    const g = out.find(([c]) => c === st.category);
    if (g) g[1].push(st);
    else out.push([st.category, [st]]);
  }
  return out;
}

/** "start, data → tx, busy": what goes in and comes out, without the clock and reset. */
function connections(st: Starter) {
  const names = (dir: "in" | "out") =>
    st.ports
      .filter((p) => p.dir === dir && p.name !== st.clock && p.name !== st.reset)
      .map((p) => (p.width > 1 ? `${p.name}[${p.width}]` : p.name))
      .join(", ");
  return `${names("in") || "—"} → ${names("out")}`;
}
