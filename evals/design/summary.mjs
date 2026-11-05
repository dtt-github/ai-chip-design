#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Pass rate and cost per task for one or more eval result folders, as Markdown:
//   node evals/design/summary.mjs evals/design/results/assistant evals/design/results/agent
import { existsSync, readFileSync } from 'node:fs';
import { basename, join } from 'node:path';

const dirs = process.argv.slice(2);
if (!dirs.length) { console.error('usage: node evals/design/summary.mjs <flow-dir>...'); process.exit(2); }
const pct = x => `${Math.round(x * 100)}%`;
const usd = x => `$${x.toFixed(3)}`;
const lines = [];
for (const dir of dirs) {
  const file = join(dir, 'baseline', 'results.jsonl');
  if (!existsSync(file)) { console.error(`no results in ${dir}`); process.exit(2); }
  const rows = readFileSync(file, 'utf8').split('\n').filter(Boolean).map(l => JSON.parse(l));
  const errors = join(dir, 'baseline', 'errors.jsonl');
  const failed = existsSync(errors) ? readFileSync(errors, 'utf8').split('\n').filter(Boolean).length : 0;
  const byTask = new Map();
  for (const r of rows) byTask.set(r.prompt_id, [...(byTask.get(r.prompt_id) ?? []), r]);
  const mean = (rs, f) => rs.reduce((s, r) => s + f(r), 0) / rs.length;
  lines.push(`## ${basename(dir)}`, '',
    '| Task | Kind | Reps | Pass | Cost per run | Time per run |', '|---|---|---|---|---|---|');
  for (const [id, rs] of [...byTask].sort()) {
    lines.push(`| ${id} | ${rs[0].meta?.kind ?? ''} | ${rs.length} | ${pct(mean(rs, r => r.grade.pass))} | ` +
      `${rs.every(r => typeof r.cost_usd === 'number') ? usd(mean(rs, r => r.cost_usd)) : 'n/a'} | ${Math.round(mean(rs, r => r.latency_s))} s |`);
  }
  const priced = rows.filter(r => typeof r.cost_usd === 'number');
  const total = priced.reduce((s, r) => s + r.cost_usd, 0);
  lines.push('', `**Pass rate ${pct(mean(rows, r => r.grade.pass))}** over ${rows.length} runs of ${byTask.size} tasks; ` +
    `cost ${usd(total)} in total, ${usd(total / Math.max(1, priced.length))} per run` +
    (priced.length < rows.length ? ` (the server reported no cost for ${rows.length - priced.length} run(s))` : '') +
    (failed ? `; ${failed} attempt(s) failed to run (see errors.jsonl)` : '') + '.', '');
}
process.stdout.write(lines.join('\n'));
