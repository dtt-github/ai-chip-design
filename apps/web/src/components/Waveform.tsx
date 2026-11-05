import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SimSignal } from "../sim/protocol";

const ROW = 26;
const AXIS = 24;
const NAME_W = 230;

type Radix = "hex" | "dec" | "bin";

interface View {
  t0: number;
  t1: number;
}

/** Index of the last change at or before `t` (the value in effect at `t`). */
function at(times: Float64Array, t: number): number {
  let lo = 0;
  let hi = times.length - 1;
  if (hi < 0 || t < times[0]) return -1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (times[mid] <= t) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

function fmt(v: bigint, width: number, radix: Radix): string {
  if (width === 1) return v.toString();
  switch (radix) {
    case "dec":
      return v.toString(10);
    case "bin":
      return v.toString(2).padStart(width, "0");
    default:
      return v.toString(16).padStart(Math.ceil(width / 4), "0");
  }
}

function unitName(fs: number): string {
  const units: [number, string][] = [
    [1e15, "s"],
    [1e12, "ms"],
    [1e9, "µs"],
    [1e6, "ns"],
    [1e3, "ps"],
    [1, "fs"],
  ];
  for (const [size, name] of units) if (fs >= size && fs % size === 0) return fs / size === 1 ? name : `×${fs / size}${name}`;
  return "units";
}

/** Signals in the top scope (and the DUT's ports one level down) are shown first. */
function defaultSelection(signals: SimSignal[]): string[] {
  const depth = (n: string) => n.split(".").length;
  const top = signals.filter((s) => depth(s.name) === 2).map((s) => s.name);
  return (top.length ? top : signals.map((s) => s.name)).slice(0, 24);
}

export function Waveform({ signals, endTime, timescaleFs }: { signals: SimSignal[]; endTime: number; timescaleFs: number }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [shown, setShown] = useState<string[]>(() => defaultSelection(signals));
  const [view, setView] = useState<View>({ t0: 0, t1: Math.max(endTime, 1) });
  const [cursor, setCursor] = useState<number | null>(null);
  const [radix, setRadix] = useState<Radix>("hex");
  const [filter, setFilter] = useState("");
  const [picker, setPicker] = useState(false);
  const drag = useRef<{ x: number; view: View; moved: boolean } | null>(null);

  const byName = useMemo(() => new Map(signals.map((s) => [s.name, s])), [signals]);
  const rows = shown.map((n) => byName.get(n)).filter((s): s is SimSignal => !!s);

  useEffect(() => {
    setShown(defaultSelection(signals));
    setView({ t0: 0, t1: Math.max(endTime, 1) });
    setCursor(null);
  }, [signals, endTime]);

  const draw = useCallback(() => {
    const c = canvasRef.current;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const w = c.clientWidth;
    const h = c.clientHeight;
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(c);
    const fg = css.getPropertyValue("--text").trim() || "#ddd";
    const muted = css.getPropertyValue("--muted").trim() || "#888";
    const border = css.getPropertyValue("--border").trim() || "#444";
    const wave = "#4cc38a";
    const bus = "#58a6ff";
    ctx.clearRect(0, 0, w, h);
    const span = view.t1 - view.t0 || 1;
    const x = (t: number) => ((t - view.t0) / span) * w;

    // Time axis with "nice" tick spacing.
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillStyle = muted;
    ctx.strokeStyle = border;
    const raw = span / Math.max(1, w / 90);
    const pow = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 5, 10].map((m) => m * pow).find((s) => s >= raw) ?? raw;
    for (let t = Math.ceil(view.t0 / step) * step; t <= view.t1; t += step) {
      const px = Math.round(x(t)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(px, AXIS - 6);
      ctx.lineTo(px, h);
      ctx.globalAlpha = 0.35;
      ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.fillText(String(Math.round(t * 1000) / 1000), px + 3, 12);
    }

    rows.forEach((s, r) => {
      const top = AXIS + r * ROW + 4;
      const bot = top + ROW - 8;
      const mid = (top + bot) / 2;
      let i = Math.max(0, at(s.times, view.t0));
      ctx.lineWidth = 1.5;
      for (; i < s.times.length && s.times[i] <= view.t1; i++) {
        const xa = Math.max(0, x(s.times[i]));
        const xb = Math.min(w, i + 1 < s.times.length ? x(s.times[i + 1]) : x(endTime));
        if (xb < 0 || xa > w) continue;
        const v = s.values[i];
        if (s.width === 1) {
          const y = v ? top : bot;
          ctx.strokeStyle = wave;
          ctx.beginPath();
          if (i > 0 && xa > 0) {
            ctx.moveTo(xa, s.values[i - 1] ? top : bot);
            ctx.lineTo(xa, y);
          } else ctx.moveTo(xa, y);
          ctx.lineTo(xb, y);
          ctx.stroke();
        } else {
          const e = Math.min(3, (xb - xa) / 2);
          ctx.strokeStyle = bus;
          ctx.beginPath();
          ctx.moveTo(xa, mid);
          ctx.lineTo(xa + e, top);
          ctx.lineTo(xb - e, top);
          ctx.lineTo(xb, mid);
          ctx.lineTo(xb - e, bot);
          ctx.lineTo(xa + e, bot);
          ctx.closePath();
          ctx.stroke();
          const label = fmt(v, s.width, radix);
          const tw = ctx.measureText(label).width;
          if (xb - xa > tw + 10) {
            ctx.fillStyle = fg;
            ctx.fillText(label, Math.max(xa, 0) + 6, mid + 4);
          }
        }
      }
      ctx.strokeStyle = border;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, AXIS + (r + 1) * ROW + 0.5);
      ctx.lineTo(w, AXIS + (r + 1) * ROW + 0.5);
      ctx.globalAlpha = 0.4;
      ctx.stroke();
      ctx.globalAlpha = 1;
    });

    if (cursor !== null) {
      const px = Math.round(x(cursor)) + 0.5;
      ctx.strokeStyle = "#f0b429";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(px, 0);
      ctx.lineTo(px, h);
      ctx.stroke();
      ctx.fillStyle = "#f0b429";
      ctx.fillText(String(Math.round(cursor * 1000) / 1000), px + 3, AXIS - 2);
    }
  }, [rows, view, cursor, radix, endTime]);

  useEffect(() => {
    draw();
  }, [draw]);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => draw());
    ro.observe(el);
    return () => ro.disconnect();
  }, [draw]);

  // React's wheel listener is passive; keep the page from scrolling while zooming.
  useEffect(() => {
    const c = canvasRef.current;
    const stop = (e: WheelEvent) => e.preventDefault();
    c?.addEventListener("wheel", stop, { passive: false });
    return () => c?.removeEventListener("wheel", stop);
  }, []);

  const timeAt = (clientX: number) => {
    const c = canvasRef.current!;
    const r = c.getBoundingClientRect();
    return view.t0 + ((clientX - r.left) / r.width) * (view.t1 - view.t0);
  };

  const zoom = (factor: number, center?: number) => {
    const span = view.t1 - view.t0;
    const c = center ?? (view.t0 + view.t1) / 2;
    const nspan = Math.min(Math.max(span * factor, 1e-3), Math.max(endTime, 1) * 4);
    const k = (c - view.t0) / span;
    setView({ t0: c - k * nspan, t1: c - k * nspan + nspan });
  };

  const valueAt = (s: SimSignal, t: number) => {
    const i = at(s.times, t);
    return i < 0 ? "" : fmt(s.values[i], s.width, radix);
  };

  const filtered = signals.filter((s) => s.name.toLowerCase().includes(filter.toLowerCase()));
  const height = AXIS + Math.max(rows.length, 1) * ROW + 4;

  return (
    <div className="waves">
      <div className="waves-toolbar">
        <button className="small" onClick={() => setView({ t0: 0, t1: Math.max(endTime, 1) })}>
          Fit
        </button>
        <button className="small" onClick={() => zoom(0.5)} aria-label="Zoom in">
          +
        </button>
        <button className="small" onClick={() => zoom(2)} aria-label="Zoom out">
          −
        </button>
        <select className="small" value={radix} onChange={(e) => setRadix(e.target.value as Radix)} aria-label="Radix">
          <option value="hex">hex</option>
          <option value="dec">dec</option>
          <option value="bin">bin</option>
        </select>
        <button className="small" onClick={() => setPicker((p) => !p)} aria-expanded={picker}>
          Signals ({rows.length}/{signals.length})
        </button>
        <span className="spacer" />
        <span className="muted small">
          {Math.round(view.t0)}–{Math.round(view.t1)} {unitName(timescaleFs)}
          {cursor !== null && <> · cursor {Math.round(cursor * 1000) / 1000}</>}
        </span>
      </div>
      {picker && (
        <div className="wave-picker">
          <input placeholder="Filter signals" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <div className="row small-gap">
            <button className="small" onClick={() => setShown(filtered.map((s) => s.name))}>
              Show matching
            </button>
            <button className="small" onClick={() => setShown([])}>
              Clear
            </button>
          </div>
          <ul>
            {filtered.slice(0, 500).map((s) => (
              <li key={s.name}>
                <label>
                  <input
                    type="checkbox"
                    checked={shown.includes(s.name)}
                    onChange={(e) =>
                      setShown((cur) => (e.target.checked ? [...cur, s.name] : cur.filter((n) => n !== s.name)))
                    }
                  />
                  <span className="mono">{s.name}</span> <span className="muted small">[{s.width}]</span>
                </label>
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="waves-body" ref={wrapRef}>
        <div className="wave-names" style={{ width: NAME_W, height }}>
          <div className="wave-name header" style={{ height: AXIS }} />
          {rows.map((s) => (
            <div key={s.name} className="wave-name" style={{ height: ROW }} title={s.name}>
              <span className="mono">{s.name.split(".").slice(1).join(".")}</span>
              {s.width > 1 && <span className="muted small">[{s.width - 1}:0]</span>}
              <span className="spacer" />
              {cursor !== null && <span className="mono wave-val">{valueAt(s, cursor)}</span>}
            </div>
          ))}
        </div>
        <canvas
          ref={canvasRef}
          className="wave-canvas"
          style={{ height }}
          onWheel={(e) => zoom(Math.exp(e.deltaY * 0.002), timeAt(e.clientX))}
          onPointerDown={(e) => {
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            drag.current = { x: e.clientX, view, moved: false };
          }}
          onPointerMove={(e) => {
            const d = drag.current;
            if (!d) return;
            const dx = e.clientX - d.x;
            if (Math.abs(dx) > 3) d.moved = true;
            const c = canvasRef.current!;
            const dt = (dx / c.clientWidth) * (d.view.t1 - d.view.t0);
            setView({ t0: d.view.t0 - dt, t1: d.view.t1 - dt });
          }}
          onPointerUp={(e) => {
            if (drag.current && !drag.current.moved) setCursor(timeAt(e.clientX));
            drag.current = null;
          }}
        />
      </div>
    </div>
  );
}
