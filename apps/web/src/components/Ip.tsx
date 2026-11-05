import { useEffect, useState } from "react";
import { api, type ImportCheck, type IpBlock, type IpLockEntry, type LicenceStatus, type Project } from "../api";

const STATUS: Record<LicenceStatus, [string, string]> = {
  compatible: ["licence compatible", "status-succeeded"],
  conditions: ["compatible with conditions", "status-running"],
  unknown: ["licence not judged", "status-queued"],
  incompatible: ["licence incompatible", "status-failed"],
};

function LicenceBadge({ status }: { status: LicenceStatus }) {
  const [label, cls] = STATUS[status];
  return <span className={`status ${cls}`}>{label}</span>;
}

/** Bump the patch of the newest version, or start at 0.1.0. */
function nextVersion(block: IpBlock | undefined): string {
  const v = block?.versions[0]?.version;
  if (!v) return "0.1.0";
  const [a, b, c] = v.split(".").map(Number);
  return `${a}.${b}.${c + 1}`;
}

interface Props {
  project: Project;
  head: string | null;
  canEdit: boolean;
  dirty: boolean;
}

/** IP registry: what this project imports, blocks it can import, and publishing this project as a block. */
export function IpPanel({ project, head, canEdit, dirty }: Props) {
  const [blocks, setBlocks] = useState<IpBlock[] | null>(null);
  const [imports, setImports] = useState<Record<string, IpLockEntry>>({});
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let cancelled = false;
    Promise.all([api.ipBlocks(), api.ipImports(project.id)]).then(
      ([b, i]) => {
        if (cancelled) return;
        setBlocks(b);
        setImports(i.blocks ?? {});
      },
      (e: Error) => !cancelled && setError(e.message),
    );
    return () => {
      cancelled = true;
    };
    // A new head means an import may have changed things; `reload` follows a publish.
  }, [project.id, head, reload]);

  const own = blocks?.filter((b) => b.project_id === project.id) ?? [];
  const others = blocks?.filter((b) => b.project_id !== project.id) ?? [];
  const importedNames = Object.keys(imports).sort();

  return (
    <div className="ip">
      {error && <p className="error small">{error}</p>}
      <p className="muted small">
        This project's licence: <code>{project.licence}</code>
      </p>

      <h3>Imported ({importedNames.length})</h3>
      {importedNames.length === 0 && <p className="muted small">No IP blocks yet. Import one from the registry below.</p>}
      <ul className="ip-list">
        {importedNames.map((name) => {
          const e = imports[name];
          return (
            <li key={name} className="ip-item">
              <div className="ip-row">
                <strong>{name}</strong> <code>{e.version}</code>
                <span className="spacer" />
                <LicenceBadge status={e.licence_verdict.status} />
              </div>
              <p className="muted small">
                <code>{e.licence}</code> · from {e.source_name} @ <code>{e.commit.slice(0, 8)}</code> · <code>ip/{name}/</code>
                {e.licence_accepted_by && <> · licence accepted by {e.licence_accepted_by}</>}
              </p>
            </li>
          );
        })}
      </ul>

      <h3>Registry</h3>
      {blocks === null && !error && <p className="muted small">Loading…</p>}
      {blocks && others.length === 0 && <p className="muted small">No blocks from other projects you can see.</p>}
      <ul className="ip-list">
        {others.map((b) => (
          <li key={b.name} className="ip-item">
            <button className="ip-row link-row" onClick={() => setOpen(open === b.name ? null : b.name)} aria-expanded={open === b.name}>
              <strong>{b.name}</strong> <code>{b.versions[0]?.version}</code>
              <span className="spacer" />
              {imports[b.name] && <span className="status status-succeeded">imported {imports[b.name].version}</span>}
              <span className="muted small">{open === b.name ? "▾" : "▸"}</span>
            </button>
            <p className="muted small">
              {b.description || "No description"} · {b.project_name} · <code>{b.versions[0]?.licence}</code>
            </p>
            {open === b.name && <BlockDetail block={b} project={project} canEdit={canEdit} dirty={dirty} />}
          </li>
        ))}
      </ul>

      {canEdit && <Publish project={project} head={head} dirty={dirty} existing={own} onPublished={() => setReload((n) => n + 1)} />}
    </div>
  );
}

