//! "Find an example": searches for inputs that make the design do something no example pins down
//! yet, and writes them as a draft example for a person to check. Two targets: a coverage gap (a
//! branch no example takes) and a planted mistake that slipped through (inputs under which the
//! mistaken design behaves differently from the real one).
//!
//! The search simulates input sequences: the existing examples' inputs, as they are and with
//! changes, and fresh ones made of values the design and its examples use (0, 1, all ones, the
//! design's constants) held for a cycle or for many. It keeps the first sequence that reaches the
//! target, cuts it to the shortest that still does, undoes every input change it doesn't need, and
//! ends with checks of what the design did. The same design and spec always give the same example.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use wafer_sim::SimResult;
use wafer_sim::ast::Module;

use crate::spec::{Example, Spec, Step};

/// What to find an example for.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "for", rename_all = "snake_case")]
pub enum Target {
    /// A coverage gap, as `crate::coverage` reported it.
    Gap { file: String, line: u32, text: String },
    /// A planted mistake that slipped through, as `crate::mistakes` reported it.
    Mistake { file: String, line: u32, kind: String, original: String, mistake: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    /// The draft example (not added to the spec); None if the search found nothing.
    pub example: Option<Example>,
    /// Input sequences simulated.
    pub tried: usize,
    pub elapsed_ms: u64,
    /// Why nothing was found, in words.
    pub message: Option<String>,
}

/// A stretch of an example: every input's value from its start, held for `hold` cycles.
#[derive(Debug, Clone, PartialEq)]
struct Seg {
    set: Vec<u64>,
    hold: u32,
}

/// The longest input sequence tried, in cycles.
const MAX_CYCLES: u32 = 2000;
/// How long a stretch of inputs is held: mostly short, sometimes long enough for a counter.
const HOLDS: [u32; 12] = [1, 1, 1, 1, 2, 2, 3, 5, 8, 20, 60, 250];
const NOTE: &str = "what the design did when found: check it is right";

/// Searches for an example that reaches `target`, within `budget`. Fails (in words) if the design
/// doesn't parse, has no inputs to set, or no longer has the target. Call on a big stack.
pub fn find(spec: &Spec, files: &BTreeMap<String, String>, target: &Target, budget: Duration) -> Result<Found, String> {
    let started = Instant::now();
    let hdl: Vec<(&str, &str)> = files
        .iter()
        .filter(|(p, _)| crate::gate::is_hdl(p) && !p.starts_with("tb/"))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let (design, _) = wafer_sim::parse(&hdl).map_err(|e| format!("the design doesn't parse: {e}"))?;
    let mutant = match target {
        Target::Gap { .. } => None,
        Target::Mistake { file, line, original, mistake, .. } => Some(
            crate::mistakes::planted(&design, files, file, *line, original, mistake)
                .ok_or("that mistake can't be planted any more: the design has changed since it was checked")?,
        ),
    };
    let clock = spec.clock.as_ref().map(|c| c.port.as_str());
    let reset = spec.reset.as_ref().map(|r| r.port.as_str());
    let inputs: Vec<(String, u32)> = spec
        .ports
        .iter()
        .filter(|p| p.dir == "in" && Some(p.name.as_str()) != clock && Some(p.name.as_str()) != reset)
        .map(|p| (p.name.clone(), p.width.min(64)))
        .collect();
    if inputs.is_empty() {
        return Err("the design has no inputs to set, so there is nothing to search".into());
    }
    let outputs = spec.ports.iter().filter(|p| p.dir == "out").map(|p| (p.name.clone(), p.width.min(64))).collect();
    let ids: BTreeSet<&str> = spec.examples.iter().map(|e| e.id.as_str()).collect();
    let id = (1..).map(|n| format!("found_{n}")).find(|i| !ids.contains(i.as_str())).expect("a free id");
    let mut s = Search {
        spec: Spec { examples: Vec::new(), ..spec.clone() },
        design: &design,
        mutant,
        target,
        pools: pools(spec, &inputs, &hdl),
        inputs,
        outputs,
        id,
        tried: 0,
        exists: false,
        rng: Rng(0x9E37_79B9_7F4A_7C15),
    };

    let mut found = None;
    let starts: Vec<Vec<Seg>> = spec.examples.iter().filter_map(|e| s.segments(e)).collect();
    // The examples' inputs as they are first (a mistake may already show at an output no example
    // checks), then changed, then fresh.
    let mut queue: Vec<Vec<Seg>> = starts.clone();
    let mut round = 0;
    while started.elapsed() < budget {
        let cand = match queue.pop() {
            Some(c) => c,
            None => {
                round += 1;
                s.candidate(&starts, round)
            }
        };
        if let Some(k) = s.reaches(&cand) {
            found = Some(s.cut(&cand, k));
            break;
        }
        if round == 1 && !s.exists && matches!(target, Target::Gap { .. }) {
            return Err("that case isn't in the design any more: run the examples again".into());
        }
    }
    let example = match found {
        Some(segs) => {
            let segs = shrink(&mut s, segs, started, budget);
            s.finish(segs)
        }
        None => None,
    };
    let message = example.is_none().then(|| match target {
        Target::Gap { .. } => format!(
            "No input sequence the search tried ({} in {} s) reached this case. It may be impossible by design, or need \
             inputs in an order the search didn't try: describe the example in words instead.",
            s.tried,
            budget.as_secs()
        ),
        Target::Mistake { .. } => format!(
            "No input sequence the search tried ({} in {} s) made the mistaken design behave differently at its \
             outputs. The mistake may change nothing that can be seen, so no example can catch it.",
            s.tried,
            budget.as_secs()
        ),
    });
    Ok(Found { example, tried: s.tried, elapsed_ms: started.elapsed().as_millis() as u64, message })
}

struct Search<'a> {
    /// The spec without its examples: the search's examples are run one at a time.
    spec: Spec,
    design: &'a [Module],
    /// The design with the mistake planted, when the target is a mistake.
    mutant: Option<Vec<Module>>,
    target: &'a Target,
    inputs: Vec<(String, u32)>,
    outputs: Vec<(String, u32)>,
    /// Values worth trying, per input.
    pools: Vec<Vec<u64>>,
    id: String,
    tried: usize,
    /// Whether the gap's branch was seen in a run at all (it is, unless the design changed).
    exists: bool,
    rng: Rng,
}

