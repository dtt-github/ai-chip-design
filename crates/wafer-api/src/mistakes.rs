//! "Would your examples catch a mistake?": plants small, typical mistakes in the design, one at a
//! time, and runs the examples against each (mutation testing). A mistake some example notices is
//! caught; one every example still passes with slips through, and is reported in plain words with
//! a suggestion for an example that would catch it.
//!
//! The mistakes are made in the parsed design (the syntax tree), never in its files: comparisons
//! turned around (`<` for `<=`, `!=` for `==`), constants one off, conditions reversed, a `!` or
//! `~` left out, parity inverted, `&&` for `||`, `-` for `+`, and assignments left out. Testbenches and generated files are left alone.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use wafer_sim::ast::{Expr, Item, Module, Stmt};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    /// Mistakes some example noticed, of those tried.
    pub caught: usize,
    pub tried: usize,
    /// Places in the design a mistake could be planted; `tried` is a spread sample when there are
    /// more than the limit, or fewer if time ran out.
    pub sites: usize,
    pub slipped: Vec<Slipped>,
    pub elapsed_ms: u64,
}

/// A planted mistake no example noticed.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Slipped {
    pub file: String,
    pub line: u32,
    /// `compare`, `off_by_one`, `reverse` (a condition), `reverse_value` (a `!`, `~` or parity),
    /// `logic`, `arith` or `forget`.
    pub kind: String,
    pub original: String,
    pub mistake: String,
    /// In words, code in backticks: "If `cnt == N - 1` were `cnt == N` (line 15), every example
    /// would still pass."
    pub text: String,
    /// A starting point for an example that would catch it.
    pub suggestion: String,
}

/// Plants up to `max` mistakes in the design files of `files` and runs the testbenches named in
/// `examples` against each, within `budget`. Fails (in words) if the design doesn't parse or an
/// example fails before any mistake is planted. Call on a big stack.
pub fn check(
    files: &BTreeMap<String, String>,
    examples: &[String],
    max: usize,
    budget: Duration,
) -> Result<Report, String> {
    let started = Instant::now();
    if examples.is_empty() {
        return Err("there are no examples to check".into());
    }
    let hdl: Vec<(&str, &str)> =
        files.iter().filter(|(p, _)| crate::gate::is_hdl(p)).map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let (modules, timescale) = wafer_sim::parse(&hdl).map_err(|e| format!("the design doesn't parse: {e}"))?;
    let limits = crate::gate::limits();
    let notices = |ms: &[Module], top: &str| match wafer_sim::run_parsed(ms, timescale, top, limits) {
        Ok(r) => !r.finished || crate::gate::verdict(&r.log).0 != "pass",
        // A mistake that stops the design from building at all counts as noticed.
        Err(_) => true,
    };
    if let Some(top) = examples.iter().find(|t| notices(&modules, t)) {
        return Err(format!(
            "example {} fails before any mistake is planted; make every example pass first",
            top.trim_start_matches("tb_ex_")
        ));
    }

    // Which files are the design's own: not testbenches, not generated.
    let mine =
        |m: &Module| !m.file.starts_with("tb/") && files.get(&m.file).is_some_and(|c| !crate::blocks::is_generated(c));
    let mut probe = modules.clone();
    let mut w = Walker::new(None);
    for m in probe.iter_mut().filter(|m| mine(m)) {
        w.module(m);
    }
    let sites = w.sites;
    // A spread sample when there are more places than the limit.
    let chosen: Vec<usize> =
        if sites.len() <= max { (0..sites.len()).collect() } else { (0..max).map(|i| i * sites.len() / max).collect() };

    let (mut caught, mut tried, mut slipped) = (0, 0, Vec::new());
    for i in chosen {
        if started.elapsed() > budget {
            break;
        }
        let mut ms = modules.clone();
        let mut w = Walker::new(Some(i));
        for m in ms.iter_mut().filter(|m| mine(m)) {
            w.module(m);
        }
        tried += 1;
        if examples.iter().any(|t| notices(&ms, t)) {
            caught += 1;
        } else {
            slipped.push(sites[i].clone());
        }
    }
    Ok(Report { caught, tried, sites: sites.len(), slipped, elapsed_ms: started.elapsed().as_millis() as u64 })
}

