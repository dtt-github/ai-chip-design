//! An AI agent that proposes RTL changes. It works on a private copy of the
//! project at the current commit, with four tools (list, read, write, simulate),
//! and can never commit. When it finishes, the server runs the simulation gate
//! ([`crate::gate`]) on the tree it produced; a person may merge the proposal
//! only if that gate passes, and the gate runs again at merge time.
//!
//! Everything the agent did is kept as an evidence log: the request, the model
//! that served each turn, a hash of the instructions it was given, its reasoning
//! summaries and messages, every tool call with its result, token usage, the
//! gate report with input file hashes, and a hash over all of it that goes into
//! the merge commit.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use sc_git::{Author, Change};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::claude::Claude;
use crate::error::{ApiError, ApiResult};
use crate::gate::{self, GateReport};
use crate::projects::{Role, authorize};

const MAX_TURNS: usize = 30;
const MAX_FILE_BYTES: usize = 256 * 1024;
const TOOL_OUTPUT_LINES: usize = 200;
const MAX_FILES_TEXT: usize = 1024 * 1024;

pub struct AgentService {
    /// `None` when no API key is configured: proposing answers 503.
    pub claude: Option<Arc<Claude>>,
    runs: Arc<Semaphore>,
}

impl AgentService {
    pub fn new(claude: Option<Claude>, max_concurrent: usize) -> Self {
        AgentService { claude: claude.map(Arc::new), runs: Arc::new(Semaphore::new(max_concurrent.max(1))) }
    }
}

const SYSTEM: &str = "You are the RTL design agent in SiliconCommons, a collaborative chip design platform. \
Projects are Verilog designs taken to silicon with LibreLane on the IHP SG13G2 130 nm open PDK; \
`config.json` holds the LibreLane flow configuration (DESIGN_NAME is the top module, VERILOG_FILES the RTL).

You work on a private copy of the project at its latest commit. Use the tools to look at the files, \
make the change the user asks for, and check it in simulation. Your changes become a proposal that a \
person reviews; it can only be merged if every testbench in the project passes the server's simulation \
gate. A testbench is a top-level module named tb*, *_tb or *test*; it passes when it reaches $finish, \
prints a line containing PASS, and prints no line containing FAIL, ERROR or MISMATCH.

Guidelines:
- Keep the RTL synthesizable and the change focused on the request; preserve existing behaviour that \
the request doesn't ask you to change, and keep SPDX headers.
- When you change behaviour, add or extend a self-checking testbench that exercises it. Never weaken, \
delete or bypass an existing check to make a test pass; if a test fails, fix the design or explain why \
the test is wrong.
- The simulator is 2-state (x and z read as 0), supports signals up to 64 bits, and does not support \
functions, tasks or generate blocks yet; write code within those limits.
- Run the simulation after your last change, then call submit_proposal once with a commit title and a \
short summary of what you changed and the simulation results, including anything you could not verify.";

fn tools() -> Value {
    json!([
        {
            "name": "list_files",
            "description": "List every file in your copy of the project, with sizes and whether you have modified it. Call this first to see the project's layout.",
            "strict": true,
            "input_schema": { "type": "object", "properties": {}, "required": [], "additionalProperties": false }
        },
        {
            "name": "read_file",
            "description": "Read the full text of one file from your copy of the project (including your own edits). Read a file before changing it.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Path relative to the project root, e.g. src/uart_tx.v" } },
                "required": ["path"],
                "additionalProperties": false
            }
        },
        {
            "name": "write_file",
            "description": "Create a file or replace a file's entire content in your copy of the project. Always pass the complete new file, not a fragment. Use for RTL, testbenches and config changes.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path relative to the project root" },
                    "content": { "type": "string", "description": "The complete new file content" }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }
        },
        {
            "name": "run_simulation",
            "description": "Simulate the Verilog in your copy of the project with the project's event-driven simulator and return the $display output. Call it after changing RTL or testbenches, and again after every fix, until the testbenches pass.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "top": { "type": "string", "description": "Top module to simulate (a testbench), or an empty string to pick the testbench automatically" }
                },
                "required": ["top"],
                "additionalProperties": false
            }
        },
        {
            "name": "submit_proposal",
            "description": "Finish: submit your changes as a proposal for review. Call it once, after your last simulation run; nothing you do afterwards is kept. The title becomes the commit's subject line if the proposal is merged.",
            "strict": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Commit title: what the change does, imperative mood, at most 72 characters, no trailing period. E.g. \"Add optional even-parity bit to uart_tx\"" },
                    "summary": { "type": "string", "description": "For the reviewer: what you changed and why, the simulation results, and anything you could not verify. Plain text or Markdown." }
                },
                "required": ["title", "summary"],
                "additionalProperties": false
            }
        }
    ])
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Proposal {
    pub id: Uuid,
    pub project_id: Uuid,
    pub created_by: Uuid,
    pub request: String,
    pub base_commit: String,
    pub status: String,
    pub model: String,
    /// Commit title the agent proposed (older proposals and plain-text finishes have none).
    pub title: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub files: Value,
    pub gate: Option<Value>,
    pub evidence: Value,
    pub evidence_hash: Option<String>,
    pub merged_commit: Option<String>,
    pub merged_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
}

