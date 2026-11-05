import { useEffect, useState, type FormEvent } from "react";
import { api, type Member } from "../api";

/** Project members, and (for owners) adding people by email. */
export function ShareBox({ projectId, canManage }: { projectId: string; canManage: boolean }) {
  const [members, setMembers] = useState<Member[]>([]);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<"editor" | "viewer">("editor");
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (open) api.members(projectId).then(setMembers, (e: Error) => setError(e.message));
  }, [open, projectId]);

  async function add(e: FormEvent) {
    e.preventDefault();
    setError(null);
    try {
      const m = await api.addMember(projectId, { email: email.trim(), role });
      setMembers((all) => [...all.filter((x) => x.user_id !== m.user_id), m]);
      setEmail("");
    } catch (err) {
      setError((err as Error).message);
    }
  }

  return (
    <div className="share">
      <button className="ghost small" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        {open ? "▾" : "▸"} Members
      </button>
      {open && (
        <div className="share-body">
          <ul>
            {members.map((m) => (
              <li key={m.user_id}>
                <span>{m.display_name}</span> <span className="muted small">{m.email}</span>{" "}
                <span className="badge">{m.role}</span>
              </li>
            ))}
          </ul>
          {canManage && (
            <form onSubmit={add} className="share-form">
              <input
                type="email"
                placeholder="Email of someone who has signed in"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                aria-label="Member email"
              />
              <select value={role} onChange={(e) => setRole(e.target.value as "editor" | "viewer")} aria-label="Role">
                <option value="editor">editor</option>
                <option value="viewer">viewer</option>
              </select>
              <button className="small" disabled={!email.trim()}>
                Add
              </button>
            </form>
          )}
          {error && <p className="error small">{error}</p>}
        </div>
      )}
    </div>
  );
}