/// The design with one planted mistake in it: the one at `file`/`line` that turns `original` into
/// `mistake`, as [`check`] reported it. None if the design no longer has it.
pub(crate) fn planted(
    modules: &[Module],
    files: &BTreeMap<String, String>,
    file: &str,
    line: u32,
    original: &str,
    mistake: &str,
) -> Option<Vec<Module>> {
    let mine =
        |m: &Module| !m.file.starts_with("tb/") && files.get(&m.file).is_some_and(|c| !crate::blocks::is_generated(c));
    let mut probe = modules.to_vec();
    let mut w = Walker::new(None);
    for m in probe.iter_mut().filter(|m| mine(m)) {
        w.module(m);
    }
    let i = w
        .sites
        .iter()
        .position(|s| s.file == file && s.line == line && s.original == original && s.mistake == mistake)?;
    let mut ms = modules.to_vec();
    let mut w = Walker::new(Some(i));
    for m in ms.iter_mut().filter(|m| mine(m)) {
        w.module(m);
    }
    Some(ms)
}

/// Walks the design in a fixed order, numbering every place a mistake could go. Without a target
/// it lists them; with one it plants that mistake and leaves the rest as they are.
pub(crate) struct Walker {
    target: Option<usize>,
    next: usize,
    file: String,
    pub(crate) sites: Vec<Slipped>,
    /// Looking for repairs rather than mistakes (`crate::fixes`): constants are tried one off in
    /// both directions, assigned constants changed too, and nothing is left out.
    fix: bool,
}

impl Walker {
    pub(crate) fn new(target: Option<usize>) -> Self {
        Walker { target, next: 0, file: String::new(), sites: Vec::new(), fix: false }
    }

    pub(crate) fn repairs(target: Option<usize>) -> Self {
        Walker { fix: true, ..Walker::new(target) }
    }

    /// Numbers one possible mistake; true if it is the one to plant.
    fn site(&mut self, line: u32, kind: &str, original: String, mistake: String) -> bool {
        let i = self.next;
        self.next += 1;
        if self.target.is_none() {
            let change = match kind {
                "reverse" => format!("the condition `{original}` were the other way round"),
                "forget" => format!("`{original}` were left out"),
                _ => format!("`{original}` were `{mistake}`"),
            };
            let at = if kind == "off_by_one" { format!("one off, line {line}") } else { format!("line {line}") };
            self.sites.push(Slipped {
                file: self.file.clone(),
                line,
                kind: kind.into(),
                text: format!("If {change} ({at}), every example would still pass."),
                suggestion: format!(
                    "An example that would notice if {} (line {line} of {}).",
                    change.replace('`', ""),
                    self.file
                ),
                original,
                mistake,
            });
        }
        self.target == Some(i)
    }

    pub(crate) fn module(&mut self, m: &mut Module) {
        self.file = m.file.clone();
        for item in &mut m.items {
            match item {
                Item::Always { body, .. } => self.stmt(body),
                Item::Assign { rhs, line, .. } => self.expr(rhs, *line),
                Item::Function(f) | Item::Task(f) => self.stmt(&mut f.body),
                _ => {}
            }
        }
    }

