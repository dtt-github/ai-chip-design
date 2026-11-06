//! HTTP side of intent-first design ([`crate::spec`]): read and save the spec
//! (saving regenerates the example testbenches in the same commit), run the
//! examples, and let the assistant draft or revise the spec from plain language.

use std::collections::BTreeMap;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use wafer_git::{Author, Change};

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::gate::{self, TestbenchRun};
use crate::projects::{Role, authorize};
use crate::spec::{self, EXAMPLES_DIR, SPEC_FILE, Spec};

const MAX_TEXT: usize = 4 << 20;
/// HDL sent to the assistant as context, at most.
const CONTEXT_BYTES: usize = 60_000;
const DRAFT_TURNS: usize = 4;

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

/// The chip from the Blocks view, if the project has one (examples may target it).
pub(crate) fn chip_of(files: &BTreeMap<String, String>) -> Option<crate::blocks::Blocks> {
    files.get(crate::blocks::BLOCKS_FILE).and_then(|t| serde_json::from_str(t).ok())
}

fn parse_spec(text: &str) -> ApiResult<Spec> {
    serde_json::from_str(text).map_err(|e| ApiError::Conflict(format!("{SPEC_FILE} can't be read: {e}")))
}

#[derive(Debug, Serialize)]
pub struct SpecView {
    pub head: String,
    /// `None` when the project has no spec yet.
    pub spec: Option<Spec>,
    /// Problems that keep the examples from being compiled.
    pub problems: Vec<String>,
    /// Whether `tb/examples/` matches the spec (someone may have edited a generated file).
    pub in_sync: bool,
    /// The flow config's top and clock, as defaults for a new spec.
    pub design_name: Option<String>,
    pub clock_mhz: Option<f64>,
    /// The chip from the Blocks view, which examples can target instead of the design.
    pub chip: Option<crate::blocks::Blocks>,
}

pub async fn get(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<SpecView>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let head = head(&state, id).await?;
    let files = files_at(&state, id, &head).await?;
    let config: Option<Value> = files.get("config.json").and_then(|c| serde_json::from_str(c).ok());
    let design_name = config.as_ref().and_then(|c| c["DESIGN_NAME"].as_str().map(String::from));
    let clock_mhz = config.as_ref().and_then(|c| c["CLOCK_PERIOD"].as_f64()).filter(|p| *p > 0.0).map(|p| 1000.0 / p);
    let chip = chip_of(&files);
    let (spec, problems, in_sync) = match files.get(SPEC_FILE) {
        None => (None, vec![], true),
        Some(text) => {
            let s = parse_spec(text)?;
            let problems = s.check(chip.as_ref());
            let in_sync =
                problems.is_empty() && s.testbenches_for(chip.as_ref()).iter().all(|(p, c)| files.get(p) == Some(c));
            (Some(s), problems, in_sync)
        }
    };
    Ok(Json(SpecView { head, spec, problems, in_sync, design_name, clock_mhz, chip }))
}

#[derive(Debug, Deserialize)]
pub struct SaveReq {
    pub spec: Spec,
    /// The head the edit is based on; the save is refused if the branch moved.
    pub parent: String,
    pub message: Option<String>,
}