impl Search<'_> {
    /// Runs `ex` on `modules`; None if it doesn't build or finish.
    fn run(&mut self, modules: &[Module], ex: &Example) -> Option<SimResult> {
        self.tried += 1;
        let one = Spec { examples: vec![ex.clone()], ..self.spec.clone() };
        let tb = one.testbench(ex);
        let (tbm, ts) = wafer_sim::parse(&[("tb/examples/search.v", tb.as_str())]).ok()?;
        let mut all = modules.to_vec();
        all.extend(tbm);
        let r = wafer_sim::run_parsed(&all, ts, &crate::spec::tb_name(&ex.id), crate::gate::limits()).ok()?;
        r.finished.then_some(r)
    }

    /// The example made of `segs`, then `tail`.
    fn example(&self, segs: &[Seg], tail: Vec<Step>) -> Example {
        let mut steps: Vec<Step> = Vec::new();
        let mut now = vec![0u64; self.inputs.len()];
        for s in segs {
            for (i, (name, w)) in self.inputs.iter().enumerate() {
                if s.set[i] != now[i] {
                    steps.push(step("set", name, &write(s.set[i], *w), 0, ""));
                    now[i] = s.set[i];
                }
            }
            match steps.last_mut() {
                Some(l) if l.op == "wait" => l.cycles += s.hold,
                _ => steps.push(step("wait", "", "", s.hold, "")),
            }
        }
        steps.extend(tail);
        Example {
            id: self.id.clone(),
            title: String::new(),
            given: String::new(),
            expect: String::new(),
            params: Vec::new(),
            steps,
            target: None,
        }
    }

    /// An existing example's inputs as stretches (its checks dropped); None for an example with
    /// settings of its own or for the chip, which run a different design.
    fn segments(&self, e: &Example) -> Option<Vec<Seg>> {
        if !e.params.is_empty() || e.target.as_deref().is_some_and(|t| t != self.spec.top) {
            return None;
        }
        let mut now = vec![0u64; self.inputs.len()];
        let mut segs = Vec::new();
        let mut pending = false;
        for st in &e.steps {
            match st.op.as_str() {
                "set" => {
                    if let Some(i) = self.inputs.iter().position(|(n, _)| *n == st.signal) {
                        now[i] = crate::spec::parse_value(&st.value).unwrap_or(0) & mask(self.inputs[i].1);
                        pending = true;
                    }
                }
                "wait" | "wait_until" | "expect_for" => {
                    segs.push(Seg { set: now.clone(), hold: st.cycles.max(1) });
                    pending = false;
                }
                _ => {}
            }
        }
        if pending || segs.is_empty() {
            segs.push(Seg { set: now, hold: 1 });
        }
        Some(segs)
    }

    /// A new sequence to try: an example's with its end changed, or a fresh one.
    fn candidate(&mut self, starts: &[Vec<Seg>], round: usize) -> Vec<Seg> {
        let mut segs = if !starts.is_empty() && !round.is_multiple_of(3) {
            let base = &starts[self.rng.below(starts.len())];
            let keep = self.rng.below(base.len() + 1);
            let mut segs = base[..keep].to_vec();
            if self.rng.below(2) == 0 && keep > 0 {
                // Change one stretch in the middle and keep the rest.
                let at = self.rng.below(keep);
                segs[at] = self.seg(&segs[at].set.clone());
                segs.extend_from_slice(&base[keep..]);
            }
            segs
        } else {
            Vec::new()
        };
        for _ in 0..1 + self.rng.below(8) {
            let prev = segs.last().map_or_else(|| vec![0; self.inputs.len()], |s: &Seg| s.set.clone());
            segs.push(self.seg(&prev));
        }
        truncate(&segs, MAX_CYCLES)
    }

    /// A stretch after `prev`: some inputs changed (a flip, a value worth trying or any value).
    fn seg(&mut self, prev: &[u64]) -> Seg {
        let p = [15, 40, 80][self.rng.below(3)];
        let mut set = prev.to_vec();
        for (i, (_, w)) in self.inputs.iter().enumerate() {
            if self.rng.below(100) >= p {
                continue;
            }
            set[i] = if *w == 1 {
                prev[i] ^ 1
            } else if self.rng.below(5) < 3 {
                self.pools[i][self.rng.below(self.pools[i].len())]
            } else {
                self.rng.next() & mask(*w)
            };
        }
        Seg { set, hold: HOLDS[self.rng.below(HOLDS.len())] }
    }

    /// The cycle by which `segs` reach the target, if they do: for a mistake the first cycle at
    /// which an output differs; for a gap the end (cut later).
    fn reaches(&mut self, segs: &[Seg]) -> Option<u32> {
        let segs = one_more(segs)?;
        let ex = self.example(&segs, Vec::new());
        let total = cycles(&segs);
        match self.target {
            Target::Gap { file, line, text } => {
                let r = self.run(self.design, &ex)?;
                let mut hit = false;
                for (p, n) in &r.coverage {
                    if p.file == *file && p.line == *line && crate::coverage::text_of(p) == *text {
                        self.exists = true;
                        hit |= *n > 0;
                    }
                }
                hit.then_some(total)
            }
            Target::Mistake { .. } => {
                let a = self.run(self.design, &ex)?;
                let mutant = self.mutant.clone().expect("a mistake has a mutant");
                let b = match self.run(&mutant, &ex) {
                    Some(b) => b,
                    // The mistake hangs or breaks the design: noticeable from the first cycle.
                    None => return Some(1),
                };
                (1..=total).find(|&k| {
                    let t = self.sample(k);
                    let top = crate::spec::tb_name(&ex.id);
                    self.outputs.iter().any(|(o, _)| at(&a, &top, o, t) != at(&b, &top, o, t))
                })
            }
        }
    }

    /// The time just before the example's `k`th cycle ends: what a check after `k` cycles sees.
    fn sample(&self, k: u32) -> u64 {
        let period = 2 * self.spec.half_ps();
        let base = if self.spec.reset.is_some() { 2 } else { 1 };
        (base + k as u64) * period - 1
    }

    /// The shortest start of `segs` that reaches the target, given it does by cycle `k`.
    fn cut(&mut self, segs: &[Seg], k: u32) -> Vec<Seg> {
        match self.target {
            Target::Mistake { .. } => truncate(&one_more(segs).unwrap_or_default(), k),
            Target::Gap { .. } => {
                // Reaching a branch only gets more likely with more cycles: search the cut point.
                let (mut lo, mut hi) = (1, k.max(1));
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if self.reaches(&truncate(segs, mid)).is_some() {
                        hi = mid;
                    } else {
                        lo = mid + 1;
                    }
                }
                truncate(segs, hi)
            }
        }
    }

    /// The draft example for `segs` (which reach the target), with checks of what the design did,
    /// if it passes on the design (and, for a mistake, fails with it planted).
    fn finish(&mut self, segs: Vec<Seg>) -> Option<Example> {
        // A gap's sequence is checked one cycle on, as `reaches` ran it; a mistake's was cut at the
        // cycle the difference shows.
        let segs = if self.mutant.is_none() { one_more(&segs)? } else { segs };
        let k = cycles(&segs);
        let probe = self.example(&segs, Vec::new());
        let top = crate::spec::tb_name(&probe.id);
        let t = self.sample(k);
        let a = self.run(self.design, &probe)?;
        let b = self.mutant.clone().map(|m| self.run(&m, &probe));
        let mut tail = Vec::new();
        for (o, w) in &self.outputs {
            let v = at(&a, &top, o, t)?;
            // For a mistake, only the outputs that tell the two apart.
            if let Some(Some(b)) = &b
                && at(b, &top, o, t) == Some(v)
            {
                continue;
            }
            tail.push(step("expect", o, &write(v, *w), 0, NOTE));
        }
        if tail.is_empty() && b.is_none() {
            return None;
        }
        let mut ex = self.example(&segs, tail);
        let passes = |r: &Option<SimResult>| r.as_ref().is_some_and(|r| crate::gate::verdict(&r.log).0 == "pass");
        if !passes(&self.run(self.design, &ex)) {
            return None;
        }
        if let Some(m) = self.mutant.clone()
            && passes(&self.run(&m, &ex))
        {
            return None;
        }
        let plain = |s: &str| s.replace('`', "");
        let (title, given) = match self.target {
            Target::Gap { text, .. } => (
                format!("Tries {}", plain(text.trim_start_matches("Never tried: "))),
                "Found by searching the design's inputs for a case no example tried.".to_string(),
            ),
            Target::Mistake { kind, original, mistake, line, .. } => (
                match kind.as_str() {
                    "reverse" => format!("Notices if {} were the other way round", plain(original)),
                    "forget" => format!("Notices if {} were left out", plain(original)),
                    _ => format!("Notices if {} were {}", plain(original), plain(mistake)),
                },
                format!(
                    "Found by searching the design's inputs for a difference this mistake (line {line}) would make."
                ),
            ),
        };
        ex.title = title;
        ex.given = given;
        ex.expect = "The checks at the end are what the design did: make sure that is what it should do.".into();
        Some(ex)
    }
}

