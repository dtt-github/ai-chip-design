//! IP registry: publish a project's design at one commit as a versioned block,
//! and import a block into another project, pinned to that commit.
//!
//! Publishing takes the top module (the flow config's `DESIGN_NAME`) and every
//! module it instantiates, with their files. Every published file needs an
//! `SPDX-License-Identifier` header (they combine into the block's licence),
//! and the project's simulation gate must pass at that commit. Versions are
//! semver and immutable; a block name belongs to the project that first used it.
//!
//! Importing copies the files, byte for byte as published (checked against
//! their sha256), into `ip/<block>/` of the importing project, adds them to its
//! flow configs' `VERILOG_FILES`, and records the pin in `ip.lock.json`, all in
//! one commit by the person importing. The block's licence must be compatible
//! with the project's ([`crate::licence`]): incompatible licences are refused,
//! and a licence the check doesn't know needs the project owner to accept it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use sc_git::{Author, Change};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::gate;
use crate::licence::{self, Status, Verdict};
use crate::projects::{Role, authorize};

pub const LOCK_FILE: &str = "ip.lock.json";
const IP_DIR: &str = "ip";
const MAX_FILE_BYTES: usize = 4 << 20;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Version {
    pub block: String,
    pub version: String,
    pub commit_id: String,
    pub top: String,
    pub licence: String,
    pub files: Value,
    pub ports: Value,
    pub params: Value,
    pub gate: Value,
    pub published_by: Uuid,
    pub published_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct Port {
    pub name: String,
    pub dir: &'static str,
    pub width: Option<u32>,
}

fn valid_name(name: &str) -> bool {
    let mut c = name.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase())
        && (2..=63).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn parse_version(v: &str) -> Option<(i32, i32, i32)> {
    let parts: Vec<&str> = v.split('.').collect();
    let num = |s: &str| {
        (!s.is_empty() && s.len() <= 6 && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse().ok()).flatten()
    };
    match parts.as_slice() {
        [a, b, c] if !(a.len() > 1 && a.starts_with('0')) => Some((num(a)?, num(b)?, num(c)?)),
        _ => None,
    }
}

fn is_hdl(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| matches!(e, "v" | "sv"))
}

async fn files_at(state: &AppState, project: Uuid, rev: &str) -> ApiResult<BTreeMap<String, String>> {
    Ok(crate::workspace::text_files(state.repos.clone(), project, rev.to_string(), MAX_FILE_BYTES)
        .await?
        .into_iter()
        .collect())
}

async fn resolve(state: &AppState, project: Uuid, rev: Option<String>) -> ApiResult<String> {
    let repos = state.repos.clone();
    Ok(tokio::task::spawn_blocking(move || -> sc_git::Result<String> {
        let repo = repos.open(&project.to_string())?;
        match rev {
            Some(r) => repo.resolve(&r),
            None => repo.head()?.ok_or_else(|| sc_git::GitError::NotFound("empty repository".into())),
        }
    })
    .await??)
}

// ---- publish -------------------------------------------------------------------

/// What a publish would contain, worked out from the files at the commit.
struct Analysis {
    files: BTreeMap<String, String>,
    licence: String,
    ports: Vec<Port>,
    params: BTreeMap<String, i64>,
    gate: gate::GateReport,
}

