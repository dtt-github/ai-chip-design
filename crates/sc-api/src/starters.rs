//! Starter designs: small, verified designs to start a project from or to add to one as a block.
//!
//! They are the design-eval tasks (`evals/design/tasks/<id>/`): each has a plain-language spec with
//! examples, and a reference design that passes those examples and a hidden randomised testbench
//! (`tests/evals.rs`). `build.rs` builds the spec, the design and the description into the server,
//! so the starters and the evals can't drift apart.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use axum::Json;
use axum::extract::{Path, State};
use sc_git::{Author, Change};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::AppState;
use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::projects::{Role, authorize};
use crate::spec::{Port, SPEC_FILE, Spec};

include!(concat!(env!("OUT_DIR"), "/starters.rs"));

/// Folder a starter added to an existing project goes in; the Blocks view lists it as a starter.
pub const DIR: &str = "starters";

#[derive(Debug, Clone, Serialize)]
pub struct Starter {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub category: String,
    /// The design's module name.
    pub top: String,
    #[serde(skip)]
    pub spec: Spec,
    #[serde(skip)]
    pub rtl: String,
}

/// What the gallery shows: the starter, its connections and settings, and how many examples it has.
#[derive(Debug, Serialize)]
pub struct StarterView {
    #[serde(flatten)]
    pub starter: Starter,
    pub ports: Vec<Port>,
    pub params: Vec<crate::spec::Param>,
    pub clock_mhz: Option<f64>,
    /// The clock and reset connections, which the gallery leaves out when listing connections.
    pub clock: Option<String>,
    pub reset: Option<String>,
    pub examples: usize,
}

/// Every starter, in the order the gallery shows them (by category, then title).
pub fn all() -> &'static [Starter] {
    static ALL: OnceLock<Vec<Starter>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut v: Vec<Starter> = RAW
            .iter()
            .map(|(id, task, spec, rtl)| {
                let task: Value = serde_json::from_str(task).expect("starter task.json");
                let spec: Spec = serde_json::from_str(spec).expect("starter spec.json");
                let s = &task["starter"];
                Starter {
                    id: id.to_string(),
                    title: s["title"].as_str().unwrap_or(id).to_string(),
                    summary: s["summary"].as_str().unwrap_or_default().to_string(),
                    category: s["category"].as_str().unwrap_or("Other").to_string(),
                    top: spec.top.clone(),
                    spec,
                    rtl: rtl.to_string(),
                }
            })
            .collect();
        let rank = |c: &str| ["Basics", "Inputs", "Data", "Communication", "Control"].iter().position(|x| *x == c);
        v.sort_by(|a, b| (rank(&a.category), &a.title).cmp(&(rank(&b.category), &b.title)));
        v
    })
}

pub fn get(id: &str) -> Option<&'static Starter> {
    all().iter().find(|s| s.id == id)
}

impl Starter {
    /// The design's source, marked as a SiliconCommons starter (it keeps its Apache-2.0 header).
    pub fn source(&self) -> String {
        let (first, rest) = self.rtl.split_once('\n').unwrap_or((&self.rtl, ""));
        let rest = rest.trim_start_matches(&format!("// Reference design for eval task \"{}\".\n", self.id));
        format!(
            "{first}\n// Starter design \"{}\" from SiliconCommons: {}\n// Verified against its examples and a randomised reference testbench.\n{rest}",
            self.title, self.summary
        )
    }

    /// Every file of a new project started from this design: the design, its spec and example
    /// testbenches, a flow config and a README.
    pub fn project_files(&self, name: &str) -> Vec<(String, Vec<u8>)> {
        let top = &self.top;
        let mut files = vec![
            (
                "README.md".to_string(),
                format!(
                    "# {name}\n\n\
                     Started from the **{}** starter design: {}\n\n\
                     - `spec.json` - what the design does, with the examples that are its tests (Design view)\n\
                     - `src/{top}.v` - the design, verified against those examples and a randomised reference testbench\n\
                     - `tb/examples/` - testbenches generated from the examples\n\
                     - `config.json` - LibreLane flow configuration for the IHP SG13G2 130 nm open PDK\n\n\
                     Change it in the Design view: describe what should be different, or ask the agent.\n",
                    self.title, self.summary
                ),
            ),
            (format!("src/{top}.v"), self.source()),
            (SPEC_FILE.to_string(), serde_json::to_string_pretty(&self.spec).expect("spec serialises") + "\n"),
            ("config.json".to_string(), self.config()),
        ]
        .into_iter()
        .map(|(p, c)| (p, c.into_bytes()))
        .collect::<Vec<_>>();
        files.extend(self.spec.testbenches().into_iter().map(|(p, c)| (p, c.into_bytes())));
        files
    }

    fn config(&self) -> String {
        let mut cfg = json!({
            "DESIGN_NAME": self.top,
            "VERILOG_FILES": format!("dir::src/{}.v", self.top),
            // Leave room for hold buffers; tiny designs fail detailed placement when packed tighter.
            "FP_CORE_UTIL": 40,
        });
        if let Some(c) = &self.spec.clock {
            cfg["CLOCK_PORT"] = json!(c.port);
            cfg["CLOCK_PERIOD"] = json!((1000.0 / c.mhz * 100.0).round() / 100.0);
        }
        serde_json::to_string_pretty(&cfg).expect("static json") + "\n"
    }
}