/// Undoes every input change and shortens every hold that the target doesn't need, while time is
/// left.
fn shrink(s: &mut Search, mut segs: Vec<Seg>, started: Instant, budget: Duration) -> Vec<Seg> {
    // Shrinking may take half the budget again.
    let time = |_: &Search| started.elapsed() < budget + budget / 2;
    for i in (0..segs.len()).rev() {
        for j in 0..s.inputs.len() {
            let prev = if i == 0 { 0 } else { segs[i - 1].set[j] };
            if segs[i].set[j] == prev || !time(s) {
                continue;
            }
            let mut c = segs.clone();
            c[i].set[j] = prev;
            if let Some(k) = s.reaches(&c) {
                segs = s.cut(&c, k);
                if i >= segs.len() {
                    break;
                }
            }
        }
        while i < segs.len() && segs[i].hold > 1 && time(s) {
            let mut c = segs.clone();
            c[i].hold /= 2;
            match s.reaches(&c) {
                Some(k) => segs = s.cut(&c, k),
                None => break,
            }
        }
    }
    segs
}

/// `segs` and one more cycle with the same inputs, so the last stretch's inputs take effect.
fn one_more(segs: &[Seg]) -> Option<Vec<Seg>> {
    let mut out = segs.to_vec();
    out.push(Seg { set: segs.last()?.set.clone(), hold: 1 });
    Some(out)
}

