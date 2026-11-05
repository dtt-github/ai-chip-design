//! HTTP side of the Blocks view ([`crate::blocks`]): list the blocks that can be placed, check a
//! layout of blocks and connections, save it (with the generated top and the flow config in the
//! same commit), and have the assistant review the connections for meaning.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::Json;
use axum::extract::{Path, State};
use sc_git::{Author, Change};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::blocks::{self, ADAPTERS_FILE, BLOCKS_FILE, Blocks, Issue, ModuleInfo, PortInfo};
use crate::error::{ApiError, ApiResult};
use crate::projects::{Role, authorize};

const MAX_TEXT: usize = 4 << 20;
const REVIEW_TURNS: usize = 3;

async fn files_at(state: &AppState, project: Uuid, rev: &str) -> ApiResult<BTreeMap<String, String>> {
    Ok(crate::workspace::text_files(state.repos.clone(), project, rev.to_string(), MAX_TEXT)
        .await?
        .into_iter()
        .collect())
}

async fn head(state: &AppState, project: Uuid) -> ApiResult<String> {
    crate::workspace::head(state.repos.clone(), project)
        .await?
        .ok_or_else(|| ApiError::NotFound("the project has no commits".into()))
}

fn parse_blocks(text: &str) -> ApiResult<Blocks> {
    serde_json::from_str(text).map_err(|e| ApiError::Conflict(format!("{BLOCKS_FILE} can't be read: {e}")))
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub issues: Vec<Issue>,
    /// Each block's connections, with widths for its settings.
    pub ports: BTreeMap<String, Vec<PortInfo>>,
    /// The generated top, when there are no errors.
    pub verilog: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BlocksView {
    pub head: String,
    pub blocks: Option<Blocks>,
    pub modules: Vec<ModuleInfo>,
    pub problems: Vec<String>,
    pub check: Option<CheckResult>,
    /// The flow config's top, as the default chip name for a new layout.
    pub design_name: Option<String>,
    /// Port meanings from the spec (module → port → meaning), to show next to connections.
    pub meanings: BTreeMap<String, BTreeMap<String, String>>,
}

fn meanings(files: &BTreeMap<String, String>) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if let Some(s) = files.get(crate::spec::SPEC_FILE).and_then(|t| serde_json::from_str::<crate::spec::Spec>(t).ok()) {
        out.insert(s.top.clone(), s.ports.iter().map(|p| (p.name.clone(), p.meaning.clone())).collect());
    }
    out
}

pub async fn get(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<BlocksView>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let head = head(&state, id).await?;
    let files = files_at(&state, id, &head).await?;
    let design_name = files
        .get("config.json")
        .and_then(|c| serde_json::from_str::<Value>(c).ok())
        .and_then(|c| c["DESIGN_NAME"].as_str().map(String::from));
    let saved = files.get(BLOCKS_FILE).map(|t| parse_blocks(t)).transpose()?;
    let saved_copy = saved.clone();
    let top = saved.as_ref().map(|b| b.top.clone());
    let meanings = meanings(&files);
    let (modules, problems, check) = tokio::task::spawn_blocking(move || {
        sc_sim::on_big_stack(|| {
            let (modules, problems) = blocks::discover(&files, top.as_deref());
            let check = saved.as_ref().map(|b| {
                let c = b.check(&files);
                CheckResult { issues: c.issues, ports: c.ports, verilog: c.verilog }
            });
            (modules, problems, check)
        })
    })
    .await?;
    Ok(Json(BlocksView { head, blocks: saved_copy, modules, problems, check, design_name, meanings }))
}

#[derive(Debug, Deserialize)]
pub struct CheckReq {
    pub blocks: Blocks,
}

/// Checks a layout without saving it (the canvas calls this as people edit).
pub async fn check(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CheckReq>,
) -> ApiResult<Json<CheckResult>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let files = files_at(&state, id, &head(&state, id).await?).await?;
    let c = tokio::task::spawn_blocking(move || {
        crate::gate::with_slot(|| sc_sim::on_big_stack(|| req.blocks.check(&files)))
    })
    .await?;
    Ok(Json(CheckResult { issues: c.issues, ports: c.ports, verilog: c.verilog }))
}

#[derive(Debug, Deserialize)]
pub struct SaveReq {
    pub blocks: Blocks,
    pub parent: String,
    pub message: Option<String>,
}

