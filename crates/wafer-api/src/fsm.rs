//! State machines found in the design's Verilog, for the "How it works" diagram: a `case` on a
//! register whose arms assign that register (directly, or through a next-state variable that is
//! copied into it), with the conditions on the way to each assignment.
//!
//! Each transition remembers the branch it is in (the innermost `if`/`else` or `case` arm), the
//! same coverage point the simulator counts, so the examples' coverage says which transitions
//! they take.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use wafer_sim::ast::{Expr, Item, Module, Stmt};
use wafer_sim::elab::{CoverKind, CoverPoint};

use crate::gate::TestbenchRun;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Machine {
    pub module: String,
    pub file: String,
    /// The state register.
    pub state: String,
    pub states: Vec<String>,
    /// Each state's value, when it is a number or a parameter set to one (for live highlighting).
    pub values: Vec<Option<u64>>,
    pub transitions: Vec<Transition>,
    /// The state after reset, if the design sets one outside the `case`.
    pub reset: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Transition {
    pub from: String,
    pub to: String,
    /// When it happens, with conditions in backticks ("`rx` = 0"), or "always".
    pub when: String,
    pub line: u32,
    /// Whether any example took it; `None` before the examples run.
    pub covered: Option<bool>,
    #[serde(skip)]
    key: Option<(u32, CoverKind, String)>,
}

/// The state machines of the design files (not testbenches), in file and module order.
pub fn find(modules: &[Module]) -> Vec<Machine> {
    modules.iter().filter(|m| !m.file.starts_with("tb/")).flat_map(machines).collect()
}

/// Marks each transition taken or not, from the example runs' branch coverage.
pub fn mark(machines: &mut [Machine], runs: &[&TestbenchRun]) {
    let mut hits: BTreeMap<(String, u32, CoverKind, String), u64> = BTreeMap::new();
    for r in runs {
        for (p, n) in &r.coverage {
            let CoverPoint { file, line, kind, what, .. } = p;
            *hits.entry((file.clone(), *line, kind.clone(), what.clone())).or_default() += n;
        }
    }
    for m in machines {
        for t in &mut m.transitions {
            if let Some((line, kind, what)) = t.key.clone() {
                t.covered = Some(hits.get(&(m.file.clone(), line, kind, what)).is_some_and(|n| *n > 0));
            }
        }
    }
}

fn machines(m: &Module) -> Vec<Machine> {
    let params: BTreeSet<&str> = m
        .items
        .iter()
        .filter_map(|i| if let Item::Param { name, .. } = i { Some(name.as_str()) } else { None })
        .collect();
    let bodies: Vec<&Stmt> =
        m.items.iter().filter_map(|i| if let Item::Always { body, .. } = i { Some(body) } else { None }).collect();
    let mut cases = Vec::new();
    for b in &bodies {
        collect_cases(b, &mut cases);
    }
    let mut out: Vec<Machine> = Vec::new();
    for (sel, arms, default, line) in cases {
        let Expr::Ident(reg, _) = sel else { continue };
        if out.iter().any(|x| x.state == *reg) {
            continue;
        }
        // The register itself, and variables copied into it (two-process style: state <= next).
        let mut targets: BTreeSet<String> = BTreeSet::from([reg.clone()]);
        for b in &bodies {
            copies_into(b, reg, &mut targets);
        }
        let mut found = Found { params: &params, targets: &targets, out: Vec::new() };
        for (labels, body) in arms {
            let arm = labels.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
            for from in labels {
                let key = (*line, CoverKind::Arm { labels: arm.clone() }, sel.to_string());
                found.walk(body, &from.to_string(), &mut Vec::new(), &key);
            }
        }
        // States reached but without an arm of their own are handled by `default`.
        if let Some(d) = default {
            let listed: Vec<String> = arms.iter().flat_map(|(ls, _)| ls.iter().map(|l| l.to_string())).collect();
            let mut unlisted: Vec<String> =
                found.out.iter().map(|t| t.to.clone()).filter(|t| !listed.contains(t)).collect();
            unlisted.dedup();
            let key = (*line, CoverKind::Default { written: true }, sel.to_string());
            for from in unlisted.iter().collect::<BTreeSet<_>>() {
                found.walk(d, from, &mut Vec::new(), &key);
            }
        }
        let transitions = found.out;
        if transitions.is_empty() {
            continue;
        }
        let mut states: Vec<String> = Vec::new();
        let mut add = |s: &str| {
            if !states.iter().any(|x| x == s) {
                states.push(s.to_string());
            }
        };
        for (labels, _) in arms {
            labels.iter().for_each(|l| add(&l.to_string()));
        }
        transitions.iter().for_each(|t| add(&t.to));
        if states.len() < 2 {
            continue;
        }
        let reset = bodies.iter().find_map(|b| reset_state(b, &targets, &params));
        let values = states.iter().map(|st| value_of(m, st)).collect();
        out.push(Machine {
            values,
            module: m.name.clone(),
            file: m.file.clone(),
            state: reg.clone(),
            states,
            transitions,
            reset,
        });
    }
    out
}

/// A state's number: written as one (as `Expr`'s printer writes it: `3`, `2'h1a`, `2'b01`), or a
/// parameter set to one.
fn value_of(m: &Module, state: &str) -> Option<u64> {
    let param = m.items.iter().find_map(|i| match i {
        Item::Param { name, value, .. } if name == state => Some(value),
        _ => None,
    });
    match param {
        Some(Expr::Num(n)) => Some(n.value),
        Some(_) => None,
        None => match state.split_once('\'') {
            None => state.parse().ok(),
            Some((_, rest)) => {
                let radix = match rest.chars().next()? {
                    'h' => 16,
                    'b' => 2,
                    'd' => 10,
                    _ => return None,
                };
                u64::from_str_radix(&rest[1..], radix).ok()
            }
        },
    }
}

type Arms<'a> = &'a [(Vec<Expr>, Stmt)];
type CaseRef<'a> = (&'a Expr, Arms<'a>, Option<&'a Stmt>, &'a u32);

fn collect_cases<'a>(s: &'a Stmt, out: &mut Vec<CaseRef<'a>>) {
    match s {
        Stmt::Case { sel, items, default, line, .. } => {
            out.push((sel, items, default.as_deref(), line));
            items.iter().for_each(|(_, b)| collect_cases(b, out));
        }
        Stmt::Block(v) => v.iter().for_each(|x| collect_cases(x, out)),
        Stmt::If(_, t, e) => {
            collect_cases(t, out);
            if let Some(e) = e {
                collect_cases(e, out);
            }
        }
        Stmt::Event(_, b, _) | Stmt::Delay(_, b) | Stmt::For { body: b, .. } | Stmt::While(_, b) => {
            collect_cases(b, out)
        }
        _ => {}
    }
}

