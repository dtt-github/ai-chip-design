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
  readFile: (id: string, path: string, rev?: string) =>
    request<FileView>("GET", `/projects/${enc(id)}/files/${filePath(path)}${rev ? `?rev=${enc(rev)}` : ""}`),
  log: (id: string) => request<CommitInfo[]>("GET", `/projects/${enc(id)}/commits`),
  commit: (id: string, body: { message: string; parent: string | null; changes: Change[] }) =>
    request<CommitInfo>("POST", `/projects/${enc(id)}/commits`, body),
  createJob: (id: string, body: { commit?: string; config?: string } = {}) =>
    request<Job>("POST", `/projects/${enc(id)}/jobs`, body),
  listJobs: (id: string) => request<Job[]>("GET", `/projects/${enc(id)}/jobs`),
  getJob: (job: string) => request<Job>("GET", `/jobs/${enc(job)}`),
  jobLog: (job: string, offset: number) => request<LogChunk>("GET", `/jobs/${enc(job)}/log?offset=${offset}`),
  cancelJob: (job: string) => request<Job>("POST", `/jobs/${enc(job)}/cancel`),
  artifacts: (job: string) => request<Artifact[]>("GET", `/jobs/${enc(job)}/artifacts`),
  artifactBlob: async (job: string, path: string) =>
    (await send("GET", `/jobs/${enc(job)}/artifacts/${filePath(path)}`)).blob(),
};