fn analyse(all: &BTreeMap<String, String>, top: &str) -> Result<Analysis, ApiError> {
    let hdl: Vec<(&str, &str)> = all.iter().filter(|(p, _)| is_hdl(p)).map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let (modules, _) =
        sc_sim::parse(&hdl).map_err(|e| ApiError::BadRequest(format!("the design does not parse: {e}")))?;
    let Some(top_mod) = modules.iter().find(|m| m.name == top) else {
        return Err(ApiError::BadRequest(format!("no module named {top}")));
    };
    if gate::looks_like_tb(top) {
        return Err(ApiError::BadRequest(format!("{top} looks like a testbench; publish the design's top module")));
    }

    // The top and everything it instantiates, with the files that define them.
    let by_name: HashMap<&str, &sc_sim::ast::Module> = modules.iter().map(|m| (m.name.as_str(), m)).collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![top];
    while let Some(name) = stack.pop() {
        if !seen.insert(name) {
            continue;
        }
        if let Some(m) = by_name.get(name) {
            for item in &m.items {
                if let sc_sim::ast::Item::Instance { module, .. } = item {
                    stack.push(module.as_str());
                }
            }
        }
    }
    let paths: BTreeSet<&str> = seen.iter().filter_map(|n| by_name.get(n)).map(|m| m.file.as_str()).collect();

    let mut licences = BTreeSet::new();
    let mut missing = Vec::new();
    for p in &paths {
        match licence::spdx_header(&all[*p]) {
            Some(l) => {
                licence::validate(&l).map_err(|e| ApiError::BadRequest(format!("{p}: {e}")))?;
                licences.insert(l);
            }
            None => missing.push(*p),
        }
    }
    if !missing.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "every published file needs an SPDX-License-Identifier header; missing in {}",
            missing.join(", ")
        )));
    }
    let licence = licences
        .iter()
        .map(|l| if licences.len() > 1 && l.contains(' ') { format!("({l})") } else { l.clone() })
        .collect::<Vec<_>>()
        .join(" AND ");

    let design = sc_sim::elab::elaborate(&modules, top)
        .map_err(|e| ApiError::BadRequest(format!("cannot elaborate {top}: {e}")))?;
    let params = design.instances[0].params.clone();
    let mut dirs = HashMap::new();
    for item in &top_mod.items {
        if let sc_sim::ast::Item::Decl(d) = item
            && let Some(dir) = d.dir
        {
            for n in &d.names {
                dirs.insert(n.name.as_str(), dir);
            }
        }
    }
    let ports = top_mod
        .ports
        .iter()
        .map(|p| Port {
            name: p.clone(),
            dir: match dirs.get(p.as_str()) {
                Some(sc_sim::ast::Dir::Input) => "input",
                Some(sc_sim::ast::Dir::Output) => "output",
                Some(sc_sim::ast::Dir::Inout) => "inout",
                None => "unknown",
            },
            width: design.names.get(&format!("{top}.{p}")).map(|&i| design.signals[i].width),
        })
        .collect();

    let report = gate::run(all);
    if !report.passed {
        return Err(ApiError::Conflict(format!(
            "the simulation gate fails at this commit, so it can't be published: {}",
            report.summary
        )));
    }
    let files = paths.iter().map(|p| (p.to_string(), gate::sha256_hex(all[*p].as_bytes()))).collect();
    Ok(Analysis { files, licence, ports, params, gate: report })
}

#[derive(Debug, Deserialize)]
pub struct PublishReq {
    /// Registry name; defaults to the top module's name.
    pub name: Option<String>,
    /// Semver `MAJOR.MINOR.PATCH`, higher than the block's previous versions.
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Commit to publish; defaults to the branch tip.
    pub commit: Option<String>,
    /// Top module; defaults to `DESIGN_NAME` in `config.json`.
    pub top: Option<String>,
}

pub async fn publish(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
    Json(req): Json<PublishReq>,
) -> ApiResult<(StatusCode, Json<Version>)> {
    authorize(&state, &user, project_id, Role::Editor).await?;
    let Some((major, minor, patch)) = parse_version(req.version.trim()) else {
        return Err(ApiError::BadRequest("version must be MAJOR.MINOR.PATCH, e.g. 1.0.0".into()));
    };
    let commit = resolve(&state, project_id, req.commit).await?;
    let all = files_at(&state, project_id, &commit).await?;
    let top = match req.top.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => all
            .get("config.json")
            .and_then(|c| serde_json::from_str::<Value>(c).ok())
            .and_then(|c| c["DESIGN_NAME"].as_str().map(String::from))
            .ok_or_else(|| ApiError::BadRequest("name the top module (no DESIGN_NAME in config.json)".into()))?,
    };
    let name = req.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| top.clone());
    if !valid_name(&name) {
        return Err(ApiError::BadRequest(
            "block names are 2-63 characters: lowercase letters, digits and _, starting with a letter".into(),
        ));
    }

    let owner: Option<Uuid> = sqlx::query_scalar("SELECT project_id FROM ip_blocks WHERE name = $1")
        .bind(&name)
        .fetch_optional(&state.db)
        .await?;
    if owner.is_some_and(|o| o != project_id) {
        return Err(ApiError::Conflict(format!("the block name '{name}' belongs to another project")));
    }
    let latest: Option<(String, i32, i32, i32)> = sqlx::query_as(
        "SELECT version, major, minor, patch FROM ip_versions WHERE block = $1
         ORDER BY major DESC, minor DESC, patch DESC LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(&state.db)
    .await?;
    if let Some((v, a, b, c)) = latest
        && (major, minor, patch) <= (a, b, c)
    {
        return Err(ApiError::Conflict(format!("{name} is already at {v}; publish a higher version")));
    }

    let top2 = top.clone();
    let a = tokio::task::spawn_blocking(move || sc_sim::on_big_stack(|| analyse(&all, &top2))).await??;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO ip_blocks (name, project_id, description, created_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (name) DO UPDATE SET description = CASE WHEN EXCLUDED.description = '' THEN ip_blocks.description ELSE EXCLUDED.description END",
    )
    .bind(&name)
    .bind(project_id)
    .bind(req.description.trim())
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    let version: Version = sqlx::query_as(
        "INSERT INTO ip_versions (block, version, major, minor, patch, commit_id, top, licence, files, ports, params, gate, published_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         RETURNING block, version, commit_id, top, licence, files, ports, params, gate, published_by, published_at",
    )
    .bind(&name)
    .bind(format!("{major}.{minor}.{patch}"))
    .bind(major)
    .bind(minor)
    .bind(patch)
    .bind(&commit)
    .bind(&top)
    .bind(&a.licence)
    .bind(json!(a.files))
    .bind(json!(a.ports))
    .bind(json!(a.params))
    .bind(serde_json::to_value(&a.gate).map_err(anyhow::Error::from)?)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(block = %name, version = %version.version, %commit, "IP published");
    Ok((StatusCode::CREATED, Json(version)))
}