async fn load(state: &AppState, user: &AuthUser, id: Uuid, needed: Role) -> ApiResult<Proposal> {
    let p: Option<Proposal> =
        sqlx::query_as("SELECT * FROM agent_proposals WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let p = p.ok_or_else(|| ApiError::NotFound("proposal not found".into()))?;
    authorize(state, user, p.project_id, needed).await.map_err(|e| match e {
        ApiError::NotFound(_) => ApiError::NotFound("proposal not found".into()),
        other => other,
    })?;
    Ok(p)
}

// ---- endpoints ---------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateProposal {
    pub request: String,
}

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateProposal>,
) -> ApiResult<(StatusCode, Json<Proposal>)> {
    authorize(&state, &user, project_id, Role::Editor).await?;
    let Some(claude) = state.agent.claude.clone() else {
        return Err(ApiError::Unavailable("the agent is not configured on this server (no ANTHROPIC_API_KEY)".into()));
    };
    let request = req.request.trim().to_string();
    if request.is_empty() || request.len() > 20_000 {
        return Err(ApiError::BadRequest("describe the change you want (up to 20 000 characters)".into()));
    }
    let base = crate::workspace::head(state.repos.clone(), project_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest("the project has no commits yet".into()))?;
    let p: Proposal = sqlx::query_as(
        "INSERT INTO agent_proposals (id, project_id, created_by, request, base_commit, model)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(project_id)
    .bind(user.id)
    .bind(&request)
    .bind(&base)
    .bind(&claude.cfg.model)
    .fetch_one(&state.db)
    .await?;
    let (st, id) = (state.clone(), p.id);
    tokio::spawn(async move {
        let _permit = st.agent.runs.clone().acquire_owned().await;
        if let Err(e) = run(&st, &claude, id, project_id, &base, &request, &user).await {
            tracing::error!(proposal = %id, error = ?e, "agent run failed");
            let _ = sqlx::query(
                "UPDATE agent_proposals SET status = 'failed', error = $2, finished_at = now() WHERE id = $1 AND status = 'running'",
            )
            .bind(id)
            .bind(format!("{e:#}"))
            .execute(&st.db)
            .await;
        }
    });
    tracing::info!(proposal = %p.id, project = %project_id, "agent proposal started");
    Ok((StatusCode::ACCEPTED, Json(p)))
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Value>>> {
    authorize(&state, &user, project_id, Role::Viewer).await?;
    #[derive(Serialize, sqlx::FromRow)]
    struct Item {
        id: Uuid,
        request: String,
        status: String,
        title: Option<String>,
        summary: Option<String>,
        base_commit: String,
        created_at: DateTime<Utc>,
        merged_commit: Option<String>,
    }
    let rows: Vec<Item> = sqlx::query_as(
        "SELECT id, request, status, title, summary, base_commit, created_at, merged_commit FROM agent_proposals
         WHERE project_id = $1 ORDER BY created_at DESC LIMIT 50",
    )
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(|r| serde_json::to_value(r).expect("serializable")).collect()))
}

/// The proposal plus the base content of every file it changes (for diffs).
pub async fn get(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = load(&state, &user, id, Role::Viewer).await?;
    let base =
        crate::workspace::text_files(state.repos.clone(), p.project_id, p.base_commit.clone(), MAX_FILES_TEXT).await?;
    let base_files: BTreeMap<&String, Option<&String>> =
        p.files.as_object().map(|o| o.keys().map(|k| (k, base.get(k))).collect()).unwrap_or_default();
    let mut v = serde_json::to_value(&p).map_err(anyhow::Error::from)?;
    v["base_files"] = serde_json::to_value(base_files).map_err(anyhow::Error::from)?;
    Ok(Json(v))
}

