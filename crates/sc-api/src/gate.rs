//! The simulation gate: a change may only be merged when every testbench in
//! the tree it would produce passes. Runs the same `sc-sim` engine as the
//! browser's Simulate button, on the server, so nobody (human or agent) can
//! report their own result.
//!
//! A testbench is a top-level module whose name looks like one (`tb*`,
//! `*_tb`, `*test*`). It passes when it reaches `$finish` and prints a line
//! containing `PASS` and none containing `FAIL`, `ERROR` or `MISMATCH`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bounds for one testbench: generous for unit tests, but a hung testbench can't stall the gate.
pub(crate) const MAX_TIME: u64 = 50_000_000;
const MAX_STEPS: u64 = 50_000_000;
const MAX_LOG_LINES: usize = 400;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestbenchRun {
    pub top: String,
    /// `pass`, `fail`, `no_verdict` (no PASS line), or `error` (didn't compile/elaborate/run).
    pub verdict: String,
    pub finished: bool,
    pub time: u64,
    pub steps: u64,
    pub log: Vec<String>,
    pub reason: Option<String>,
    /// Branch coverage of this run (`crate::coverage`); not part of stored reports.
    #[serde(skip)]
    pub coverage: Vec<(sc_sim::elab::CoverPoint, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateReport {
    pub passed: bool,
    pub summary: String,
    pub engine: String,
    pub runs: Vec<TestbenchRun>,
    /// sha256 of every simulated file, so the result can be tied to exact inputs.
    pub inputs: BTreeMap<String, String>,
}

pub fn engine() -> String {
    format!("sc-sim {}", env!("CARGO_PKG_VERSION"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn is_hdl(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| matches!(e.to_ascii_lowercase().as_str(), "v" | "sv"))
}

pub(crate) fn looks_like_tb(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("tb") || n.ends_with("_tb") || n.contains("test")
}

pub(crate) fn verdict(log: &[String]) -> (&'static str, Option<String>) {
    let has = |word: &str| log.iter().find(|l| l.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == word));
    for bad in ["FAIL", "ERROR", "MISMATCH"] {
        if let Some(line) = has(bad) {
            return ("fail", Some(format!("printed: {line}")));
        }
    }
    if has("PASS").is_some() { ("pass", None) } else { ("no_verdict", Some("printed no PASS line".into())) }
}

/// Simulations run in the API process; at most this many at once, server-wide.
const MAX_CONCURRENT: usize = 3;

struct Slots {
    used: std::sync::Mutex<usize>,
    freed: std::sync::Condvar,
}

static SLOTS: Slots = Slots { used: std::sync::Mutex::new(0), freed: std::sync::Condvar::new() };

/// Waits for one of the [`MAX_CONCURRENT`] simulation slots (call from a blocking thread).
pub(crate) fn with_slot<R>(f: impl FnOnce() -> R) -> R {
    let mut used = SLOTS.used.lock().unwrap_or_else(|e| e.into_inner());
    while *used >= MAX_CONCURRENT {
        used = SLOTS.freed.wait(used).unwrap_or_else(|e| e.into_inner());
    }
    *used += 1;
    drop(used);
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            *SLOTS.used.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            SLOTS.freed.notify_one();
        }
    }
    let _release = Release;
    f()
}

/// Runs every testbench in `files` (path → content). Deterministic; blocks, so call it from a
/// blocking thread. It runs on a large stack and waits for a free simulation slot.
pub fn run(files: &BTreeMap<String, String>) -> GateReport {
    with_slot(|| sc_sim::on_big_stack(|| run_inner(files)))
}

fn run_inner(files: &BTreeMap<String, String>) -> GateReport {
    let hdl: Vec<(&str, &str)> =
        files.iter().filter(|(p, _)| is_hdl(p)).map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let inputs = hdl.iter().map(|(p, c)| (p.to_string(), sha256_hex(c.as_bytes()))).collect();
    let fail = |summary: String, runs: Vec<TestbenchRun>, inputs| GateReport {
        passed: false,
        summary,
        engine: engine(),
        runs,
        inputs,
    };
    let modules = match sc_sim::parse(&hdl) {
        Ok((m, _)) => m,
        Err(e) => return fail(format!("the design does not parse: {e}"), Vec::new(), inputs),
    };
    let tbs: Vec<String> = sc_sim::top_candidates(&modules).into_iter().filter(|t| looks_like_tb(t)).collect();
    if tbs.is_empty() {
        return fail("no testbench found (a top-level module named tb*, *_tb or *test*)".into(), Vec::new(), inputs);
    }
    let limits =
        sc_sim::Limits { max_time: MAX_TIME, max_steps: MAX_STEPS, max_log_lines: MAX_LOG_LINES, ..Default::default() };
    let mut runs = Vec::new();
    for top in tbs {
        let run = match sc_sim::simulate(&hdl, Some(&top), limits) {
            Err(e) => TestbenchRun {
                top,
                verdict: "error".into(),
                finished: false,
                time: 0,
                steps: 0,
                log: Vec::new(),
                reason: Some(e.to_string()),
                coverage: Vec::new(),
            },
            Ok(r) => {
                let (v, reason) = if !r.finished {
                    ("fail", Some(r.stopped.clone().unwrap_or_else(|| "did not reach $finish".into())))
                } else {
                    verdict(&r.log)
                };
                TestbenchRun {
                    top,
                    verdict: v.into(),
                    finished: r.finished,
                    time: r.time,
                    steps: r.steps,
                    log: r.log,
                    reason,
                    coverage: r.coverage,
                }
            }
        };
        runs.push(run);
    }
    let failed: Vec<&TestbenchRun> = runs.iter().filter(|r| r.verdict != "pass").collect();
    let summary = if failed.is_empty() {
        format!("{} of {} testbenches passed", runs.len(), runs.len())
    } else {
        format!(
            "{} of {} testbenches failed: {}",
            failed.len(),
            runs.len(),
            failed
                .iter()
                .map(|r| format!("{} ({})", r.top, r.reason.as_deref().unwrap_or(&r.verdict)))
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    GateReport { passed: failed.is_empty(), summary, engine: engine(), runs, inputs }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUT: &str = "module inv(input a, output y); assign y = ~a; endmodule\n";

    fn tb(name: &str, body: &str) -> String {
        format!("module {name}; reg a = 0; wire y; inv d(a, y); initial begin #1 {body} $finish; end endmodule\n")
    }

    fn files(extra: &[(&str, String)]) -> BTreeMap<String, String> {
        let mut m = BTreeMap::from([("src/inv.v".to_string(), DUT.to_string()), ("README.md".into(), "x".into())]);
        for (p, c) in extra {
            m.insert(p.to_string(), c.clone());
        }
        m
    }

    #[test]
    fn passing_and_failing_testbenches() {
        let ok = tb("tb_inv", r#"if (y) $display("PASS"); else $display("FAIL: y=%b", y);"#);
        let r = run(&files(&[("tb/tb_inv.v", ok.clone())]));
        assert!(r.passed, "{r:?}");
        assert_eq!(r.inputs.len(), 2);
        assert_eq!(r.inputs["src/inv.v"], sha256_hex(DUT.as_bytes()));

        let bad = tb("tb_bad", r#"if (!y) $display("PASS"); else $display("FAIL: y=%b", y);"#);
        let r = run(&files(&[("tb/tb_inv.v", ok), ("tb/tb_bad.v", bad)]));
        assert!(!r.passed);
        assert!(r.summary.contains("tb_bad"), "{}", r.summary);
        assert_eq!(r.runs.iter().filter(|x| x.verdict == "pass").count(), 1);
    }

    #[test]
    fn silence_hangs_and_errors_fail() {
        let quiet = tb("tb_quiet", r#"$display("done");"#);
        assert_eq!(run(&files(&[("t.v", quiet)])).runs[0].verdict, "no_verdict");

        let hang = "module tb_hang; reg c = 0; always #1 c = ~c; initial $display(\"PASS\"); endmodule\n".to_string();
        let r = run(&files(&[("t.v", hang)]));
        assert!(!r.passed);
        assert!(!r.runs[0].finished);

        let broken = files(&[("t.v", "module tb_x; initial begin ; endmodule".to_string())]);
        assert!(run(&broken).summary.contains("does not parse"));

        assert!(run(&files(&[])).summary.contains("no testbench"));
        // "PASSED" alone isn't the word PASS; "ERRORS: 0" contains ERRORS, not ERROR.
        let wordy = tb("tb_w", r#"$display("ERRORS: 0, PASS");"#);
        assert!(run(&files(&[("t.v", wordy)])).passed);
    }
}