/// Commits `blocks.json`, the generated top (and adapters), and points the flow config at the chip.
pub async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SaveReq>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let files = files_at(&state, id, &req.parent).await?;
    let b = req.blocks;
    let (checked, b, files) = tokio::task::spawn_blocking(move || {
        let c = crate::gate::with_slot(|| sc_sim::on_big_stack(|| b.check(&files)));
        (c, b, files)
    })
    .await?;
    let Some(verilog) = checked.verilog else {
        let errors: Vec<String> =
            checked.issues.iter().filter(|i| i.level == "error").map(|i| i.message.clone()).collect();
        return Err(ApiError::BadRequest(errors.join("\n")));
    };

    let mut changes = Vec::new();
    let top_path = blocks::top_path(&b.top);
    stage(&mut changes, &top_path, verilog.clone(), &files)?;
    if checked.uses_adapters {
        stage(&mut changes, ADAPTERS_FILE, blocks::adapters_source(), &files)?;
    } else if files.get(ADAPTERS_FILE).is_some_and(|c| blocks::is_generated(c)) {
        changes.push(Change::Delete { path: ADAPTERS_FILE.into() });
    }
    // A previous chip's generated top, under another name, goes.
    if let Some(old) = files.get(BLOCKS_FILE).and_then(|t| serde_json::from_str::<Blocks>(t).ok())
        && old.top != b.top
        && files.get(&blocks::top_path(&old.top)).is_some_and(|c| blocks::is_generated(c))
    {
        changes.push(Change::Delete { path: blocks::top_path(&old.top) });
    }
    stage(&mut changes, BLOCKS_FILE, serde_json::to_string_pretty(&b).map_err(anyhow::Error::from)? + "\n", &files)?;

    // Examples of the chip drive its pins: check them against the new chip and regenerate them.
    if let Some(spec) =
        files.get(crate::spec::SPEC_FILE).and_then(|t| serde_json::from_str::<crate::spec::Spec>(t).ok())
    {
        let problems = spec.check(Some(&b));
        if !problems.is_empty() {
            return Err(ApiError::Conflict(format!(
                "the examples don't fit this chip any more; change them first:\n{}",
                problems.join("\n")
            )));
        }
        for (path, content) in spec.testbenches_for(Some(&b)) {
            if files.get(&path) != Some(&content) {
                changes.push(Change::Write { path, content: content.into_bytes() });
            }
        }
    }

    // The flow builds the chip: its top and every file it uses.
    let mut with_new = files.clone();
    with_new.insert(top_path.clone(), verilog);
    if checked.uses_adapters {
        with_new.insert(ADAPTERS_FILE.into(), blocks::adapters_source());
    }
    let top = b.top.clone();
    let used = tokio::task::spawn_blocking(move || sc_sim::on_big_stack(|| files_used(&with_new, &top))).await?;
    if let Some(cfg_text) = files.get("config.json") {
        let mut cfg: Value =
            serde_json::from_str(cfg_text).map_err(|e| ApiError::Conflict(format!("config.json: {e}")))?;
        cfg["DESIGN_NAME"] = json!(b.top);
        cfg["VERILOG_FILES"] = json!(used.iter().map(|f| format!("dir::{f}")).collect::<Vec<_>>());
        stage(
            &mut changes,
            "config.json",
            serde_json::to_string_pretty(&cfg).map_err(anyhow::Error::from)? + "\n",
            &files,
        )?;
    }
    if changes.is_empty() {
        return Ok(Json(json!({ "commit": req.parent, "changed": false })));
    }
    let message = req.message.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| {
        format!("Update the chip's blocks ({} blocks, {} connections)", b.blocks.len(), b.wires.len())
    });
    let message = format!("{message}\n\nRegenerated {top_path} from {BLOCKS_FILE}; config.json builds {}.\n", b.top);
    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let (repos, parent) = (state.repos.clone(), req.parent.clone());
    let info = tokio::task::spawn_blocking(move || {
        repos.open(&id.to_string())?.commit(changes, &message, &author, Some(&parent))
    })
    .await??;
    sqlx::query("UPDATE projects SET updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    state.collab.committed(&state, id, &info.id).await;
    Ok(Json(json!({ "commit": info.id, "changed": true })))
}

