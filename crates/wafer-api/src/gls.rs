//! Gate-level simulation: after a flow run succeeds, the project's testbenches
//! are run again against the synthesised netlist (Icarus Verilog with the
//! PDK's standard-cell models, zero-delay), so a run shows not only that the
//! layout is clean but that the gates still do what the RTL did.
//!
//! The netlist is built for one set of top-level parameter values (module
//! defaults plus the config's `SYNTH_PARAMETERS`). A testbench only runs
//! against it when it instantiates the design with exactly those values;
//! the others are reported as skipped, with the difference. The netlist has
//! no parameters of its own, so it gets the build's values declared as dummy
//! parameters, which lets the testbench's `#(...)` overrides elaborate.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wafer_jobs::{Backend, CancellationToken, JobSpec, Outcome};

use crate::gate;

/// Per testbench; a hung testbench is stopped after this.
const RUN_TIMEOUT_SECS: u32 = 300;
const MAX_LOG_LINES: usize = 400;
/// Next to the job's `work/`: the simulation's own work directory, created fresh by the server
/// after the flow container has exited (so nothing in it was planted by the flow), and its logs,
/// which no container mounts.
const DIR: &str = "gls";
const LOGS: &str = "gls-logs";

/// A plain Verilog identifier: safe in file names and command lines (escaped identifiers are not).
pub fn plain_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && s.len() <= 128
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlsReport {
    /// `passed` (every testbench that applies passed), `failed`, `skipped` (none applies),
    /// or `unavailable` (could not be set up; see `summary`).
    pub status: String,
    pub summary: String,
    pub design: String,
    /// Top-level parameter values the netlist was built with.
    pub params: BTreeMap<String, i64>,
    pub simulator: String,
    pub runs: Vec<GlsRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlsRun {
    pub top: String,
    /// `pass`, `fail`, `no_verdict`, `error` (did not compile) or `skipped`.
    pub verdict: String,
    pub finished: bool,
    pub reason: Option<String>,
    pub log: Vec<String>,
}

impl GlsReport {
    fn unavailable(design: &str, why: impl Into<String>) -> Self {
        GlsReport {
            status: "unavailable".into(),
            summary: why.into(),
            design: design.into(),
            params: BTreeMap::new(),
            simulator: SIMULATOR.into(),
            runs: Vec::new(),
        }
    }
}

const SIMULATOR: &str = "Icarus Verilog (zero-delay, PDK cell models)";

/// What to simulate, worked out from the sources before anything runs.
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub design: String,
    pub params: BTreeMap<String, i64>,
    /// Testbench top → project files it needs besides the netlist, or why it is skipped.
    pub benches: Vec<(String, Result<Vec<String>, String>)>,
}