    fn stmt(&mut self, s: &mut Stmt) {
        let arrow = if matches!(s, Stmt::NonBlocking { .. }) { "<=" } else { "=" };
        match s {
            Stmt::Block(v) => v.iter_mut().for_each(|x| self.stmt(x)),
            Stmt::If(c, t, e) => {
                let line = c.line().unwrap_or(0);
                let flipped = Expr::Unary("!", Box::new(c.clone()));
                // A repair writes it as one would (`!rst_n`) and leaves a `!` already there to the
                // repair that takes it out.
                let mistake = if self.fix { flipped.to_string() } else { format!("!({c})") };
                if !(self.fix && matches!(c, Expr::Unary("!", _))) && self.site(line, "reverse", c.to_string(), mistake)
                {
                    *c = flipped;
                }
                self.expr(c, line);
                self.stmt(t);
                if let Some(e) = e {
                    self.stmt(e);
                }
            }
            Stmt::Case { items, default, .. } => {
                items.iter_mut().for_each(|(_, b)| self.stmt(b));
                if let Some(d) = default {
                    self.stmt(d);
                }
            }
            Stmt::Blocking { lhs, rhs, line } | Stmt::NonBlocking { lhs, rhs, line, .. } => {
                let line = *line;
                // Clearing to 0 is often the same as not clearing in a 2-state simulation (and
                // after reset); leaving it out would mostly report mistakes that change nothing.
                let zero = matches!(rhs, Expr::Num(n) if n.value == 0);
                let text = format!("{lhs} {arrow} {rhs}");
                if self.fix {
                    // A signal assigned as it is that should have been inverted (`led <= ~led`).
                    if let Expr::Ident(..) = rhs {
                        let inv = Expr::Unary("~", Box::new(rhs.clone()));
                        if self.site(line, "reverse_value", text.clone(), format!("{lhs} {arrow} {inv}")) {
                            *rhs = inv;
                            return;
                        }
                    }
                    // The value assigned, one off (for one bit: the other value).
                    if let Expr::Num(n) = rhs {
                        let n = *n;
                        let wide = n.width.is_none_or(|w| w > 1);
                        let mut tries = vec![n.value.wrapping_add(1)];
                        if n.value > 0 {
                            tries.insert(0, n.value - 1);
                        }
                        for v in tries {
                            if !wide && v > 1 {
                                continue;
                            }
                            let mut m = n;
                            m.value = v;
                            let to = format!("{lhs} {arrow} {}", Expr::Num(m));
                            if self.site(line, "value", text.clone(), to) {
                                *rhs = Expr::Num(m);
                                return;
                            }
                        }
                    }
                } else if !zero && self.site(line, "forget", text, String::new()) {
                    *s = Stmt::Null;
                    return;
                }
                self.expr(rhs, line);
            }
            Stmt::For { body, .. } | Stmt::While(_, body) | Stmt::Repeat(_, body) | Stmt::Forever(body) => {
                self.stmt(body)
            }
            Stmt::Event(_, body, _) | Stmt::Delay(_, body) | Stmt::Wait(_, body) => self.stmt(body),
            _ => {}
        }
    }

