// Line diff (longest common subsequence) for reviewing small files.
export type DiffLine = { kind: "same" | "add" | "del"; text: string; a?: number; b?: number };

const MAX_CELLS = 4_000_000;

export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before === "" ? [] : before.replace(/\n$/, "").split("\n");
  const b = after === "" ? [] : after.replace(/\n$/, "").split("\n");
  if (a.length * b.length > MAX_CELLS) {
    // Too big for a table: show as replaced.
    return [...a.map((t, i) => ({ kind: "del" as const, text: t, a: i + 1 })), ...b.map((t, i) => ({ kind: "add" as const, text: t, b: i + 1 }))];
  }
  const n = a.length;
  const m = b.length;
  const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      out.push({ kind: "same", text: a[i], a: i + 1, b: j + 1 });
      i++;
      j++;
    } else if (j < m && (i === n || lcs[i][j + 1] >= lcs[i + 1][j])) {
      out.push({ kind: "add", text: b[j], b: j + 1 });
      j++;
    } else {
      out.push({ kind: "del", text: a[i], a: i + 1 });
      i++;
    }
  }
  return out;
}

/** Keep changed lines plus `context` lines around them; gaps become null. */
export function hunks(lines: DiffLine[], context = 3): (DiffLine | null)[] {
  const keep = new Array(lines.length).fill(false);
  lines.forEach((l, i) => {
    if (l.kind !== "same") for (let k = Math.max(0, i - context); k <= Math.min(lines.length - 1, i + context); k++) keep[k] = true;
  });
  const out: (DiffLine | null)[] = [];
  lines.forEach((l, i) => {
    if (keep[i]) out.push(l);
    else if (out.length && out[out.length - 1] !== null) out.push(null);
  });
  return out;
}