/// Decides which testbenches apply to the netlist built from `config`.
pub fn plan(files: &BTreeMap<String, String>, config: &Value) -> Result<Plan, String> {
    let design = config["DESIGN_NAME"].as_str().ok_or("the flow config has no DESIGN_NAME")?.to_string();
    if !plain_ident(&design) {
        return Err(format!("DESIGN_NAME {design:?} is not a plain Verilog name"));
    }
    let mut overrides = HashMap::new();
    if let Some(list) = config.get("SYNTH_PARAMETERS").filter(|v| !v.is_null()) {
        let list = list.as_array().ok_or("SYNTH_PARAMETERS must be a list of \"NAME=VALUE\" strings")?;
        for item in list {
            let item = item.as_str().ok_or("SYNTH_PARAMETERS must be a list of \"NAME=VALUE\" strings")?;
            let (k, v) =
                item.split_once('=').ok_or_else(|| format!("SYNTH_PARAMETERS entry {item:?} is not NAME=VALUE"))?;
            let v = parse_int(v.trim()).ok_or_else(|| format!("SYNTH_PARAMETERS value {v:?} is not an integer"))?;
            overrides.insert(k.trim().to_string(), v);
        }
    }

    let hdl: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| p.rsplit_once('.').is_some_and(|(_, e)| matches!(e, "v" | "sv")))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let (modules, _) = wafer_sim::parse(&hdl).map_err(|e| format!("the sources do not parse: {e}"))?;
    let build = wafer_sim::elab::elaborate_with(&modules, &design, &overrides)
        .map_err(|e| format!("cannot elaborate {design}: {e}"))?;
    let params = build.instances[0].params.clone();

    let children: HashMap<&str, Vec<&str>> = modules.iter().map(|m| (m.name.as_str(), m.instantiated())).collect();
    let reach = |root: &str| {
        let mut seen = BTreeSet::new();
        let mut stack = vec![root.to_string()];
        while let Some(m) = stack.pop() {
            if seen.insert(m.clone()) {
                stack.extend(children.get(m.as_str()).into_iter().flatten().map(|s| s.to_string()));
            }
        }
        seen
    };
    let in_netlist = reach(&design);
    let file_of: HashMap<&str, &str> = modules.iter().map(|m| (m.name.as_str(), m.file.as_str())).collect();

    let mut benches = Vec::new();
    for top in wafer_sim::top_candidates(&modules).into_iter().filter(|t| gate::looks_like_tb(t)) {
        if !plain_ident(&top) {
            benches.push((top, Err("its name isn't a plain Verilog name, so it can't be run here".into())));
            continue;
        }
        let verdict = match wafer_sim::elab::elaborate(&modules, &top) {
            Err(e) => Err(format!("could not check its parameters: {e}")),
            Ok(d) => {
                let dut: Vec<_> = d.instances.iter().filter(|i| i.module == design).collect();
                if dut.is_empty() {
                    Err(format!("does not instantiate {design}"))
                } else if let Some(diff) = dut.iter().find_map(|i| differs(&i.params, &params)) {
                    Err(format!("{diff}, so it tests a different build"))
                } else {
                    let needed: BTreeSet<&str> = reach(&top)
                        .iter()
                        .filter(|m| !in_netlist.contains(*m))
                        .filter_map(|m| file_of.get(m.as_str()).copied())
                        .collect();
                    Ok(needed.into_iter().map(String::from).collect())
                }
            }
        };
        benches.push((top, verdict));
    }
    Ok(Plan { design, params, benches })
}

/// "sets PARITY=1 where this build has PARITY=0", or `None` when they agree.
fn differs(tb: &BTreeMap<String, i64>, build: &BTreeMap<String, i64>) -> Option<String> {
    let diffs: Vec<String> = build
        .iter()
        .filter(|(k, v)| tb.get(*k) != Some(v))
        .map(|(k, v)| format!("{k}={} where this build has {k}={v}", tb.get(k).map_or("?".into(), |x| x.to_string())))
        .collect();
    if diffs.is_empty() { None } else { Some(format!("it uses {}", diffs.join(", "))) }
}

/// Verilog integer literal: `12`, `1_000`, `8'hff`, `'b101`, `4'd9`.
fn parse_int(s: &str) -> Option<i64> {
    let s = s.replace('_', "");
    let Some((_, rest)) = s.split_once('\'') else { return s.parse().ok() };
    let rest = rest.trim_start_matches(['s', 'S']);
    let (radix, digits) = match rest.chars().next()?.to_ascii_lowercase() {
        'h' => (16, &rest[1..]),
        'b' => (2, &rest[1..]),
        'o' => (8, &rest[1..]),
        'd' => (10, &rest[1..]),
        _ => return None,
    };
    i64::from_str_radix(digits, radix).ok()
}

/// Declares the build's parameter values inside the netlist's top module, after its header.
pub fn add_params(netlist: &str, design: &str, params: &BTreeMap<String, i64>) -> Option<String> {
    let mut at = 0;
    let start = loop {
        let i = at + netlist[at..].find("module ")?;
        let after = netlist[i + 7..].trim_start();
        if after
            .strip_prefix(design)
            .is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '(' || c == ';'))
        {
            break i;
        }
        at = i + 7;
    };
    let end = start + netlist[start..].find(';')? + 1;
    let decls: String = params.iter().map(|(k, v)| format!("\n  parameter {k} = {v};")).collect();
    Some(format!("{}{decls}{}", &netlist[..end], &netlist[end..]))
}