/// Commits `spec.json` and the regenerated example testbenches together.
pub async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SaveReq>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let files = files_at(&state, id, &req.parent).await?;
    let chip = chip_of(&files);
    let problems = req.spec.check(chip.as_ref());
    if !problems.is_empty() {
        return Err(ApiError::BadRequest(problems.join("\n")));
    }
    let tbs = req.spec.testbenches_for(chip.as_ref());
    let mut changes = Vec::new();
    for (path, content) in &tbs {
        match files.get(path) {
            Some(old) if old == content => {}
            Some(old) if !spec::is_generated(old) => {
                return Err(ApiError::Conflict(format!(
                    "{path} exists and wasn't generated from the spec; rename or remove it"
                )));
            }
            _ => changes.push(Change::Write { path: path.clone(), content: content.clone().into_bytes() }),
        }
    }
    // Examples that were removed take their testbench with them.
    for (path, content) in &files {
        if path.starts_with(&format!("{EXAMPLES_DIR}/")) && !tbs.contains_key(path) && spec::is_generated(content) {
            changes.push(Change::Delete { path: path.clone() });
        }
    }
    let text = serde_json::to_string_pretty(&req.spec).map_err(anyhow::Error::from)? + "\n";
    if files.get(SPEC_FILE) != Some(&text) {
        changes.push(Change::Write { path: SPEC_FILE.into(), content: text.into_bytes() });
    }
    if changes.is_empty() {
        return Ok(Json(json!({ "commit": req.parent, "changed": false })));
    }
    let message = req.message.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| {
        format!(
            "Update the spec ({} example{})",
            req.spec.examples.len(),
            if req.spec.examples.len() == 1 { "" } else { "s" }
        )
    });
    let message = format!("{message}\n\nRegenerated {EXAMPLES_DIR}/ from {SPEC_FILE}.\n");
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

#[derive(Debug, Serialize)]
pub struct ExampleResult {
    pub id: String,
    pub title: String,
    /// `pass`, `fail`, `no_verdict`, `error`, or `missing` (no testbench at this commit).
    pub verdict: String,
    pub reason: Option<String>,
    pub log: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub commit: String,
    pub examples: Vec<ExampleResult>,
    /// The project's other testbenches, run alongside.
    pub others: Vec<TestbenchRun>,
    /// What the examples exercise in the design; `None` without examples or branches.
    pub coverage: Option<crate::coverage::Coverage>,
    /// State machines in the design, each transition marked taken by an example or not.
    pub machines: Vec<crate::fsm::Machine>,
    pub engine: String,
}

/// Runs every testbench at the branch tip and reports the examples by name (editors: it costs server time).
pub async fn run(State(state): State<AppState>, user: AuthUser, Path(id): Path<Uuid>) -> ApiResult<Json<RunResult>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let commit = head(&state, id).await?;
    let files = files_at(&state, id, &commit).await?;
    let spec = match files.get(SPEC_FILE) {
        Some(t) => parse_spec(t)?,
        None => return Err(ApiError::NotFound("this project has no spec yet".into())),
    };
    let mut machines = machines_in(&files);
    let report = tokio::task::spawn_blocking(move || gate::run(&files)).await?;
    let example_tops: Vec<String> = spec.examples.iter().map(|e| spec::tb_name(&e.id)).collect();
    let example_runs: Vec<&TestbenchRun> = report.runs.iter().filter(|r| example_tops.contains(&r.top)).collect();
    let coverage = crate::coverage::of(&example_runs);
    crate::fsm::mark(&mut machines, &example_runs);
    let mut runs: BTreeMap<String, TestbenchRun> = report.runs.into_iter().map(|r| (r.top.clone(), r)).collect();
    let examples = spec
        .examples
        .iter()
        .map(|e| match runs.remove(&spec::tb_name(&e.id)) {
            Some(r) => ExampleResult {
                id: e.id.clone(),
                title: e.title.clone(),
                verdict: r.verdict,
                reason: r.reason,
                log: r.log,
            },
            None => ExampleResult {
                id: e.id.clone(),
                title: e.title.clone(),
                verdict: if report.summary.contains("does not parse") { "error".into() } else { "missing".into() },
                reason: Some(if report.summary.contains("does not parse") {
                    report.summary.clone()
                } else {
                    "save the spec to generate its testbench".into()
                }),
                log: vec![],
            },
        })
        .collect();
    Ok(Json(RunResult {
        commit,
        examples,
        others: runs.into_values().collect(),
        coverage,
        machines,
        engine: report.engine,
    }))
}

