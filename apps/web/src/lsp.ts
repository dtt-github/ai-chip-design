// A small LSP client for Monaco, talking to the API's Verible bridge
// (`/api/projects/{id}/lsp`). One instance per open project: it syncs the
// project's Verilog models, shows diagnostics and registers the editor
// features below. `stop()` removes all of it again.
import * as monaco from "monaco-editor/editor/editor.api";
import { currentUser } from "./auth";

export type LspStatus = "connecting" | "ready" | "unavailable" | "disconnected";

const LANGUAGES = ["verilog", "systemverilog"];
const REQUEST_TIMEOUT_MS = 15_000;

/* Minimal LSP shapes (positions are zero-based). */
interface LspPosition {
  line: number;
  character: number;
}
interface LspRange {
  start: LspPosition;
  end: LspPosition;
}
interface LspLocation {
  uri: string;
  range: LspRange;
}
interface LspLocationLink {
  targetUri: string;
  targetRange: LspRange;
  targetSelectionRange?: LspRange;
}
interface LspTextEdit {
  range: LspRange;
  newText: string;
}
interface LspWorkspaceEdit {
  changes?: Record<string, LspTextEdit[]>;
  documentChanges?: { textDocument: { uri: string }; edits: LspTextEdit[] }[];
}
interface LspDiagnostic {
  range: LspRange;
  severity?: number;
  code?: string | number;
  source?: string;
  message: string;
}
type LspMarkup = string | { kind?: string; language?: string; value: string };
interface LspDocumentSymbol {
  name: string;
  detail?: string;
  kind: number;
  range: LspRange;
  selectionRange: LspRange;
  children?: LspDocumentSymbol[];
}

const toRange = (r: LspRange) =>
  new monaco.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1);
const toPosition = (p: monaco.IPosition): LspPosition => ({ line: p.lineNumber - 1, character: p.column - 1 });
const toLspRange = (r: monaco.IRange): LspRange => ({
  start: { line: r.startLineNumber - 1, character: r.startColumn - 1 },
  end: { line: r.endLineNumber - 1, character: r.endColumn - 1 },
});

function markupToString(m: LspMarkup | LspMarkup[] | undefined): string {
  if (!m) return "";
  if (Array.isArray(m)) return m.map(markupToString).join("\n\n");
  if (typeof m === "string") return m;
  if (m.language) return "```" + m.language + "\n" + m.value + "\n```";
  return m.value;
}

const severity = (s?: number) =>
  s === 1
    ? monaco.MarkerSeverity.Error
    : s === 2
      ? monaco.MarkerSeverity.Warning
      : s === 3
        ? monaco.MarkerSeverity.Info
        : monaco.MarkerSeverity.Hint;

// LSP CompletionItemKind (1-based) → Monaco.
const K = monaco.languages.CompletionItemKind;
const completionKinds = [K.Text, K.Text, K.Method, K.Function, K.Constructor, K.Field, K.Variable, K.Class, K.Interface,
  K.Module, K.Property, K.Unit, K.Value, K.Enum, K.Keyword, K.Snippet, K.Color, K.File, K.Reference, K.Folder,
  K.EnumMember, K.Constant, K.Struct, K.Event, K.Operator, K.TypeParameter];

interface Options {
  projectId: string;
  onStatus: (status: LspStatus, detail?: string) => void;
  /** Open a project file (path relative to the project root) and select `range`. */
  openFile: (path: string, range?: monaco.IRange) => void;
}

export class VerilogLsp {
  private ws: WebSocket | null = null;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  private opened = new Map<monaco.editor.ITextModel, { version: number; listener: monaco.IDisposable }>();
  private disposables: monaco.IDisposable[] = [];
  private stopped = false;
  private ready = false;
  private retries = 0;
  private retryTimer: number | undefined;
  private readonly prefix: string;

  constructor(private readonly opts: Options) {
    this.prefix = `file:///${opts.projectId}/`;
  }

  /** The model URI the workspace should use for a project file. */
  static uriFor(projectId: string, path: string) {
    return `file:///${projectId}/${path}`;
  }

  start() {
    this.registerFeatures();
    this.connect();
  }

  stop() {
    this.stopped = true;
    window.clearTimeout(this.retryTimer);
    this.ws?.close();
    this.teardownSession();
    this.disposables.forEach((d) => d.dispose());
    this.disposables = [];
  }

  // ---- connection -------------------------------------------------------