function BlockDetail({ block, project, canEdit, dirty }: { block: IpBlock; project: Project; canEdit: boolean; dirty: boolean }) {
  const [version, setVersion] = useState(block.versions[0]?.version ?? "");
  const [check, setCheck] = useState<ImportCheck | null>(null);
  const [accept, setAccept] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const v = block.versions.find((x) => x.version === version);

  useEffect(() => {
    setCheck(null);
    setAccept(false);
    setDone(null);
    if (!canEdit) return;
    let cancelled = false;
    api.ipCheck(project.id, block.name, version).then(
      (c) => !cancelled && setCheck(c),
      (e: Error) => !cancelled && setError(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [project.id, block.name, version, canEdit]);

  async function doImport() {
    setBusy(true);
    setError(null);
    try {
      const r = await api.ipImport(project.id, { block: block.name, version, accept_licence: accept || undefined });
      setDone(r.imported_commit);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  if (!v) return null;
  const params = Object.entries(v.params);
  const needsAccept = check?.licence.status === "unknown";
  return (
    <div className="ip-detail">
      <label className="ip-field">
        <span className="muted small">Version</span>
        <select value={version} onChange={(e) => setVersion(e.target.value)}>
          {block.versions.map((x) => (
            <option key={x.version} value={x.version}>
              {x.version} · {new Date(x.published_at).toLocaleDateString()}
            </option>
          ))}
        </select>
      </label>
      <p className="small">
        Top <code>{v.top}</code> · pinned to <code>{v.commit_id.slice(0, 8)}</code> · gate: {v.gate.summary}
      </p>
      <p className="small ip-ports">
        {v.ports.map((p) => (
          <code key={p.name} title={p.dir}>
            {p.dir === "input" ? "→" : p.dir === "output" ? "←" : "↔"} {p.name}
            {p.width && p.width > 1 ? `[${p.width - 1}:0]` : ""}
          </code>
        ))}
      </p>
      {params.length > 0 && (
        <p className="muted small">
          Parameters:{" "}
          {params.map(([k, val]) => (
            <code key={k}>{`${k}=${val} `}</code>
          ))}
        </p>
      )}
      <p className="muted small">Files: {Object.keys(v.files).map((f) => <code key={f}>{f} </code>)}</p>

      {check && (
        <div className="ip-check">
          <div className="ip-row">
            <LicenceBadge status={check.licence.status} />
            <span className="small">
              <code>{check.licence.block}</code> into <code>{check.licence.project}</code>
            </span>
          </div>
          <ul className="small">
            {check.licence.reasons.map((r) => (
              <li key={r}>{r}</li>
            ))}
          </ul>
          {check.licence.obligations.length > 0 && (
            <>
              <p className="small">If you import it, you agree to:</p>
              <ul className="small">
                {check.licence.obligations.map((o) => (
                  <li key={o}>{o}</li>
                ))}
              </ul>
            </>
          )}
          <p className="muted small">{check.licence.note}</p>
          {check.problems.map((p) => (
            <p key={p} className="error small">
              {p}
            </p>
          ))}
          {check.allowed && (
            <p className="muted small">
              {check.replaces ? `Replaces ${check.replaces}. ` : ""}Writes {check.files.join(", ")}
              {check.configs.length > 0 && <>, adds them to VERILOG_FILES in {check.configs.join(", ")}</>}, and pins the
              version in <code>ip.lock.json</code>, in one commit.
            </p>
          )}
          {check.allowed && needsAccept && (
            <label className="small ip-accept">
              <input type="checkbox" checked={accept} onChange={(e) => setAccept(e.target.checked)} /> I have read{" "}
              <code>{check.licence.block}</code> and accept it for this project
            </label>
          )}
          {check.allowed && (
            <button className="primary" onClick={doImport} disabled={busy || (needsAccept && !accept) || done !== null}>
              {busy ? "Importing…" : check.replaces ? `Update to ${version}` : `Import ${version}`}
            </button>
          )}
          {dirty && check.allowed && <p className="warn small">Uncommitted changes stay as they are; the import is its own commit.</p>}
        </div>
      )}
      {done && (
        <p className="ok small">
          Imported as commit <code>{done.slice(0, 8)}</code>.
        </p>
      )}
      {error && <p className="error small">{error}</p>}
    </div>
  );
}

function Publish({
  project,
  head,
  dirty,
  existing,
  onPublished,
}: {
  project: Project;
  head: string | null;
  dirty: boolean;
  existing: IpBlock[];
  onPublished: () => void;
}) {
  const [name, setName] = useState(existing[0]?.name ?? "");
  const [version, setVersion] = useState(nextVersion(existing[0]));
  const [description, setDescription] = useState(existing[0]?.description ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => {
    setName(existing[0]?.name ?? "");
    setVersion(nextVersion(existing[0]));
  }, [existing[0]?.name, existing[0]?.versions[0]?.version]);

  async function publish() {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      const v = await api.ipPublish(project.id, {
        version: version.trim(),
        name: name.trim() || undefined,
        description: description.trim(),
        commit: head ?? undefined,
      });
      setDone(`${v.block}@${v.version} (${v.licence})`);
      onPublished();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <h3>Publish this project</h3>
      {existing.map((b) => (
        <p key={b.name} className="muted small">
          Published as <strong>{b.name}</strong>: {b.versions.map((v) => v.version).join(", ")}
        </p>
      ))}
      <div className="ip-publish">
        <label className="ip-field">
          <span className="muted small">Block name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder="top module's name" disabled={existing.length > 0} />
        </label>
        <label className="ip-field">
          <span className="muted small">Version</span>
          <input value={version} onChange={(e) => setVersion(e.target.value)} placeholder="1.0.0" />
        </label>
        <label className="ip-field">
          <span className="muted small">Description</span>
          <input value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What the block does" />
        </label>
        <button className="primary" onClick={publish} disabled={busy || !head || !version.trim()}>
          {busy ? "Publishing…" : "Publish"}
        </button>
      </div>
      <p className="muted small">
        Publishes the top module from <code>config.json</code> and the modules it uses, at commit{" "}
        <code>{head?.slice(0, 8)}</code>. Every file needs an SPDX-License-Identifier header and the simulation gate must pass.
        {dirty && <span className="warn"> Uncommitted changes are not included.</span>}
      </p>
      {done && <p className="ok small">Published {done}.</p>}
      {error && <p className="error small">{error}</p>}
    </>
  );
}