// ---- registry ------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct Block {
    pub name: String,
    pub description: String,
    pub project_id: Uuid,
    pub project_name: String,
    /// Newest first.
    pub versions: Vec<Version>,
}

/// Blocks the caller can see: from public projects and projects they are a member of.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Block>>> {
    let blocks: Vec<(String, String, Uuid, String)> = sqlx::query_as(
        "SELECT b.name, b.description, b.project_id, p.name FROM ip_blocks b JOIN projects p ON p.id = b.project_id
         WHERE p.visibility = 'public'
            OR EXISTS (SELECT 1 FROM project_members m WHERE m.project_id = p.id AND m.user_id = $1)
         ORDER BY b.name",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let names: Vec<String> = blocks.iter().map(|b| b.0.clone()).collect();
    let versions: Vec<Version> = sqlx::query_as(
        "SELECT block, version, commit_id, top, licence, files, ports, params, gate, published_by, published_at
         FROM ip_versions WHERE block = ANY($1) ORDER BY major DESC, minor DESC, patch DESC",
    )
    .bind(&names)
    .fetch_all(&state.db)
    .await?;
    let mut by_block: HashMap<String, Vec<Version>> = HashMap::new();
    for v in versions {
        by_block.entry(v.block.clone()).or_default().push(v);
    }
    Ok(Json(
        blocks
            .into_iter()
            .map(|(name, description, project_id, project_name)| Block {
                versions: by_block.remove(&name).unwrap_or_default(),
                name,
                description,
                project_id,
                project_name,
            })
            .collect(),
    ))
}

// ---- import --------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ImportQuery {
    pub block: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct ImportCheck {
    pub block: String,
    pub version: String,
    pub source_project: String,
    pub commit: String,
    pub licence: Verdict,
    /// Whether the import can go ahead (with `accept_licence` when the licence is unknown).
    pub allowed: bool,
    /// The caller may accept an unknown licence (project owners only).
    pub can_accept: bool,
    /// Modules the block defines that the project already has.
    pub collisions: Vec<String>,
    /// Version currently imported, if any (an import replaces it).
    pub replaces: Option<String>,
    /// Files the import writes, and flow configs it updates.
    pub files: Vec<String>,
    pub configs: Vec<String>,
    pub problems: Vec<String>,
}

struct Plan {
    check: ImportCheck,
    head: String,
    changes: Vec<Change>,
    lock: Value,
}