/// Adds `n` to `targets` for every `reg <= n` / `reg = n`.
fn copies_into(s: &Stmt, reg: &str, targets: &mut BTreeSet<String>) {
    match s {
        Stmt::Blocking { lhs: Expr::Ident(l, _), rhs: Expr::Ident(r, _), .. }
        | Stmt::NonBlocking { lhs: Expr::Ident(l, _), rhs: Expr::Ident(r, _), .. }
            if l == reg && r != reg =>
        {
            targets.insert(r.clone());
        }
        Stmt::Block(v) => v.iter().for_each(|x| copies_into(x, reg, targets)),
        Stmt::If(_, t, e) => {
            copies_into(t, reg, targets);
            if let Some(e) = e {
                copies_into(e, reg, targets);
            }
        }
        Stmt::Event(_, b, _) => copies_into(b, reg, targets),
        _ => {}
    }
}

/// A constant state value: a number or a parameter.
fn constant(e: &Expr, params: &BTreeSet<&str>) -> Option<String> {
    match e {
        Expr::Num(_) => Some(e.to_string()),
        Expr::Ident(n, _) if params.contains(n.as_str()) => Some(n.clone()),
        _ => None,
    }
}

/// The state set outside the `case`, under a reset condition (`if (!rst_n) state <= IDLE`).
fn reset_state(s: &Stmt, targets: &BTreeSet<String>, params: &BTreeSet<&str>) -> Option<String> {
    match s {
        Stmt::If(c, t, e) => {
            let txt = c.to_string().to_lowercase();
            if (txt.contains("rst") || txt.contains("reset"))
                && let Some(v) = assigned(t, targets, params)
            {
                return Some(v);
            }
            reset_state(t, targets, params).or_else(|| e.as_ref().and_then(|e| reset_state(e, targets, params)))
        }
        Stmt::Block(v) => v.iter().find_map(|x| reset_state(x, targets, params)),
        Stmt::Event(_, b, _) => reset_state(b, targets, params),
        _ => None,
    }
}

fn assigned(s: &Stmt, targets: &BTreeSet<String>, params: &BTreeSet<&str>) -> Option<String> {
    match s {
        Stmt::Blocking { lhs: Expr::Ident(l, _), rhs, .. } | Stmt::NonBlocking { lhs: Expr::Ident(l, _), rhs, .. }
            if targets.contains(l) =>
        {
            constant(rhs, params)
        }
        Stmt::Block(v) => v.iter().find_map(|x| assigned(x, targets, params)),
        _ => None,
    }
}

struct Found<'a> {
    params: &'a BTreeSet<&'a str>,
    targets: &'a BTreeSet<String>,
    out: Vec<Transition>,
}

