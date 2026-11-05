import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router";
import Editor, { type OnMount } from "@monaco-editor/react";
import * as monaco from "monaco-editor/editor/editor.api";
import { api, ApiError, type Change, type CommitInfo, type Project } from "../api";
import { languageFor } from "../monaco";
import { FileTree } from "../components/FileTree";
import { VerilogLsp, type LspStatus } from "../lsp";

/** An open or edited file. `base` is the committed content (`null` for a new file). */
interface Buffer {
  base: string | null;
  content: string;
  deleted: boolean;
  binary: boolean;
}

const isDirty = (b: Buffer) => b.deleted || b.base === null || b.content !== b.base;

export function WorkspacePage() {
  const { id = "" } = useParams();
  const [project, setProject] = useState<Project | null>(null);
  const [head, setHead] = useState<string | null>(null);
  const [paths, setPaths] = useState<string[]>([]);
  const [buffers, setBuffers] = useState<Record<string, Buffer>>({});
  const [tabs, setTabs] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [history, setHistory] = useState<CommitInfo[]>([]);
  const [message, setMessage] = useState("");
  const [status, setStatus] = useState<{ kind: "error" | "info"; text: string } | null>(null);
  const [committing, setCommitting] = useState(false);
  const messageRef = useRef<HTMLInputElement>(null);
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const pendingReveal = useRef<{ path: string; range: monaco.IRange } | null>(null);
  const [lsp, setLsp] = useState<{ status: LspStatus; detail?: string }>({ status: "connecting" });

  const readOnly = !project?.role || project.role === "viewer";

  const refresh = useCallback(async () => {
    const [p, tree, log] = await Promise.all([api.getProject(id), api.tree(id), api.log(id)]);
    setProject(p);
    setHead(tree.commit);
    setPaths(tree.files.map((f) => f.path));
    setHistory(log);
    return tree;
  }, [id]);

  useEffect(() => {
    setBuffers({});
    setTabs([]);
    setActive(null);
    refresh()
      .then((tree) => {
        const first = tree.files.find((f) => /\.s?v$/.test(f.path)) ?? tree.files[0];
        if (first) openFile(first.path, tree.commit ?? undefined);
      })
      .catch((e: Error) => setStatus({ kind: "error", text: e.message }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  // The language server needs the latest openFile (it closes over state).
  const openFileRef = useRef<(path: string, range?: monaco.IRange) => void>(() => {});
  openFileRef.current = (path, range) => {
    if (range) pendingReveal.current = { path, range };
    openFile(path);
  };

  useEffect(() => {
    const client = new VerilogLsp({
      projectId: id,
      onStatus: (status, detail) => setLsp({ status, detail }),
      openFile: (path, range) => openFileRef.current(path, range),
    });
    client.start();
    return () => {
      client.stop();
      // Drop this project's editor models (the mounted one is disposed by the editor itself).
      const prefix = VerilogLsp.uriFor(id, "");
      for (const m of monaco.editor.getModels()) {
        if (m.uri.toString().startsWith(prefix) && !m.isAttachedToEditor()) m.dispose();
      }
    };
  }, [id]);

  // Select a pending go-to-definition target once its file is showing. Runs
  // after renders and from onMount, since a newly opened file remounts the editor.
  const revealPending = () => {
    const target = pendingReveal.current;
    const editor = editorRef.current;
    if (!target || !editor) return;
    if (editor.getModel()?.uri.toString() !== VerilogLsp.uriFor(id, target.path)) return;
    pendingReveal.current = null;
    editor.setSelection(target.range);
    editor.revealRangeInCenter(target.range);
    editor.focus();
  };
  useEffect(revealPending);

  async function openFile(path: string, rev = head ?? undefined) {
    setTabs((t) => (t.includes(path) ? t : [...t, path]));
    setActive(path);
    if (buffers[path]) return;
    try {
      const f = await api.readFile(id, path, rev);
      setBuffers((b) =>
        b[path] ? b : { ...b, [path]: { base: f.content, content: f.content, deleted: false, binary: !f.text } },
      );
    } catch (e) {
      setStatus({ kind: "error", text: (e as Error).message });
    }
  }

  function closeTab(path: string) {
    setTabs((t) => {
      const next = t.filter((p) => p !== path);
      if (active === path) setActive(next[next.length - 1] ?? null);
      return next;
    });
  }

  function newFile() {
    const path = window.prompt("New file path", "src/new_module.v")?.trim().replace(/^\/+/, "");
    if (!path) return;
    if (paths.includes(path) || buffers[path]) {
      openFile(path);
      return;
    }
    setBuffers((b) => ({ ...b, [path]: { base: null, content: "", deleted: false, binary: false } }));
    setTabs((t) => [...t, path]);
    setActive(path);
  }

  async function deleteFile(path: string) {
    if (!window.confirm(`Delete ${path}? The deletion is staged until you commit.`)) return;
    const existing = buffers[path];
    if (existing?.base === null) {
      // Never committed: just drop it.
      setBuffers(({ [path]: _, ...rest }) => rest);
      closeTab(path);
      return;
    }
    let base = existing?.base;
    if (base === undefined) base = (await api.readFile(id, path, head ?? undefined)).content;
    setBuffers((b) => ({ ...b, [path]: { base: base!, content: base!, deleted: true, binary: false } }));
    closeTab(path);
  }

  const changes = useMemo(
    () =>
      Object.entries(buffers)
        .filter(([, b]) => isDirty(b))
        .sort(([a], [b]) => a.localeCompare(b)),
    [buffers],
  );

  async function commit() {
    if (!changes.length || committing) return;
    const msg = message.trim() || `Update ${changes.map(([p]) => p.split("/").pop()).join(", ")}`;
    const body: Change[] = changes.map(([path, b]) =>
      b.deleted ? { op: "delete", path } : { op: "write", path, content: b.content },
    );
    setCommitting(true);
    setStatus(null);
    try {
      const c = await api.commit(id, { message: msg, parent: head, changes: body });
      setBuffers((all) => {
        const next: Record<string, Buffer> = {};
        for (const [p, b] of Object.entries(all)) {
          if (b.deleted) continue;
          next[p] = { ...b, base: b.content };
        }
        return next;
      });
      setMessage("");
      await refresh();
      setStatus({ kind: "info", text: `Committed ${c.id.slice(0, 8)}` });
    } catch (e) {
      const text =
        e instanceof ApiError && e.status === 409
          ? "Someone committed since you loaded this project. Your edits are kept; reload to see their changes."
          : (e as Error).message;
      setStatus({ kind: "error", text });
    } finally {
      setCommitting(false);
    }
  }

  // Keep the latest commit() for the Monaco keybinding, which is registered once.
  const commitRef = useRef(commit);
  commitRef.current = commit;
  const onMount: OnMount = (editor, monaco) => {
    editorRef.current = editor;
    revealPending();
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
      if (messageRef.current && !messageRef.current.value) messageRef.current.focus();
      else commitRef.current();
    });
  };

  const visiblePaths = useMemo(() => {
    const set = new Set(paths);
    for (const [p, b] of Object.entries(buffers)) {
      if (b.deleted) set.delete(p);
      else set.add(p);
    }
    return [...set].sort();
  }, [paths, buffers]);

  const current = active ? buffers[active] : undefined;

  return (
    <div className="workspace">
      <aside className="sidebar">
        <div className="sidebar-head">
          <div>
            <div className="project-title">{project?.name ?? "…"}</div>
            <div className="muted small">
              {project?.pdk} · {project?.visibility} · {project?.role ?? "read-only"}
            </div>
          </div>
        </div>
        <div className="sidebar-actions">
          <span className="muted small">FILES</span>
          <span className="spacer" />
          {!readOnly && (
            <button className="ghost small" onClick={newFile} title="New file">
              + New
            </button>
          )}
        </div>
        <FileTree
          paths={visiblePaths}
          active={active}
          dirty={new Set(changes.map(([p]) => p))}
          onOpen={(p) => openFile(p)}
          onDelete={readOnly ? undefined : deleteFile}
        />
      </aside>

      <section className="editor-area">
        <div className="tabs" role="tablist">
          {tabs.map((p) => (
            <div key={p} className={`tab ${p === active ? "active" : ""}`} role="tab" aria-selected={p === active}>
              <button className="tab-label" onClick={() => setActive(p)} title={p}>
                {p.split("/").pop()}
                {buffers[p] && isDirty(buffers[p]) ? " •" : ""}
              </button>
              <button className="tab-close" onClick={() => closeTab(p)} aria-label={`Close ${p}`}>
                ×
              </button>
            </div>
          ))}
        </div>
        <div className="editor">
          {active && current && !current.binary ? (
            <Editor
              path={VerilogLsp.uriFor(id, active)}
              language={languageFor(active)}
              value={current.content}
              theme="vs-dark"
              onMount={onMount}
              onChange={(v) =>
                setBuffers((b) => (b[active] ? { ...b, [active]: { ...b[active], content: v ?? "" } } : b))
              }
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
              {current?.binary ? "Binary file: no text preview." : active ? "Loading…" : "Open a file from the tree."}
            </div>
          )}
        </div>
        <footer className="statusbar">
          <span>main @ {head?.slice(0, 8) ?? "—"}</span>
          {active && <span>{languageFor(active)}</span>}
          <span className={`lsp-status ${lsp.status}`} title={lsp.detail}>
            Verible: {lsp.status}
          </span>
          <span className="spacer" />
          {status && <span className={status.kind === "error" ? "error" : ""}>{status.text}</span>}
        </footer>
      </section>

      <aside className="panel">
        <h3>Changes ({changes.length})</h3>
        <ul className="changes">
          {changes.map(([p, b]) => (
            <li key={p}>
              <span className={`change-kind ${b.deleted ? "del" : b.base === null ? "add" : "mod"}`}>
                {b.deleted ? "D" : b.base === null ? "A" : "M"}
              </span>
              <button className="link" onClick={() => !b.deleted && openFile(p)}>
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
            <input
              ref={messageRef}
              placeholder="Commit message"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
            />
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
      </aside>
    </div>
  );
}
