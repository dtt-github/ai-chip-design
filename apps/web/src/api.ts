import { currentUser, login } from "./auth";

export type Role = "owner" | "editor" | "viewer";

export interface Project {
  id: string;
  owner_id: string;
  slug: string;
  name: string;
  description: string;
  pdk: string;
  visibility: "private" | "public";
  /** SPDX identifier of the project's own licence. */
  licence: string;
  created_at: string;
  updated_at: string;
  role: Role | null;
  head?: string | null;
}

export interface FileEntry {
  path: string;
  size: number;
}

export interface Tree {
  commit: string | null;
  files: FileEntry[];
}

export interface FileView {
  path: string;
  commit: string;
  text: boolean;
  content: string;
}

export interface CommitInfo {
  id: string;
  message: string;
  author_name: string;
  author_email: string;
  time: number;
}

export interface Member {
  user_id: string;
  email: string;
  display_name: string;
  role: Role;
}

export type ProposalStatus = "running" | "ready" | "gate_failed" | "failed" | "merged" | "discarded";

export interface GateRun {
  top: string;
  verdict: "pass" | "fail" | "no_verdict" | "error";
  finished: boolean;
  time: number;
  steps: number;
  log: string[];
  reason: string | null;
}

export interface GateReport {
  passed: boolean;
  summary: string;
  engine: string;
  runs: GateRun[];
  inputs: Record<string, string>;
}

export interface EvidenceStep {
  turn: number;
  kind: "reasoning" | "message" | "tool" | "fallback";
  text?: string;
  name?: string;
  input?: Record<string, unknown>;
  output?: string;
  is_error?: boolean;
  ms?: number;
  from?: { model: string };
  to?: { model: string };
}

export interface Proposal {
  id: string;
  project_id: string;
  request: string;
  base_commit: string;
  status: ProposalStatus;
  model: string;
  /** The agent's commit title, when it gave one. */
  title: string | null;
  summary: string | null;
  error: string | null;
  files: Record<string, string>;
  base_files: Record<string, string | null>;
  gate: GateReport | null;
  evidence: {
    steps?: EvidenceStep[];
    models_served?: string[];
    usage?: Record<string, number>;
    system_prompt_sha256?: string;
    engine?: string;
    effort?: string;
    elapsed_ms?: number;
  };
  evidence_hash: string | null;
  merged_commit: string | null;
  created_at: string;
  finished_at: string | null;
  merged_at: string | null;
}

export interface ProposalListItem {
  id: string;
  request: string;
  status: ProposalStatus;
  title: string | null;
  summary: string | null;
  base_commit: string;
  created_at: string;
  merged_commit: string | null;
}

export type JobStatus = "queued" | "running" | "succeeded" | "failed" | "canceled";

export interface GlsRun {
  top: string;
  verdict: "pass" | "fail" | "no_verdict" | "error" | "skipped";
  finished: boolean;
  reason: string | null;
  log: string[];
}

export interface GlsReport {
  status: "passed" | "failed" | "skipped" | "unavailable";
  summary: string;
  design: string;
  params: Record<string, number>;
  simulator: string;
  runs: GlsRun[];
}

/** Flow configs offered in the Runs panel: `config*.json` / `.yaml` / `.tcl` anywhere in the project. */
export function flowConfigs(paths: string[]): string[] {
  const configs = paths.filter((p) => /^config[^/]*\.(json|ya?ml|tcl)$/i.test(p.split("/").pop() ?? ""));
  // The default first, then by path.
  return configs.sort((a, b) => Number(b === "config.json") - Number(a === "config.json") || a.localeCompare(b));
}

export type StepOp = "set" | "wait" | "wait_until" | "expect" | "expect_for";

export interface SpecStep {
  op: StepOp;
  signal: string;
  value: string;
  cycles: number;
  note: string;
}

export interface SpecExample {
  /** The chip's name when the example tests the whole chip; absent for the design. */
  target?: string;
  id: string;
  title: string;
  given: string;
  expect: string;
  params: { name: string; value: string }[];
  steps: SpecStep[];
}

export interface SpecPort {
  name: string;
  dir: "in" | "out";
  width: number;
  meaning: string;
}

/** The project's spec.json: what the design should do, and the examples that test it. */
export interface Spec {
  version: number;
  intent: string;
  top: string;
  clock: { port: string; mhz: number } | null;
  reset: { port: string; active_low: boolean } | null;
  ports: SpecPort[];
  params: { name: string; value: string; meaning: string }[];
  decisions: { question: string; answer: string }[];
  questions: { question: string; options: string[] }[];
  examples: SpecExample[];
}

