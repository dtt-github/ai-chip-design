import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router";
import Editor, { type OnMount } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api";
import * as Y from "yjs";
import { MonacoBinding } from "y-monaco";
import { api, ApiError, type Change, type CommitInfo, type Project } from "../api";
import { currentUser, displayName } from "../auth";
import { CollabSession, type Peer } from "../collab";
import { languageFor } from "../monaco";
import { FileTree } from "../components/FileTree";
import { VerilogLsp, type LspStatus } from "../lsp";
import { RunsList, RunView } from "../components/Runs";
import { SimPanel, type SimFile } from "../components/SimPanel";
import { ShareBox } from "../components/ShareBox";

const HDL = /\.(s?v|vh|svh)$/i;

type Bottom = { kind: "run"; jobId: string } | { kind: "sim" } | null;
type ChangeKind = "A" | "M" | "D";

export function WorkspacePage() {
  const { id = "" } = useParams();
  const [project, setProject] = useState<Project | null>(null);
  const [session, setSession] = useState<CollabSession | null>(null);
  const [, setVersion] = useState(0);
  const [head, setHead] = useState<string | null>(null);
  /** Committed text at `head` (the base for changes); paths missing here but in `tree` are binary or large. */
  const [base, setBase] = useState<Record<string, string>>({});
  const [tree, setTree] = useState<string[]>([]);
  const [tabs, setTabs] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [history, setHistory] = useState<CommitInfo[]>([]);
  const [message, setMessage] = useState("");
  const [status, setStatus] = useState<{ kind: "error" | "info"; text: string } | null>(null);
  const [panelTab, setPanelTab] = useState<"changes" | "runs">("changes");
  const [bottom, setBottom] = useState<Bottom>(null);
  const [maximized, setMaximized] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [editor, setEditor] = useState<monaco.editor.IStandaloneCodeEditor | null>(null);
  const messageRef = useRef<HTMLInputElement>(null);
  const pendingReveal = useRef<{ path: string; range: monaco.IRange } | null>(null);
  const [lsp, setLsp] = useState<{ status: LspStatus; detail?: string }>({ status: "connecting" });

  const readOnly = !project?.role || project.role === "viewer" || session?.role === "viewer";

  // ---- the shared working copy ----

  useEffect(() => {
    let s: CollabSession | null = null;
    let disposed = false;
    currentUser().then((u) => {
      if (disposed || !u) return;
      s = new CollabSession(id, { name: displayName(u) });
      setSession(s);
    });
    return () => {
      disposed = true;
      s?.destroy();
      setSession(null);
    };
  }, [id]);

  // Re-render on document, presence and connection changes (batched per frame).
  useEffect(() => {
    if (!session) return;
    let frame = 0;
    const bump = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setVersion((v) => v + 1));
    };
    session.doc.on("update", bump);
    const unsub = session.subscribe(bump);
    return () => {
      cancelAnimationFrame(frame);
      session.doc.off("update", bump);
      unsub();
    };
  }, [session]);

  const loadHead = useCallback(async () => {
    const [p, t, log, snap] = await Promise.all([api.getProject(id), api.tree(id), api.log(id), api.snapshot(id)]);
    setProject(p);
    setTree(t.files.map((f) => f.path));
    setHistory(log);
    setBase(snap.files);
    setHead(snap.commit);
    return snap;
  }, [id]);

  useEffect(() => {
    setTabs([]);
    setActive(null);
    loadHead().catch((e: Error) => setStatus({ kind: "error", text: e.message }));
  }, [loadHead]);

  // When someone commits, the room moves to the new head: reload the base.
  const sharedHead = session?.meta.get("head") ?? null;
  useEffect(() => {
    if (sharedHead && head && sharedHead !== head) {
      loadHead().catch((e: Error) => setStatus({ kind: "error", text: e.message }));
    }
  }, [sharedHead, head, loadHead]);

  // Open a first HDL file once the working copy has arrived.
  const synced = session?.status === "synced";
  useEffect(() => {
    if (!synced || !session || active) return;
    const first = [...session.files.keys()].filter((p) => HDL.test(p)).sort()[0];
    if (first) openFile(first);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [synced]);

  useEffect(() => session?.setActiveFile(active), [session, active]);

  const textPaths = session ? [...session.files.keys()] : [];
  const deleted = new Set(session ? [...session.deleted.keys()] : []);
  const visiblePaths = useMemo(() => {
    const set = new Set(textPaths);
    for (const p of tree) if (!(p in base) && !deleted.has(p)) set.add(p); // binary/large files
    return [...set].sort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [textPaths.join("\n"), tree, base, [...deleted].join("\n")]);

  const changes: [string, ChangeKind][] = [];
  for (const p of textPaths) {
    const text = session!.files.get(p)!.toString();
    if (!(p in base)) {
      if (!tree.includes(p)) changes.push([p, "A"]);
    } else if (text !== base[p]) changes.push([p, "M"]);
  }
  for (const p of deleted) changes.push([p, "D"]);
  changes.sort(([a], [b]) => a.localeCompare(b));
  const dirty = new Set(changes.map(([p]) => p));

  // ---- editor ----

  // The language server needs the latest openFile (it closes over state).
  const openFileRef = useRef<(path: string, range?: monaco.IRange) => void>(() => {});
  openFileRef.current = (path, range) => {
    if (range) pendingReveal.current = { path, range };
    openFile(path);
  };

  useEffect(() => {
    const client = new VerilogLsp({
      projectId: id,
      onStatus: (s, detail) => setLsp({ status: s, detail }),
      openFile: (path, range) => openFileRef.current(path, range),
    });
    client.start();
    return () => {
      client.stop();
      const prefix = VerilogLsp.uriFor(id, "");
      for (const m of monaco.editor.getModels()) {
        if (m.uri.toString().startsWith(prefix) && !m.isAttachedToEditor()) m.dispose();
      }
    };
  }, [id]);

  // Bind the open file's model to its shared text; remote cursors come with it.
  const ytext: Y.Text | undefined = active ? session?.files.get(active) : undefined;
  useEffect(() => {
    const model = editor?.getModel();
    if (!editor || !model || !ytext || !session) return;
    if (model.uri.toString() !== VerilogLsp.uriFor(id, active!)) return;
    const binding = new MonacoBinding(ytext, model, new Set([editor]), session.awareness);
    revealPending();
    return () => binding.destroy();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, ytext, session, active]);

  const revealPending = () => {
    const target = pendingReveal.current;
    if (!target || !editor) return;
    if (editor.getModel()?.uri.toString() !== VerilogLsp.uriFor(id, target.path)) return;
    pendingReveal.current = null;
    editor.setSelection(target.range);
    editor.revealRangeInCenter(target.range);
    editor.focus();
  };

  function openFile(path: string) {
    setTabs((t) => (t.includes(path) ? t : [...t, path]));
    setActive(path);
  }

  function closeTab(path: string) {
    setTabs((t) => {
      const next = t.filter((p) => p !== path);
      if (active === path) setActive(next[next.length - 1] ?? null);
      return next;
    });
  }

  function newFile() {
    if (!session) return;
    const path = window.prompt("New file path", "src/new_module.v")?.trim().replace(/^\/+/, "");
    if (!path) return;
    if (!session.files.has(path)) {
      session.doc.transact(() => {
        session.files.set(path, new Y.Text());
        session.deleted.delete(path);
      });
    }
    openFile(path);
  }

  function deleteFile(path: string) {
    if (!session || !window.confirm(`Delete ${path}? The deletion is staged until you commit.`)) return;
    session.doc.transact(() => {
      session.files.delete(path);
      if (path in base) session.deleted.set(path, true);
    });
    closeTab(path);
  }

  async function commit() {
    if (!session || !changes.length || committing) return;
    const msg = message.trim() || `Update ${changes.map(([p]) => p.split("/").pop()).join(", ")}`;
    const body: Change[] = changes.map(([path, kind]) =>
      kind === "D" ? { op: "delete", path } : { op: "write", path, content: session.files.get(path)!.toString() },
    );
    setCommitting(true);
    setStatus(null);
    try {
      const c = await api.commit(id, { message: msg, parent: head, changes: body });
      setMessage("");
      await loadHead();
      setStatus({ kind: "info", text: `Committed ${c.id.slice(0, 8)}` });
    } catch (e) {
      const text =
        e instanceof ApiError && e.status === 409
          ? "Someone else committed first. The shared copy is being updated; review the changes and commit again."
          : (e as Error).message;
      setStatus({ kind: "error", text });
    } finally {
      setCommitting(false);
    }
  }

  const commitRef = useRef(commit);
  commitRef.current = commit;
  const onMount: OnMount = (ed, m) => {
    setEditor(ed);
    ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => {
      if (messageRef.current && !messageRef.current.value) messageRef.current.focus();
      else commitRef.current();
    });
  };

  // ---- simulation ----

  const gatherFiles = useCallback(async (): Promise<SimFile[]> => {
    if (!session) return [];
    return [...session.files.entries()]
      .filter(([p]) => HDL.test(p) && !p.endsWith(".vh") && !p.endsWith(".svh"))
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, t]) => ({ name, source: t.toString() }));
  }, [session]);

  // ---- presence ----

  const peers: Peer[] = session?.peers() ?? [];
  const peerFiles = new Map<string, Peer[]>();
  for (const p of peers) if (p.file) peerFiles.set(p.file, [...(peerFiles.get(p.file) ?? []), p]);
  const cursorCss = peers
    .map(
      (p) => `.yRemoteSelection-${p.clientId}{background-color:${p.color}33}
.yRemoteSelectionHead-${p.clientId}{position:absolute;border-left:2px solid ${p.color};height:100%;box-sizing:border-box}
.yRemoteSelectionHead-${p.clientId}::after{content:${JSON.stringify(p.name)};position:absolute;top:-1.3em;left:-2px;background:${p.color};color:#fff;font:11px system-ui;padding:0 4px;border-radius:3px 3px 3px 0;white-space:nowrap;pointer-events:none}`,
    )
    .join("\n");

  const isText = active ? !!session?.files.get(active) : false;
  const collabLabel = !session
    ? "connecting"
    : session.status === "synced"
      ? `live${peers.length ? ` · ${peers.length + 1} here` : ""}`
      : session.status;

  return (
    <div className="workspace">
      <style>{cursorCss}</style>
      <aside className="sidebar">
        <div className="sidebar-head">
          <div>
            <div className="project-title">{project?.name ?? "…"}</div>
            <div className="muted small">
              {project?.pdk} · {project?.visibility} · {project?.role ?? "read-only"}
            </div>
          </div>
          {peers.length > 0 && (
            <div className="peers" aria-label="People in this project">
              {peers.map((p) => (
                <span key={p.clientId} className="peer" style={{ background: p.color }} title={`${p.name}${p.file ? ` · ${p.file}` : ""}`}>
                  {p.name.slice(0, 1).toUpperCase()}
                </span>
              ))}
            </div>
          )}
          <ShareBox projectId={id} canManage={project?.role === "owner"} />
          <button
            className="primary sim-button"
            onClick={() => {
              setBottom({ kind: "sim" });
              setMaximized(false);
            }}
            disabled={!synced}
            title="Simulate the working copy in your browser"
          >
            ▶ Simulate
          </button>
        </div>
        <div className="sidebar-actions">
          <span className="muted small">FILES</span>
          <span className="spacer" />
          {!readOnly && (
            <button className="ghost small" onClick={newFile} title="New file" disabled={!synced}>
              + New
            </button>
          )}
        </div>
        <FileTree
          paths={visiblePaths}
          active={active}
          dirty={dirty}
          peers={peerFiles}
          onOpen={(p) => openFile(p)}
          onDelete={readOnly ? undefined : deleteFile}
        />
      </aside>

      <section className={`editor-area ${bottom && maximized ? "run-max" : ""}`}>
        <div className="tabs" role="tablist">
          {tabs.map((p) => (
            <div key={p} className={`tab ${p === active ? "active" : ""}`} role="tab" aria-selected={p === active}>
              <button className="tab-label" onClick={() => setActive(p)} title={p}>
                {p.split("/").pop()}
                {dirty.has(p) ? " •" : ""}
              </button>
              <button className="tab-close" onClick={() => closeTab(p)} aria-label={`Close ${p}`}>
                ×
              </button>
            </div>
          ))}
        </div>
        <div className="editor">
          {active && isText ? (
            <Editor
              path={VerilogLsp.uriFor(id, active)}
              language={languageFor(active)}
              defaultValue=""
              theme="vs-dark"
              onMount={onMount}
              options={{
                readOnly,
                fontSize: 13,
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                tabSize: 4,
                automaticLayout: true,
              }}
            />
          ) : (
            <div className="center muted">
              {!synced
                ? "Connecting to the shared working copy…"
                : active
                  ? "Binary or large file: not editable in the browser."
                  : "Open a file from the tree."}
            </div>
          )}
        </div>
        <footer className="statusbar">
          <span>main @ {head?.slice(0, 8) ?? "—"}</span>
          <span className={`lsp-status ${session?.status === "synced" ? "ready" : session?.status === "refused" ? "unavailable" : "connecting"}`}>
            {collabLabel}
            {session?.role === "viewer" ? " · view only" : ""}
          </span>
          {active && <span>{languageFor(active)}</span>}
          <span className={`lsp-status ${lsp.status}`} title={lsp.detail}>
            Verible: {lsp.status}
          </span>
          <span className="spacer" />
          {status && <span className={status.kind === "error" ? "error" : ""}>{status.text}</span>}
        </footer>
        {bottom?.kind === "run" && (
          <RunView
            jobId={bottom.jobId}
            canCancel={!readOnly}
            maximized={maximized}
            onMaximize={setMaximized}
            onClose={() => {
              setBottom(null);
              setMaximized(false);
            }}
          />
        )}
        {bottom?.kind === "sim" && (
          <SimPanel
            gatherFiles={gatherFiles}
            maximized={maximized}
            onMaximize={setMaximized}
            onOpenFile={(path, line) => {
              setMaximized(false);
              openFileRef.current(path, { startLineNumber: line, startColumn: 1, endLineNumber: line, endColumn: 1000 });
            }}
            onClose={() => {
              setBottom(null);
              setMaximized(false);
            }}
          />
        )}
      </section>

      <aside className="panel">
        <div className="panel-tabs" role="tablist">
          <button role="tab" aria-selected={panelTab === "changes"} className={panelTab === "changes" ? "active" : ""} onClick={() => setPanelTab("changes")}>
            Changes{changes.length ? ` (${changes.length})` : ""}
          </button>
          <button role="tab" aria-selected={panelTab === "runs"} className={panelTab === "runs" ? "active" : ""} onClick={() => setPanelTab("runs")}>
            Runs
          </button>
        </div>
        {panelTab === "runs" ? (
          <RunsList
            projectId={id}
            head={head}
            canRun={!readOnly}
            dirty={changes.length > 0}
            selected={bottom?.kind === "run" ? bottom.jobId : null}
            onSelect={(j) => setBottom({ kind: "run", jobId: j.id })}
          />
        ) : (
          <>
            <h3>Changes ({changes.length})</h3>
            <p className="muted small">Shared with everyone in this project until committed.</p>
            <ul className="changes">
              {changes.map(([p, kind]) => (
                <li key={p}>
                  <span className={`change-kind ${kind === "D" ? "del" : kind === "A" ? "add" : "mod"}`}>{kind}</span>
                  <button className="link" onClick={() => kind !== "D" && openFile(p)}>
                    {p}
                  </button>
                </li>
              ))}
            </ul>
            {!readOnly && (
              <form
                className="commit-form"
                onSubmit={(e) => {
                  e.preventDefault();
                  commit();
                }}
              >
                <input ref={messageRef} placeholder="Commit message" value={message} onChange={(e) => setMessage(e.target.value)} />
                <button className="primary" disabled={!changes.length || committing}>
                  {committing ? "Committing…" : "Commit"}
                </button>
                <p className="muted small">Ctrl/⌘+S commits</p>
              </form>
            )}

            <h3>History</h3>
            <ol className="history">
              {history.map((c) => (
                <li key={c.id}>
                  <div className="history-msg">{c.message.split("\n")[0]}</div>
                  <div className="muted small">
                    <code>{c.id.slice(0, 8)}</code> · {c.author_name} · {new Date(c.time * 1000).toLocaleString()}
                  </div>
                </li>
              ))}
            </ol>
          </>
        )}
      </aside>
    </div>
  );
}