  private async connect() {
    if (this.stopped) return;
    this.opts.onStatus("connecting");
    const user = await currentUser();
    if (!user || this.stopped) return;
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${window.location.host}/api/projects/${this.opts.projectId}/lsp`);
    this.ws = ws;
    let handshakeDone = false;
    let fatal = false;

    ws.onopen = () => ws.send(JSON.stringify({ token: user.access_token }));
    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data);
      if (!handshakeDone) {
        handshakeDone = true;
        if (msg.type === "ready") {
          this.initialize();
        } else {
          // Not installed or not allowed: retrying won't help (except when busy).
          fatal = msg.error !== "busy";
          this.opts.onStatus("unavailable", msg.message);
        }
        return;
      }
      this.handle(msg);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.teardownSession();
      if (this.stopped || fatal) return;
      this.opts.onStatus("disconnected");
      const delay = Math.min(30_000, 1000 * 2 ** this.retries++);
      this.retryTimer = window.setTimeout(() => this.connect(), delay);
    };
  }

  private async initialize() {
    try {
      await this.request("initialize", {
        processId: null,
        rootUri: null, // the bridge points this at the project workspace
        capabilities: {
          textDocument: {
            synchronization: { didSave: false },
            hover: { contentFormat: ["markdown", "plaintext"] },
            completion: { completionItem: { snippetSupport: false } },
            publishDiagnostics: {},
            definition: { linkSupport: true },
            documentSymbol: { hierarchicalDocumentSymbolSupport: true },
            codeAction: { codeActionLiteralSupport: { codeActionKind: { valueSet: ["quickfix", "refactor", "source"] } } },
            rename: { prepareSupport: false },
          },
          workspace: { workspaceEdit: { documentChanges: true } },
        },
      });
      this.notify("initialized", {});
      this.ready = true;
      this.retries = 0;
      this.opts.onStatus("ready");
      for (const model of monaco.editor.getModels()) this.maybeOpen(model);
    } catch (e) {
      this.opts.onStatus("unavailable", (e as Error).message);
    }
  }

  private teardownSession() {
    this.ready = false;
    for (const [model, doc] of this.opened) {
      doc.listener.dispose();
      if (!model.isDisposed()) monaco.editor.setModelMarkers(model, "verible", []);
    }
    this.opened.clear();
    for (const p of this.pending.values()) p.reject(new Error("language server disconnected"));
    this.pending.clear();
  }

  private send(msg: object) {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify({ jsonrpc: "2.0", ...msg }));
  }

  private notify(method: string, params: unknown) {
    this.send({ method, params });
  }

  private request<T>(method: string, params: unknown): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, {
        resolve: (v) => {
          window.clearTimeout(timer);
          resolve(v as T);
        },
        reject: (e) => {
          window.clearTimeout(timer);
          reject(e);
        },
      });
      this.send({ id, method, params });
    });
  }

  private handle(msg: { id?: number; method?: string; params?: any; result?: unknown; error?: { message: string } }) {
    if (msg.method === undefined && msg.id !== undefined) {
      const p = this.pending.get(msg.id);
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.reject(new Error(msg.error.message));
      else p.resolve(msg.result);
      return;
    }
    if (msg.method === "textDocument/publishDiagnostics") {
      const model = monaco.editor.getModel(monaco.Uri.parse(msg.params.uri));
      if (!model) return;
      const markers = (msg.params.diagnostics as LspDiagnostic[]).map((d) => ({
        ...toRange(d.range),
        severity: severity(d.severity),
        message: d.message,
        source: d.source ?? "verible",
        code: d.code === undefined ? undefined : String(d.code),
      }));
      monaco.editor.setModelMarkers(model, "verible", markers);
      return;
    }
    if (msg.method === "window/logMessage" || msg.method === "window/showMessage") {
      console.debug("[verible]", msg.params?.message);
    }
    // Server-to-client requests (registerCapability, configuration, progress): accept with null.
    if (msg.method !== undefined && msg.id !== undefined) this.send({ id: msg.id, result: null });
  }

  // ---- documents --------------------------------------------------------

  private owns(model: monaco.editor.ITextModel) {
    return model.uri.toString().startsWith(this.prefix) && LANGUAGES.includes(model.getLanguageId());
  }

  private maybeOpen(model: monaco.editor.ITextModel) {
    if (!this.ready || this.opened.has(model) || !this.owns(model)) return;
    const uri = model.uri.toString();
    const doc = { version: 1, listener: null as unknown as monaco.IDisposable };
    doc.listener = model.onDidChangeContent(() => {
      doc.version++;
      this.notify("textDocument/didChange", {
        textDocument: { uri, version: doc.version },
        contentChanges: [{ text: model.getValue() }],
      });
    });
    this.opened.set(model, doc);
    this.notify("textDocument/didOpen", {
      textDocument: { uri, languageId: model.getLanguageId(), version: doc.version, text: model.getValue() },
    });
  }

  private close(model: monaco.editor.ITextModel) {
    const doc = this.opened.get(model);
    if (!doc) return;
    doc.listener.dispose();
    this.opened.delete(model);
    this.notify("textDocument/didClose", { textDocument: { uri: model.uri.toString() } });
  }

  // ---- editor features --------------------------------------------------

  private registerFeatures() {
    const d = this.disposables;
    const L = monaco.languages;
    const doc = (model: monaco.editor.ITextModel) => ({ uri: model.uri.toString() });
    const active = (model: monaco.editor.ITextModel) => this.ready && this.opened.has(model);
    const safe = async <T>(f: () => Promise<T>): Promise<T | null> => {
      try {
        return await f();
      } catch {
        return null;
      }
    };

    d.push(monaco.editor.onDidCreateModel((m) => this.maybeOpen(m)));
    d.push(monaco.editor.onWillDisposeModel((m) => this.close(m)));
    d.push(
      monaco.editor.onDidChangeModelLanguage(({ model }) => {
        this.close(model);
        this.maybeOpen(model);
      }),
    );

    // Jumping to a definition in a file that isn't open: let the workspace open it.
    d.push(
      monaco.editor.registerEditorOpener({
        openCodeEditor: (_source, resource, selection) => {
          const uri = resource.toString();
          if (!uri.startsWith(this.prefix)) return false;
          const range =
            selection && "startLineNumber" in selection
              ? selection
              : selection
                ? new monaco.Range(selection.lineNumber, selection.column, selection.lineNumber, selection.column)
                : undefined;
          this.opts.openFile(decodeURIComponent(uri.slice(this.prefix.length)), range);
          return true;
        },
      }),
    );

    d.push(
      L.registerHoverProvider(LANGUAGES, {
        provideHover: (model, position) =>
          active(model)
            ? safe(async () => {
                const r = await this.request<{ contents: LspMarkup | LspMarkup[]; range?: LspRange } | null>(
                  "textDocument/hover",
                  { textDocument: doc(model), position: toPosition(position) },
                );
                const value = markupToString(r?.contents);
                return value ? { contents: [{ value }], range: r?.range && toRange(r.range) } : null;
              })
            : null,
      }),
    );

    d.push(
      L.registerCompletionItemProvider(LANGUAGES, {
        triggerCharacters: [".", "`", "$"],
        provideCompletionItems: async (model, position) => {
          if (!active(model)) return { suggestions: [] };
          const r = await safe(() =>
            this.request<{ items: any[] } | any[] | null>("textDocument/completion", {
              textDocument: doc(model),
              position: toPosition(position),
            }),
          );
          const items = Array.isArray(r) ? r : (r?.items ?? []);
          const word = model.getWordUntilPosition(position);
          const fallback = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
          return {
            suggestions: items.map((it) => ({
              label: it.label,
              kind: completionKinds[it.kind ?? 1] ?? K.Text,
              detail: it.detail,
              documentation: it.documentation ? { value: markupToString(it.documentation) } : undefined,
              insertText: it.textEdit?.newText ?? it.insertText ?? it.label,
              range: it.textEdit?.range ? toRange(it.textEdit.range) : fallback,
              sortText: it.sortText,
              filterText: it.filterText,
            })),
          };
        },
      }),
    );

    const locations = (r: LspLocation | LspLocation[] | LspLocationLink[] | null): monaco.languages.Location[] =>
      (Array.isArray(r) ? r : r ? [r] : []).map((l) =>
        "targetUri" in l
          ? { uri: monaco.Uri.parse(l.targetUri), range: toRange(l.targetSelectionRange ?? l.targetRange) }
          : { uri: monaco.Uri.parse(l.uri), range: toRange(l.range) },
      );

    d.push(
      L.registerDefinitionProvider(LANGUAGES, {
        provideDefinition: (model, position) =>
          active(model)
            ? safe(async () =>
                locations(
                  await this.request("textDocument/definition", {
                    textDocument: doc(model),
                    position: toPosition(position),
                  }),
                ),
              )
            : null,
      }),
    );

    d.push(
      L.registerReferenceProvider(LANGUAGES, {
        provideReferences: (model, position, context) =>
          active(model)
            ? safe(async () =>
                locations(
                  await this.request("textDocument/references", {
                    textDocument: doc(model),
                    position: toPosition(position),
                    context,
                  }),
                ),
              )
            : null,
      }),
    );

    d.push(
      L.registerDocumentHighlightProvider(LANGUAGES, {
        provideDocumentHighlights: (model, position) =>
          active(model)
            ? safe(async () => {
                const r = await this.request<{ range: LspRange; kind?: number }[] | null>(
                  "textDocument/documentHighlight",
                  { textDocument: doc(model), position: toPosition(position) },
                );
                return (r ?? []).map((h) => ({ range: toRange(h.range), kind: (h.kind ?? 1) - 1 }));
              })
            : null,
      }),
    );

    const symbols = (list: LspDocumentSymbol[]): monaco.languages.DocumentSymbol[] =>
      list.map((s) => ({
        name: s.name,
        detail: s.detail ?? "",
        kind: Math.max(0, s.kind - 1),
        tags: [],
        range: toRange(s.range),
        selectionRange: toRange(s.selectionRange),
        children: symbols(s.children ?? []),
      }));

    d.push(
      L.registerDocumentSymbolProvider(LANGUAGES, {
        provideDocumentSymbols: (model) =>
          active(model)
            ? safe(async () => {
                const r = await this.request<LspDocumentSymbol[] | null>("textDocument/documentSymbol", {
                  textDocument: doc(model),
                });
                return symbols((r ?? []).filter((s) => "selectionRange" in s));
              })
            : null,
      }),
    );

    const textEdits = (edits: LspTextEdit[] | null) =>
      (edits ?? []).map((e) => ({ range: toRange(e.range), text: e.newText }));

    d.push(
      L.registerDocumentFormattingEditProvider(LANGUAGES, {
        provideDocumentFormattingEdits: (model, options) =>
          active(model)
            ? safe(async () =>
                textEdits(
                  await this.request("textDocument/formatting", {
                    textDocument: doc(model),
                    options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
                  }),
                ),
              )
            : null,
      }),
    );

    d.push(
      L.registerDocumentRangeFormattingEditProvider(LANGUAGES, {
        provideDocumentRangeFormattingEdits: (model, range, options) =>
          active(model)
            ? safe(async () =>
                textEdits(
                  await this.request("textDocument/rangeFormatting", {
                    textDocument: doc(model),
                    range: toLspRange(range),
                    options: { tabSize: options.tabSize, insertSpaces: options.insertSpaces },
                  }),
                ),
              )
            : null,
      }),
    );

    const workspaceEdit = (w: LspWorkspaceEdit | null | undefined): monaco.languages.WorkspaceEdit => {
      const edits: monaco.languages.IWorkspaceTextEdit[] = [];
      const add = (uri: string, list: LspTextEdit[]) =>
        list.forEach((e) =>
          edits.push({
            resource: monaco.Uri.parse(uri),
            textEdit: { range: toRange(e.range), text: e.newText },
            versionId: undefined,
          }),
        );
      for (const [uri, list] of Object.entries(w?.changes ?? {})) add(uri, list);
      for (const dc of w?.documentChanges ?? []) if ("edits" in dc) add(dc.textDocument.uri, dc.edits);
      return { edits };
    };

    d.push(
      L.registerCodeActionProvider(LANGUAGES, {
        provideCodeActions: async (model, range, context) => {
          const none = { actions: [], dispose() {} };
          if (!active(model)) return none;
          const r = await safe(() =>
            this.request<any[] | null>("textDocument/codeAction", {
              textDocument: doc(model),
              range: toLspRange(range),
              context: {
                diagnostics: context.markers.map((m) => ({
                  range: toLspRange(m),
                  message: m.message,
                  severity: m.severity === monaco.MarkerSeverity.Error ? 1 : 2,
                  code: typeof m.code === "string" ? m.code : m.code?.value,
                  source: m.source,
                })),
              },
            }),
          );
          const actions = (r ?? [])
            .filter((a) => a.edit) // commands are not supported
            .map((a) => ({ title: a.title, kind: a.kind, isPreferred: a.isPreferred, edit: workspaceEdit(a.edit) }));
          return { actions, dispose() {} };
        },
      }),
    );

    d.push(
      L.registerRenameProvider(LANGUAGES, {
        provideRenameEdits: async (model, position, newName) => {
          if (!active(model)) return { edits: [] };
          const r = await this.request<LspWorkspaceEdit | null>("textDocument/rename", {
            textDocument: doc(model),
            position: toPosition(position),
            newName,
          });
          return workspaceEdit(r);
        },
      }),
    );
  }
}