/// Runs the plan's testbenches in the job's toolchain. `files` are the project files at the job's commit.
pub async fn run(
    backend: &dyn Backend,
    spec: &JobSpec,
    files: &BTreeMap<String, String>,
    config: &Value,
    cancel: CancellationToken,
) -> GlsReport {
    let design = config["DESIGN_NAME"].as_str().unwrap_or("").to_string();
    let (f, c) = (files.clone(), config.clone());
    let planned = tokio::task::spawn_blocking(move || wafer_sim::on_big_stack(|| plan(&f, &c))).await;
    let plan = match planned.unwrap_or_else(|e| Err(format!("planning failed: {e}"))) {
        Ok(p) => p,
        Err(e) => return GlsReport::unavailable(&design, e),
    };
    let job_dir = spec.workdir.parent().unwrap_or(&spec.workdir).to_path_buf();
    let sim_dir = job_dir.join(DIR);
    let logs = job_dir.join(LOGS);
    let netlist_rel = format!("./{design}.nl.v");
    let models = match prepare(&spec.workdir, &sim_dir, &logs, &plan, files).await {
        Ok(m) => m,
        Err(e) => return GlsReport::unavailable(&design, e),
    };
    let sim_spec = JobSpec { workdir: sim_dir, ..spec.clone() };

    let mut runs = Vec::new();
    for (top, bench) in &plan.benches {
        let needed = match bench {
            Err(why) => {
                runs.push(GlsRun {
                    top: top.clone(),
                    verdict: "skipped".into(),
                    finished: false,
                    reason: Some(why.clone()),
                    log: vec![],
                });
                continue;
            }
            Ok(n) => n,
        };
        if cancel.is_cancelled() {
            break;
        }
        let log_base = logs.join(runs.len().to_string());
        runs.push(simulate(backend, &sim_spec, &log_base, top, needed, &models, &netlist_rel, cancel.clone()).await);
    }

    let ran: Vec<&GlsRun> = runs.iter().filter(|r| r.verdict != "skipped").collect();
    let failed: Vec<&&GlsRun> = ran.iter().filter(|r| r.verdict != "pass").collect();
    let skipped = runs.len() - ran.len();
    let (status, mut summary) = if ran.is_empty() {
        let other_top = runs.iter().all(|r| r.reason.as_deref().is_some_and(|w| w.starts_with("does not instantiate")));
        let why = if other_top {
            format!("no testbench uses {design} itself (they test its blocks)")
        } else {
            "no testbench matches this build's parameters".to_string()
        };
        ("skipped", why)
    } else if failed.is_empty() {
        ("passed", format!("{} of {} testbenches passed on the netlist", ran.len(), ran.len()))
    } else {
        let names: Vec<String> = failed.iter().map(|r| r.top.clone()).collect();
        ("failed", format!("{} of {} testbenches failed on the netlist: {}", failed.len(), ran.len(), names.join(", ")))
    };
    if runs.is_empty() {
        summary = "no testbench found (a top-level module named tb*, *_tb or *test*)".into();
    } else if skipped > 0 && !ran.is_empty() {
        let blocks = runs
            .iter()
            .filter(|r| {
                r.verdict == "skipped" && r.reason.as_deref().is_some_and(|w| w.starts_with("does not instantiate"))
            })
            .count();
        let mut parts = Vec::new();
        if blocks > 0 {
            parts.push(format!("{blocks} test a block, not {design}"));
        }
        if skipped > blocks {
            parts.push(format!("{} use different parameters", skipped - blocks));
        }
        summary.push_str(&format!("; {skipped} skipped ({})", parts.join(", ")));
    }
    GlsReport { status: status.into(), summary, design, params: plan.params, simulator: SIMULATOR.into(), runs }
}

