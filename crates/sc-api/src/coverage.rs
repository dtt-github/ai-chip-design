//! What the examples exercise: branch coverage of the design (every `if`/`else` branch and `case`
//! arm, counted by the simulator) over the example testbenches, with each gap in plain words.

use std::collections::BTreeMap;

use sc_sim::elab::{CoverKind, CoverPoint};
use serde::Serialize;

use crate::gate::TestbenchRun;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Coverage {
    /// Branches the examples took at least once, of all the design's branches.
    pub covered: usize,
    pub total: usize,
    pub gaps: Vec<Gap>,
}

/// A branch no example takes.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Gap {
    pub file: String,
    pub line: u32,
    /// `then`, `else`, `arm` or `default`.
    pub kind: String,
    /// The condition or `case` selector as written.
    pub what: String,
    /// For an arm: its labels as written.
    pub labels: Option<String>,
    /// The gap in words: "`full` is never true in the examples".
    pub text: String,
    /// A starting point for an example that would take this branch.
    pub suggestion: String,
}

/// Coverage of the design files (not testbenches) over `runs`. A branch counts once however many
/// instances of its module there are; it is covered if any run takes it in any instance. An
/// implicit `default` (a `case` without one) is left out: it is often impossible by design.
pub fn of(runs: &[&TestbenchRun]) -> Option<Coverage> {
    let mut hits: BTreeMap<(String, u32, CoverKind, String), u64> = BTreeMap::new();
    for r in runs {
        for (p, n) in &r.coverage {
            if !counts(p) {
                continue;
            }
            *hits.entry((p.file.clone(), p.line, p.kind.clone(), p.what.clone())).or_default() += n;
        }
    }
    if hits.is_empty() {
        return None;
    }
    let gaps = hits
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|((file, line, kind, what), _)| gap(file, *line, kind, what))
        .collect();
    Some(Coverage { covered: hits.values().filter(|n| **n > 0).count(), total: hits.len(), gaps })
}

fn counts(p: &CoverPoint) -> bool {
    !p.file.starts_with("tb/") && !matches!(p.kind, CoverKind::Default { written: false })
}

fn gap(file: &str, line: u32, kind: &CoverKind, what: &str) -> Gap {
    let at = format!("checked at line {line} of {file}");
    let (k, labels, text, suggestion) = match kind {
        CoverKind::Then | CoverKind::Else { .. } => {
            let (sym, words) = situation(what, matches!(kind, CoverKind::Then));
            let k = if matches!(kind, CoverKind::Then) { "then" } else { "else" };
            (k, None, format!("Never tried: {sym}"), format!("An example where {words} ({at})."))
        }
        CoverKind::Arm { labels } => (
            "arm",
            Some(labels.clone()),
            format!("Never tried: `{what}` = {}", labels.replace(", ", " or ")),
            format!("An example where {what} is {} ({at}).", labels.replace(", ", " or ")),
        ),
        CoverKind::Default { .. } => (
            "default",
            None,
            format!("Never tried: `{what}` with a value the case doesn't list (that may be impossible by design)"),
            format!("An example where {what} takes a value the case doesn't list ({at})."),
        ),
    };
    Gap { file: file.into(), line, kind: k.into(), what: what.into(), labels, text, suggestion }
}

/// The situation in which condition `what` is `truth`: as symbols for the gap ("`en` = 0") and in
/// words for an example ("en is 0"). Plain names, negations and (in)equalities are spelled out.
pub(crate) fn situation(what: &str, truth: bool) -> (String, String) {
    let name = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_.[]".contains(c));
    let eq = |a: &str, b: &str, is: bool| {
        if is {
            (format!("`{a}` = {b}"), format!("{a} is {b}"))
        } else {
            (format!("`{a}` ≠ {b}"), format!("{a} is not {b}"))
        }
    };
    if let Some(x) = what.strip_prefix('!').filter(|x| name(x)) {
        return eq(x, "0", truth);
    }
    if name(what) {
        return eq(what, "0", !truth);
    }
    for (op, same) in [(" == ", true), (" != ", false)] {
        if let Some((a, b)) = what.split_once(op)
            && name(a)
            && !b.contains(' ')
        {
            return eq(a, b, truth == same);
        }
    }
    let v = if truth { "true" } else { "false" };
    (format!("`{what}` is {v}"), format!("{what} is {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(file: &str, line: u32, kind: CoverKind, what: &str) -> CoverPoint {
        CoverPoint { file: file.into(), line, scope: "tb.dut".into(), kind, what: what.into() }
    }

    fn run(cov: Vec<(CoverPoint, u64)>) -> TestbenchRun {
        TestbenchRun {
            top: "tb_ex_a".into(),
            verdict: "pass".into(),
            finished: true,
            time: 0,
            steps: 0,
            log: vec![],
            reason: None,
            coverage: cov,
        }
    }

    #[test]
    fn merges_runs_and_explains_gaps() {
        let a = run(vec![
            (point("src/f.v", 10, CoverKind::Then, "wr && !full"), 3),
            (point("src/f.v", 10, CoverKind::Else { written: false }, "wr && !full"), 0),
            (point("src/f.v", 20, CoverKind::Arm { labels: "2'h2".into() }, "state"), 0),
            (point("src/f.v", 20, CoverKind::Default { written: false }, "state"), 0),
            (point("tb/tb_f.v", 5, CoverKind::Then, "done"), 0),
        ]);
        let b = run(vec![(point("src/f.v", 10, CoverKind::Else { written: false }, "wr && !full"), 1)]);
        let c = of(&[&a, &b]).expect("coverage");
        assert_eq!((c.covered, c.total), (2, 3), "testbenches and implicit defaults don't count");
        assert_eq!(c.gaps.len(), 1);
        assert_eq!(c.gaps[0].text, "Never tried: `state` = 2'h2");
        assert!(c.gaps[0].suggestion.contains("checked at line 20 of src/f.v"));
        assert_eq!(of(&[&run(vec![])]), None);
    }

    #[test]
    fn situations_in_plain_terms() {
        assert_eq!(situation("!en", true), ("`en` = 0".into(), "en is 0".into()));
        assert_eq!(situation("!en", false), ("`en` ≠ 0".into(), "en is not 0".into()));
        assert_eq!(situation("full", true), ("`full` ≠ 0".into(), "full is not 0".into()));
        assert_eq!(situation("cnt == N - 1", true).1, "cnt == N - 1 is true", "only simple right-hand sides");
        assert_eq!(situation("bitn == 9", false), ("`bitn` ≠ 9".into(), "bitn is not 9".into()));
        assert_eq!(situation("din != dout", true).1, "din is not dout");
        assert_eq!(situation("wr && !full", false), ("`wr && !full` is false".into(), "wr && !full is false".into()));
    }
}