pub async fn discard(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Proposal>> {
    load(&state, &user, id, Role::Editor).await?;
    let p: Option<Proposal> = sqlx::query_as(
        "UPDATE agent_proposals SET status = 'discarded', finished_at = coalesce(finished_at, now())
         WHERE id = $1 AND status IN ('ready', 'gate_failed', 'failed') RETURNING *",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    p.map(Json).ok_or_else(|| ApiError::Conflict("only finished, unmerged proposals can be discarded".into()))
}

/// The tree the proposal would produce: its base commit with the agent's files on top.
async fn proposed_tree(state: &AppState, p: &Proposal) -> ApiResult<BTreeMap<String, String>> {
    let mut tree: BTreeMap<String, String> =
        crate::workspace::text_files(state.repos.clone(), p.project_id, p.base_commit.clone(), MAX_FILES_TEXT)
            .await?
            .into_iter()
            .collect();
    if let Some(files) = p.files.as_object() {
        for (path, content) in files {
            tree.insert(path.clone(), content.as_str().unwrap_or_default().to_string());
        }
    }
    Ok(tree)
}

async fn run_gate(tree: BTreeMap<String, String>) -> ApiResult<GateReport> {
    Ok(tokio::task::spawn_blocking(move || gate::run(&tree)).await?)
}

/// Re-runs the gate on the proposal's exact inputs and says whether it reproduces.
pub async fn regate(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Value>> {
    let p = load(&state, &user, id, Role::Viewer).await?;
    let report = run_gate(proposed_tree(&state, &p).await?).await?;
    let recorded: Option<GateReport> = p.gate.as_ref().and_then(|g| serde_json::from_value(g.clone()).ok());
    let reproduced = recorded.as_ref().is_some_and(|r| {
        r.passed == report.passed
            && r.inputs == report.inputs
            && r.runs
                .iter()
                .map(|x| (&x.top, &x.verdict, &x.log))
                .eq(report.runs.iter().map(|x| (&x.top, &x.verdict, &x.log)))
    });
    Ok(Json(json!({ "report": report, "reproduced": reproduced })))
}

pub async fn merge(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<Proposal>> {
    let p = load(&state, &user, id, Role::Editor).await?;
    if p.status != "ready" {
        return Err(ApiError::Conflict(match p.status.as_str() {
            "gate_failed" => "the simulation gate did not pass; this proposal cannot be merged".into(),
            "merged" => "already merged".into(),
            s => format!("a {s} proposal cannot be merged"),
        }));
    }
    let head = crate::workspace::head(state.repos.clone(), p.project_id).await?;
    if head.as_deref() != Some(p.base_commit.as_str()) {
        return Err(ApiError::Conflict(
            "the project has new commits since this proposal was made; ask the agent again on the current head".into(),
        ));
    }
    // Same door for everyone: the gate decides, again, on the exact tree being committed.
    let report = run_gate(proposed_tree(&state, &p).await?).await?;
    if !report.passed {
        sqlx::query("UPDATE agent_proposals SET status = 'gate_failed', gate = $2 WHERE id = $1")
            .bind(id)
            .bind(serde_json::to_value(&report).map_err(anyhow::Error::from)?)
            .execute(&state.db)
            .await?;
        return Err(ApiError::Conflict(format!("the simulation gate failed at merge time: {}", report.summary)));
    }
    let changes: Vec<Change> = p
        .files
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(path, c)| Change::Write {
                    path: path.clone(),
                    content: c.as_str().unwrap_or_default().as_bytes().to_vec(),
                })
                .collect()
        })
        .unwrap_or_default();
    // The agent's title is the subject (else the person's request); its summary goes in the body.
    let title = match p.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => commit_title(t),
        None => commit_title(&p.request),
    };
    let summary = p.summary.as_deref().unwrap_or_default().trim();
    let message = format!(
        "{title}\n\n{summary}{sep}Agent proposal {id} ({model}), merged by {who}.\nSimulation gate: passed ({gate}, {engine}).\nEvidence: sha256:{hash}\n\nCo-Authored-By: SiliconCommons agent <agent@siliconcommons.dev>\n",
        sep = if summary.is_empty() { "" } else { "\n\n" },
        model = p.model,
        who = user.display_name,
        gate = report.summary,
        engine = report.engine,
        hash = p.evidence_hash.as_deref().unwrap_or("-"),
    );
    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let (repos, base, pid) = (state.repos.clone(), p.base_commit.clone(), p.project_id);
    let info = tokio::task::spawn_blocking(move || {
        repos.open(&pid.to_string())?.commit(changes, &message, &author, Some(&base))
    })
    .await??;
    let merged: Proposal = sqlx::query_as(
        "UPDATE agent_proposals SET status = 'merged', merged_commit = $2, merged_by = $3, merged_at = now(), gate = $4
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(&info.id)
    .bind(user.id)
    .bind(serde_json::to_value(&report).map_err(anyhow::Error::from)?)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("UPDATE projects SET updated_at = now() WHERE id = $1").bind(pid).execute(&state.db).await?;
    state.collab.committed(&state, pid, &info.id).await;
    tracing::info!(proposal = %id, commit = %info.id, "agent proposal merged");
    Ok(Json(merged))
}

/// First line of `text`, at most 72 characters, cut at a word boundary, without a trailing period.
fn commit_title(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Agent change").trim();
    let line = line.strip_suffix('.').filter(|l| !l.ends_with('.')).unwrap_or(line);
    if line.chars().count() <= 72 {
        return line.to_string();
    }
    let cut: String = line.chars().take(71).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head).filter(|h| h.len() > 30).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':', ' ']))
}

