import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type BlockModule, type BlockPort, type Blocks, type BlocksCheck, type BlocksView, type ReviewNote } from "../api";

// Canvas geometry.
const BW = 210;
const HEAD = 30;
const ROW = 22;
const PIN_W = 120;
const CANVAS_W = 1100;

type Sel = { kind: "block"; name: string } | { kind: "wire"; index: number } | { kind: "pin"; name: string } | null;

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

const ADAPTER_PORTS: Record<string, [string, string]> = {
  sc_rise_pulse: ["level", "pulse"],
  sc_sync2: ["d", "q"],
  sc_invert: ["a", "y"],
  sc_resize: ["a", "y"],
};

const ADAPTER_LABEL: Record<string, string> = {
  sc_rise_pulse: "level → one-clock pulse",
  sc_sync2: "synchronizer (outside signal → this clock)",
  sc_invert: "invert",
  sc_resize: "change width",
};

/** A chip from blocks: place modules, wire their connections, check, and save. */
export function BlocksStep({ projectId, head, canEdit }: { projectId: string; head: string | null; canEdit: boolean }) {
  const [view, setView] = useState<BlocksView | null>(null);
  const [draft, setDraft] = useState<Blocks | null>(null);
  const [check, setCheck] = useState<BlocksCheck | null>(null);
  const [sel, setSel] = useState<Sel>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [review, setReview] = useState<{ notes: ReviewNote[]; summary: string } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    const v = await api.getBlocks(projectId);
    setView(v);
    setDraft((d) => (d && view?.blocks && !same(d, view.blocks) ? d : v.blocks));
    setCheck(v.check);
    // `view` is only read to keep unsaved edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);
  useEffect(() => {
    load().catch((e: Error) => setError(e.message));
  }, [load, head]);

  // Re-check as the layout changes (debounced).
  useEffect(() => {
    if (!draft) return;
    const t = window.setTimeout(() => {
      api.checkBlocks(projectId, draft).then(setCheck, (e: Error) => setError(e.message));
    }, 350);
    return () => window.clearTimeout(t);
  }, [projectId, draft]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setPending(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (!view) return <p className="muted pad">{error ?? "Loading the blocks…"}</p>;
  const modules = view.modules;
  const moduleOf = (name: string) => modules.find((m) => m.name === name);

  if (!draft) {
    const design = view.design_name ? moduleOf(view.design_name) : undefined;
    return (
      <div className="design-col">
        <section className="design-card">
          <h3>Build the chip from blocks</h3>
          <p className="small" style={{ margin: 0 }}>
            Put your design together with blocks from the IP registry and small adapters, and connect them. SiliconCommons checks every
            connection and writes the chip's top level for you; saving makes the chip what Health builds.
          </p>
          {canEdit && design && (
            <div className="design-row">
              <button className="primary" onClick={() => setDraft(startFrom(design, view.design_name!))}>
                Start with {design.name}
              </button>
              <span className="muted small">Adds your design as the first block, with a chip pin for each of its connections.</span>
            </div>
          )}
          {view.problems.map((p) => (
            <p key={p} className="error small">
              {p}
            </p>
          ))}
        </section>
      </div>
    );
  }

  const dirty = !same(draft, view.blocks);
  const ports = check?.ports ?? {};
  const portsOf = (block: string): BlockPort[] => ports[block] ?? moduleOf(draft.blocks.find((b) => b.name === block)?.module ?? "")?.ports ?? [];
  const issuesFor = (i: number) => (check?.issues ?? []).filter((x) => x.wire === i);
  const noteFor = (i: number) => review?.notes.find((n) => n.wire === i);
  const errors = (check?.issues ?? []).filter((x) => x.level === "error").length;
  const update = (patch: Partial<Blocks>) => setDraft({ ...draft, ...patch });

  function clickEnd(end: string) {
    if (!canEdit) return;
    if (!pending) return setPending(end);
    if (pending !== end) {
      update({ wires: [...draft!.wires, { from: pending, to: end }] });
      setSel({ kind: "wire", index: draft!.wires.length });
      setReview(null);
    }
    setPending(null);
  }

  function addBlock(m: BlockModule) {
    const base = m.name.replace(/^sc_/, "");
    let n = 1;
    while (draft!.blocks.some((b) => b.name === `${base}_${n}`) || draft!.pins.some((p) => p.name === `${base}_${n}`)) n++;
    const name = `${base}_${n}`;
    const spot = freeSpot(draft!, portsOf, CANVAS_W / 2 - BW / 2, 40, m.ports.length);
    update({ blocks: [...draft!.blocks, { name, module: m.name, params: {}, x: spot.x, y: spot.y }] });
    setSel({ kind: "block", name });
  }

  /** Puts adapter `kind` into connection `i`: source → adapter → destination. */
  function insertAdapter(i: number, kind: string) {
    const w = draft!.wires[i];
    const isDriver = (end: string) => {
      const [b, p] = end.includes(".") ? end.split(".") : [null, end];
      if (!b) return draft!.pins.find((x) => x.name === p)?.dir === "in";
      return portsOf(b).find((x) => x.name === p)?.dir === "out";
    };
    const [src, dst] = isDriver(w.from) ? [w.from, w.to] : [w.to, w.from];
    const widthOf = (end: string) => {
      const [b, p] = end.includes(".") ? end.split(".") : [null, end];
      return b ? (portsOf(b).find((x) => x.name === p)?.width ?? 1) : (draft!.pins.find((x) => x.name === p)?.width ?? 1);
    };
    let n = 1;
    const base = kind.replace(/^sc_/, "");
    while (draft!.blocks.some((b) => b.name === `${base}_${n}`)) n++;
    const name = `${base}_${n}`;
    const params: Record<string, string> =
      kind === "sc_invert" ? { W: String(widthOf(src)) } : kind === "sc_resize" ? { IN: String(widthOf(src)), OUT: String(widthOf(dst)) } : {};
    const pos = (end: string) => endPoint(end, draft!, portsOf, "out");
    const a = pos(src);
    const b = pos(dst);
    const [pin, pout] = ADAPTER_PORTS[kind];
    const wires = draft!.wires.filter((_, j) => j !== i);
    const spot = freeSpot(draft!, portsOf, (a.x + b.x) / 2 - BW / 2, (a.y + b.y) / 2 - 40, 4);
    update({
      blocks: [...draft!.blocks, { name, module: kind, params, x: spot.x, y: spot.y }],
      wires: [...wires, { from: src, to: `${name}.${pin}` }, { from: `${name}.${pout}`, to: dst }],
    });
    setReview(null);
    setSel({ kind: "block", name });
  }

  async function save() {
    setBusy("Saving the chip…");
    setError(null);
    try {
      await api.saveBlocks(projectId, { blocks: draft!, parent: view!.head });
      await load();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  async function askReview() {
    setBusy("The assistant is reviewing the connections…");
    setError(null);
    try {
      setReview(await api.reviewBlocks(projectId, draft!));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  }

  const height = Math.max(
    560,
    ...draft.blocks.map((b) => b.y + blockHeight(portsOf(b.name)) + 40),
    40 + ROW * 1.5 * Math.max(draft.pins.filter((p) => p.dir === "in").length, draft.pins.filter((p) => p.dir === "out").length),
  );

  return (
    <div className="design-col blocks">
      <section className="design-card design-row" style={{ justifyContent: "space-between" }}>
        <div>
          <h3 style={{ margin: 0 }}>
            Chip <code>{draft.top}</code>
          </h3>
          <p className="muted small" style={{ margin: 0 }}>
            {pending
              ? `Connecting from ${pending}: click the other end (Esc to cancel).`
              : "Click a connection point, then another, to connect them. Drag blocks by their title."}
          </p>
        </div>
        <div className="design-row">
          <span className={`status ${errors ? "status-failed" : "status-succeeded"}`}>{errors ? `${errors} problem${errors === 1 ? "" : "s"}` : "connections check out"}</span>
          {canEdit && (
            <>
              <button onClick={askReview} disabled={!!busy || draft.wires.length === 0}>
                Ask the assistant to review
              </button>
              <button className="primary" onClick={save} disabled={!!busy || !dirty || errors > 0}>
                Save chip
              </button>
            </>
          )}
        </div>
      </section>
      {(busy || error || review) && (
        <div className="design-row small">
          {busy && <span className="muted">{busy}</span>}
          {error && <span className="error" style={{ whiteSpace: "pre-line" }}>{error}</span>}
          {review && !busy && <span>Assistant: {review.summary}</span>}
        </div>
      )}

      <div className="blocks-layout">
        <aside className="design-card blocks-palette">
          <h3>Blocks</h3>
          {groups(modules, draft.top).map(([title, ms]) => (
            <div key={title} className="blocks-group">
              <span className="design-label">{title}</span>
              {ms.map((m) => (
                <button key={m.name} className="blocks-mod" onClick={() => addBlock(m)} disabled={!canEdit} title={m.description || m.file}>
                  <code>{m.name}</code>
                  <span className="muted small">{m.description || `${m.ports.length} connections`}</span>
                </button>
              ))}
            </div>
          ))}
          <h3>Chip pins</h3>
          {canEdit && (
            <div className="design-row">
              <button className="small" onClick={() => addPin(draft, update, "in", setSel)}>
                + Input
              </button>
              <button className="small" onClick={() => addPin(draft, update, "out", setSel)}>
                + Output
              </button>
            </div>
          )}
        </aside>

        <div className="design-card blocks-canvas-wrap">
          <Canvas
            draft={draft}
            height={height}
            portsOf={portsOf}
            issuesFor={issuesFor}
            noteFor={noteFor}
            sel={sel}
            pending={pending}
            canEdit={canEdit}
            onSelect={setSel}
            onEnd={clickEnd}
            onMove={(name, x, y) => update({ blocks: draft.blocks.map((b) => (b.name === name ? { ...b, x, y } : b)) })}
          />
        </div>

        <aside className="design-card blocks-inspector">
          <Inspector
            draft={draft}
            sel={sel}
            canEdit={canEdit}
            portsOf={portsOf}
            moduleOf={moduleOf}
            meanings={view.meanings}
            issues={check?.issues ?? []}
            noteFor={noteFor}
            update={update}
            setSel={setSel}
            insertAdapter={insertAdapter}
          />
        </aside>
      </div>
    </div>
  );
}

function startFrom(m: BlockModule, design: string): Blocks {
  const name = m.name;
  const pins = m.ports.filter((p) => p.dir !== "inout").map((p) => ({ name: p.name, dir: p.dir as "in" | "out", width: p.width, meaning: "" }));
  return {
    top: `${design}_chip`,
    clock: pins.find((p) => p.name === "clk" && p.dir === "in") ? "clk" : null,
    reset: pins.find((p) => /^rst/.test(p.name) && p.dir === "in")?.name ?? null,
    pins,
    blocks: [{ name, module: m.name, params: {}, x: 260, y: 60 }],
    wires: pins
      .filter((p) => p.name !== "clk" && !/^rst/.test(p.name))
      .map((p) => (p.dir === "in" ? { from: p.name, to: `${name}.${p.name}` } : { from: `${name}.${p.name}`, to: p.name })),
  };
}

function groups(modules: BlockModule[], top: string): [string, BlockModule[]][] {
  const mine = modules.filter((m) => m.source === "project" && m.name !== top);
  const ip = modules.filter((m) => m.source.startsWith("ip:"));
  const adapters = modules.filter((m) => m.source === "adapter");
  return (
    [
      ["Your design", mine],
      ["Imported IP", ip],
      ["Adapters", adapters],
    ] as [string, BlockModule[]][]
  ).filter(([, ms]) => ms.length);
}

function addPin(draft: Blocks, update: (p: Partial<Blocks>) => void, dir: "in" | "out", setSel: (s: Sel) => void) {
  let n = 1;
  const base = dir === "in" ? "in" : "out";
  while (draft.pins.some((p) => p.name === `${base}${n}`)) n++;
  update({ pins: [...draft.pins, { name: `${base}${n}`, dir, width: 1, meaning: "" }] });
  setSel({ kind: "pin", name: `${base}${n}` });
}

/** Near (x, y), a place for a new block (`n` connections) that doesn't overlap the others. */
function freeSpot(draft: Blocks, portsOf: (b: string) => BlockPort[], x: number, y: number, n: number) {
  const h = HEAD + ROW * Math.max(1, Math.ceil(n / 2)) + 8;
  const clampX = (v: number) => Math.round(Math.max(PIN_W + 30, Math.min(CANVAS_W - PIN_W - 30 - BW, v)));
  const free = (cx: number, cy: number) =>
    draft.blocks.every((b) => {
      const bh = blockHeight(portsOf(b.name));
      return cx + BW + 20 <= b.x || b.x + BW + 20 <= cx || cy + h + 20 <= b.y || b.y + bh + 20 <= cy;
    });
  const candidates: [number, number][] = [[clampX(x), Math.max(4, y)]];
  for (let dy = 0; dy <= 1200; dy += 40) {
    for (const cx of [clampX(x), clampX(x - BW - 40), clampX(x + BW + 40)]) candidates.push([cx, Math.max(4, y + dy)]);
  }
  const [fx, fy] = candidates.find(([cx, cy]) => free(cx, cy)) ?? candidates[0];
  return { x: fx, y: Math.round(fy) };
}

function blockHeight(ports: BlockPort[]) {
  const ins = ports.filter((p) => p.dir !== "out").length;
  const outs = ports.filter((p) => p.dir === "out").length;
  return HEAD + ROW * Math.max(ins, outs, 1) + 8;
}

/** Where a connection end sits on the canvas. */
function endPoint(end: string, draft: Blocks, portsOf: (b: string) => BlockPort[], _side: string): { x: number; y: number } {
  if (!end.includes(".")) {
    const pin = draft.pins.find((p) => p.name === end);
    const list = draft.pins.filter((p) => p.dir === pin?.dir);
    const i = Math.max(0, list.findIndex((p) => p.name === end));
    const y = 40 + i * ROW * 1.5 + ROW / 2;
    return pin?.dir === "out" ? { x: CANVAS_W - PIN_W - 10, y } : { x: 10 + PIN_W, y };
  }
  const [bn, pn] = end.split(".");
  const b = draft.blocks.find((x) => x.name === bn);
  if (!b) return { x: 0, y: 0 };
  const ps = portsOf(bn);
  const p = ps.find((x) => x.name === pn);
  const side = ps.filter((x) => (p?.dir === "out" ? x.dir === "out" : x.dir !== "out"));
  const i = Math.max(0, side.findIndex((x) => x.name === pn));
  return { x: p?.dir === "out" ? b.x + BW : b.x, y: b.y + HEAD + i * ROW + ROW / 2 };
}

function Canvas({
  draft,
  height,
  portsOf,
  issuesFor,
  noteFor,
  sel,
  pending,
  canEdit,
  onSelect,
  onEnd,
  onMove,
}: {
  draft: Blocks;
  height: number;
  portsOf: (b: string) => BlockPort[];
  issuesFor: (i: number) => { level: string; message: string }[];
  noteFor: (i: number) => ReviewNote | undefined;
  sel: Sel;
  pending: string | null;
  canEdit: boolean;
  onSelect: (s: Sel) => void;
  onEnd: (end: string) => void;
  onMove: (name: string, x: number, y: number) => void;
}) {
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<{ name: string; dx: number; dy: number } | null>(null);
  const toLocal = (e: React.PointerEvent) => {
    const r = svg.current!.getBoundingClientRect();
    return { x: ((e.clientX - r.left) / r.width) * CANVAS_W, y: ((e.clientY - r.top) / r.height) * height };
  };
  const ins = draft.pins.filter((p) => p.dir === "in");
  const outs = draft.pins.filter((p) => p.dir === "out");
  const wires = useMemo(
    () =>
      draft.wires.map((w, i) => {
        const a = endPoint(w.from, draft, portsOf, "");
        const b = endPoint(w.to, draft, portsOf, "");
        const issues = issuesFor(i);
        const note = noteFor(i);
        const cls = issues.some((x) => x.level === "error") ? "bad" : issues.length || note?.level === "warning" ? "warn" : "ok";
        const dx = Math.max(40, Math.abs(b.x - a.x) / 2);
        const [s, t] = a.x <= b.x ? [a, b] : [b, a];
        return { i, cls, d: `M${s.x},${s.y} C${s.x + dx},${s.y} ${t.x - dx},${t.y} ${t.x},${t.y}`, title: [...issues.map((x) => x.message), note?.note].filter(Boolean).join("\n") };
      }),
    [draft, portsOf, issuesFor, noteFor],
  );
  const endCls = (end: string) => `blk-end ${pending === end ? "pending" : ""}`;

  return (
    <svg
      ref={svg}
      className="blocks-canvas"
      viewBox={`0 0 ${CANVAS_W} ${height}`}
      style={{ minWidth: 760, aspectRatio: `${CANVAS_W} / ${height}` }}
      role="img"
      aria-label={`Chip ${draft.top}: ${draft.blocks.length} blocks, ${draft.wires.length} connections`}
      onPointerMove={(e) => {
        if (!drag.current) return;
        const p = toLocal(e);
        onMove(drag.current.name, Math.round(Math.max(PIN_W + 30, Math.min(CANVAS_W - PIN_W - 30 - BW, p.x - drag.current.dx))), Math.round(Math.max(4, p.y - drag.current.dy)));
      }}
      onPointerUp={() => (drag.current = null)}
      onPointerLeave={() => (drag.current = null)}
    >
      <text x={10} y={22} className="blk-col">
        chip inputs
      </text>
      <text x={CANVAS_W - 10} y={22} className="blk-col" textAnchor="end">
        chip outputs
      </text>
      {[...ins, ...outs].map((p) => {
        const pt = endPoint(p.name, draft, portsOf, "");
        const x = p.dir === "in" ? 10 : CANVAS_W - PIN_W - 10;
        const on = sel?.kind === "pin" && sel.name === p.name;
        const role = p.name === draft.clock ? " (clock)" : p.name === draft.reset ? " (reset)" : "";
        return (
          <g key={`pin-${p.name}`}>
            <rect x={x} y={pt.y - 11} width={PIN_W} height={22} rx={4} className={`blk-pin ${on ? "on" : ""}`} onClick={() => onSelect({ kind: "pin", name: p.name })} />
            <text x={x + 6} y={pt.y + 4} className="blk-label" onClick={() => onSelect({ kind: "pin", name: p.name })}>
              {p.name}
              {p.width > 1 ? ` [${p.width}]` : ""}
              {role}
            </text>
            <circle cx={pt.x} cy={pt.y} r={6} className={endCls(p.name)} onClick={() => onEnd(p.name)}>
              <title>{`${p.name}: click to connect`}</title>
            </circle>
          </g>
        );
      })}
      {wires.map((w) => (
        <g key={`w-${w.i}`} onClick={() => onSelect({ kind: "wire", index: w.i })} className="blk-wire-g">
          <path d={w.d} className="blk-wire-hit" />
          <path d={w.d} className={`blk-wire ${w.cls} ${sel?.kind === "wire" && sel.index === w.i ? "on" : ""}`}>
            {w.title && <title>{w.title}</title>}
          </path>
        </g>
      ))}
      {draft.blocks.map((b) => {
        const ps = portsOf(b.name);
        const h = blockHeight(ps);
        const on = sel?.kind === "block" && sel.name === b.name;
        const inputs = ps.filter((p) => p.dir !== "out");
        const outputs = ps.filter((p) => p.dir === "out");
        return (
          <g key={`b-${b.name}`}>
            <rect x={b.x} y={b.y} width={BW} height={h} rx={8} className={`blk-box ${on ? "on" : ""} ${b.module.startsWith("sc_") ? "adapter" : ""}`} />
            <rect
              x={b.x}
              y={b.y}
              width={BW}
              height={HEAD - 4}
              rx={8}
              className="blk-head"
              style={{ cursor: canEdit ? "grab" : "pointer" }}
              onPointerDown={(e) => {
                onSelect({ kind: "block", name: b.name });
                if (!canEdit) return;
                const p = toLocal(e);
                drag.current = { name: b.name, dx: p.x - b.x, dy: p.y - b.y };
              }}
            />
            <text x={b.x + 10} y={b.y + 18} className="blk-title" pointerEvents="none">
              {b.name}
              <tspan className="blk-mod"> {b.module}</tspan>
            </text>
            {inputs.map((p, i) => {
              const y = b.y + HEAD + i * ROW + ROW / 2;
              const end = `${b.name}.${p.name}`;
              return (
                <g key={`i-${p.name}`}>
                  <circle cx={b.x} cy={y} r={6} className={endCls(end)} onClick={() => onEnd(end)}>
                    <title>{`${end} (in, ${p.width} bit${p.width === 1 ? "" : "s"}): click to connect`}</title>
                  </circle>
                  <text x={b.x + 10} y={y + 4} className="blk-label">
                    {p.name}
                    {p.width > 1 ? ` [${p.width}]` : ""}
                  </text>
                </g>
              );
            })}
            {outputs.map((p, i) => {
              const y = b.y + HEAD + i * ROW + ROW / 2;
              const end = `${b.name}.${p.name}`;
              return (
                <g key={`o-${p.name}`}>
                  <text x={b.x + BW - 10} y={y + 4} className="blk-label" textAnchor="end">
                    {p.width > 1 ? `[${p.width}] ` : ""}
                    {p.name}
                  </text>
                  <circle cx={b.x + BW} cy={y} r={6} className={endCls(end)} onClick={() => onEnd(end)}>
                    <title>{`${end} (out, ${p.width} bit${p.width === 1 ? "" : "s"}): click to connect`}</title>
                  </circle>
                </g>
              );
            })}
          </g>
        );
      })}
    </svg>
  );
}

function Inspector({
  draft,
  sel,
  canEdit,
  portsOf,
  moduleOf,
  meanings,
  issues,
  noteFor,
  update,
  setSel,
  insertAdapter,
}: {
  draft: Blocks;
  sel: Sel;
  canEdit: boolean;
  portsOf: (b: string) => BlockPort[];
  moduleOf: (m: string) => BlockModule | undefined;
  meanings: Record<string, Record<string, string>>;
  issues: { level: string; wire: number | null; block: string | null; message: string }[];
  noteFor: (i: number) => ReviewNote | undefined;
  update: (p: Partial<Blocks>) => void;
  setSel: (s: Sel) => void;
  insertAdapter: (i: number, kind: string) => void;
}) {
  const general = issues.filter((i) => i.wire === null);
  const ro = !canEdit;

  if (sel?.kind === "wire" && draft.wires[sel.index]) {
    const w = draft.wires[sel.index];
    const note = noteFor(sel.index);
    const mine = issues.filter((i) => i.wire === sel.index);
    return (
      <>
        <h3>Connection {sel.index + 1}</h3>
        <p className="small">
          <code>{w.from}</code> → <code>{w.to}</code>
        </p>
        {mine.map((i) => (
          <p key={i.message} className={`small ${i.level === "error" ? "error" : "warn"}`}>
            {i.message}
          </p>
        ))}
        {!mine.length && <p className="ok small">The ends exist, the direction is right and the widths match.</p>}
        {note && (
          <div className={`blocks-note ${note.level}`}>
            <span className="small">Assistant: {note.note}</span>
            {note.adapter && canEdit && (
              <button className="primary small" onClick={() => insertAdapter(sel.index, note.adapter)}>
                Add adapter: {ADAPTER_LABEL[note.adapter]}
              </button>
            )}
          </div>
        )}
        {canEdit && (
          <div className="design-row">
            <select aria-label="Insert an adapter" value="" onChange={(e) => e.target.value && insertAdapter(sel.index, e.target.value)}>
              <option value="">Insert an adapter…</option>
              {Object.entries(ADAPTER_LABEL).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
            <button className="small" onClick={() => (update({ wires: draft.wires.filter((_, j) => j !== sel.index) }), setSel(null))}>
              Remove connection
            </button>
          </div>
        )}
      </>
    );
  }

  if (sel?.kind === "block") {
    const b = draft.blocks.find((x) => x.name === sel.name);
    if (b) {
      const m = moduleOf(b.module);
      const mean = meanings[b.module] ?? {};
      const mine = issues.filter((i) => i.block === b.name);
      return (
        <>
          <h3>
            Block <code>{b.name}</code>
          </h3>
          <p className="muted small">
            {b.module}
            {m?.description ? ` · ${m.description}` : m?.file ? ` · ${m.file}` : ""}
          </p>
          {mine.map((i) => (
            <p key={i.message} className={`small ${i.level === "error" ? "error" : "warn"}`}>
              {i.message}
            </p>
          ))}
          {m && Object.keys(m.params).length > 0 && (
            <>
              <span className="design-label">Settings</span>
              {Object.entries(m.params).map(([k, def]) => (
                <label key={k} className="design-row small">
                  <code style={{ minWidth: 110 }}>{k}</code>
                  <input
                    aria-label={`${k} for ${b.name}`}
                    placeholder={String(def)}
                    value={b.params[k] ?? ""}
                    readOnly={ro}
                    onChange={(e) => {
                      const params = { ...b.params };
                      if (e.target.value.trim()) params[k] = e.target.value.trim();
                      else delete params[k];
                      update({ blocks: draft.blocks.map((x) => (x.name === b.name ? { ...x, params } : x)) });
                    }}
                    style={{ width: 90 }}
                  />
                  <span className="muted">default {def}</span>
                </label>
              ))}
            </>
          )}
          <span className="design-label">Connections</span>
          <ul className="design-list small">
            {portsOf(b.name).map((p) => (
              <li key={p.name}>
                <code>{p.name}</code> {p.dir}, {p.width} bit{p.width === 1 ? "" : "s"}
                {mean[p.name] ? ` · ${mean[p.name]}` : ""}
              </li>
            ))}
          </ul>
          {canEdit && (
            <button
              className="small"
              onClick={() => {
                update({
                  blocks: draft.blocks.filter((x) => x.name !== b.name),
                  wires: draft.wires.filter((w) => !w.from.startsWith(`${b.name}.`) && !w.to.startsWith(`${b.name}.`)),
                });
                setSel(null);
              }}
            >
              Remove block
            </button>
          )}
        </>
      );
    }
  }

  if (sel?.kind === "pin") {
    const p = draft.pins.find((x) => x.name === sel.name);
    if (p) {
      const rename = (name: string) => {
        const swap = (end: string) => (end === p.name ? name : end);
        update({
          pins: draft.pins.map((x) => (x.name === p.name ? { ...x, name } : x)),
          wires: draft.wires.map((w) => ({ from: swap(w.from), to: swap(w.to) })),
          clock: draft.clock === p.name ? name : draft.clock,
          reset: draft.reset === p.name ? name : draft.reset,
        });
        setSel({ kind: "pin", name });
      };
      const set = (patch: Partial<typeof p>) => update({ pins: draft.pins.map((x) => (x.name === p.name ? { ...x, ...patch } : x)) });
      return (
        <>
          <h3>Chip pin</h3>
          <label className="design-field small">
            <span className="design-label">Name</span>
            <input value={p.name} readOnly={ro} onChange={(e) => rename(e.target.value.replace(/[^A-Za-z0-9_]/g, ""))} />
          </label>
          <label className="design-field small">
            <span className="design-label">What it is</span>
            <input value={p.meaning} readOnly={ro} placeholder="e.g. Push-button" onChange={(e) => set({ meaning: e.target.value })} />
          </label>
          <label className="design-row small">
            Bits
            <input type="number" min={1} max={64} value={p.width} readOnly={ro} onChange={(e) => set({ width: Number(e.target.value) })} style={{ width: 70 }} />
          </label>
          {p.dir === "in" && p.width === 1 && canEdit && (
            <div className="design-row small">
              <label>
                <input type="checkbox" checked={draft.clock === p.name} onChange={(e) => update({ clock: e.target.checked ? p.name : null })} /> clock
              </label>
              <label>
                <input type="checkbox" checked={draft.reset === p.name} onChange={(e) => update({ reset: e.target.checked ? p.name : null })} /> reset
              </label>
            </div>
          )}
          {canEdit && (
            <button
              className="small"
              onClick={() => {
                update({
                  pins: draft.pins.filter((x) => x.name !== p.name),
                  wires: draft.wires.filter((w) => w.from !== p.name && w.to !== p.name),
                  clock: draft.clock === p.name ? null : draft.clock,
                  reset: draft.reset === p.name ? null : draft.reset,
                });
                setSel(null);
              }}
            >
              Remove pin
            </button>
          )}
        </>
      );
    }
  }

  return (
    <>
      <h3>Checks</h3>
      {general.length === 0 && <p className="ok small">Nothing to fix outside the connections.</p>}
      {general.map((i) => (
        <p key={i.message} className={`small ${i.level === "error" ? "error" : "warn"}`}>
          {i.message}
        </p>
      ))}
      <p className="muted small">
        Select a block, pin or connection to see and change it. Ports named like the clock and reset pins are connected to them
        automatically.
      </p>
    </>
  );
}