async fn plan(state: &AppState, user: &AuthUser, project_id: Uuid, q: &ImportQuery) -> ApiResult<Plan> {
    let (project, role) = authorize(state, user, project_id, Role::Editor).await?;
    let v: Option<Version> = sqlx::query_as(
        "SELECT block, version, commit_id, top, licence, files, ports, params, gate, published_by, published_at
         FROM ip_versions WHERE block = $1 AND version = $2",
    )
    .bind(&q.block)
    .bind(&q.version)
    .fetch_optional(&state.db)
    .await?;
    let v = v.ok_or_else(|| ApiError::NotFound(format!("no block {}@{}", q.block, q.version)))?;
    let source_id: Uuid = sqlx::query_scalar("SELECT project_id FROM ip_blocks WHERE name = $1")
        .bind(&v.block)
        .fetch_one(&state.db)
        .await?;
    // Same visibility as the source project, and the same "not found" when it's hidden.
    let (source, _) = authorize(state, user, source_id, Role::Viewer)
        .await
        .map_err(|_| ApiError::NotFound(format!("no block {}@{}", q.block, q.version)))?;
    if source_id == project_id {
        return Err(ApiError::BadRequest("a project can't import its own block".into()));
    }

    // The published files, exactly as published.
    let manifest: BTreeMap<String, String> = serde_json::from_value(v.files.clone()).map_err(anyhow::Error::from)?;
    let src = files_at(state, source_id, &v.commit_id).await?;
    let mut block_files = BTreeMap::new();
    for (path, sha) in &manifest {
        let content =
            src.get(path).ok_or_else(|| anyhow::anyhow!("{path} is missing from the source at {}", v.commit_id))?;
        if &gate::sha256_hex(content.as_bytes()) != sha {
            return Err(anyhow::anyhow!("{path} at {} does not match its published hash", v.commit_id).into());
        }
        block_files.insert(format!("{IP_DIR}/{}/{path}", v.block), content.clone());
    }

    let head = resolve(state, project_id, None).await?;
    let here = files_at(state, project_id, &head).await?;
    let mut lock: Value = match here.get(LOCK_FILE) {
        Some(text) => {
            serde_json::from_str(text).map_err(|e| ApiError::Conflict(format!("{LOCK_FILE} is not valid JSON: {e}")))?
        }
        None => json!({ "blocks": {} }),
    };
    if !lock["blocks"].is_object() {
        lock["blocks"] = json!({});
    }
    let previous = lock["blocks"][&v.block].clone();
    let replaces = previous["version"].as_str().map(String::from);
    let old_files: BTreeSet<String> =
        previous["files"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();

    let mut problems = Vec::new();
    if replaces.as_deref() == Some(v.version.as_str()) {
        problems.push(format!("{}@{} is already imported", v.block, v.version));
    }
    // Module names must stay unique across the project.
    let names = |files: Vec<(String, String)>| {
        tokio::task::spawn_blocking(move || {
            sc_sim::on_big_stack(|| {
                let mut out = Vec::new();
                for (path, text) in &files {
                    if let Ok((mods, _)) = sc_sim::parse(&[(path.as_str(), text.as_str())]) {
                        out.extend(mods.into_iter().map(|m| (m.name, path.clone())));
                    }
                }
                out
            })
        })
    };
    let ours: Vec<(String, String)> = here
        .iter()
        .filter(|(p, _)| is_hdl(p) && !old_files.contains(*p))
        .map(|(p, c)| (p.clone(), c.clone()))
        .collect();
    let theirs: Vec<(String, String)> =
        block_files.iter().filter(|(p, _)| is_hdl(p)).map(|(p, c)| (p.clone(), c.clone())).collect();
    let defined: HashMap<String, String> = names(ours).await?.into_iter().collect();
    let mut collisions = Vec::new();
    for (name, _) in names(theirs).await? {
        if let Some(at) = defined.get(&name) {
            collisions.push(format!("module {name} (already in {at})"));
        }
    }

    let verdict = licence::check(&v.licence, &project.licence);
    let can_accept = role == Some(Role::Owner);
    match verdict.status {
        Status::Incompatible => {
            problems.push(format!("licence {} can't go into this {} project", v.licence, project.licence))
        }
        Status::Unknown if !can_accept => {
            problems.push("the licence needs a project owner to read and accept it".into())
        }
        _ => {}
    }
    if !collisions.is_empty() {
        problems.push(format!("module name clash: {}", collisions.join(", ")));
    }

    // Flow configs that list RTL get the block's files (and lose the replaced version's).
    let new_entries: Vec<String> = block_files.keys().filter(|p| is_hdl(p)).map(|p| format!("dir::{p}")).collect();
    let mut changes = Vec::new();
    let mut configs = Vec::new();
    for (path, text) in &here {
        let base = path.rsplit('/').next().unwrap_or(path);
        if !(base.starts_with("config") && base.ends_with(".json")) {
            continue;
        }
        let Ok(mut cfg) = serde_json::from_str::<Value>(text) else { continue };
        let list: Vec<String> = match &cfg["VERILOG_FILES"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            _ => continue,
        };
        let mut updated: Vec<String> =
            list.iter().filter(|e| !old_files.iter().any(|o| e.as_str() == format!("dir::{o}"))).cloned().collect();
        for e in &new_entries {
            if !updated.contains(e) {
                updated.push(e.clone());
            }
        }
        if updated != list {
            cfg["VERILOG_FILES"] = json!(updated);
            let content = serde_json::to_string_pretty(&cfg).map_err(anyhow::Error::from)? + "\n";
            changes.push(Change::Write { path: path.clone(), content: content.into_bytes() });
            configs.push(path.clone());
        }
    }
    for old in old_files.iter().filter(|o| !block_files.contains_key(*o) && here.contains_key(*o)) {
        changes.push(Change::Delete { path: old.clone() });
    }
    for (path, content) in &block_files {
        changes.push(Change::Write { path: path.clone(), content: content.clone().into_bytes() });
    }
    lock["blocks"][&v.block] = json!({
        "version": v.version,
        "commit": v.commit_id,
        "source_project": source_id,
        "source_name": source.name,
        "top": v.top,
        "licence": v.licence,
        "licence_check": verdict.status,
        "files": block_files.iter().map(|(p, c)| (p.clone(), gate::sha256_hex(c.as_bytes()))).collect::<BTreeMap<_, _>>(),
    });

    let check = ImportCheck {
        block: v.block.clone(),
        version: v.version.clone(),
        source_project: source.name,
        commit: v.commit_id.clone(),
        allowed: problems.is_empty(),
        can_accept,
        licence: verdict,
        collisions,
        replaces,
        files: block_files.keys().cloned().collect(),
        configs,
        problems,
    };
    Ok(Plan { check, head, changes, lock })
}

/// What importing a block would do, without doing it.
pub async fn check(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
    Query(q): Query<ImportQuery>,
) -> ApiResult<Json<ImportCheck>> {
    Ok(Json(plan(&state, &user, project_id, &q).await?.check))
}

#[derive(Debug, Deserialize)]
pub struct ImportReq {
    pub block: String,
    pub version: String,
    /// Required (from a project owner) when the licence check can't judge the block's licence.
    #[serde(default)]
    pub accept_licence: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    /// The import commit in this project (`commit` is the block's pinned source commit).
    pub imported_commit: String,
    #[serde(flatten)]
    pub check: ImportCheck,
}

pub async fn import(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
    Json(req): Json<ImportReq>,
) -> ApiResult<(StatusCode, Json<ImportResult>)> {
    let q = ImportQuery { block: req.block, version: req.version };
    let Plan { check, head, mut changes, mut lock } = plan(&state, &user, project_id, &q).await?;
    if !check.allowed {
        return Err(ApiError::Conflict(check.problems.join("; ")));
    }
    if check.licence.status == Status::Unknown {
        if !req.accept_licence {
            return Err(ApiError::Conflict(format!(
                "the licence check can't judge {}; read it and import again with accept_licence",
                check.licence.block
            )));
        }
        lock["blocks"][&check.block]["licence_accepted_by"] = json!(user.email);
    }
    changes.push(Change::Write {
        path: LOCK_FILE.into(),
        content: (serde_json::to_string_pretty(&lock).map_err(anyhow::Error::from)? + "\n").into_bytes(),
    });

    let action = match &check.replaces {
        Some(old) => format!("Update IP block {} {old} -> {}", check.block, check.version),
        None => format!("Import IP block {} {}", check.block, check.version),
    };
    let mut message = format!(
        "{action}\n\nFrom project \"{}\" at commit {}.\nLicence: {} ({} with this project's {}).\n",
        check.source_project,
        check.commit,
        check.licence.block,
        match check.licence.status {
            Status::Compatible => "compatible",
            Status::Conditions => "compatible with conditions",
            Status::Unknown => "not judged; accepted by the project owner",
            Status::Incompatible => "incompatible",
        },
        check.licence.project,
    );
    for o in &check.licence.obligations {
        message.push_str(&format!("- {o}\n"));
    }
    message.push_str(&format!("Files: {}\n", check.files.join(", ")));
    if !check.configs.is_empty() {
        message.push_str(&format!("Added to VERILOG_FILES in {}.\n", check.configs.join(", ")));
    }

    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let repos = state.repos.clone();
    let info = tokio::task::spawn_blocking(move || {
        repos.open(&project_id.to_string())?.commit(changes, &message, &author, Some(&head))
    })
    .await??;
    sqlx::query("UPDATE projects SET updated_at = now() WHERE id = $1").bind(project_id).execute(&state.db).await?;
    state.collab.committed(&state, project_id, &info.id).await;
    tracing::info!(project = %project_id, block = %check.block, version = %check.version, commit = %info.id, "IP imported");
    Ok((StatusCode::CREATED, Json(ImportResult { imported_commit: info.id, check })))
}

/// The project's `ip.lock.json` at the branch tip, with each block's licence re-checked.
pub async fn imports(
    State(state): State<AppState>,
    user: AuthUser,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    let (project, _) = authorize(&state, &user, project_id, Role::Viewer).await?;
    let head = resolve(&state, project_id, None).await?;
    let repos = state.repos.clone();
    let text = tokio::task::spawn_blocking(move || {
        match repos.open(&project_id.to_string())?.read_file(Some(&head), LOCK_FILE) {
            Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
            Err(sc_git::GitError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await??;
    let mut lock: Value = match text {
        Some(t) => {
            serde_json::from_str(&t).map_err(|e| ApiError::Conflict(format!("{LOCK_FILE} is not valid JSON: {e}")))?
        }
        None => json!({ "blocks": {} }),
    };
    if let Some(blocks) = lock["blocks"].as_object_mut() {
        for entry in blocks.values_mut() {
            let l = entry["licence"].as_str().unwrap_or_default().to_string();
            entry["licence_verdict"] = json!(licence::check(&l, &project.licence));
        }
    }
    Ok(Json(lock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_versions() {
        assert!(valid_name("uart_tx"));
        assert!(!valid_name("Uart"));
        assert!(!valid_name("1uart"));
        assert!(!valid_name("u"));
        assert!(!valid_name("uart-tx"));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.x"), None);
        assert_eq!(parse_version("01.2.3"), None);
        assert_eq!(parse_version("1.2.3-rc1"), None);
    }

    const UART: &str = "// SPDX-License-Identifier: MIT\nmodule uart #(parameter W = 8) (input wire clk, input wire [W-1:0] d, output wire q);\n  sub s(.a(clk), .y(q));\nendmodule\n";
    const SUB: &str =
        "// SPDX-License-Identifier: Apache-2.0\nmodule sub(input wire a, output wire y); assign y = a; endmodule\n";
    const TB: &str = "module tb; reg c = 0; reg [7:0] d = 0; wire q; uart u(.clk(c), .d(d), .q(q)); initial begin #1 $display(\"PASS\"); $finish; end endmodule\n";

    fn tree(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut m: BTreeMap<String, String> = [("src/uart.v", UART), ("src/sub.v", SUB), ("tb/tb.v", TB)]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        for (a, b) in extra {
            m.insert(a.to_string(), b.to_string());
        }
        m
    }

    #[test]
    fn publish_analysis() {
        let a = analyse(&tree(&[]), "uart").unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(a.files.keys().collect::<Vec<_>>(), vec!["src/sub.v", "src/uart.v"], "testbenches stay out");
        assert_eq!(a.licence, "Apache-2.0 AND MIT");
        assert_eq!(a.params["W"], 8);
        let d = a.ports.iter().find(|p| p.name == "d").unwrap();
        assert_eq!((d.dir, d.width), ("input", Some(8)));
        assert!(a.gate.passed);

        let no_header = tree(&[("src/sub.v", "module sub(input wire a, output wire y); assign y = a; endmodule\n")]);
        assert!(matches!(analyse(&no_header, "uart"), Err(ApiError::BadRequest(m)) if m.contains("src/sub.v")));
        let failing = tree(&[("tb/tb.v", &TB.replace("PASS", "FAIL"))]);
        assert!(matches!(analyse(&failing, "uart"), Err(ApiError::Conflict(_))));
        assert!(analyse(&tree(&[]), "tb").is_err());
        assert!(analyse(&tree(&[]), "nope").is_err());
    }
}
