import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { api, PROJECT_LICENCES, type Project } from "../api";

/** Starting points for people who don't know where to begin: a name and a description for Describe. */
const IDEAS = [
  {
    name: "LED blinker",
    intent: "An LED blinker. While enable is on, the LED output toggles every N clock cycles, where N is a setting (use 4 so the tests stay short). When enable is off, the LED is off.",
  },
  {
    name: "Button debouncer",
    intent: "Clean up a noisy push-button. The output follows the button input only after the input has stayed the same for N clock cycles in a row, where N is a setting (use 4 so the tests stay short).",
  },
  {
    name: "PWM output",
    intent: "A PWM output for dimming an LED or driving a motor. An 8-bit duty value sets how many clock cycles out of every 256 the output is on. Duty 0 means always off.",
  },
  {
    name: "UART transmitter",
    intent: "Send bytes over a serial line like a PC's COM port: when start is pulsed, send a start bit (0), the 8 data bits lowest first, then a stop bit (1). Each bit lasts N clock cycles, where N is a setting (use 8 so the tests stay short). The line is 1 when idle, and busy is on while sending.",
  },
];

export function ProjectsPage() {
  const navigate = useNavigate();
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [visibility, setVisibility] = useState<"private" | "public">("private");
  const [licence, setLicence] = useState("Apache-2.0");
  const [intent, setIntent] = useState<string | null>(null);
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
      navigate(`/p/${p.id}`, { state: intent ? { intent } : undefined });
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
        <div className="ideas">
          <span className="muted small">New to chip design? Start from an idea; you'll describe it in your own words next.</span>
          <div className="ideas-row">
            {IDEAS.map((i) => (
              <button
                key={i.name}
                type="button"
                className={`idea ${intent === i.intent ? "on" : ""}`}
                aria-pressed={intent === i.intent}
                onClick={() => {
                  setName(i.name);
                  setIntent(i.intent);
                }}
              >
                {i.name}
              </button>
            ))}
          </div>
          {intent && <p className="small idea-text">{intent}</p>}
        </div>
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