impl Found<'_> {
    /// Walks one arm's statements, keeping the conditions on the way and the innermost branch.
    fn walk(&mut self, s: &Stmt, from: &str, path: &mut Vec<(String, bool)>, key: &(u32, CoverKind, String)) {
        match s {
            Stmt::Block(v) => v.iter().for_each(|x| self.walk(x, from, path, key)),
            Stmt::If(c, t, e) => {
                let (line, what) = (c.line().unwrap_or(0), c.to_string());
                path.push((what.clone(), true));
                self.walk(t, from, path, &(line, CoverKind::Then, what.clone()));
                path.pop();
                path.push((what.clone(), false));
                if let Some(e) = e {
                    self.walk(e, from, path, &(line, CoverKind::Else { written: true }, what));
                }
                path.pop();
            }
            Stmt::Case { sel, items, line, .. } => {
                for (labels, body) in items {
                    let arm = labels.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
                    path.push((format!("{sel} == {}", labels[0]), true));
                    self.walk(body, from, path, &(*line, CoverKind::Arm { labels: arm }, sel.to_string()));
                    path.pop();
                }
            }
            Stmt::Blocking { lhs: Expr::Ident(l, _), rhs, line }
            | Stmt::NonBlocking { lhs: Expr::Ident(l, _), rhs, line, .. }
                if self.targets.contains(l) =>
            {
                match rhs {
                    // `state <= go ? A : B`: two transitions, on the condition; each side of the
                    // choice is its own coverage point.
                    Expr::Ternary(c, a, b) => {
                        let cline = c.line().unwrap_or(*line);
                        for (e, truth) in [(a, true), (b, false)] {
                            if let Some(to) = constant(e, self.params) {
                                path.push((c.to_string(), truth));
                                let side = (cline, CoverKind::Side { value: truth }, c.to_string());
                                self.add(from, to, path, *line, Some(&side));
                                path.pop();
                            }
                        }
                    }
                    e => {
                        if let Some(to) = constant(e, self.params) {
                            self.add(from, to, path, *line, Some(key));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn add(
        &mut self,
        from: &str,
        to: String,
        path: &[(String, bool)],
        line: u32,
        key: Option<&(u32, CoverKind, String)>,
    ) {
        let when = if path.is_empty() {
            "always".to_string()
        } else {
            path.iter().map(|(c, t)| crate::coverage::situation(c, *t).0).collect::<Vec<_>>().join(" and ")
        };
        self.out.push(Transition { from: from.to_string(), to, when, line, covered: None, key: key.cloned() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<Machine> {
        let (m, _) = wafer_sim::parse(&[("src/d.v", src)]).expect("parses");
        find(&m)
    }

    #[test]
    fn finds_one_process_machines_with_conditions() {
        let src = crate::starters::get("uart_rx").expect("starter").rtl.clone();
        let ms = parse(&src);
        assert_eq!(ms.len(), 1, "{ms:#?}");
        let m = &ms[0];
        assert_eq!((m.module.as_str(), m.state.as_str()), ("uart_rx", "state"));
        assert_eq!(m.states, ["0", "1", "2", "3"]);
        assert_eq!(m.values, [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(m.reset.as_deref(), Some("0"));
        let t =
            |from: &str, to: &str| m.transitions.iter().find(|t| t.from == from && t.to == to).map(|t| t.when.clone());
        assert_eq!(t("0", "1").as_deref(), Some("`rx` = 0"));
        assert_eq!(t("1", "2").as_deref(), Some("`cnt == CLKS_PER_BIT / 2 - 1` is true and `rx` = 0"));
        assert_eq!(t("1", "0").as_deref(), Some("`cnt == CLKS_PER_BIT / 2 - 1` is true and `rx` ≠ 0"));
        assert_eq!(t("2", "3").as_deref(), Some("`cnt == CLKS_PER_BIT - 1` is true and `bitn` = 7"));
        // State 3 is handled by `default:`.
        assert_eq!(t("3", "0").as_deref(), Some("`cnt == CLKS_PER_BIT - 1` is true"));
    }

    #[test]
    fn finds_two_process_machines_with_named_states() {
        let ms = parse(
            "module tl(input wire clk, input wire rst_n, input wire go, output reg on);
                localparam IDLE = 0, RUN = 1, DONE = 2;
                reg [1:0] state, next;
                always @(posedge clk or negedge rst_n) if (!rst_n) state <= IDLE; else state <= next;
                always @(*) begin
                    next = state;
                    on = 0;
                    case (state)
                        IDLE: if (go) next = RUN;
                        RUN: begin on = 1; next = DONE; end
                        DONE: next = go ? DONE : IDLE;
                    endcase
                end
            endmodule",
        );
        assert_eq!(ms.len(), 1, "{ms:#?}");
        let m = &ms[0];
        assert_eq!(m.states, ["IDLE", "RUN", "DONE"]);
        assert_eq!(m.values, [Some(0), Some(1), Some(2)]);
        assert_eq!(m.reset.as_deref(), Some("IDLE"));
        let pairs: Vec<(&str, &str, &str)> =
            m.transitions.iter().map(|t| (t.from.as_str(), t.to.as_str(), t.when.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("IDLE", "RUN", "`go` ≠ 0"),
                ("RUN", "DONE", "always"),
                ("DONE", "DONE", "`go` ≠ 0"),
                ("DONE", "IDLE", "`go` = 0")
            ]
        );
    }

    #[test]
    fn counters_and_plain_logic_are_not_machines() {
        for id in ["blinker", "fifo4", "uart_tx", "parity8"] {
            let src = crate::starters::get(id).expect("starter").rtl.clone();
            assert!(parse(&src).is_empty(), "{id}");
        }
    }
}