/// Adds a write unless the file already has that content. Generated files may be replaced; a
/// hand-written file in the way of the generated top is not (blocks.json and config.json may).
fn stage(changes: &mut Vec<Change>, path: &str, content: String, files: &BTreeMap<String, String>) -> ApiResult<()> {
    match files.get(path) {
        Some(old) if *old == content => {}
        Some(old) if path.ends_with(".v") && !blocks::is_generated(old) => {
            return Err(ApiError::Conflict(format!(
                "{path} exists and wasn't generated from the blocks; pick another chip name or rename that file"
            )));
        }
        _ => changes.push(Change::Write { path: path.into(), content: content.into_bytes() }),
    }
    Ok(())
}

/// The files defining `top` and every module it uses, sorted.
fn files_used(files: &BTreeMap<String, String>, top: &str) -> Vec<String> {
    let hdl: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| p.ends_with(".v") || p.ends_with(".sv"))
        .filter(|(p, _)| !p.starts_with("tb/"))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let Ok((modules, _)) = sc_sim::parse(&hdl) else { return vec![blocks::top_path(top)] };
    let by: HashMap<&str, &sc_sim::ast::Module> = modules.iter().map(|m| (m.name.as_str(), m)).collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![top];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        if let Some(m) = by.get(n) {
            stack.extend(m.instantiated());
        }
    }
    let used: BTreeSet<String> = seen.iter().filter_map(|n| by.get(n)).map(|m| m.file.clone()).collect();
    used.into_iter().collect()
}

// ---- the assistant's review --------------------------------------------------------

const REVIEW_SYSTEM: &str = "You review how blocks of a chip are connected, for engineers who are not \
chip-design experts. The server has already checked that every connection exists, goes from an output to an \
input and has matching widths. Your job is meaning: does each connection make sense given what the two ends \
do? Typical problems: a level (stays on) going into an input that expects a one-clock pulse, or the reverse; an \
active-low signal into an active-high input; a signal from outside the chip (a button, another clock) used \
directly instead of through a synchronizer; a value cut to fewer bits that matter.

Fix a problem only with one of these adapters, placed in the connection:
- sc_rise_pulse: a level in, a one-clock pulse out when it goes from 0 to 1 (ports level -> pulse).
- sc_sync2: two flip-flops to bring an outside signal safely into this clock (d -> q).
- sc_invert: flips every bit (a -> y; setting W = width).
- sc_resize: changes width, padding with zeros or keeping the lowest bits (a -> y; settings IN, OUT).
If none fits, describe the problem and leave adapter empty. Write in plain words, one or two sentences per \
note; say what will go wrong, not just that something is unusual. Call submit_review once with a note for every \
connection (ok ones too, briefly).";

fn review_tool() -> Value {
    json!([{
        "name": "submit_review",
        "description": "Submit a note for every connection, by its number.",
        "strict": true,
        "input_schema": {
            "type": "object",
            "properties": {
                "notes": { "type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "wire": { "type": "integer", "description": "Connection number, starting at 1" },
                        "level": { "type": "string", "enum": ["ok", "warning"] },
                        "note": { "type": "string", "description": "Plain words: what this connection does or what will go wrong" },
                        "adapter": { "type": "string", "enum": ["", "sc_rise_pulse", "sc_sync2", "sc_invert", "sc_resize"] }
                    },
                    "required": ["wire", "level", "note", "adapter"],
                    "additionalProperties": false
                } },
                "summary": { "type": "string", "description": "One sentence on the whole chip" }
            },
            "required": ["notes", "summary"],
            "additionalProperties": false
        }
    }])
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReviewNote {
    /// Index into `wires` (from 0).
    pub wire: usize,
    pub level: String,
    pub note: String,
    pub adapter: String,
}

#[derive(Debug, Serialize)]
pub struct Review {
    pub notes: Vec<ReviewNote>,
    pub summary: String,
    pub usage: BTreeMap<String, u64>,
}