/// Sets up the simulation directory: the parameterised netlist and the project files the testbenches
/// need (from Git, not from the flow's directory). Returns the cell model paths (inside the container).
async fn prepare(
    work: &Path,
    sim_dir: &Path,
    logs: &Path,
    plan: &Plan,
    files: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    // The flow's outputs are read through a check that refuses symlinks and anything outside `work`.
    let run_dir = wafer_jobs::run_dir(work);
    let nl_rel = wafer_jobs::final_dir(work).join("nl").join(format!("{}.nl.v", plan.design));
    let netlist =
        wafer_jobs::read_inside(work, &nl_rel).await.map_err(|_| "the run produced no netlist".to_string())?;
    let netlist = String::from_utf8(netlist).map_err(|_| "the netlist is not text".to_string())?;
    let patched = add_params(&netlist, &plan.design, &plan.params)
        .ok_or_else(|| format!("module {} not found in the netlist", plan.design))?;

    for dir in [sim_dir, logs] {
        let _ = tokio::fs::remove_dir_all(dir).await;
        tokio::fs::create_dir_all(dir).await.map_err(|e| e.to_string())?;
    }
    tokio::fs::write(sim_dir.join(format!("{}.nl.v", plan.design)), patched).await.map_err(|e| e.to_string())?;
    let needed: BTreeSet<&String> = plan.benches.iter().filter_map(|(_, b)| b.as_ref().ok()).flatten().collect();
    for rel in needed {
        let (Some(content), Ok(rel)) = (files.get(rel), wafer_git::normalize_path(rel)) else { continue };
        let path = sim_dir.join(&rel);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
        }
        tokio::fs::write(&path, content).await.map_err(|e| e.to_string())?;
    }

    let resolved = wafer_jobs::read_inside(work, &run_dir.join("resolved.json"))
        .await
        .map_err(|_| "the run has no resolved.json")?;
    let resolved: Value = serde_json::from_slice(&resolved).map_err(|e| format!("bad resolved.json: {e}"))?;
    // Written by the flow, so only PDK paths are taken.
    let models: Vec<String> = ["CELL_VERILOG_MODELS", "EXTRA_VERILOG_MODELS"]
        .iter()
        .flat_map(|k| resolved[*k].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().map(String::from))
        .filter(|p| p.starts_with("/pdk/") && !p.contains("..") && !p.contains(char::is_whitespace))
        .collect();
    if models.is_empty() {
        return Err("the PDK lists no standard-cell Verilog models (CELL_VERILOG_MODELS)".into());
    }
    Ok(models)
}

#[allow(clippy::too_many_arguments)]
async fn simulate(
    backend: &dyn Backend,
    spec: &JobSpec,
    log_base: &Path,
    top: &str,
    needed: &[String],
    models: &[String],
    netlist_rel: &str,
    cancel: CancellationToken,
) -> GlsRun {
    let compiled = format!("./{top}.vvp");
    let compile_log = log_base.with_extension("compile.log");
    let run_log = log_base.with_extension("log");
    let mut argv: Vec<String> =
        ["iverilog", "-g2012", "-o", &compiled, "-s", top].into_iter().map(String::from).collect();
    argv.extend(models.iter().cloned());
    argv.push(netlist_rel.to_string());
    // `./` so a file name can never be read as an option.
    argv.extend(needed.iter().map(|p| format!("./{p}")));
    let result = |verdict: &str, finished, reason: Option<String>, log| GlsRun {
        top: top.into(),
        verdict: verdict.into(),
        finished,
        reason,
        log,
    };

    match backend.exec(spec, &argv, &compile_log, cancel.clone()).await {
        Ok(Outcome::Exited(0)) => {}
        Ok(Outcome::Canceled) => return result("error", false, Some("canceled".into()), vec![]),
        Ok(Outcome::TimedOut) => return result("error", false, Some("compiling took too long".into()), vec![]),
        Ok(Outcome::Exited(_)) => {
            let log = read_lines(&compile_log).await.into_iter().filter(|l| !l.contains("sorry:")).collect();
            return result("error", false, Some("the testbench does not compile against the netlist".into()), log);
        }
        Err(e) => return result("error", false, Some(e.to_string()), vec![]),
    }

    let argv: Vec<String> =
        ["timeout", &RUN_TIMEOUT_SECS.to_string(), "vvp", "-n", &compiled].into_iter().map(String::from).collect();
    let outcome = backend.exec(spec, &argv, &run_log, cancel).await;
    let raw = read_lines(&run_log).await;
    let finished = raw.iter().any(|l| l.contains("$finish called at"));
    let log: Vec<String> = raw.into_iter().filter(|l| !l.contains("$finish called at")).take(MAX_LOG_LINES).collect();
    match outcome {
        Ok(Outcome::Canceled) => result("error", false, Some("canceled".into()), log),
        Ok(Outcome::TimedOut) => result("fail", false, Some("stopped: took too long".into()), log),
        Err(e) => result("error", false, Some(e.to_string()), log),
        Ok(Outcome::Exited(124)) => {
            result("fail", false, Some(format!("did not reach $finish within {RUN_TIMEOUT_SECS} s")), log)
        }
        Ok(Outcome::Exited(_)) if !finished => result("fail", false, Some("did not reach $finish".into()), log),
        Ok(Outcome::Exited(_)) => {
            let (v, reason) = gate::verdict(&log);
            result(v, true, reason, log)
        }
    }
}