/// The first `n` cycles of `segs`.
fn truncate(segs: &[Seg], n: u32) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut left = n;
    for s in segs {
        if left == 0 {
            break;
        }
        out.push(Seg { set: s.set.clone(), hold: s.hold.min(left) });
        left -= s.hold.min(left);
    }
    out
}

fn cycles(segs: &[Seg]) -> u32 {
    segs.iter().map(|s| s.hold).sum()
}

/// Values worth trying for each input: 0, 1, all ones, half, the numbers written in the design
/// (and one either side), and the values the examples set.
fn pools(spec: &Spec, inputs: &[(String, u32)], hdl: &[(&str, &str)]) -> Vec<Vec<u64>> {
    let mut consts = BTreeSet::new();
    for (_, src) in hdl {
        for tok in src.split(|c: char| !(c.is_ascii_alphanumeric() || c == '\'' || c == '_')) {
            if tok.starts_with(|c: char| c.is_ascii_digit() || c == '\'')
                && let Some(v) = crate::spec::parse_value(tok)
            {
                consts.extend([v, v.wrapping_sub(1), v.wrapping_add(1)]);
            }
            if consts.len() > 200 {
                break;
            }
        }
    }
    for p in &spec.params {
        if let Some(v) = crate::spec::parse_value(&p.value) {
            consts.extend([v, v.wrapping_sub(1), v.wrapping_add(1)]);
        }
    }
    inputs
        .iter()
        .map(|(name, w)| {
            let m = mask(*w);
            let mut pool: BTreeSet<u64> = [0, 1, m, m >> 1, (m >> 1) + 1].into_iter().map(|v| v & m).collect();
            pool.extend(consts.iter().map(|v| v & m));
            for e in &spec.examples {
                for st in e.steps.iter().filter(|s| s.op == "set" && s.signal == *name) {
                    if let Some(v) = crate::spec::parse_value(&st.value) {
                        pool.insert(v & m);
                    }
                }
            }
            pool.into_iter().collect()
        })
        .collect()
}