/// How well a set of files is tested: examples passing, coverage, state machines (moves marked).
#[derive(Debug, Serialize)]
pub struct Quality {
    pub examples: usize,
    pub passing: usize,
    pub coverage: Option<crate::coverage::Coverage>,
    pub machines: Vec<crate::fsm::Machine>,
}

/// [`Quality`] of `files` (spec, design and example testbenches). Blocks; runs the gate.
pub(crate) fn quality(files: &BTreeMap<String, String>) -> Quality {
    let tops: Vec<String> = files
        .get(SPEC_FILE)
        .and_then(|t| serde_json::from_str::<Spec>(t).ok())
        .map(|s| s.examples.iter().map(|e| spec::tb_name(&e.id)).collect())
        .unwrap_or_default();
    let mut machines = machines_in(files);
    let report = gate::run(files);
    let runs: Vec<&TestbenchRun> = report.runs.iter().filter(|r| tops.contains(&r.top)).collect();
    crate::fsm::mark(&mut machines, &runs);
    Quality {
        examples: tops.len(),
        passing: runs.iter().filter(|r| r.verdict == "pass").count(),
        coverage: crate::coverage::of(&runs),
        machines,
    }
}

/// The design's state machines (`crate::fsm`), or none if the design doesn't parse.
fn machines_in(files: &BTreeMap<String, String>) -> Vec<crate::fsm::Machine> {
    let hdl: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| gate::is_hdl(p) && !p.starts_with("tb/"))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    wafer_sim::parse(&hdl).map(|(m, _)| crate::fsm::find(&m)).unwrap_or_default()
}

/// Planted mistakes tried per check, and how long a check may take.
const MAX_MISTAKES: usize = 40;
const MISTAKES_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// "Would your examples catch a mistake?" (`crate::mistakes`), at the branch tip (editors: it costs
/// server time, one simulation slot like the gate).
pub async fn mistakes(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<crate::mistakes::Report>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let commit = head(&state, id).await?;
    let files = files_at(&state, id, &commit).await?;
    let spec = match files.get(SPEC_FILE) {
        Some(t) => parse_spec(t)?,
        None => return Err(ApiError::NotFound("this project has no spec yet".into())),
    };
    let tops: Vec<String> = spec.examples.iter().map(|e| spec::tb_name(&e.id)).collect();
    let report = tokio::task::spawn_blocking(move || {
        gate::with_slot(|| {
            wafer_sim::on_big_stack(|| crate::mistakes::check(&files, &tops, MAX_MISTAKES, MISTAKES_BUDGET))
        })
    })
    .await?;
    report.map(Json).map_err(ApiError::Conflict)
}

/// How long a search for an example may take (shrinking the one found may take half again).
const FIND_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// "Find an example" (`crate::search`) for a coverage gap or a planted mistake that slipped
/// through, at the branch tip. Returns a draft example; nothing is saved. Editors: it costs server
/// time.
pub async fn find_example(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(target): Json<crate::search::Target>,
) -> ApiResult<Json<crate::search::Found>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let commit = head(&state, id).await?;
    let files = files_at(&state, id, &commit).await?;
    let spec = match files.get(SPEC_FILE) {
        Some(t) => parse_spec(t)?,
        None => return Err(ApiError::NotFound("this project has no spec yet".into())),
    };
    let found = tokio::task::spawn_blocking(move || {
        gate::with_slot(|| wafer_sim::on_big_stack(|| crate::search::find(&spec, &files, &target, FIND_BUDGET)))
    })
    .await?;
    found.map(Json).map_err(ApiError::Conflict)
}

/// How long "Try a small fix" may search.
const FIX_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Deserialize, Default)]
pub struct SmallFixReq {
    /// Lines to try first (those "Why not?" pointed at).
    #[serde(default)]
    pub focus: Vec<crate::fixes::Focus>,
}

