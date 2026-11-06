//! A small event-driven Verilog simulator that runs anywhere Rust does,
//! including the browser (see `wafer-sim-wasm`).
//!
//! Supported: modules with parameters and hierarchy, `wire`/`reg`/`logic`/
//! `integer`, memories, continuous assignments, `always` (edge, `@*`, delay
//! loops), `initial`, blocking and nonblocking assignments, `if`/`case`/
//! `casez`/`for`/`while`/`repeat`/`forever`/`wait`, `#delays`, `@` events,
//! `$display`/`$write`/`$strobe`/`$monitor`/`$finish`, `$time`, `$signed`.
//! Values are 2-state (no x/z) and at most 64 bits wide. Unsupported
//! constructs are reported with file and line rather than mis-simulated.

pub mod ast;
pub mod elab;
pub mod lexer;
pub mod parser;
pub mod sim;
pub mod vcd;

use std::collections::{HashMap, HashSet};

pub use sim::{Limits, SimResult, WaveSignal};

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("{file}:{line}: {msg}")]
    Syntax { file: String, line: u32, msg: String },
    #[error("{file}:{line}: {msg}")]
    Elab { file: String, line: u32, msg: String },
    #[error("{file}:{line}: at time {time}: {msg}")]
    Runtime { file: String, line: u32, time: u64, msg: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Stack for [`on_big_stack`]: parsing, elaboration and simulation recurse over the syntax tree,
/// whose depth the parser bounds; this is comfortably more than that bound needs.
pub const STACK_BYTES: usize = 256 << 20;

/// Runs `f` on a thread with a [`STACK_BYTES`] stack (address space, committed only as used).
/// Servers should run untrusted designs through this rather than on a default-sized thread.
pub fn on_big_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("wafer-sim".into())
            .stack_size(STACK_BYTES)
            .spawn_scoped(s, f)
            .expect("spawn simulator thread")
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

/// Parses all files and returns the modules plus the timescale (fs per unit) in effect.
pub fn parse(files: &[(&str, &str)]) -> Result<(Vec<ast::Module>, u64)> {
    let mut defines = HashMap::new();
    let mut modules = Vec::new();
    let mut timescale = 1_000_000;
    for (name, src) in files {
        let lexed = lexer::lex(src, name, &mut defines)?;
        if lexed.timescale_fs != 1_000_000 || timescale == 1_000_000 {
            timescale = lexed.timescale_fs;
        }
        let mut p = parser::Parser::new(&lexed.tokens, name);
        modules.extend(p.source()?);
    }
    let mut seen = HashSet::new();
    for m in &modules {
        if !seen.insert(m.name.clone()) {
            return Err(Error::Elab {
                file: m.file.clone(),
                line: m.line,
                msg: format!("module {} is defined twice", m.name),
            });
        }
    }
    Ok((modules, timescale))
}

/// Modules that nothing instantiates; testbench-looking names first.
pub fn top_candidates(modules: &[ast::Module]) -> Vec<String> {
    let used: HashSet<&str> = modules.iter().flat_map(|m| m.instantiated()).collect();
    let mut tops: Vec<&ast::Module> = modules.iter().filter(|m| !used.contains(m.name.as_str())).collect();
    let looks_like_tb = |n: &str| {
        let n = n.to_ascii_lowercase();
        n.starts_with("tb") || n.ends_with("_tb") || n.contains("test")
    };
    tops.sort_by_key(|m| (!looks_like_tb(&m.name), !m.ports.is_empty()));
    tops.into_iter().map(|m| m.name.clone()).collect()
}

/// Parses, elaborates and runs. `top` defaults to the first of [`top_candidates`].
pub fn simulate(files: &[(&str, &str)], top: Option<&str>, limits: Limits) -> Result<SimResult> {
    let (modules, timescale) = parse(files)?;
    let top = match top {
        Some(t) => t.to_string(),
        None => top_candidates(&modules).into_iter().next().ok_or_else(|| Error::Elab {
            file: String::new(),
            line: 0,
            msg: "no modules found".into(),
        })?,
    };
    run_parsed(&modules, timescale, &top, limits)
}

/// Elaborates and runs modules already parsed (and perhaps changed, e.g. with a planted mistake).
pub fn run_parsed(modules: &[ast::Module], timescale: u64, top: &str, limits: Limits) -> Result<SimResult> {
    let design = elab::elaborate(modules, top)?;
    sim::Sim::new(&design, limits).run(timescale)
}

#[cfg(test)]
mod tests;
