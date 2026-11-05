//! Flow runs: a Postgres-backed queue, in-process workers that hand jobs to
//! a [`sc_jobs::Backend`], and the HTTP endpoints around them.
//!
//! Each job lives in `<jobs root>/<job id>/`: `work/` holds the project files
//! at the job's commit (LibreLane writes `work/runs/sc/` into it) and
//! `flow.log` the flow's output.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use sc_jobs::{Backend, CancellationToken, JobSpec, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::projects::{Role, authorize};

const LOG_CHUNK_BYTES: u64 = 256 * 1024;
const IDLE_POLL: Duration = Duration::from_secs(5);

pub struct JobService {
    /// `None` when no backend could be set up: job creation answers 503.
    pub backend: Option<Arc<dyn Backend>>,
    pub root: PathBuf,
    wake: Notify,
    running: Mutex<HashMap<Uuid, CancellationToken>>,
}

impl JobService {
    pub fn new(backend: Option<Arc<dyn Backend>>, root: PathBuf) -> Self {
        Self { backend, root, wake: Notify::new(), running: Mutex::new(HashMap::new()) }
    }

    fn dir(&self, id: Uuid) -> PathBuf {
        self.root.join(id.to_string())
    }

    fn workdir(&self, id: Uuid) -> PathBuf {
        self.dir(id).join("work")
    }