/// "Try a small fix" (`crate::fixes`) at the branch tip: small repairs after which every example
/// passes, each with the edit to make. Nothing is saved. Editors: it costs server time.
pub async fn small_fix(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SmallFixReq>,
) -> ApiResult<Json<crate::fixes::Fixes>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let commit = head(&state, id).await?;
    let files = files_at(&state, id, &commit).await?;
    let spec = match files.get(SPEC_FILE) {
        Some(t) => parse_spec(t)?,
        None => return Err(ApiError::NotFound("this project has no spec yet".into())),
    };
    let tops: Vec<String> = spec.examples.iter().map(|e| spec::tb_name(&e.id)).collect();
    let focus: Vec<crate::fixes::Focus> = req.focus.into_iter().take(64).collect();
    let found = tokio::task::spawn_blocking(move || {
        gate::with_slot(|| wafer_sim::on_big_stack(|| crate::fixes::find(&files, &tops, &focus, FIX_BUDGET)))
    })
    .await?;
    found.map(Json).map_err(ApiError::Conflict)
}

#[derive(Debug, Deserialize)]
pub struct TestbenchReq {
    /// The spec being edited (unsaved edits included).
    pub spec: Spec,
    pub example: spec::Example,
}

/// The testbench one example compiles to, without saving anything: the "Try it" playground
/// simulates its session this way, so it runs exactly as the example will once saved.
pub async fn testbench(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<TestbenchReq>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &user, id, Role::Viewer).await?;
    let chip = match req.example.target.as_deref().filter(|t| *t != req.spec.top) {
        Some(_) => chip_of(&files_at(&state, id, &head(&state, id).await?).await?),
        None => None,
    };
    let tb = spec::tb_name(&req.example.id);
    let one = Spec { examples: vec![req.example], ..req.spec };
    let problems = one.check(chip.as_ref());
    if !problems.is_empty() {
        return Err(ApiError::BadRequest(problems.join("; ")));
    }
    let (path, source) = one.testbenches_for(chip.as_ref()).into_iter().next().expect("one example");
    Ok(Json(json!({ "path": path, "top": tb, "source": source })))
}

// ---- drafting with the assistant -----------------------------------------------

const DRAFT_SYSTEM: &str = "You are the design assistant in Wafersmith, a platform where engineers \
who are not Verilog or chip-design experts design digital chips (IHP SG13G2 130 nm open PDK). You keep the \
project's spec: a plain-language description of what the design does, plus worked examples that become its \
tests. Another agent writes the Verilog later; a change is only accepted when every example passes, so the \
examples must be exactly right.

