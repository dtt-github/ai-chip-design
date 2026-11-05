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
  summary: string | null;
  base_commit: string;
  created_at: string;
  merged_commit: string | null;
}

export type JobStatus = "queued" | "running" | "succeeded" | "failed" | "canceled";

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

export const api = {
  listProjects: () => request<Project[]>("GET", "/projects"),
  createProject: (body: { name: string; description?: string; visibility?: string }) =>
    request<Project>("POST", "/projects", body),
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