pub async fn review(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<CheckReq>,
) -> ApiResult<Json<Review>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let Some(claude) = state.agent.claude.clone() else {
        return Err(ApiError::Unavailable("the assistant is not configured on this server (no API key)".into()));
    };
    crate::spend::check(&state, &user).await?;
    if req.blocks.wires.is_empty() {
        return Err(ApiError::BadRequest("there are no connections to review".into()));
    }
    let files = files_at(&state, id, &head(&state, id).await?).await?;
    let meanings = meanings(&files);
    let b = req.blocks;
    let (b, checked, files) = tokio::task::spawn_blocking(move || {
        let c = sc_sim::on_big_stack(|| b.check(&files));
        (b, c, files)
    })
    .await?;

    // What each end is, with the spec's meanings and the modules' source.
    let mut ctx = String::from("Blocks:\n");
    for blk in &b.blocks {
        ctx.push_str(&format!("- {} is a {} with settings {:?}\n", blk.name, blk.module, blk.params));
        for p in checked.ports.get(&blk.name).into_iter().flatten() {
            let m =
                meanings.get(&blk.module).and_then(|m| m.get(&p.name)).map(|m| format!(": {m}")).unwrap_or_default();
            ctx.push_str(&format!("    {} {} {} bit(s){m}\n", p.dir, p.name, p.width));
        }
    }
    ctx.push_str("Chip pins:\n");
    for p in &b.pins {
        ctx.push_str(&format!("- {} {} {} bit(s): {}\n", p.dir, p.name, p.width, p.meaning));
    }
    ctx.push_str(&format!(
        "Clock pin: {:?}, reset pin: {:?} (connected automatically to ports with the same name)\n",
        b.clock, b.reset
    ));
    ctx.push_str("Connections:\n");
    for (i, w) in b.wires.iter().enumerate() {
        ctx.push_str(&format!("{}. {} -> {}\n", i + 1, w.from, w.to));
    }
    let used: BTreeSet<&str> = b.blocks.iter().map(|x| x.module.as_str()).collect();
    let mut budget = 40_000usize;
    for (p, c) in files.iter().filter(|(p, _)| p.ends_with(".v") && !p.starts_with("tb/")) {
        if used.iter().any(|m| c.contains(&format!("module {m}"))) && c.len() < budget {
            budget -= c.len();
            ctx.push_str(&format!("--- {p} ---\n{c}\n"));
        }
    }

    let tools = review_tool();
    let mut messages = vec![json!({ "role": "user", "content": ctx })];
    let mut served = claude.cfg.model.clone();
    let mut usage: BTreeMap<String, u64> = BTreeMap::new();
    let result: ApiResult<Review> = async {
        for _ in 0..REVIEW_TURNS {
            let resp = claude
                .create(REVIEW_SYSTEM, &tools, &messages)
                .await
                .map_err(|e| ApiError::Unavailable(e.to_string()))?;
            if let Some(m) = resp["model"].as_str().filter(|m| !m.is_empty()) {
                served = m.to_string();
            }
            if let Some(u) = resp["usage"].as_object() {
                for k in ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"] {
                    *usage.entry(k.to_string()).or_default() += u.get(k).and_then(Value::as_u64).unwrap_or(0);
                }
            }
            let content = resp["content"].as_array().cloned().unwrap_or_default();
            let call = content.iter().find(|x| x["type"] == "tool_use" && x["name"] == "submit_review").cloned();
            messages.push(json!({ "role": "assistant", "content": content }));
            let Some(call) = call else {
                messages.push(json!({ "role": "user", "content": "Please call submit_review." }));
                continue;
            };
            let mut notes = Vec::new();
            for n in call["input"]["notes"].as_array().into_iter().flatten() {
                let w = n["wire"].as_u64().unwrap_or(0) as usize;
                if w == 0 || w > b.wires.len() {
                    continue;
                }
                notes.push(ReviewNote {
                    wire: w - 1,
                    level: n["level"].as_str().unwrap_or("ok").into(),
                    note: n["note"].as_str().unwrap_or_default().into(),
                    adapter: n["adapter"]
                        .as_str()
                        .filter(|a| blocks::adapter_description(a).is_some())
                        .unwrap_or("")
                        .into(),
                });
            }
            return Ok(Review {
                notes,
                summary: call["input"]["summary"].as_str().unwrap_or_default().into(),
                usage: usage.clone(),
            });
        }
        Err(ApiError::Conflict("the assistant could not review the connections; try again".into()))
    }
    .await;
    if !usage.is_empty() {
        crate::spend::record(&state, user.id, id, "review", &served, &usage).await;
    }
    result.map(Json)
}