    fn expr(&mut self, e: &mut Expr, line: u32) {
        let line = e.line().unwrap_or(line);
        let original = e.to_string();
        if let Expr::Binary(op, a, b) = e {
            let swap = match *op {
                "==" => Some(("!=", "compare")),
                "!=" => Some(("==", "compare")),
                "<" => Some(("<=", "compare")),
                "<=" => Some(("<", "compare")),
                ">" => Some((">=", "compare")),
                ">=" => Some((">", "compare")),
                "&&" => Some(("||", "logic")),
                "||" => Some(("&&", "logic")),
                "+" => Some(("-", "arith")),
                "-" => Some(("+", "arith")),
                _ => None,
            };
            if let Some((new, kind)) = swap {
                let mistake = Expr::Binary(new, a.clone(), b.clone()).to_string();
                if self.site(line, kind, original.clone(), mistake) {
                    *op = new;
                    return;
                }
            }
            // A repair: a `!` missing from one side of `&&` / `||` (`up && down` → `up && !down`).
            if self.fix && matches!(*op, "&&" | "||") {
                for right in [false, true] {
                    let side: Box<Expr> = if right { b.clone() } else { a.clone() };
                    if !matches!(side.as_ref(), Expr::Ident(..)) {
                        continue;
                    }
                    let not = Box::new(Expr::Unary("!", side));
                    let (na, nb) = if right { (a.clone(), not.clone()) } else { (not.clone(), b.clone()) };
                    if self.site(line, "reverse_value", original.clone(), Expr::Binary(op, na, nb).to_string()) {
                        if right {
                            *b = not;
                        } else {
                            *a = not;
                        }
                        return;
                    }
                }
            }
            if matches!(*op, "==" | "!=" | "<" | "<=" | ">" | ">=") {
                // The constant compared with, one off: `cnt == 7` → `cnt == 8`, `cnt == N - 1` → `cnt == N`.
                let one = |v: u64| {
                    Box::new(Expr::Num(wafer_sim::lexer::Num {
                        value: v,
                        xz: 0,
                        width: None,
                        signed: false,
                        fill: false,
                        real: false,
                    }))
                };
                let mut one_off = Vec::new();
                match b.as_ref() {
                    Expr::Num(n) => {
                        let mut m = *n;
                        m.value = m.value.wrapping_add(1);
                        one_off.push(Expr::Num(m));
                        if self.fix && n.value > 0 {
                            m.value = n.value - 1;
                            one_off.push(Expr::Num(m));
                        }
                    }
                    Expr::Binary(pm @ ("-" | "+"), x, k) if matches!(k.as_ref(), Expr::Num(n) if n.value == 1) => {
                        if *pm == "-" || self.fix {
                            one_off.push(x.as_ref().clone());
                        }
                    }
                    Expr::Ident(..) if self.fix => {
                        one_off.push(Expr::Binary("-", b.clone(), one(1)));
                        one_off.push(Expr::Binary("+", b.clone(), one(1)));
                    }
                    _ => {}
                }
                if self.fix
                    && let Expr::Binary("-", x, k) = b.as_ref()
                    && matches!(k.as_ref(), Expr::Num(n) if n.value == 1)
                {
                    one_off.push(Expr::Binary("-", x.clone(), one(2)));
                }
                for nb in one_off {
                    let mistake = Expr::Binary(op, a.clone(), Box::new(nb.clone())).to_string();
                    if self.site(line, "off_by_one", original.clone(), mistake) {
                        **b = nb;
                        return;
                    }
                }
            }
            self.expr(a, line);
            self.expr(b, line);
            return;
        }
        if let Expr::Unary(op, a) = e {
            // A negation left out (`!busy` → `busy`), or parity turned around (`^d` → `~^d`).
            let flip = match *op {
                "!" | "~" => Some(a.as_ref().clone()),
                "^" => Some(Expr::Unary("~^", a.clone())),
                "~^" => Some(Expr::Unary("^", a.clone())),
                _ => None,
            };
            if let Some(m) = flip
                && self.site(line, "reverse_value", original, m.to_string())
            {
                *e = m;
                return;
            }
            if let Expr::Unary(_, a) = e {
                self.expr(a, line);
            }
            return;
        }
        match e {
            Expr::Ternary(c, t, f) => {
                self.expr(c, line);
                self.expr(t, line);
                self.expr(f, line);
            }
            Expr::Concat(v) => v.iter_mut().for_each(|x| self.expr(x, line)),
            Expr::Call(_, args, _) => args.iter_mut().for_each(|x| self.expr(x, line)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A starter's design and examples, as a project would have them.
    fn project(id: &str) -> (BTreeMap<String, String>, Vec<String>) {
        let st = crate::starters::get(id).expect("starter");
        let files = st
            .project_files(&st.title)
            .into_iter()
            .filter_map(|(p, c)| String::from_utf8(c).ok().map(|c| (p, c)))
            .collect();
        (files, st.spec.examples.iter().map(|e| crate::spec::tb_name(&e.id)).collect())
    }

    fn run(files: &BTreeMap<String, String>, examples: &[String]) -> Result<Report, String> {
        wafer_sim::on_big_stack(|| check(files, examples, 40, Duration::from_secs(60)))
    }

    #[test]
    fn strong_examples_catch_mistakes_weak_ones_let_them_through() {
        // The blinker's examples catch every mistake planted in it.
        let (files, examples) = project("blinker");
        let all = run(&files, &examples).expect("checks");
        assert!(all.tried > 5 && all.tried == all.sites.min(40) && all.slipped.is_empty(), "{all:#?}");

        // The up/down counter with only "both held keeps the count": counting up or down is never
        // tried, so getting it backwards slips through.
        let (files, examples) = project("updown");
        let full = run(&files, &examples).expect("checks");
        let weak = run(&files, &examples[1..2]).expect("checks");
        assert!(weak.slipped.len() > full.slipped.len(), "{weak:#?}\n{full:#?}");
        let s = weak.slipped.iter().find(|s| s.original == "count + 1").expect("count + 1 slips through");
        assert_eq!(
            s.text,
            format!("If `count + 1` were `count - 1` (line {}), every example would still pass.", s.line)
        );
        assert!(
            s.suggestion.starts_with("An example that would notice if count + 1 were count - 1"),
            "{}",
            s.suggestion
        );
        assert!(s.suggestion.contains("src/updown.v"));
    }

    #[test]
    fn mistakes_are_planted_in_the_tree_one_at_a_time() {
        let (files, _) = project("blinker");
        let hdl: Vec<(&str, &str)> =
            files.iter().filter(|(p, _)| p.ends_with(".v")).map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let (modules, _) = wafer_sim::parse(&hdl).unwrap();
        let design = |ms: &mut Vec<Module>| {
            ms.iter_mut().find(|m| m.name == "blinker").map(|m| format!("{:?}", m.items)).unwrap()
        };
        let mut w = Walker::new(None);
        let mut probe = modules.clone();
        probe.iter_mut().filter(|m| m.name == "blinker").for_each(|m| w.module(m));
        let kinds: Vec<&str> = w.sites.iter().map(|s| s.kind.as_str()).collect();
        for k in ["reverse", "compare", "off_by_one", "arith", "forget"] {
            assert!(kinds.contains(&k), "{k}: {kinds:?}");
        }
        let off = w.sites.iter().position(|s| s.kind == "off_by_one").unwrap();
        assert_eq!((w.sites[off].original.as_str(), w.sites[off].mistake.as_str()), ("cnt == N - 1", "cnt == N"));
        let before = design(&mut modules.clone());
        let mut ms = modules.clone();
        let mut w = Walker::new(Some(off));
        ms.iter_mut().filter(|m| m.name == "blinker").for_each(|m| w.module(m));
        let after = design(&mut ms);
        assert_ne!(before, after, "the planted copy differs");
        assert_eq!(design(&mut modules.clone()), before, "the original is untouched");
    }

    /// The starters' examples catch every planted mistake except these, which change nothing the
    /// description fixes. A new survivor means a starter's examples got weaker; a missing one means
    /// they got stronger (update the list).
    #[test]
    fn starter_examples_catch_every_mistake_that_matters() {
        let allowed: &[(&str, &str, &str)] = &[
            // The start-bit check one cycle later: only a pulse of exactly 3/4 of a bit is treated
            // differently, and the description says nothing about those.
            ("uart_rx", "cnt == CLKS_PER_BIT / 2 - 1", "cnt == CLKS_PER_BIT / 2"),
            // valid one cycle later: still "no later than one bit time after the stop bit".
            ("uart_rx", "cnt == CLKS_PER_BIT - 1", "cnt == CLKS_PER_BIT"),
            // The line is already 1 from the stop bit, so setting it again changes nothing.
            ("uart_tx", "tx <= 1", ""),
        ];
        let mut found = Vec::new();
        for st in crate::starters::all() {
            let (files, examples) = project(&st.id);
            let r = run(&files, &examples).unwrap_or_else(|e| panic!("{}: {e}", st.id));
            assert!(r.tried > 0 && r.tried == r.sites.min(40), "{}", st.id);
            found.extend(r.slipped.iter().map(|x| (st.id.clone(), x.original.clone(), x.mistake.clone())));
        }
        let found: Vec<(&str, &str, &str)> =
            found.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
        assert_eq!(found, allowed);
    }

    #[test]
    fn failing_examples_are_refused() {
        let (mut files, examples) = project("blinker");
        let src = files["src/blinker.v"].replace("cnt == N - 1", "cnt == N");
        files.insert("src/blinker.v".into(), src);
        let e = run(&files, &examples).unwrap_err();
        assert!(e.contains("fails before any mistake is planted"), "{e}");
    }
}