export interface SpecView {
  head: string;
  spec: Spec | null;
  problems: string[];
  in_sync: boolean;
  design_name: string | null;
  clock_mhz: number | null;
  /** The chip from the Blocks view; examples can target it. */
  chip: Blocks | null;
}

export interface ExampleResult {
  id: string;
  title: string;
  verdict: "pass" | "fail" | "no_verdict" | "error" | "missing";
  reason: string | null;
  log: string[];
}

/** A branch of the design no example takes. */
export interface CoverageGap {
  file: string;
  line: number;
  kind: "then" | "else" | "arm" | "default";
  what: string;
  labels: string | null;
  /** In words, with the condition in backticks: "`full` is never true in the examples". */
  text: string;
  /** A starting point for an example that would take this branch. */
  suggestion: string;
}

/** What the examples exercise: every if/else branch and case arm of the design. */
export interface Coverage {
  covered: number;
  total: number;
  gaps: CoverageGap[];
}

export interface SpecRun {
  commit: string;
  examples: ExampleResult[];
  others: GateReport["runs"];
  /** Null without examples, or for a design without any if or case. */
  coverage: Coverage | null;
  engine: string;
}

export interface SpecDraft {
  spec: Spec;
  note: string;
  model: string;
  usage: Record<string, number>;
  elapsed_ms: number;
}

export interface ChipPin {
  name: string;
  dir: "in" | "out";
  width: number;
  meaning: string;
}

export interface ChipBlock {
  name: string;
  module: string;
  params: Record<string, string>;
  x: number;
  y: number;
}

export interface ChipWire {
  from: string;
  to: string;
}

/** blocks.json: the chip built from blocks. */
export interface Blocks {
  top: string;
  clock: string | null;
  reset: string | null;
  pins: ChipPin[];
  blocks: ChipBlock[];
  wires: ChipWire[];
}

export interface BlockPort {
  name: string;
  dir: "in" | "out" | "inout";
  width: number;
}

export interface BlockModule {
  name: string;
  file: string;
  source: string;
  params: Record<string, number>;
  ports: BlockPort[];
  description: string;
}

export interface BlockIssue {
  level: "error" | "warning";
  wire: number | null;
  block: string | null;
  message: string;
}

export interface BlocksCheck {
  issues: BlockIssue[];
  ports: Record<string, BlockPort[]>;
  verilog: string | null;
}

export interface BlocksView {
  head: string;
  blocks: Blocks | null;
  modules: BlockModule[];
  problems: string[];
  check: BlocksCheck | null;
  design_name: string | null;
  meanings: Record<string, Record<string, string>>;
}

export interface ReviewNote {
  wire: number;
  level: "ok" | "warning";
  note: string;
  adapter: "" | "sc_rise_pulse" | "sc_sync2" | "sc_invert" | "sc_resize";
}

/** Licences offered when creating a project (the server accepts any it knows). */
export const PROJECT_LICENCES = [
  "Apache-2.0",
  "MIT",
  "BSD-3-Clause",
  "SHL-2.1",
  "CERN-OHL-P-2.0",
  "CERN-OHL-W-2.0",
  "CERN-OHL-S-2.0",
  "LGPL-3.0-or-later",
  "GPL-3.0-or-later",
  "GPL-2.0-only",
];

export interface IpPort {
  name: string;
  dir: "input" | "output" | "inout" | "unknown";
  width: number | null;
}

export interface IpVersion {
  block: string;
  version: string;
  commit_id: string;
  top: string;
  licence: string;
  files: Record<string, string>;
  ports: IpPort[];
  params: Record<string, number>;
  gate: GateReport;
  published_by: string;
  published_at: string;
}

export interface IpBlock {
  name: string;
  description: string;
  project_id: string;
  project_name: string;
  versions: IpVersion[];
}

export type LicenceStatus = "compatible" | "conditions" | "unknown" | "incompatible";

export interface LicenceVerdict {
  status: LicenceStatus;
  block: string;
  project: string;
  reasons: string[];
  obligations: string[];
  note: string;
}

export interface ImportCheck {
  block: string;
  version: string;
  source_project: string;
  commit: string;
  licence: LicenceVerdict;
  allowed: boolean;
  can_accept: boolean;
  collisions: string[];
  replaces: string | null;
  files: string[];
  configs: string[];
  problems: string[];
}

export interface IpLockEntry {
  version: string;
  commit: string;
  source_project: string;
  source_name: string;
  top: string;
  licence: string;
  licence_check: LicenceStatus;
  licence_accepted_by?: string;
  files: Record<string, string>;
  licence_verdict: LicenceVerdict;
}