/// `port`'s value in the testbench `top` at time `t`.
fn at(r: &SimResult, top: &str, port: &str, t: u64) -> Option<u64> {
    let name = format!("{top}.{port}");
    let s = r.signals.iter().find(|s| s.name == name)?;
    let i = s.times.partition_point(|&x| x <= t);
    (i > 0).then(|| s.values[i - 1])
}

fn mask(w: u32) -> u64 {
    if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }
}

/// A value as a person would write it: decimal up to 16 bits, hexadecimal beyond.
fn write(v: u64, w: u32) -> String {
    if w <= 16 { v.to_string() } else { format!("0x{v:X}") }
}

fn step(op: &str, signal: &str, value: &str, cycles: u32, note: &str) -> Step {
    Step { op: op.into(), signal: signal.into(), value: value.into(), cycles, note: note.into() }
}

/// xorshift64*: small, fast and the same everywhere, so a search is repeatable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The up/down counter's design with only some of its examples, as a project would have it.
    fn project(keep: &[usize]) -> (Spec, BTreeMap<String, String>) {
        let st = crate::starters::get("updown").expect("starter");
        let mut spec = st.spec.clone();
        spec.examples = keep.iter().map(|&i| spec.examples[i].clone()).collect();
        (spec.clone(), files(&spec))
    }

    fn files(spec: &Spec) -> BTreeMap<String, String> {
        let st = crate::starters::get("updown").expect("starter");
        let mut files: BTreeMap<String, String> = st
            .project_files(&st.title)
            .into_iter()
            .filter(|(p, _)| !p.starts_with("tb/"))
            .filter_map(|(p, c)| String::from_utf8(c).ok().map(|c| (p, c)))
            .collect();
        files.extend(spec.testbenches());
        files
    }

    fn gaps(spec: &Spec) -> Vec<crate::coverage::Gap> {
        let report = crate::gate::run(&files(spec));
        assert!(report.passed, "{}", report.summary);
        let runs: Vec<_> = report.runs.iter().collect();
        crate::coverage::of(&runs).expect("coverage").gaps
    }

    fn search(spec: &Spec, target: &Target) -> Found {
        let f = files(spec);
        wafer_sim::on_big_stack(|| find(spec, &f, target, Duration::from_secs(20))).expect("searches")
    }

    #[test]
    fn finds_examples_that_try_untried_cases() {
        let (mut spec, _) = project(&[1]);
        let before = gaps(&spec);
        let targets: Vec<Target> = before
            .iter()
            .filter(|g| g.kind != "default")
            .take(3)
            .map(|g| Target::Gap { file: g.file.clone(), line: g.line, text: g.text.clone() })
            .collect();
        assert!(!targets.is_empty(), "the weak examples leave gaps");
        for t in &targets {
            let found = search(&spec, t);
            let ex = found.example.clone().unwrap_or_else(|| panic!("{t:?}: {:?}", found.message));
            assert!(ex.title.starts_with("Tries "), "{}", ex.title);
            assert!(ex.steps.iter().any(|s| s.op == "expect" && s.note == NOTE), "{:?}", ex.steps);
            // The same design and spec give the same example.
            assert_eq!(search(&spec, t).example, Some(ex.clone()));
            spec.examples.push(ex);
        }
        let after = gaps(&spec);
        for t in &targets {
            let Target::Gap { text, line, .. } = t else { unreachable!() };
            assert!(!after.iter().any(|g| g.text == *text && g.line == *line), "{text} is still a gap");
        }
    }

    #[test]
    fn finds_examples_that_catch_slipped_mistakes() {
        let (mut spec, f) = project(&[1]);
        let tops: Vec<String> = spec.examples.iter().map(|e| crate::spec::tb_name(&e.id)).collect();
        let check = |f: &BTreeMap<String, String>, tops: &[String]| {
            wafer_sim::on_big_stack(|| crate::mistakes::check(f, tops, 40, Duration::from_secs(60))).expect("checks")
        };
        let weak = check(&f, &tops);
        let s = weak.slipped.iter().find(|s| s.original == "count + 1").expect("count + 1 slips through");
        let target = Target::Mistake {
            file: s.file.clone(),
            line: s.line,
            kind: s.kind.clone(),
            original: s.original.clone(),
            mistake: s.mistake.clone(),
        };
        let found = search(&spec, &target);
        let ex = found.example.unwrap_or_else(|| panic!("{:?}", found.message));
        assert_eq!(ex.title, "Notices if count + 1 were count - 1");
        spec.examples.push(ex);
        let tops: Vec<String> = spec.examples.iter().map(|e| crate::spec::tb_name(&e.id)).collect();
        let strong = check(&files(&spec), &tops);
        assert!(!strong.slipped.iter().any(|x| x.original == "count + 1"), "{strong:#?}");
        assert!(strong.slipped.len() < weak.slipped.len());
    }

    #[test]
    fn unreachable_targets_say_why() {
        let (spec, f) = project(&[1]);
        let t = Target::Gap { file: "src/updown.v".into(), line: 1, text: "Never tried: nothing".into() };
        let err = wafer_sim::on_big_stack(|| find(&spec, &f, &t, Duration::from_secs(2))).unwrap_err();
        assert!(err.contains("isn't in the design"), "{err}");
        let t = Target::Mistake {
            file: "src/updown.v".into(),
            line: 1,
            kind: "compare".into(),
            original: "a".into(),
            mistake: "b".into(),
        };
        let err = wafer_sim::on_big_stack(|| find(&spec, &f, &t, Duration::from_secs(2))).unwrap_err();
        assert!(err.contains("can't be planted"), "{err}");
    }
}