    fn log_path(&self, id: Uuid) -> PathBuf {
        self.dir(id).join("flow.log")
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Job {
    pub id: Uuid,
    pub project_id: Uuid,
    pub created_by: Uuid,
    pub kind: String,
    pub commit_id: String,
    pub config_path: String,
    pub pdk: String,
    pub toolchain: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub summary: Option<Value>,
    /// Gate-level simulation of the netlist ([`crate::gls::GlsReport`]); set after a successful flow.
    pub gls: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Job {
    fn is_finished(&self) -> bool {
        matches!(self.status.as_str(), "succeeded" | "failed" | "canceled")
    }
}

async fn load(state: &AppState, user: &AuthUser, id: Uuid, needed: Role) -> ApiResult<Job> {
    let job: Option<Job> =
        sqlx::query_as("SELECT * FROM jobs WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let job = job.ok_or_else(|| ApiError::NotFound("job not found".into()))?;
    authorize(state, user, job.project_id, needed).await.map_err(|e| match e {
        ApiError::NotFound(_) => ApiError::NotFound("job not found".into()),
        other => other,
    })?;
    Ok(job)
}

// ---- endpoints ---------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct CreateJob {
    /// Commit to build; defaults to the branch tip.
    pub commit: Option<String>,
    /// Flow config path in the project; defaults to `config.json`.
    pub config: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(project_id): UrlPath<Uuid>,
    body: Option<Json<CreateJob>>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let (project, _) = authorize(&state, &user, project_id, Role::Editor).await?;
    let Some(backend) = state.jobs.backend.clone() else {
        return Err(ApiError::Unavailable("flow runs are not available on this server".into()));
    };
    let config = sc_git::normalize_path(req.config.as_deref().unwrap_or("config.json"))?;

    // Pin the commit and check the config before queuing, so mistakes fail fast.
    let repos = state.repos.clone();
    let (commit, cfg_check) = {
        let config = config.clone();
        tokio::task::spawn_blocking(move || -> sc_git::Result<(String, Result<(), String>)> {
            let repo = repos.open(&project_id.to_string())?;
            let commit = match req.commit {
                Some(c) => repo.resolve(&c)?,
                None => repo.head()?.ok_or_else(|| sc_git::GitError::NotFound("empty repository".into()))?,
            };
            let check = match repo.read_file(Some(&commit), &config) {
                Err(sc_git::GitError::NotFound(_)) => Err(format!("{config} not found at commit {commit}")),
                Err(e) => return Err(e),
                Ok(bytes) => check_config(&config, &bytes),
            };
            Ok((commit, check))
        })
        .await??
    };
    cfg_check.map_err(ApiError::BadRequest)?;

    let job: Job = sqlx::query_as(
        "INSERT INTO jobs (id, project_id, created_by, commit_id, config_path, pdk, toolchain)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(user.id)
    .bind(&commit)
    .bind(&config)
    .bind(&project.pdk)
    .bind(backend.toolchain())
    .fetch_one(&state.db)
    .await?;
    tracing::info!(job = %job.id, project = %project_id, %commit, "job queued");
    state.jobs.wake.notify_one();
    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// Light validation of a LibreLane config: JSON configs must be an object naming the design.
fn check_config(path: &str, bytes: &[u8]) -> Result<(), String> {
    if !path.ends_with(".json") {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("{path} is not valid JSON: {e}"))?;
    match v.get("DESIGN_NAME") {
        Some(Value::String(s)) if !s.is_empty() => Ok(()),
        _ => Err(format!("{path} must set DESIGN_NAME")),
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(project_id): UrlPath<Uuid>,
) -> ApiResult<Json<Vec<Job>>> {
    authorize(&state, &user, project_id, Role::Viewer).await?;
    let jobs = sqlx::query_as("SELECT * FROM jobs WHERE project_id = $1 ORDER BY created_at DESC LIMIT 50")
        .bind(project_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(jobs))
}

pub async fn get(State(state): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<Uuid>) -> ApiResult<Json<Job>> {
    Ok(Json(load(&state, &user, id, Role::Viewer).await?))
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    #[serde(default)]
    pub offset: u64,
}

#[derive(Debug, Serialize)]
pub struct LogChunk {
    pub content: String,
    /// Pass this as `offset` to continue.
    pub offset: u64,
    /// No more output will come (the job finished and everything was read).
    pub done: bool,
}

/// Log output from `offset`; poll until `done`.
pub async fn log(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(id): UrlPath<Uuid>,
    Query(q): Query<LogQuery>,
) -> ApiResult<Json<LogChunk>> {
    let job = load(&state, &user, id, Role::Viewer).await?;
    let mut buf = Vec::new();
    let mut len = 0;
    if let Ok(mut file) = tokio::fs::File::open(state.jobs.log_path(id)).await {
        len = file.metadata().await.map_err(anyhow::Error::from)?.len();
        if q.offset < len {
            file.seek(std::io::SeekFrom::Start(q.offset)).await.map_err(anyhow::Error::from)?;
            file.take(LOG_CHUNK_BYTES).read_to_end(&mut buf).await.map_err(anyhow::Error::from)?;
        }
    }
    // Don't split a UTF-8 sequence across chunks.
    let valid = match std::str::from_utf8(&buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    };
    let offset = q.offset.min(len) + valid as u64;
    let content = String::from_utf8_lossy(&buf[..valid]).into_owned();
    Ok(Json(LogChunk { content, offset, done: job.is_finished() && offset >= len }))
}

pub async fn cancel(State(state): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<Uuid>) -> ApiResult<Json<Job>> {
    load(&state, &user, id, Role::Editor).await?;
    // A queued job is canceled in place; a running one is signalled and its worker records the result.
    let updated: Option<Job> = sqlx::query_as(
        "UPDATE jobs SET status = 'canceled', finished_at = now() WHERE id = $1 AND status = 'queued' RETURNING *",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    if let Some(job) = updated {
        return Ok(Json(job));
    }
    if let Some(token) = state.jobs.running.lock().expect("lock").get(&id) {
        token.cancel();
    }
    Ok(Json(load(&state, &user, id, Role::Viewer).await?))
}

#[derive(Debug, Serialize)]
pub struct Artifact {
    pub path: String,
    pub size: u64,
}

/// Files in the run's `final/` directory (GDS, DEF, netlists, reports, render).
pub async fn artifacts(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(id): UrlPath<Uuid>,
) -> ApiResult<Json<Vec<Artifact>>> {
    load(&state, &user, id, Role::Viewer).await?;
    let root = sc_jobs::final_dir(&state.jobs.workdir(id));
    let list = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        collect(&root, &root, &mut out);
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    })
    .await?;
    Ok(Json(list))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Artifact>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        if meta.is_dir() {
            collect(root, &path, out);
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(Artifact { path: rel.to_string_lossy().into_owned(), size: meta.len() });
        }
    }
}

pub async fn artifact(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath((id, path)): UrlPath<(Uuid, String)>,
) -> ApiResult<Response> {
    load(&state, &user, id, Role::Viewer).await?;
    let rel = sc_git::normalize_path(&path)?;
    let root = sc_jobs::final_dir(&state.jobs.workdir(id));
    let full = root.join(&rel);
    // `normalize_path` rejects `..`; also refuse symlinks pointing out of the run.
    let canonical =
        tokio::fs::canonicalize(&full).await.map_err(|_| ApiError::NotFound("artifact not found".into()))?;
    let root = tokio::fs::canonicalize(&root).await.map_err(|_| ApiError::NotFound("artifact not found".into()))?;
    if !canonical.starts_with(&root) || !canonical.is_file() {
        return Err(ApiError::NotFound("artifact not found".into()));
    }
    let file = tokio::fs::File::open(&canonical).await.map_err(anyhow::Error::from)?;
    let size = file.metadata().await.map_err(anyhow::Error::from)?.len();
    let name = canonical.file_name().unwrap_or_default().to_string_lossy().replace('"', "");
    Ok((
        [
            (header::CONTENT_TYPE, content_type(&rel).to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::CONTENT_DISPOSITION, format!("inline; filename=\"{name}\"")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        Body::from_stream(tokio_util::io::ReaderStream::new(file)),
    )
        .into_response())
}

/// DRC violations with geometry from the run's KLayout, Magic and router DRC reports.
pub async fn drc(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(id): UrlPath<Uuid>,
) -> ApiResult<Json<sc_jobs::reports::DrcReport>> {
    load(&state, &user, id, Role::Viewer).await?;
    let run = sc_jobs::run_dir(&state.jobs.workdir(id));
    Ok(Json(tokio::task::spawn_blocking(move || sc_jobs::reports::drc_report(&run)).await?))
}

/// Timing by corner, worst setup/hold paths, and area by cell class.
pub async fn report(
    State(state): State<AppState>,
    user: AuthUser,
    UrlPath(id): UrlPath<Uuid>,
) -> ApiResult<Json<sc_jobs::reports::RunReport>> {
    load(&state, &user, id, Role::Viewer).await?;
    let workdir = state.jobs.workdir(id);
    let report = tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(sc_jobs::final_dir(&workdir).join("metrics.json")).ok()?;
        let metrics = sc_jobs::parse_metrics(&bytes).ok()?;
        Some(sc_jobs::reports::run_report(&sc_jobs::run_dir(&workdir), &metrics))
    })
    .await?;
    report.map(Json).ok_or_else(|| ApiError::NotFound("this run has no metrics".into()))
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("json") => "application/json",
        Some("csv") => "text/csv; charset=utf-8",
        Some("v" | "vh" | "sdc" | "def" | "lef" | "lib" | "spice" | "spef" | "sdf" | "txt" | "rpt" | "log") => {
            "text/plain; charset=utf-8"
        }
        _ => "application/octet-stream",
    }
}

// ---- workers -----------------------------------------------------------------

/// Marks jobs left `running` by a previous process as failed, then starts `count` workers.
pub async fn start_workers(state: AppState, count: usize) -> anyhow::Result<()> {
    let n = sqlx::query(
        "UPDATE jobs SET status = 'failed', error = 'interrupted: the server restarted', finished_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if n > 0 {
        tracing::warn!(count = n, "marked interrupted jobs as failed");
    }
    let Some(backend) = state.jobs.backend.clone() else {
        return Ok(());
    };
    backend.cleanup_orphans().await;
    for i in 0..count {
        let state = state.clone();
        tokio::spawn(async move {
            tracing::info!(worker = i, "job worker started");
            loop {
                match claim(&state).await {
                    Ok(Some(job)) => execute(&state, job).await,
                    Ok(None) => {
                        let _ = tokio::time::timeout(IDLE_POLL, state.jobs.wake.notified()).await;
                    }
                    Err(e) => {
                        tracing::error!(error = ?e, "claiming a job failed");
                        tokio::time::sleep(IDLE_POLL).await;
                    }
                }
            }
        });
    }
    Ok(())
}

async fn claim(state: &AppState) -> sqlx::Result<Option<Job>> {
    sqlx::query_as(
        "UPDATE jobs SET status = 'running', started_at = now()
         WHERE id = (SELECT id FROM jobs WHERE status = 'queued' ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1)
         RETURNING *",
    )
    .fetch_optional(&state.db)
    .await
}

struct Finish {
    status: &'static str,
    exit_code: Option<i32>,
    error: Option<String>,
    summary: Option<Value>,
    gls: Option<Value>,
}

async fn execute(state: &AppState, job: Job) {
    let id = job.id;
    tracing::info!(job = %id, project = %job.project_id, "job started");
    let token = CancellationToken::new();
    state.jobs.running.lock().expect("lock").insert(id, token.clone());
    let finish = match run(state, &job, token).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(job = %id, error = ?e, "job failed to run");
            Finish { status: "failed", exit_code: None, error: Some(format!("{e:#}")), summary: None, gls: None }
        }
    };
    state.jobs.running.lock().expect("lock").remove(&id);
    let res = sqlx::query(
        "UPDATE jobs SET status = $2, exit_code = $3, error = $4, summary = $5, gls = $6, finished_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(finish.status)
    .bind(finish.exit_code)
    .bind(&finish.error)
    .bind(&finish.summary)
    .bind(&finish.gls)
    .execute(&state.db)
    .await;
    if let Err(e) = res {
        tracing::error!(job = %id, error = ?e, "recording job result failed");
    }
    tracing::info!(job = %id, status = finish.status, "job finished");
}

async fn run(state: &AppState, job: &Job, cancel: CancellationToken) -> anyhow::Result<Finish> {
    let backend = state.jobs.backend.clone().expect("workers only run with a backend");
    let dir = state.jobs.dir(job.id);
    let workdir = state.jobs.workdir(job.id);
    tokio::fs::create_dir_all(&workdir).await?;
    crate::workspace::export(state.repos.clone(), job.project_id, Some(job.commit_id.clone()), workdir.clone()).await?;

    let spec = JobSpec {
        id: job.id.to_string(),
        workdir: workdir.clone(),
        config: job.config_path.clone(),
        pdk: job.pdk.clone(),
        log_path: dir.join("flow.log"),
    };
    let outcome = backend.run(&spec, cancel.clone()).await?;

    let summary = match tokio::fs::read(sc_jobs::final_dir(&workdir).join("metrics.json")).await {
        Ok(bytes) => match sc_jobs::parse_metrics(&bytes) {
            Ok(m) => Some(sc_jobs::summarize(&m)),
            Err(e) => {
                tracing::warn!(job = %job.id, error = %e, "unreadable metrics.json");
                None
            }
        },
        Err(_) => None,
    };
    Ok(match outcome {
        Outcome::Exited(0) => {
            let gls = gate_level(state, job, backend.as_ref(), &spec, cancel.clone()).await;
            if cancel.is_cancelled() {
                Finish { status: "canceled", exit_code: None, error: None, summary, gls: None }
            } else {
                Finish { status: "succeeded", exit_code: Some(0), error: None, summary, gls }
            }
        }
        Outcome::Exited(code) => Finish {
            status: "failed",
            exit_code: Some(code),
            error: Some(error_excerpt(&spec.log_path, code).await),
            summary,
            gls: None,
        },
        Outcome::Canceled => Finish { status: "canceled", exit_code: None, error: None, summary, gls: None },
    })
}

/// Reruns the matching testbenches on the netlist and notes the result in the flow log.
async fn gate_level(
    state: &AppState,
    job: &Job,
    backend: &dyn Backend,
    spec: &JobSpec,
    cancel: CancellationToken,
) -> Option<Value> {
    let files: std::collections::BTreeMap<String, String> =
        match crate::workspace::text_files(state.repos.clone(), job.project_id, job.commit_id.clone(), 4 << 20).await {
            Ok(f) => f.into_iter().collect(),
            Err(e) => {
                tracing::warn!(job = %job.id, error = ?e, "gate-level simulation: reading sources failed");
                return None;
            }
        };
    let config = files.get(&job.config_path).and_then(|c| serde_json::from_str::<Value>(c).ok());
    let report = match config {
        Some(config) => crate::gls::run(backend, spec, &files, &config, cancel).await,
        None => return None,
    };
    let mut note = format!("\n[SiliconCommons] Gate-level simulation: {}\n", report.summary);
    for r in &report.runs {
        note.push_str(&format!("[SiliconCommons]   {}: {}", r.top, r.verdict));
        if let Some(why) = &r.reason {
            note.push_str(&format!(" ({why})"));
        }
        note.push('\n');
    }
    if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&spec.log_path).await {
        use tokio::io::AsyncWriteExt;
        let _ = f.write_all(note.as_bytes()).await;
    }
    tracing::info!(job = %job.id, status = %report.status, "gate-level simulation finished");
    serde_json::to_value(report).ok()
}

/// The flow's own error lines, for the job record.
async fn error_excerpt(log: &Path, code: i32) -> String {
    let text = tokio::fs::read_to_string(log).await.unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("ERROR") || l.contains("Error:")).collect();
    let mut excerpt = lines[lines.len().saturating_sub(8)..].join("\n");
    if excerpt.len() > 4000 {
        excerpt = excerpt[excerpt.len() - 4000..].to_string();
    }
    if excerpt.is_empty() { format!("flow exited with code {code}") } else { excerpt }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_checks() {
        assert!(check_config("config.json", br#"{"DESIGN_NAME": "top"}"#).is_ok());
        assert!(check_config("config.json", br#"{"DESIGN_NAME": ""}"#).is_err());
        assert!(check_config("config.json", b"{nope").is_err());
        assert!(check_config("config.yaml", b"anything").is_ok());
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("render/top.png"), "image/png");
        assert_eq!(content_type("gds/top.gds"), "application/octet-stream");
        assert_eq!(content_type("nl/top.nl.v"), "text/plain; charset=utf-8");
    }
}