export interface JobSummary {
  signoff_clean: boolean;
  checks: Record<"drc" | "lvs" | "antenna" | "setup" | "hold", boolean | null>;
  [metric: string]: unknown;
}

export interface Job {
  id: string;
  project_id: string;
  created_by: string;
  kind: string;
  commit_id: string;
  config_path: string;
  pdk: string;
  toolchain: string;
  status: JobStatus;
  exit_code: number | null;
  error: string | null;
  summary: JobSummary | null;
  /** Gate-level simulation of the netlist, after a successful flow. */
  gls: GlsReport | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface LogChunk {
  content: string;
  offset: number;
  done: boolean;
}

export interface Artifact {
  path: string;
  size: number;
}

export interface DrcViolation {
  source: string;
  rule: string;
  description: string;
  shapes: [number, number][][];
  bbox: [number, number, number, number] | null;
}

export interface DrcReport {
  violations: DrcViolation[];
  counts: { source: string; rule: string; description: string; count: number }[];
  truncated: boolean;
  sources: string[];
}

export interface PathStage {
  pin: string;
  cell: string | null;
  edge: string;
  delay_ns: number;
  time_ns: number;
  fanout: number | null;
}

export interface TimingPath {
  check: string;
  corner: string;
  startpoint: string;
  endpoint: string;
  group: string;
  arrival_ns: number | null;
  required_ns: number | null;
  slack_ns: number | null;
  stages: PathStage[];
}

export interface RunReport {
  corners: {
    name: string;
    setup_ws: number | null;
    setup_tns: number | null;
    setup_violations: number | null;
    hold_ws: number | null;
    hold_tns: number | null;
    hold_violations: number | null;
  }[];
  die_area_um2: number | null;
  core_area_um2: number | null;
  stdcell_area_um2: number | null;
  utilization: number | null;
  classes: { class: string; count: number | null; area_um2: number | null }[];
  paths: TimingPath[];
}

export const isFinished = (s: JobStatus) => s === "succeeded" || s === "failed" || s === "canceled";

export type Change = { op: "write"; path: string; content: string } | { op: "delete"; path: string };

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await send(method, path, body);
  return (res.headers.get("content-type")?.includes("json") ? await res.json() : null) as T;
}