/// Marks proposals left running by a previous process as failed.
pub async fn recover(state: &AppState) -> anyhow::Result<()> {
    let n = sqlx::query(
        "UPDATE agent_proposals SET status = 'failed', error = 'interrupted: the server restarted', finished_at = now()
         WHERE status = 'running'",
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if n > 0 {
        tracing::warn!(count = n, "marked interrupted agent proposals as failed");
    }
    Ok(())
}

// ---- the agent loop ----------------------------------------------------------

/// The agent's private copy of the project.
struct Workspace {
    base: BTreeMap<String, String>,
    changed: BTreeMap<String, String>,
}

impl Workspace {
    fn tree(&self) -> BTreeMap<String, String> {
        let mut t = self.base.clone();
        t.extend(self.changed.iter().map(|(k, v)| (k.clone(), v.clone())));
        t
    }

    fn get(&self, path: &str) -> Option<&String> {
        self.changed.get(path).or_else(|| self.base.get(path))
    }

    /// Runs one tool call; returns (result text, is_error, evidence record of the input).
    fn call(&mut self, name: &str, input: &Value) -> (String, bool, Value) {
        let path_arg = || -> Result<String, String> {
            let p = input["path"].as_str().ok_or("missing 'path'")?;
            sc_git::normalize_path(p).map_err(|e| e.to_string())
        };
        match name {
            "list_files" => {
                let lines: Vec<String> = self
                    .tree()
                    .iter()
                    .map(|(p, c)| {
                        format!(
                            "{p}\t{} bytes{}",
                            c.len(),
                            if self.changed.contains_key(p) { "\tmodified" } else { "" }
                        )
                    })
                    .collect();
                (lines.join("\n"), false, json!({}))
            }
            "read_file" => match path_arg() {
                Err(e) => (e, true, input.clone()),
                Ok(p) => match self.get(&p) {
                    Some(c) => (c.clone(), false, json!({ "path": p })),
                    None => (format!("{p} does not exist (or is binary)"), true, json!({ "path": p })),
                },
            },
            "write_file" => {
                let p = match path_arg() {
                    Ok(p) => p,
                    Err(e) => return (e, true, json!({ "path": input["path"] })),
                };
                let content = input["content"].as_str().unwrap_or_default();
                let rec = json!({ "path": p, "bytes": content.len(), "sha256": gate::sha256_hex(content.as_bytes()) });
                if content.len() > MAX_FILE_BYTES {
                    return (format!("file too large ({} bytes, max {MAX_FILE_BYTES})", content.len()), true, rec);
                }
                let existed = self.get(&p).is_some();
                self.changed.insert(p.clone(), content.to_string());
                (
                    format!("{} {p} ({} lines)", if existed { "updated" } else { "created" }, content.lines().count()),
                    false,
                    rec,
                )
            }
            "run_simulation" => {
                let top = input["top"].as_str().unwrap_or_default().trim().to_string();
                let tree = self.tree();
                let hdl: Vec<(&str, &str)> = tree
                    .iter()
                    .filter(|(p, _)| p.ends_with(".v") || p.ends_with(".sv"))
                    .map(|(p, c)| (p.as_str(), c.as_str()))
                    .collect();
                let limits = sc_sim::Limits { max_time: 50_000_000, max_steps: 50_000_000, ..Default::default() };
                let rec = json!({ "top": top });
                match sc_sim::simulate(&hdl, (!top.is_empty()).then_some(top.as_str()), limits) {
                    Err(e) => (format!("simulation error: {e}"), true, rec),
                    Ok(r) => {
                        let mut out = vec![format!("finished: {} at time {} ({} steps)", r.finished, r.time, r.steps)];
                        if let Some(s) = &r.stopped {
                            out.push(format!("stopped: {s}"));
                        }
                        out.push("--- output ---".into());
                        let n = r.log.len();
                        out.extend(r.log.into_iter().take(TOOL_OUTPUT_LINES));
                        if n > TOOL_OUTPUT_LINES {
                            out.push(format!("[{} more lines]", n - TOOL_OUTPUT_LINES));
                        }
                        out.extend(r.warnings.into_iter().map(|w| format!("warning: {w}")));
                        (out.join("\n"), false, rec)
                    }
                }
            }
            other => (format!("unknown tool {other}"), true, input.clone()),
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} more bytes]", &s[..end], s.len() - end)
}

async fn save_progress(
    state: &AppState,
    id: Uuid,
    evidence: &Value,
    files: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE agent_proposals SET evidence = $2, files = $3 WHERE id = $1")
        .bind(id)
        .bind(evidence)
        .bind(serde_json::to_value(files)?)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn run(
    state: &AppState,
    claude: &Claude,
    id: Uuid,
    project: Uuid,
    base: &str,
    request: &str,
    user: &AuthUser,
) -> anyhow::Result<()> {
    let base_files: BTreeMap<String, String> =
        crate::workspace::text_files(state.repos.clone(), project, base.to_string(), MAX_FILES_TEXT)
            .await?
            .into_iter()
            .collect();
    let mut ws = Workspace { base: base_files, changed: BTreeMap::new() };
    let tools = tools();
    let mut messages: Vec<Value> = vec![json!({
        "role": "user",
        "content": format!("Requested by {} for this project:\n\n{request}", user.display_name),
    })];
    let mut steps: Vec<Value> = Vec::new();
    let mut models: Vec<String> = Vec::new();
    let mut usage = BTreeMap::<String, u64>::new();
    let started = Instant::now();
    let evidence = |steps: &Vec<Value>, models: &Vec<String>, usage: &BTreeMap<String, u64>| {
        json!({
            "request": request,
            "requested_by": { "id": user.id, "name": user.display_name },
            "base_commit": base,
            "model_requested": claude.cfg.model,
            "models_served": models,
            "effort": claude.cfg.effort,
            "system_prompt_sha256": gate::sha256_hex(SYSTEM.as_bytes()),
            "tools": ["list_files", "read_file", "write_file", "run_simulation"],
            "engine": gate::engine(),
            "steps": steps,
            "usage": usage,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        })
    };

    let mut summary = None;
    let mut title: Option<String> = None;
    for turn in 1..=MAX_TURNS {
        let resp = claude.create(SYSTEM, &tools, &messages).await?;
        if let Some(m) = resp["model"].as_str()
            && models.last().map(String::as_str) != Some(m)
        {
            models.push(m.to_string());
        }
        if let Some(u) = resp["usage"].as_object() {
            for k in ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"] {
                *usage.entry(k.to_string()).or_default() += u.get(k).and_then(Value::as_u64).unwrap_or(0);
            }
        }
        let content = resp["content"].as_array().cloned().unwrap_or_default();
        for block in &content {
            match block["type"].as_str() {
                Some("thinking") if !block["thinking"].as_str().unwrap_or_default().is_empty() => {
                    steps.push(json!({ "turn": turn, "kind": "reasoning", "text": block["thinking"] }));
                }
                Some("text") => steps.push(json!({ "turn": turn, "kind": "message", "text": block["text"] })),
                Some("fallback") => {
                    steps.push(json!({ "turn": turn, "kind": "fallback", "from": block["from"], "to": block["to"] }))
                }
                _ => {}
            }
        }
        let stop = resp["stop_reason"].as_str().unwrap_or_default();
        match stop {
            "tool_use" => {
                let mut results = Vec::new();
                for block in content.iter().filter(|b| b["type"] == "tool_use") {
                    let name = block["name"].as_str().unwrap_or_default();
                    let t = Instant::now();
                    let input = block["input"].clone();
                    let (out, is_error, rec) = if name == "submit_proposal" {
                        let t = input["title"].as_str().unwrap_or_default().trim().to_string();
                        let sm = input["summary"].as_str().unwrap_or_default().trim().to_string();
                        title = Some(t.clone()).filter(|t| !t.is_empty());
                        summary = Some(sm);
                        ("Submitted for review.".to_string(), false, json!({ "title": t }))
                    } else if name == "run_simulation" {
                        // The simulator is CPU-bound; keep it off the async threads.
                        let mut moved =
                            std::mem::replace(&mut ws, Workspace { base: BTreeMap::new(), changed: BTreeMap::new() });
                        let n = name.to_string();
                        let (r, back) = tokio::task::spawn_blocking(move || {
                            let r = moved.call(&n, &input);
                            (r, moved)
                        })
                        .await?;
                        ws = back;
                        r
                    } else {
                        ws.call(name, &input)
                    };
                    steps.push(json!({
                        "turn": turn, "kind": "tool", "name": name, "input": rec,
                        "output": truncate(&out, 4000), "is_error": is_error, "ms": t.elapsed().as_millis() as u64,
                    }));
                    results.push(json!({ "type": "tool_result", "tool_use_id": block["id"], "content": out, "is_error": is_error }));
                }
                // Append the assistant turn exactly as returned (thinking blocks included), then all results in one message.
                messages.push(json!({ "role": "assistant", "content": content }));
                messages.push(json!({ "role": "user", "content": results }));
                save_progress(state, id, &evidence(&steps, &models, &usage), &ws.changed).await?;
                if summary.is_some() {
                    break;
                }
            }
            "end_turn" => {
                let text: Vec<&str> =
                    content.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
                summary = Some(text.join("\n\n"));
                break;
            }
            "refusal" => {
                let why = resp["stop_details"]["explanation"]
                    .as_str()
                    .or(resp["stop_details"]["category"].as_str())
                    .unwrap_or("no details");
                anyhow::bail!("the model declined this request ({why})");
            }
            "max_tokens" => anyhow::bail!("the model's response was cut off (max_tokens); try a smaller request"),
            other => anyhow::bail!("unexpected stop reason '{other}'"),
        }
    }
    let Some(summary) = summary else {
        anyhow::bail!("the agent did not finish within {MAX_TURNS} turns");
    };
    if ws.changed.is_empty() {
        let ev = evidence(&steps, &models, &usage);
        sqlx::query(
            "UPDATE agent_proposals SET status = 'failed', summary = $2, error = 'the agent made no changes', evidence = $3, finished_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(&summary)
        .bind(&ev)
        .execute(&state.db)
        .await?;
        return Ok(());
    }
    // Drop writes that left a file exactly as it was at the base commit.
    ws.changed.retain(|p, c| ws.base.get(p) != Some(c));
    let report = run_gate(ws.tree()).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut ev = evidence(&steps, &models, &usage);
    ev["summary"] = json!(summary);
    ev["title"] = json!(title);
    ev["files"] =
        json!(ws.changed.iter().map(|(p, c)| (p.clone(), gate::sha256_hex(c.as_bytes()))).collect::<BTreeMap<_, _>>());
    ev["gate"] = serde_json::to_value(&report)?;
    // serde_json maps are sorted, so this serialisation is canonical.
    let hash = gate::sha256_hex(serde_json::to_string(&ev)?.as_bytes());
    sqlx::query(
        "UPDATE agent_proposals SET status = $2, summary = $3, files = $4, gate = $5, evidence = $6, evidence_hash = $7,
         title = $8, finished_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(if report.passed { "ready" } else { "gate_failed" })
    .bind(&summary)
    .bind(serde_json::to_value(&ws.changed)?)
    .bind(serde_json::to_value(&report)?)
    .bind(&ev)
    .bind(&hash)
    .bind(&title)
    .execute(&state.db)
    .await?;
    tracing::info!(proposal = %id, passed = report.passed, files = ws.changed.len(), "agent proposal finished");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn commit_titles() {
        assert_eq!(super::commit_title("Add a parity bit\nmore detail"), "Add a parity bit");
        assert_eq!(super::commit_title("Add a parity bit."), "Add a parity bit");
        assert_eq!(super::commit_title("Wait for it..."), "Wait for it...");
        let long = "Refactor the transmitter state machine so that the stop bit length becomes configurable at elaboration time";
        let t = super::commit_title(long);
        assert!(t.chars().count() <= 72 && t.ends_with('…') && !t.contains("elabor"), "{t}");
    }
}
