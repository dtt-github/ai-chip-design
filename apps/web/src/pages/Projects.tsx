import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, PROJECT_LICENCES, type Project } from "../api";

export function ProjectsPage() {
  const navigate = useNavigate();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [visibility, setVisibility] = useState<"private" | "public">("private");
  const [licence, setLicence] = useState("Apache-2.0");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.listProjects().then(setProjects, (e: Error) => setError(e.message));
  }, []);

  async function create(e: FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const p = await api.createProject({ name: name.trim(), visibility, licence });
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
        <p className="muted small">
          Starts with a counter design and a LibreLane config for IHP SG13G2. The licence goes in every starter file's SPDX
          header, and decides which IP blocks the project can import.
        </p>
        {error && <p className="error">{error}</p>}
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