/** Sends an authenticated request; throws `ApiError` on a non-2xx answer. */
async function send(method: string, path: string, body?: unknown): Promise<Response> {
  const user = await currentUser();
  if (!user) {
    await login();
    throw new ApiError(401, "unauthorized", "signing in");
  }
  const res = await fetch(`/api${path}`, {
    method,
    headers: {
      authorization: `Bearer ${user.access_token}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    const data = res.headers.get("content-type")?.includes("json") ? await res.json() : null;
    if (res.status === 401) await login();
    throw new ApiError(res.status, data?.error ?? "error", data?.message ?? res.statusText);
  }
  return res;
}

const enc = encodeURIComponent;
const filePath = (p: string) => p.split("/").map(enc).join("/");

/** A verified starter design (`GET /starters`): start a project from it, or add it as a block. */
export interface Starter {
  id: string;
  title: string;
  summary: string;
  category: string;
  /** The design's module name. */
  top: string;
  ports: SpecPort[];
  params: { name: string; value: string; meaning: string }[];
  clock_mhz: number | null;
  clock: string | null;
  reset: string | null;
  examples: number;
  /** Branches of the design its examples take: [covered, total]; null if it has none. */
  covered: [number, number] | null;
}

export const api = {
  listProjects: () => request<Project[]>("GET", "/projects"),
  createProject: (body: { name: string; description?: string; visibility?: string; licence?: string; starter?: string }) =>
    request<Project>("POST", "/projects", body),
  starters: () => request<Starter[]>("GET", "/starters"),
  addStarter: (id: string, starter: string) =>
    request<{ commit: string; path: string; module: string }>("POST", `/projects/${enc(id)}/starters/${enc(starter)}`),
  aiUsage: () => request<{ today_usd: number; limit_usd: number | null }>("GET", "/me/ai-usage"),
  getBlocks: (id: string) => request<BlocksView>("GET", `/projects/${enc(id)}/blocks`),
  checkBlocks: (id: string, blocks: Blocks) => request<BlocksCheck>("POST", `/projects/${enc(id)}/blocks/check`, { blocks }),
  saveBlocks: (id: string, body: { blocks: Blocks; parent: string }) =>
    request<{ commit: string; changed: boolean }>("PUT", `/projects/${enc(id)}/blocks`, body),
  reviewBlocks: (id: string, blocks: Blocks) =>
    request<{ notes: ReviewNote[]; summary: string }>("POST", `/projects/${enc(id)}/blocks/review`, { blocks }),
  getSpec: (id: string) => request<SpecView>("GET", `/projects/${enc(id)}/spec`),
  saveSpec: (id: string, body: { spec: Spec; parent: string; message?: string }) =>
    request<{ commit: string; changed: boolean }>("PUT", `/projects/${enc(id)}/spec`, body),
  runSpec: (id: string) => request<SpecRun>("POST", `/projects/${enc(id)}/spec/run`),
  draftSpec: (id: string, body: { instruction: string; spec?: Spec | null }) =>
    request<SpecDraft>("POST", `/projects/${enc(id)}/spec/draft`, body),
  ipBlocks: () => request<IpBlock[]>("GET", "/ip"),
  ipImports: (id: string) => request<{ blocks: Record<string, IpLockEntry> }>("GET", `/projects/${enc(id)}/ip/imports`),
  ipCheck: (id: string, block: string, version: string) =>
    request<ImportCheck>("GET", `/projects/${enc(id)}/ip/check?block=${enc(block)}&version=${enc(version)}`),
  ipImport: (id: string, body: { block: string; version: string; accept_licence?: boolean }) =>
    request<ImportCheck & { imported_commit: string }>("POST", `/projects/${enc(id)}/ip/import`, body),
  ipPublish: (id: string, body: { version: string; name?: string; description?: string; commit?: string }) =>
    request<IpVersion>("POST", `/projects/${enc(id)}/ip`, body),
  getProject: (id: string) => request<Project>("GET", `/projects/${enc(id)}`),
  tree: (id: string) => request<Tree>("GET", `/projects/${enc(id)}/tree`),
  snapshot: (id: string, rev?: string) =>
    request<{ commit: string | null; files: Record<string, string> }>(
      "GET",
      `/projects/${enc(id)}/snapshot${rev ? `?rev=${enc(rev)}` : ""}`,
    ),
  readFile: (id: string, path: string, rev?: string) =>
    request<FileView>("GET", `/projects/${enc(id)}/files/${filePath(path)}${rev ? `?rev=${enc(rev)}` : ""}`),
  log: (id: string) => request<CommitInfo[]>("GET", `/projects/${enc(id)}/commits`),
  commit: (id: string, body: { message: string; parent: string | null; changes: Change[] }) =>
    request<CommitInfo>("POST", `/projects/${enc(id)}/commits`, body),
  members: (id: string) => request<Member[]>("GET", `/projects/${enc(id)}/members`),
  addMember: (id: string, body: { email: string; role: "editor" | "viewer" }) =>
    request<Member>("POST", `/projects/${enc(id)}/members`, body),
  proposals: (id: string) => request<ProposalListItem[]>("GET", `/projects/${enc(id)}/agent`),
  propose: (id: string, text: string) => request<Proposal>("POST", `/projects/${enc(id)}/agent`, { request: text }),
  proposal: (pid: string) => request<Proposal>("GET", `/agent/${enc(pid)}`),
  mergeProposal: (pid: string) => request<Proposal>("POST", `/agent/${enc(pid)}/merge`),
  discardProposal: (pid: string) => request<Proposal>("POST", `/agent/${enc(pid)}/discard`),
  regate: (pid: string) => request<{ report: GateReport; reproduced: boolean }>("POST", `/agent/${enc(pid)}/regate`),
  createJob: (id: string, body: { commit?: string; config?: string } = {}) =>
    request<Job>("POST", `/projects/${enc(id)}/jobs`, body),
  listJobs: (id: string) => request<Job[]>("GET", `/projects/${enc(id)}/jobs`),
  getJob: (job: string) => request<Job>("GET", `/jobs/${enc(job)}`),
  jobLog: (job: string, offset: number) => request<LogChunk>("GET", `/jobs/${enc(job)}/log?offset=${offset}`),
  cancelJob: (job: string) => request<Job>("POST", `/jobs/${enc(job)}/cancel`),
  artifacts: (job: string) => request<Artifact[]>("GET", `/jobs/${enc(job)}/artifacts`),
  drc: (job: string) => request<DrcReport>("GET", `/jobs/${enc(job)}/drc`),
  report: (job: string) => request<RunReport>("GET", `/jobs/${enc(job)}/report`),
  artifactBlob: async (job: string, path: string) =>
    (await send("GET", `/jobs/${enc(job)}/artifacts/${filePath(path)}`)).blob(),
};