// ---- endpoints -----------------------------------------------------------------

pub async fn list(_user: AuthUser) -> Json<Vec<StarterView>> {
    Json(
        all()
            .iter()
            .map(|s| StarterView {
                starter: s.clone(),
                ports: s.spec.ports.clone(),
                params: s.spec.params.clone(),
                clock_mhz: s.spec.clock.as_ref().map(|c| c.mhz),
                clock: s.spec.clock.as_ref().map(|c| c.port.clone()),
                reset: s.spec.reset.as_ref().map(|r| r.port.clone()),
                examples: s.spec.examples.len(),
            })
            .collect(),
    )
}

/// Adds a starter's design to a project as `starters/<module>.v`, listed in every flow config's
/// `VERILOG_FILES`, so it can be placed in the Blocks view. One commit by the person adding it.
pub async fn add(
    State(state): State<AppState>,
    user: AuthUser,
    Path((project, id)): Path<(Uuid, String)>,
) -> ApiResult<Json<Value>> {
    authorize(&state, &user, project, Role::Editor).await?;
    let Some(st) = get(&id) else {
        return Err(ApiError::NotFound(format!("no starter design '{id}'")));
    };
    let head = crate::workspace::head(state.repos.clone(), project)
        .await?
        .ok_or_else(|| ApiError::Conflict("the project has no commits yet".into()))?;
    let files: BTreeMap<String, String> =
        crate::workspace::text_files(state.repos.clone(), project, head.clone(), 1 << 20).await?.into_iter().collect();
    let path = format!("{DIR}/{}.v", st.top);
    if files.contains_key(&path) {
        return Err(ApiError::Conflict(format!("{path} is already in the project")));
    }
    // A module of the same name elsewhere would clash.
    let hdl: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| crate::gate::is_hdl(p) && !p.starts_with("tb/"))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    if let Ok((modules, _)) = sc_sim::parse(&hdl)
        && let Some(m) = modules.iter().find(|m| m.name == st.top)
    {
        return Err(ApiError::Conflict(format!(
            "the project already has a module called {} (in {}); rename it to add this starter",
            st.top, m.file
        )));
    }
    let mut changes = vec![Change::Write { path: path.clone(), content: st.source().into_bytes() }];
    let entry = format!("dir::{path}");
    for (p, text) in &files {
        let base = p.rsplit('/').next().unwrap_or(p);
        if !(base.starts_with("config") && base.ends_with(".json")) {
            continue;
        }
        let Ok(mut cfg) = serde_json::from_str::<Value>(text) else { continue };
        let mut list: Vec<String> = match &cfg["VERILOG_FILES"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            _ => continue,
        };
        if !list.contains(&entry) {
            list.push(entry.clone());
            cfg["VERILOG_FILES"] = json!(list);
            let content = serde_json::to_string_pretty(&cfg).map_err(anyhow::Error::from)? + "\n";
            changes.push(Change::Write { path: p.clone(), content: content.into_bytes() });
        }
    }
    let message = format!(
        "Add the {} starter design\n\n{}\nModule {} in {path}, added to VERILOG_FILES; place it in the Blocks view.\n",
        st.title, st.summary, st.top
    );
    let author = Author { name: user.display_name.clone(), email: user.email.clone() };
    let repos = state.repos.clone();
    let info = tokio::task::spawn_blocking(move || {
        repos.open(&project.to_string())?.commit(changes, &message, &author, Some(&head))
    })
    .await??;
    sqlx::query("UPDATE projects SET updated_at = now() WHERE id = $1").bind(project).execute(&state.db).await?;
    state.collab.committed(&state, project, &info.id).await;
    Ok(Json(json!({ "commit": info.id, "path": path, "module": st.top })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_is_complete_and_passes_its_examples() {
        assert_eq!(all().len(), 16, "one starter per design-eval task");
        for st in all() {
            assert!(!st.title.is_empty() && !st.summary.is_empty(), "{}", st.id);
            assert_eq!(st.spec.check(None), Vec::<String>::new(), "{}", st.id);
            assert!(st.source().starts_with("// SPDX-License-Identifier: Apache-2.0\n// Starter design"), "{}", st.id);
            assert!(!st.source().contains("eval task"), "{}", st.id);
            let files: BTreeMap<String, String> = st
                .project_files(&st.title)
                .into_iter()
                .map(|(p, c)| (p, String::from_utf8(c).expect("text")))
                .collect();
            let cfg: Value = serde_json::from_str(&files["config.json"]).expect("config");
            assert_eq!(cfg["DESIGN_NAME"], st.top.as_str());
            assert_eq!(cfg["CLOCK_PORT"].is_null(), st.spec.clock.is_none(), "{}", st.id);
            let r = crate::gate::run(&files);
            assert!(r.passed && r.runs.len() == st.spec.examples.len(), "{}: {}", st.id, r.summary);
        }
    }
}