async fn read_lines(path: &Path) -> Vec<String> {
    tokio::fs::read_to_string(path).await.unwrap_or_default().lines().map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DUT: &str = "module dut #(parameter W = 4, parameter P = 0) (input wire a); endmodule\n";

    fn files(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut m = BTreeMap::from([("src/dut.v".to_string(), DUT.to_string())]);
        for (p, c) in extra {
            m.insert(p.to_string(), c.to_string());
        }
        m
    }

    #[test]
    fn picks_testbenches_matching_the_build() {
        let f = files(&[
            ("tb/tb_a.v", "module tb_a; reg a; dut #(.W(4)) d(.a(a)); endmodule\n"),
            ("tb/tb_b.v", "module tb_b; reg a; dut #(.P(1)) d(.a(a)); endmodule\n"),
            ("tb/tb_c.v", "module tb_c; reg a; helper h(.a(a)); endmodule\n"),
            ("tb/helper.v", "module helper(input wire a); dut d(.a(a)); endmodule\n"),
            ("tb/tb_none.v", "module tb_none; endmodule\n"),
        ]);
        let p = plan(&f, &json!({"DESIGN_NAME": "dut"})).unwrap();
        assert_eq!(p.params, BTreeMap::from([("P".into(), 0), ("W".into(), 4)]));
        let get = |t: &str| p.benches.iter().find(|(n, _)| n == t).unwrap().1.clone();
        assert_eq!(get("tb_a"), Ok(vec!["tb/tb_a.v".to_string()]));
        assert!(get("tb_b").unwrap_err().contains("P=1 where this build has P=0"), "{:?}", get("tb_b"));
        // Helper modules come along; the design's own file never does.
        assert_eq!(get("tb_c"), Ok(vec!["tb/helper.v".to_string(), "tb/tb_c.v".to_string()]));
        assert!(get("tb_none").unwrap_err().contains("does not instantiate dut"));

        let p = plan(&f, &json!({"DESIGN_NAME": "dut", "SYNTH_PARAMETERS": ["P=1"]})).unwrap();
        assert!(p.benches.iter().find(|(n, _)| n == "tb_b").unwrap().1.is_ok());
        assert!(p.benches.iter().find(|(n, _)| n == "tb_a").unwrap().1.is_err());

        assert!(plan(&f, &json!({"DESIGN_NAME": "dut", "SYNTH_PARAMETERS": ["Q=1"]})).is_err());
        assert!(plan(&f, &json!({"DESIGN_NAME": "dut", "SYNTH_PARAMETERS": ["P=x"]})).is_err());
        assert!(plan(&f, &json!({})).is_err());
    }

    #[test]
    fn integer_literals() {
        assert_eq!(parse_int("12"), Some(12));
        assert_eq!(parse_int("1_000"), Some(1000));
        assert_eq!(parse_int("8'hff"), Some(255));
        assert_eq!(parse_int("'b101"), Some(5));
        assert_eq!(parse_int("4'sd9"), Some(9));
        assert_eq!(parse_int("abc"), None);
    }

    #[test]
    fn parameters_go_after_the_header() {
        let nl = "// x\nmodule other(a); endmodule\nmodule dut (a,\n b);\n input a;\nendmodule\n";
        let params = BTreeMap::from([("P".to_string(), 1), ("W".to_string(), 8)]);
        let out = add_params(nl, "dut", &params).unwrap();
        assert!(out.contains("module dut (a,\n b);\n  parameter P = 1;\n  parameter W = 8;\n input a;"), "{out}");
        assert!(out.contains("module other(a); endmodule"));
        assert!(add_params(nl, "missing", &params).is_none());
        assert!(add_params("module dut_x (a); endmodule", "dut", &params).is_none());
    }
}
