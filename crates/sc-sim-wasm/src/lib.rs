//! `simulate()` for the browser: runs in a Web Worker and returns the log and
//! waveforms as typed arrays.

use sc_sim::{Limits, SimResult};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Simulation {
    r: SimResult,
}

/// Modules nothing instantiates, testbench-like names first. Errors if the sources don't parse.
#[wasm_bindgen(js_name = topCandidates)]
pub fn top_candidates(names: Vec<String>, sources: Vec<String>) -> Result<Vec<String>, JsError> {
    let files: Vec<(&str, &str)> = names.iter().map(String::as_str).zip(sources.iter().map(String::as_str)).collect();
    let (modules, _) = sc_sim::parse(&files).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(sc_sim::top_candidates(&modules))
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
    let r = sc_sim::simulate(&files, top.as_deref(), limits).map_err(|e| JsError::new(&e.to_string()))?;
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

    pub fn vcd(&self) -> String {
        sc_sim::vcd::to_vcd(&self.r)
    }
}