Write for a non-expert: connection meanings and example titles in plain words, units where they matter \
(\"50 MHz\", \"80 clock cycles\"). Keep Verilog jargon out of the prose.

The spec:
- top: the top module's name (a Verilog identifier). Keep the existing one unless asked.
- clock: the clock input and its frequency (port \"\" if the design has no clock); reset: the reset input and \
whether 0 resets (port \"\" if none).
- ports: every connection of the top module: name, dir \"in\"/\"out\", width in bits (1-64), meaning. Include \
clock and reset. When the project already has RTL for the top module, match its names unless the request \
changes them.
- params: settings (Verilog parameters) with their default value and meaning.
- decisions: only what the user has decided: things their own words settle, and their answers to questions. \
Keep existing ones. Never record a choice of your own as a decision, however reasonable.
- questions: every behaviour the user hasn't settled that an example would depend on: how many cycles until \
something happens, what an output does right after reset, what happens on edge cases or conflicting inputs, \
whether an input from outside (a button, a sensor, another clock) is already in step with the clock or needs a \
synchronizer, and whether a button needs debouncing (both change the timing). \
Each with 2-3 short answer options, the one you recommend FIRST. Don't ask about things that don't change \
behaviour (names, coding style). The user is not an expert: phrase questions in plain words. When the \
instruction answers a question, move it to decisions with that answer and add or update the examples.
- examples: 3-8 short, focused examples covering the main behaviour and the important edge cases, but only \
behaviour that is decided: an example must not assume an answer to an open question (it comes once the \
question is answered). Before submitting, check each example against each open question: would a \
different answer change a step or an expected value (for example an output's value right after reset, or an \
exact cycle count)? An example of the very situation a question is about (a pulse shorter or longer than N when \
you asked about pulse lengths) always depends on it. List those questions in the example's `assumes`; such \
examples must be left out or rewritten until the question is answered. Always write at least one example: \
while questions are open, write ones that hold for every answer, for instance with wait_until and a generous \
limit instead of an exact count, or an input held long enough for any delay or debouncing to pass. Keep the ids of existing examples. id: lowercase letters, digits, _. params: overrides \
for that example (e.g. PARITY=1).

If the project has a chip (blocks.json), examples can test the whole chip instead of the design: set target \
to the chip's name; their steps then use the chip's pins (with blocks.json's clock and reset), and the timing \
includes everything between the pins and the design (for example 2 extra cycles through a synchronizer). Keep \
the design's own examples; when asked for chip examples, add a few that check the chip's main behaviour end to \
end. Chip examples have no params.

Example steps run on the falling clock edge; the design samples inputs on the rising edge. Before step 1 all \
inputs are 0 and reset is held for 2 cycles, then released. Ops (unused fields: \"\" or 0):
- set: drive input `signal` to `value` now (no time passes).
- wait: let `cycles` clock cycles pass.
- wait_until: wait until `signal` = `value`, failing after `cycles` cycles.
- expect: `signal` must equal `value` now.
- expect_for: `signal` must equal `value` for the next `cycles` cycles; time moves past them.
Values are numbers: 1, 0x55, 0b1010. A pulse is set 1, wait 1, set 0. Count cycles explicitly; off-by-one \
mistakes are the most common error. Call the time of step 1 cycle 0; wait N and expect_for N move time N cycles \
on, set and expect don't. After `set start 1; wait 1` (cycle 1) the design has seen start on one rising edge, and \
a register it updated on that edge shows the new value. Something that lasts exactly N cycles from cycle c is \
still on at cycle c+N-1 and off at c+N; after K shifts a bit has moved K places. For every expected value, \
write down the cycle and why, then recount before submitting. The simulator stops after 50 µs (2,500 cycles at \
50 MHz): when the real setting is long (a 10 ms debounce), give the example a small value in its params. Prefer wait_until on an output over \
counting exact latency when the request doesn't fix the latency; use exact counts when it does (\"each bit lasts \
8 clocks\"). Add a short note to steps where it helps (\"start bit\", \"bit 0\").

Finish by calling submit_spec with the whole spec and a one-paragraph note to the user on what you changed and \
why, and which questions you need answered. If it comes back with problems, fix them and call it again.";

fn draft_tool() -> Value {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let obj = |props: Value, req: &[&str]| json!({ "type": "object", "properties": props, "required": req, "additionalProperties": false });
    let step = obj(
        json!({
            "op": { "type": "string", "enum": ["set", "wait", "wait_until", "expect", "expect_for"] },
            "signal": s("Connection name, or \"\" for wait"),
            "value": s("Number such as 1, 0x55 or 0b1010, or \"\" for wait"),
            "cycles": { "type": "integer", "description": "Clock cycles for wait, wait_until (timeout) and expect_for; 0 otherwise" },
            "note": s("Optional plain-language note, or \"\"")
        }),
        &["op", "signal", "value", "cycles", "note"],
    );
    let example = obj(
        json!({
            "id": s("lowercase letters, digits and _"),
            "title": s("Short plain-language title"),
            "given": s("The situation, in plain words"),
            "expect": s("What should happen, in plain words"),
            "params": { "type": "array", "items": obj(json!({ "name": s("Setting name"), "value": s("Number") }), &["name", "value"]) },
            "steps": { "type": "array", "items": step },
            "target": s("\"\" for the design (the spec's top). The chip's name (blocks.json top) to test the whole chip through its pins."),
            "assumes": { "type": "array", "items": { "type": "string" }, "description": "Every question in `questions` (exact text) whose answer would change this example's steps or expected values. Empty only if the example holds whatever the answers are. The server refuses examples that depend on an open question." }
        }),
        &["id", "title", "given", "expect", "params", "steps", "target", "assumes"],
    );
    let spec = obj(
        json!({
            "intent": s("The request in the user's words (keep theirs; refine only if asked)"),
            "top": s("Top module name"),
            "clock": obj(json!({ "port": s("Clock input, or \"\""), "mhz": { "type": "number" } }), &["port", "mhz"]),
            "reset": obj(json!({ "port": s("Reset input, or \"\""), "active_low": { "type": "boolean" } }), &["port", "active_low"]),
            "ports": { "type": "array", "items": obj(json!({
                "name": s("Connection name"), "dir": { "type": "string", "enum": ["in", "out"] },
                "width": { "type": "integer" }, "meaning": s("Plain-language meaning")
            }), &["name", "dir", "width", "meaning"]) },
            "params": { "type": "array", "items": obj(json!({ "name": s("Parameter"), "value": s("Default"), "meaning": s("Plain-language meaning") }), &["name", "value", "meaning"]) },
            "decisions": { "type": "array", "items": obj(json!({ "question": s("Question"), "answer": s("Answer") }), &["question", "answer"]) },
            "questions": { "type": "array", "items": obj(json!({ "question": s("Open question"), "options": { "type": "array", "items": { "type": "string" } } }), &["question", "options"]) },
            "examples": { "type": "array", "items": example }
        }),
        &["intent", "top", "clock", "reset", "ports", "params", "decisions", "questions", "examples"],
    );
    json!([{
        "name": "submit_spec",
        "description": "Submit the complete updated spec. The server checks it and compiles each example into a testbench; any problems come back as an error to fix.",
        "strict": true,
        "input_schema": obj(json!({ "spec": spec, "note": s("One paragraph to the user: what you changed and why, and what you need from them") }), &["spec", "note"])
    }])
}

/// Examples that say they depend on a question that is still open.
fn assumes_open(raw: &Value, spec: &Spec) -> Vec<String> {
    let norm = |q: &str| q.trim().trim_end_matches('?').trim().to_lowercase();
    let open: Vec<String> = spec.questions.iter().map(|q| norm(&q.question)).collect();
    let mut out = Vec::new();
    for e in raw["examples"].as_array().into_iter().flatten() {
        for q in e["assumes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if open.contains(&norm(q)) {
                out.push(format!(
                    "example \"{}\" depends on the open question \"{q}\": leave it out until the user answers, or rewrite it so it holds for every answer",
                    e["title"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    out
}

/// The tool's `spec` as a JSON object. Models sometimes send the object as a JSON string; anything
/// else goes back to the model as a problem instead of failing the request.
fn spec_value(v: &Value) -> Result<Value, String> {
    let v = match v {
        Value::String(s) => serde_json::from_str(s).map_err(|_| "spec must be a JSON object, not text".to_string())?,
        other => other.clone(),
    };
    if v.is_object() { Ok(v) } else { Err("spec must be a JSON object".into()) }
}

/// The tool's input (clock and reset as possibly-empty objects) as a [`Spec`].
fn from_tool(v: &Value) -> Result<Spec, String> {
    let mut v = spec_value(v)?;
    for key in ["clock", "reset"] {
        if v[key]["port"].as_str().unwrap_or_default().is_empty() {
            v[key] = Value::Null;
        }
    }
    v["version"] = json!(1);
    for e in v["examples"].as_array_mut().into_iter().flatten() {
        if e["target"].as_str().is_none_or(str::is_empty) {
            e.as_object_mut().map(|o| o.remove("target"));
        }
    }
    serde_json::from_value(v).map_err(|e| format!("the spec doesn't have the right shape: {e}"))
}

#[derive(Debug, Deserialize)]
pub struct DraftReq {
    /// What to do: the first description, an answer to a question, a new example in words...
    pub instruction: String,
    /// The spec being edited (unsaved edits included); defaults to the saved one.
    pub spec: Option<Spec>,
}

#[derive(Debug, Serialize)]
pub struct DraftResult {
    pub spec: Spec,
    pub note: String,
    pub model: String,
    pub usage: BTreeMap<String, u64>,
    /// What the draft cost, priced for the model that served it.
    pub usd: f64,
    pub elapsed_ms: u64,
}

/// Asks the assistant to write or revise the spec. Nothing is saved: the person reviews it first.
pub async fn draft(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<DraftReq>,
) -> ApiResult<Json<DraftResult>> {
    authorize(&state, &user, id, Role::Editor).await?;
    let Some(claude) = state.agent.claude.clone() else {
        return Err(ApiError::Unavailable("the assistant is not configured on this server (no API key)".into()));
    };
    crate::spend::check(&state, &user).await?;
    let instruction = req.instruction.trim();
    if instruction.is_empty() || instruction.len() > 20_000 {
        return Err(ApiError::BadRequest("say what the design should do (up to 20,000 characters)".into()));
    }
    let head = head(&state, id).await?;
    let files = files_at(&state, id, &head).await?;
    let chip = chip_of(&files);
    let answering = instruction.starts_with("Answer to the open question");
    let current = match req.spec {
        Some(s) => Some(s),
        None => files.get(SPEC_FILE).map(|t| parse_spec(t)).transpose()?,
    };

    // Context: the flow config and the design's own HDL (not testbenches), within a budget.
    let mut context = String::new();
    if let Some(c) = files.get("config.json") {
        context.push_str(&format!("--- config.json ---\n{c}\n"));
    }
    if let Some(c) = files.get(crate::blocks::BLOCKS_FILE) {
        context.push_str(&format!(
            "--- blocks.json (the chip, built from blocks in the Blocks view; its generated top is src/<top>.v) ---\n{c}\n"
        ));
    }
    for (p, c) in files.iter().filter(|(p, _)| {
        p.ends_with(".v") && !p.starts_with("tb/") && !p.contains("/tb_") && !p.starts_with(&format!("{EXAMPLES_DIR}/"))
    }) {
        if context.len() + c.len() > CONTEXT_BYTES {
            context.push_str(&format!("--- {p} (left out: too large for context) ---\n"));
            continue;
        }
        context.push_str(&format!("--- {p} ---\n{c}\n"));
    }
    let current_text = match &current {
        Some(s) => serde_json::to_string_pretty(s).map_err(anyhow::Error::from)?,
        None => "(none yet: write the first spec)".into(),
    };
    let first = format!(
        "Project files for context:\n{context}\nCurrent spec:\n{current_text}\n\nInstruction from the user:\n{instruction}"
    );

    let tools = draft_tool();
    let mut messages = vec![json!({ "role": "user", "content": first })];
    let mut usage: BTreeMap<String, u64> = BTreeMap::new();
    let started = Instant::now();
    // Decisions the user already had, and questions they were asked: anything else new is the model deciding.
    let known: Vec<String> = current
        .iter()
        .flat_map(|s| {
            s.decisions.iter().map(|d| d.question.clone()).chain(s.questions.iter().map(|q| q.question.clone()))
        })
        .collect();
    let mut pushed_back = false;
    let mut served = claude.cfg.model.clone();
    let mut asked_for_examples = false;
    let result: ApiResult<DraftResult> = async {
    for _ in 0..DRAFT_TURNS {
        let resp =
            claude.create(DRAFT_SYSTEM, &tools, &messages).await.map_err(|e| ApiError::Unavailable(e.to_string()))?;
        let model = resp["model"].as_str().unwrap_or_default().to_string();
        if !model.is_empty() {
            served = model.clone();
        }
        if let Some(u) = resp["usage"].as_object() {
            for k in ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"] {
                *usage.entry(k.to_string()).or_default() += u.get(k).and_then(Value::as_u64).unwrap_or(0);
            }
        }
        let content = resp["content"].as_array().cloned().unwrap_or_default();
        match resp["stop_reason"].as_str().unwrap_or_default() {
            "refusal" => return Err(ApiError::Conflict("the assistant declined this request".into())),
            "max_tokens" => {
                return Err(ApiError::Conflict("the assistant's answer was cut off; try a smaller request".into()));
            }
            _ => {}
        }
        let call = content.iter().find(|b| b["type"] == "tool_use" && b["name"] == "submit_spec").cloned();
        messages.push(json!({ "role": "assistant", "content": content }));
        let Some(call) = call else {
            messages.push(json!({ "role": "user", "content": "Please call submit_spec with the complete spec." }));
            continue;
        };
        let problems = match from_tool(&call["input"]["spec"]) {
            Err(e) => vec![e],
            Ok(spec) => {
                let mut p = spec.check(chip.as_ref());
                p.extend(assumes_open(&spec_value(&call["input"]["spec"]).unwrap_or_default(), &spec));
                let invented: Vec<&str> = spec
                    .decisions
                    .iter()
                    .filter(|d| !known.iter().any(|k| k == &d.question))
                    .map(|d| d.question.as_str())
                    .collect();
                // One push back: decisions must come from the user, unless this instruction answers a question.
                if p.is_empty() && !invented.is_empty() && !answering && !pushed_back {
                    pushed_back = true;
                    p.push(format!(
                        "These decisions are new: {}. Keep one only if the user's own words settle it; otherwise make it a \
                         question (your recommended answer first) and leave out examples that depend on it",
                        invented.iter().map(|q| format!("\"{q}\"")).collect::<Vec<_>>().join("; ")
                    ));
                }
                // One push back: a spec without examples tests nothing, and some behaviour can always be checked.
                if p.is_empty() && spec.examples.is_empty() && !asked_for_examples {
                    asked_for_examples = true;
                    p.push(
                        "There are no examples. Add at least one that holds whatever the open questions' answers are: \
                         the main behaviour checked with wait_until and a generous limit instead of an exact count, or an \
                         input held long enough for any delay or debouncing to pass"
                            .into(),
                    );
                }
                if p.is_empty() {
                    tracing::info!(project = %id, examples = spec.examples.len(), ?usage, "spec drafted");
                    return Ok(DraftResult {
                        spec,
                        note: call["input"]["note"].as_str().unwrap_or_default().to_string(),
                        usd: crate::spend::usd(&model, &usage),
                        model,
                        usage: usage.clone(),
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                }
                p
            }
        };
        messages.push(json!({ "role": "user", "content": [{
            "type": "tool_result", "tool_use_id": call["id"], "is_error": true,
            "content": format!("The spec has problems; fix them and call submit_spec again:\n- {}", problems.join("\n- "))
        }] }));
    }
    Err(ApiError::Conflict("the assistant could not produce a valid spec; try rephrasing".into()))
    }
    .await;
    // Every call costs, whether or not it produced a spec.
    if !usage.is_empty() {
        crate::spend::record(&state, user.id, id, "draft", &served, &usage).await;
    }
    result.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_sent_as_text_is_read_or_refused_without_panicking() {
        let spec = json!({
            "intent": "", "top": "t", "clock": { "port": "", "mhz": 0 }, "reset": { "port": "", "active_low": false },
            "ports": [], "params": [], "decisions": [], "questions": [], "examples": []
        });
        assert_eq!(from_tool(&Value::String(spec.to_string())).expect("parsed").top, "t");
        assert!(from_tool(&json!("not json")).unwrap_err().contains("JSON object"));
        assert!(from_tool(&json!(42)).unwrap_err().contains("JSON object"));
        assert!(from_tool(&spec).is_ok());
    }
}
