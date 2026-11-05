//! The design eval tasks in `evals/design/tasks/` are graded with their reference files, so the
//! references must be right: each task's reference design passes its own examples and its hidden
//! testbench, and a design that drives every output to 0 fails both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sc_api::gate;
use sc_api::spec::Spec;
use serde_json::Value;

fn tasks_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/design/tasks")
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{}/{name}: {e}", dir.display()))
}

/// A design with the spec's connections and settings whose outputs are all 0.
fn null_design(spec: &Spec) -> String {
    let params: Vec<String> = spec.params.iter().map(|p| format!("parameter {} = {}", p.name, p.value)).collect();
    let params = if params.is_empty() { String::new() } else { format!(" #({})", params.join(", ")) };
    let ports: Vec<String> = spec
        .ports
        .iter()
        .map(|p| {
            let range = if p.width > 1 { format!("[{}:0] ", p.width - 1) } else { String::new() };
            format!("{} wire {range}{}", if p.dir == "in" { "input" } else { "output" }, p.name)
        })
        .collect();
    let zeros: String =
        spec.ports.iter().filter(|p| p.dir == "out").map(|p| format!("    assign {} = 0;\n", p.name)).collect();
    format!("module {}{params} (\n    {}\n);\n{zeros}endmodule\n", spec.top, ports.join(",\n    "))
}

fn run(design: &str, spec: &Spec, extra: BTreeMap<String, String>) -> gate::GateReport {
    let mut files = extra;
    files.insert(format!("src/{}.v", spec.top), design.to_string());
    gate::run(&files)
}

fn failures(r: &gate::GateReport) -> String {
    r.runs
        .iter()
        .filter(|t| t.verdict != "pass")
        .map(|t| {
            let log = t.log.iter().rev().take(6).cloned().collect::<Vec<_>>().join(" | ");
            format!("{} ({}, {}): {log}", t.top, t.verdict, t.reason.as_deref().unwrap_or(""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_task_is_graded_by_correct_references() {
    let mut dirs: Vec<PathBuf> =
        std::fs::read_dir(tasks_dir()).expect("evals/design/tasks").map(|e| e.unwrap().path()).collect();
    dirs.sort();
    assert!(dirs.len() >= 15, "expected the full task set, found {}", dirs.len());
    let mut problems = Vec::new();
    for dir in &dirs {
        let id = dir.file_name().unwrap().to_string_lossy().to_string();
        let task: Value = serde_json::from_str(&read(dir, "task.json")).expect("task.json");
        let spec: Spec = serde_json::from_str(&read(dir, "spec.json")).expect("spec.json");
        let reference = read(dir, "ref.v");
        let hidden = read(dir, "hidden_tb.v");

        assert_eq!(task["id"], id.as_str(), "{id}: task.json id");
        assert_eq!(task["top"], spec.top.as_str(), "{id}: task.json top");
        assert!(["clear", "vague"].contains(&task["kind"].as_str().unwrap_or("")), "{id}: kind");
        let open = task["open_points"].as_array().map(|a| a.len()).unwrap_or(0);
        assert_eq!(task["kind"] == "vague", open > 0, "{id}: vague tasks, and only they, list open points");
        assert_eq!(spec.check(None), Vec::<String>::new(), "{id}: reference spec");
        assert!(spec.questions.is_empty(), "{id}: the reference spec has no open questions");
        assert!(reference.contains("SPDX-License-Identifier") && hidden.contains("SPDX-License-Identifier"));

        let examples = spec.testbenches_for(None);
        let hidden_tb = BTreeMap::from([("tb/hidden_tb.v".to_string(), hidden)]);
        let null = null_design(&spec);

        let r = run(&reference, &spec, examples.clone());
        if !r.passed || r.runs.len() != spec.examples.len() {
            problems.push(format!("{id}: reference fails its examples\n{}", failures(&r)));
        }
        let r = run(&reference, &spec, hidden_tb.clone());
        if !r.passed {
            problems.push(format!("{id}: reference fails the hidden testbench\n{}", failures(&r)));
        }
        if run(&null, &spec, examples).passed {
            problems.push(format!("{id}: the examples pass a design that does nothing"));
        }
        if run(&null, &spec, hidden_tb).passed {
            problems.push(format!("{id}: the hidden testbench passes a design that does nothing"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}
