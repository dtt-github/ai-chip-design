import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type DrcReport, type DrcViolation, type Job } from "../api";
import { parseGds, type LoadedLayout } from "../layout/gds";
import { createRenderer, type Markers, type Renderer, type View } from "../layout/renderer";
import { cssColor, layerInfo, type LayerInfo } from "../layout/stack";

const LABEL_MIN_SCALE = 25; // px per µm before pin labels appear
const MAX_LABELS = 1500;

interface Props {
  job: Job;
  gdsPath: string;
}

export function LayoutViewer({ job, gdsPath }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const labelsRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<Renderer | null>(null);
  const viewRef = useRef<View>({ cx: 0, cy: 0, scale: 1 });
  const frame = useRef(0);

  const [layout, setLayout] = useState<LoadedLayout | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visible, setVisible] = useState<Record<number, boolean>>({});
  const [rendererKind, setRendererKind] = useState<string>("");
  const [cursor, setCursor] = useState<[number, number] | null>(null);
  const [drc, setDrc] = useState<DrcReport | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [side, setSide] = useState<"layers" | "drc">("layers");

  const infos = useMemo<LayerInfo[]>(
    () => (layout ? layout.layers.map((l) => layerInfo(job.pdk, l.key)).sort((a, b) => b.order - a.order) : []),
    [layout, job.pdk],
  );

  // Load: GDS bytes → WASM parse; DRC report in parallel.
  useEffect(() => {
    let cancelled = false;
    setLayout(null);
    setError(null);
    (async () => {
      try {
        const blob = await api.artifactBlob(job.id, gdsPath);
        const parsed = await parseGds(new Uint8Array(await blob.arrayBuffer()));
        if (cancelled) return;
        setLayout(parsed);
        setVisible(Object.fromEntries(parsed.layers.map((l) => [l.key, layerInfo(job.pdk, l.key).visible])));
      } catch (e) {
        if (!cancelled) setError((e as Error).message);
      }
    })();
    api.drc(job.id).then((r) => !cancelled && setDrc(r), () => !cancelled && setDrc(null));
    return () => {
      cancelled = true;
    };
  }, [job.id, job.pdk, gdsPath]);

  const draw = useCallback(() => {
    cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      rendererRef.current?.render(viewRef.current);
      drawLabels();
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout, visible]);

  const fit = useCallback(
    (box?: [number, number, number, number], minSpan = 0) => {
      const el = canvasRef.current;
      const b = box ?? layout?.bbox;
      if (!el || !b) return;
      const w = Math.max(b[2] - b[0], minSpan, 1e-3);
      const h = Math.max(b[3] - b[1], minSpan, 1e-3);
      const scale = 0.9 * Math.min(el.clientWidth / w, el.clientHeight / h);
      viewRef.current = { cx: (b[0] + b[2]) / 2, cy: (b[1] + b[3]) / 2, scale: Number.isFinite(scale) ? scale : 1 };
      draw();
    },
    [layout, draw],
  );

  // Renderer lifetime follows the loaded layout.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!layout || !canvas) return;
    let disposed = false;
    createRenderer(canvas).then((r) => {
      if (disposed) return r.destroy();
      rendererRef.current = r;
      setRendererKind(r.kind);
      fit();
    });
    return () => {
      disposed = true;
      rendererRef.current?.destroy();
      rendererRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout]);

  // Push layer geometry and visibility.
  useEffect(() => {
    const r = rendererRef.current;
    if (!r || !layout) return;
    r.setLayers(
      layout.layers.map((l) => {
        const info = layerInfo(job.pdk, l.key);
        return {
          key: l.key,
          color: info.color,
          order: info.order,
          visible: !!visible[l.key],
          fill: info.fill,
          triangles: l.triangles,
          edges: l.edges,
        };
      }),
    );
    draw();
  }, [layout, visible, rendererKind, job.pdk, draw]);

  // DRC markers: each violation's shapes plus a frame big enough to spot at full view.
  const markerRanges = useMemo(() => {
    if (!drc || !layout) return null;
    const span = Math.max(layout.bbox[2] - layout.bbox[0], layout.bbox[3] - layout.bbox[1]);
    const min = span * 0.03;
    const tris: number[] = [];
    const edges: number[] = [];
    const ranges: { tri: [number, number]; edge: [number, number] }[] = [];
    for (const v of drc.violations) {
      const t0 = tris.length / 2;
      const e0 = edges.length / 2;
      for (const shape of v.shapes) {
        for (let i = 1; i + 1 < shape.length; i++) tris.push(...shape[0], ...shape[i], ...shape[i + 1]);
        for (let i = 0; i < shape.length; i++) edges.push(...shape[i], ...shape[(i + 1) % shape.length]);
      }
      if (v.bbox) {
        const [x0, y0, x1, y1] = v.bbox;
        const cx = (x0 + x1) / 2;
        const cy = (y0 + y1) / 2;
        const hw = Math.max(x1 - x0, min) / 2;
        const hh = Math.max(y1 - y0, min) / 2;
        const c = [cx - hw, cy - hh, cx + hw, cy - hh, cx + hw, cy + hh, cx - hw, cy + hh];
        for (let i = 0; i < 4; i++) edges.push(c[2 * i], c[2 * i + 1], c[(2 * i + 2) % 8], c[(2 * i + 3) % 8]);
      }
      ranges.push({ tri: [t0, tris.length / 2], edge: [e0, edges.length / 2] });
    }
    return { tris: new Float32Array(tris), edges: new Float32Array(edges), ranges };
  }, [drc, layout]);

  useEffect(() => {
    const r = rendererRef.current;
    if (!r) return;
    const m: Markers | null =
      markerRanges && markerRanges.ranges.length
        ? {
            triangles: markerRanges.tris,
            edges: markerRanges.edges,
            selected: selected === null ? null : (markerRanges.ranges[selected] ?? null),
          }
        : null;
    r.setMarkers(m);
    draw();
  }, [markerRanges, selected, rendererKind, draw]);

  function drawLabels() {
    const c = labelsRef.current;
    if (!c || !layout) return;
    const dpr = window.devicePixelRatio || 1;
    const w = c.clientWidth;
    const h = c.clientHeight;
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const v = viewRef.current;
    if (v.scale < LABEL_MIN_SCALE) return;
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillStyle = "rgba(255,255,255,0.9)";
    ctx.textBaseline = "middle";
    let n = 0;
    for (const t of layout.texts) {
      if (!visible[t.key] && !visible[t.key - (t.key % 65536)]) continue;
      const x = (t.x - v.cx) * v.scale + w / 2;
      const y = h / 2 - (t.y - v.cy) * v.scale;
      if (x < -50 || y < -20 || x > w + 50 || y > h + 20) continue;
      ctx.fillText(t.text, x + 3, y);
      if (++n > MAX_LABELS) break;
    }
  }

  // Redraw on resize.
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => draw());
    ro.observe(el);
    return () => ro.disconnect();
  }, [draw]);

  // Pan and zoom.
  const drag = useRef<{ x: number; y: number; cx: number; cy: number } | null>(null);
  const toWorld = (e: { clientX: number; clientY: number }) => {
    const el = canvasRef.current!;
    const rect = el.getBoundingClientRect();
    const v = viewRef.current;
    return [
      v.cx + (e.clientX - rect.left - rect.width / 2) / v.scale,
      v.cy - (e.clientY - rect.top - rect.height / 2) / v.scale,
    ] as [number, number];
  };
  const onWheel = (e: React.WheelEvent) => {
    const [wx, wy] = toWorld(e);
    const v = viewRef.current;
    const k = Math.exp(-e.deltaY * 0.0015);
    const scale = Math.min(1e6, Math.max(1e-3, v.scale * k));
    // Keep the point under the cursor fixed.
    viewRef.current = { scale, cx: wx - (wx - v.cx) * (v.scale / scale), cy: wy - (wy - v.cy) * (v.scale / scale) };
    draw();
  };

  // React's wheel listener is passive; stop the page from scrolling with a native one.
  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const stop = (e: WheelEvent) => e.preventDefault();
    el.addEventListener("wheel", stop, { passive: false });
    return () => el.removeEventListener("wheel", stop);
  }, [layout]);

  function selectViolation(i: number, v: DrcViolation) {
    setSelected(i);
    if (v.bbox) fit(v.bbox, 2);
  }

  const groups = useMemo(() => {
    const g = new Map<string, { label: string; description: string; items: { i: number; v: DrcViolation }[] }>();
    drc?.violations.forEach((v, i) => {
      const id = `${v.source}:${v.rule}`;
      if (!g.has(id)) g.set(id, { label: `${v.rule}`, description: v.description, items: [] });
      g.get(id)!.items.push({ i, v });
    });
    return [...g.entries()];
  }, [drc]);

  if (error) return <p className="error pad">Could not open the layout: {error}</p>;

  return (
    <div className="viewer">
      <div className="viewer-stage" ref={wrapRef}>
        {!layout && <p className="muted pad viewer-loading">Loading layout…</p>}
        <canvas
          ref={canvasRef}
          className="viewer-canvas"
          tabIndex={0}
          aria-label={`Layout of ${layout?.top ?? "design"}`}
          onWheel={onWheel}
          onPointerDown={(e) => {
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            drag.current = { x: e.clientX, y: e.clientY, cx: viewRef.current.cx, cy: viewRef.current.cy };
          }}
          onPointerMove={(e) => {
            setCursor(toWorld(e));
            const d = drag.current;
            if (!d) return;
            const v = viewRef.current;
            viewRef.current = { ...v, cx: d.cx - (e.clientX - d.x) / v.scale, cy: d.cy + (e.clientY - d.y) / v.scale };
            draw();
          }}
          onPointerUp={() => (drag.current = null)}
          onPointerLeave={() => setCursor(null)}
          onDoubleClick={() => fit()}
          onKeyDown={(e) => {
            if (e.key === "f" || e.key === "F") fit();
          }}
        />
        <canvas ref={labelsRef} className="viewer-labels" aria-hidden="true" />
        <div className="viewer-hud">
          <button className="small" onClick={() => fit()} title="Fit (F or double-click)">
            Fit
          </button>
          {layout && (
            <span className="muted small">
              {layout.top} · {(layout.bbox[2] - layout.bbox[0]).toFixed(2)} × {(layout.bbox[3] - layout.bbox[1]).toFixed(2)} µm ·{" "}
              {rendererKind} · parsed in {layout.parseMs.toFixed(0)} ms
            </span>
          )}
          <span className="spacer" />
          {cursor && (
            <span className="small mono">
              x {cursor[0].toFixed(3)} y {cursor[1].toFixed(3)} µm
            </span>
          )}
        </div>
      </div>

      <aside className="viewer-side">
        <div className="panel-tabs compact" role="tablist">
          <button role="tab" aria-selected={side === "layers"} className={side === "layers" ? "active" : ""} onClick={() => setSide("layers")}>
            Layers
          </button>
          <button role="tab" aria-selected={side === "drc"} className={side === "drc" ? "active" : ""} onClick={() => setSide("drc")}>
            DRC{drc ? ` (${drc.violations.length}${drc.truncated ? "+" : ""})` : ""}
          </button>
        </div>
        {side === "layers" ? (
          <>
            <div className="row small-gap">
              <button className="small" onClick={() => setVisible(Object.fromEntries(infos.map((i) => [i.key, i.visible])))}>
                Process stack
              </button>
              <button className="small" onClick={() => setVisible(Object.fromEntries(infos.map((i) => [i.key, true])))}>
                All
              </button>
            </div>
            <ul className="layer-list">
              {infos.map((info) => {
                const shapes = layout?.layers.find((l) => l.key === info.key)?.shapes ?? 0;
                return (
                  <li key={info.key}>
                    <label>
                      <input
                        type="checkbox"
                        checked={!!visible[info.key]}
                        onChange={(e) => setVisible((v) => ({ ...v, [info.key]: e.target.checked }))}
                      />
                      <span className="swatch" style={{ background: cssColor(info.color) }} />
                      <span className="layer-name">{info.name}</span>
                      <span className="muted small">
                        {info.layer}/{info.datatype} · {shapes}
                      </span>
                    </label>
                  </li>
                );
              })}
            </ul>
          </>
        ) : (
          <div className="drc-list">
            {!drc && <p className="muted small">Loading DRC reports…</p>}
            {drc && !drc.violations.length && (
              <p className="small">
                <span className="ok">No violations</span>
                <span className="muted"> in {drc.sources.length ? drc.sources.join(", ") : "this run (no DRC reports found)"}</span>
              </p>
            )}
            {groups.map(([id, g]) => (
              <details key={id} open>
                <summary>
                  <span className="status status-failed">{g.items.length}</span> <strong>{g.label}</strong>{" "}
                  <span className="muted small">{id.split(":")[0]}</span>
                </summary>
                {g.description && <p className="muted small">{g.description}</p>}
                <ul>
                  {g.items.map(({ i, v }) => (
                    <li key={i}>
                      <button className={`link ${selected === i ? "selected" : ""}`} onClick={() => selectViolation(i, v)} disabled={!v.bbox}>
                        {v.bbox ? `@ ${v.bbox[0].toFixed(2)}, ${v.bbox[1].toFixed(2)}` : "location unknown"}
                      </button>
                    </li>
                  ))}
                </ul>
              </details>
            ))}
            {drc?.truncated && <p className="muted small">Showing the first {drc.violations.length} violations.</p>}
          </div>
        )}
      </aside>
    </div>
  );
}
