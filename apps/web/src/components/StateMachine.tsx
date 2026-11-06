// A state machine found in the design, drawn: states in a ring, numbered arrows for the
// transitions (listed underneath with when they happen), styled by whether the examples take
// them, and the current state lit when one is given (Try it).

import type { ReactNode } from "react";
import type { Machine, MachineTransition } from "../api";

const W = 104;
const H = 34;

export function StateMachine({ machine, current }: { machine: Machine; current?: string | null }) {
  const n = machine.states.length;
  const r = Math.max(80, (n * (W + 24)) / (2 * Math.PI));
  const cx = r + W / 2 + 40;
  const cy = r + H / 2 + 40;
  const pos = new Map(
    machine.states.map((s, i) => {
      const a = -Math.PI / 2 + (2 * Math.PI * i) / n;
      return [s, { x: cx + r * Math.cos(a), y: cy + r * Math.sin(a) }];
    }),
  );
  // One arrow per pair of states, labelled with its transitions' numbers.
  const groups = new Map<string, { from: string; to: string; nums: number[]; ts: MachineTransition[] }>();
  machine.transitions.forEach((t, i) => {
    const k = `${t.from}→${t.to}`;
    const g = groups.get(k) ?? { from: t.from, to: t.to, nums: [], ts: [] };
    g.nums.push(i + 1);
    g.ts.push(t);
    groups.set(k, g);
  });
  const style = (ts: MachineTransition[]) =>
    ts.some((t) => t.covered === false) ? "untested" : ts.every((t) => t.covered === true) ? "tested" : "unknown";
  const untested = machine.transitions.filter((t) => t.covered === false).length;
  const unknown = machine.transitions.filter((t) => t.covered === null).length;

  const edges: ReactNode[] = [];
  for (const [k, g] of groups) {
    const a = pos.get(g.from)!;
    const b = pos.get(g.to)!;
    const label = g.nums.join(", ");
    const title = g.ts.map((t) => `${t.from} → ${t.to}: ${plain(t.when)}`).join("\n");
    if (g.from === g.to) {
      // A loop on the outside of the ring.
      const dx = a.x - cx;
      const dy = a.y - cy;
      const d = Math.hypot(dx, dy) || 1;
      const ux = dx / d;
      const uy = dy / d;
      const sx = a.x + ux * (H / 2);
      const sy = a.y + uy * (H / 2);
      const px = -uy * 14;
      const py = ux * 14;
      const tip = { x: sx + ux * 42, y: sy + uy * 42 };
      edges.push(
        <g key={k} className={`sm-edge ${style(g.ts)}`}>
          <title>{title}</title>
          <path d={`M ${sx + px} ${sy + py} C ${tip.x + px * 2} ${tip.y + py * 2}, ${tip.x - px * 2} ${tip.y - py * 2}, ${sx - px} ${sy - py}`} markerEnd="url(#sm-arrow)" />
          <text x={tip.x + ux * 8} y={tip.y + uy * 8} className="sm-num">
            {label}
          </text>
        </g>,
      );
      continue;
    }
    // Curve a little to the side, so arrows both ways between two states don't overlap.
    const mx = (a.x + b.x) / 2;
    const my = (a.y + b.y) / 2;
    const dx = b.x - a.x;
    const dy = b.y - a.y;
    const d = Math.hypot(dx, dy) || 1;
    const nx = -dy / d;
    const ny = dx / d;
    const bend = groups.has(`${g.to}→${g.from}`) ? 26 : 12;
    const qx = mx + nx * bend;
    const qy = my + ny * bend;
    const start = edgePoint(a, qx, qy);
    const end = edgePoint(b, qx, qy);
    edges.push(
      <g key={k} className={`sm-edge ${style(g.ts)}`}>
        <title>{title}</title>
        <path d={`M ${start.x} ${start.y} Q ${qx} ${qy} ${end.x} ${end.y}`} markerEnd="url(#sm-arrow)" />
        <text x={mx + nx * (bend + 10)} y={my + ny * (bend + 10)} className="sm-num">
          {label}
        </text>
      </g>,
    );
  }

  const width = 2 * cx;
  const height = 2 * cy;
  return (
    <div className="sm">
      <svg viewBox={`0 0 ${width} ${height}`} width="100%" style={{ maxWidth: width }} role="img" aria-label={`State machine of ${machine.module}`}>
        <defs>
          <marker id="sm-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" className="sm-arrowhead" />
          </marker>
        </defs>
        {machine.reset &&
          pos.get(machine.reset) &&
          (() => {
            const p = pos.get(machine.reset)!;
            return (
              <g className="sm-reset">
                <path d={`M ${p.x - W / 2 - 34} ${p.y} L ${p.x - W / 2 - 2} ${p.y}`} markerEnd="url(#sm-arrow)" />
                <text x={p.x - W / 2 - 36} y={p.y - 6} textAnchor="end">
                  reset
                </text>
              </g>
            );
          })()}
        {edges}
        {machine.states.map((s) => {
          const p = pos.get(s)!;
          return (
            <g key={s} className={`sm-state ${current === s ? "now" : ""}`}>
              <rect x={p.x - W / 2} y={p.y - H / 2} width={W} height={H} rx={H / 2} />
              <text x={p.x} y={p.y + 4} textAnchor="middle">
                {/^\d/.test(s) ? `state ${s}` : s}
              </text>
            </g>
          );
        })}
      </svg>
      <ol className="sm-list small">
        {machine.transitions.map((t, i) => (
          <li key={i} className={t.covered === false ? "untested" : undefined}>
            <span className="mono">
              {stateName(t.from)} → {stateName(t.to)}
            </span>{" "}
            when {withCode(t.when)}
            {t.covered === false && <strong> · no example takes this</strong>}
            {t.covered === null && <span className="muted"> · not run yet</span>}
          </li>
        ))}
      </ol>
      <p className="muted small" style={{ margin: 0 }}>
        Solid arrows: an example takes them. {untested > 0 && "Dashed red: no example does yet. "}
        {unknown > 0 && "Dotted: the examples haven't run yet. "}
        Found in <code>{machine.file}</code>, register <code>{machine.state}</code>.
      </p>
    </div>
  );
}

/** Where the line from a state's centre towards (tx, ty) leaves its pill. */
function edgePoint(p: { x: number; y: number }, tx: number, ty: number) {
  const dx = tx - p.x;
  const dy = ty - p.y;
  const s = Math.min(Math.abs(W / 2 / (dx || 1e-9)), Math.abs(H / 2 / (dy || 1e-9)));
  return { x: p.x + dx * s, y: p.y + dy * s };
}

function stateName(s: string) {
  return /^\d/.test(s) ? `state ${s}` : s;
}

function plain(t: string) {
  return t.replace(/`/g, "");
}

function withCode(text: string) {
  return text.split("`").map((part, i) => (i % 2 ? <code key={i}>{part}</code> : <span key={i}>{part}</span>));
}
