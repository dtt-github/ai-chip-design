//! `simulate()` for the browser: runs in a Web Worker and returns the log and
//! waveforms as typed arrays.

use wafer_sim::{Limits, SimResult};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Simulation {
    r: SimResult,
}

/// Modules nothing instantiates, testbench-like names first. Errors if the sources don't parse.
#[wasm_bindgen(js_name = topCandidates)]
pub fn top_candidates(names: Vec<String>, sources: Vec<String>) -> Result<Vec<String>, JsError> {
    let files: Vec<(&str, &str)> = names.iter().map(String::as_str).zip(sources.iter().map(String::as_str)).collect();
    let (modules, _) = wafer_sim::parse(&files).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(wafer_sim::top_candidates(&modules))
}

#[wasm_bindgen]
pub fn simulate(
    names: Vec<String>,
    sources: Vec<String>,
    top: Option<String>,
    max_time: f64,
) -> Result<Simulation, JsError> {
    let files: Vec<(&str, &str)> = names.iter().map(String::as_str).zip(sources.iter().map(String::as_str)).collect();
    let limits = Limits {
        max_time: if max_time > 0.0 { max_time as u64 } else { Limits::default().max_time },
        ..Limits::default()
    };
    let r = wafer_sim::simulate(&files, top.as_deref(), limits).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(Simulation { r })
}

#[wasm_bindgen]
impl Simulation {
    #[wasm_bindgen(getter)]
    pub fn log(&self) -> Vec<String> {
        self.r.log.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn time(&self) -> f64 {
        self.r.time as f64
    }

    #[wasm_bindgen(getter)]
    pub fn finished(&self) -> bool {
        self.r.finished
    }

    #[wasm_bindgen(getter)]
    pub fn stopped(&self) -> Option<String> {
        self.r.stopped.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn warnings(&self) -> Vec<String> {
        self.r.warnings.clone()
    }

    #[wasm_bindgen(getter, js_name = timescaleFs)]
    pub fn timescale_fs(&self) -> f64 {
        self.r.timescale_fs as f64
    }

    #[wasm_bindgen(getter)]
    pub fn steps(&self) -> f64 {
        self.r.steps as f64
    }

    #[wasm_bindgen(getter)]
    pub fn names(&self) -> Vec<String> {
        self.r.signals.iter().map(|s| s.name.clone()).collect()
    }

    #[wasm_bindgen(getter)]
    pub fn widths(&self) -> Vec<u32> {
        self.r.signals.iter().map(|s| s.width).collect()
    }

    /// Change times of signal `i` (simulation time units).
    pub fn times(&self, i: usize) -> Vec<f64> {
        self.r.signals.get(i).map(|s| s.times.iter().map(|&t| t as f64).collect()).unwrap_or_default()
    }

    /// Values from each change time on (BigUint64Array in JS).
    pub fn values(&self, i: usize) -> Vec<u64> {
        self.r.signals.get(i).map(|s| s.values.clone()).unwrap_or_default()
    }

    /// What wrote each value of signal `i`: indexes into `origins`, 0xFFFFFFFF for nothing.
    pub fn by(&self, i: usize) -> Vec<u32> {
        self.r.signals.get(i).map(|s| s.by.clone()).unwrap_or_default()
    }

    /// When each `if`/`case` decision was made, in order (simulation time units).
    #[wasm_bindgen(js_name = decisionTimes)]
    pub fn decision_times(&self) -> Vec<f64> {
        self.r.decisions.iter().map(|&(t, _)| t as f64).collect()
    }

    /// The branch each decision took (a coverage point), in the same order as `decisionTimes`.
    #[wasm_bindgen(js_name = decisionBranches)]
    pub fn decision_branches(&self) -> Vec<u32> {
        self.r.decisions.iter().map(|&(_, p)| p).collect()
    }

    /// The decision each coverage point is a branch of; 0xFFFFFFFF for one that isn't a branch.
    #[wasm_bindgen(js_name = branchDecision)]
    pub fn branch_decision(&self) -> Vec<u32> {
        self.r.cover_decision.clone()
    }

    /// The run made more decisions than are recorded.
    #[wasm_bindgen(getter, js_name = decisionsCut)]
    pub fn decisions_cut(&self) -> bool {
        self.r.decisions_cut
    }

    /// Where written values come from, as JSON: `[{file, line, scope, text, reads, writes, conds:
    /// [{line, text, reads, taken, point}]}]`.
    pub fn origins(&self) -> String {
        let list: Vec<String> = self
            .r
            .origins
            .iter()
            .map(|o| {
                let conds: Vec<String> = o
                    .conds
                    .iter()
                    .map(|c| {
                        format!(
                            r#"{{"line":{},"text":{},"reads":{},"taken":{},"point":{}}}"#,
                            c.line,
                            json(&c.text),
                            strings(&c.reads),
                            json(&c.taken),
                            c.point
                        )
                    })
                    .collect();
                format!(
                    r#"{{"file":{},"line":{},"scope":{},"text":{},"reads":{},"writes":{},"conds":[{}]}}"#,
                    json(&o.file),
                    o.line,
                    json(&o.scope),
                    json(&o.text),
                    strings(&o.reads),
                    json(&o.writes),
                    conds.join(",")
                )
            })
            .collect();
        format!("[{}]", list.join(","))
    }

    pub fn vcd(&self) -> String {
        wafer_sim::vcd::to_vcd(&self.r)
    }
}

/// A JSON string.
fn json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn strings(v: &[String]) -> String {
    format!("[{}]", v.iter().map(|s| json(s)).collect::<Vec<_>>().join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json("a \"b\" \\ c\n"), r#""a \"b\" \\ c\u000a""#);
        assert_eq!(strings(&["x".into(), "y".into()]), r#"["x","y"]"#);
    }
}
