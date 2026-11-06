//! Event-driven scheduler and bytecode interpreter.
//!
//! Each time step runs the active region (processes ready now), then applies
//! nonblocking assignments, repeating until nothing changes; then `$strobe` and
//! `$monitor` output, then time advances to the next scheduled event.

use std::collections::{BTreeMap, VecDeque};

use crate::ast::Edge;
use crate::elab::*;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Stop at this simulation time (in timescale units).
    pub max_time: u64,
    /// Total interpreter steps before giving up.
    pub max_steps: u64,
    /// Steps within one time step before reporting a zero-delay loop.
    pub max_steps_per_time: u64,
    pub max_log_lines: usize,
    pub max_changes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_time: 100_000_000,
            max_steps: 200_000_000,
            max_steps_per_time: 5_000_000,
            max_log_lines: 20_000,
            max_changes: 5_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WaveSignal {
    pub name: String,
    pub width: u32,
    /// Change times (ascending) and the value from each time on.
    pub times: Vec<u64>,
    pub values: Vec<u64>,
    /// What wrote each value: an index into [`SimResult::origins`], or [`NO_ORIGIN`] (a starting
    /// value nothing wrote). A starting value is credited to the first assignment that writes it.
    pub by: Vec<u32>,
}

/// [`WaveSignal::by`] for a value nothing in the design wrote.
pub const NO_ORIGIN: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct SimResult {
    pub log: Vec<String>,
    /// Simulation time reached.
    pub time: u64,
    /// `$finish` was called (as opposed to running out of events or hitting a limit).
    pub finished: bool,
    /// Set when a limit stopped the run.
    pub stopped: Option<String>,
    pub timescale_fs: u64,
    pub signals: Vec<WaveSignal>,
    pub warnings: Vec<String>,
    pub steps: u64,
    /// Every branch coverage point of the design, with how often it was taken.
    pub coverage: Vec<(crate::elab::CoverPoint, u64)>,
    /// Where written values come from ([`WaveSignal::by`]).
    pub origins: Vec<crate::elab::Origin>,
    /// Every `if`/`case` decision made, in order: when, and the branch taken (a coverage point).
    /// [`crate::elab::Design::cover_decision`] says which decision a branch belongs to.
    pub decisions: Vec<(u64, u32)>,
    /// The decision of each coverage point ([`crate::elab::NO_DECISION`] for one that isn't a branch).
    pub cover_decision: Vec<u32>,
    /// Set when the run made more than [`MAX_DECISIONS`] decisions: only the first are recorded.
    pub decisions_cut: bool,
}

/// Decisions recorded per run, at most.
pub const MAX_DECISIONS: usize = 1 << 21;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Ready,
    Waiting,
    Done,
}

struct Proc {
    pc: usize,
    state: State,
    /// Bumped on every wait; stale waiter entries (older epochs) are ignored.
    epoch: u64,
    /// Values of the current wait items when the wait began.
    wait_prev: Vec<u64>,
    /// Index of the `Wait`/`WaitCond` op the process is blocked on.
    wait_op: usize,
    /// The origin of its next assignment (`Op::From`).
    from: u32,
}

enum Scheduled {
    Resume(usize),
    Nba(Vec<Write>),
}

#[derive(Clone)]
struct Write {
    slot: usize,
    sig: usize,
    lo: u32,
    w: u32,
    value: u64,
    /// The origin of the value.
    by: u32,
}

pub struct Sim<'d> {
    d: &'d Design,
    limits: Limits,
    vals: Vec<u64>,
    procs: Vec<Proc>,
    waiters: Vec<Vec<(usize, u64)>>,
    active: VecDeque<usize>,
    nba: Vec<Write>,
    wheel: BTreeMap<u64, Vec<Scheduled>>,
    now: u64,
    steps: u64,
    steps_now: u64,
    log: Vec<String>,
    partial: String,
    strobes: Vec<(Vec<CExpr>, String)>,
    /// Arguments, scope, and the argument values last printed ($time does not retrigger).
    monitor: Option<(Vec<CExpr>, String, Option<Vec<u64>>)>,
    finished: bool,
    waves: Vec<Option<WaveSignal>>,
    changes: usize,
    warnings: Vec<String>,
    /// Read by expression evaluation too (`?:` sides), hence cells.
    cover_hits: Vec<std::cell::Cell<u64>>,
    decisions: Vec<(u64, u32)>,
    decisions_cut: bool,
}

const fn mask(w: u32) -> u64 {
    if w >= 64 { u64::MAX } else { (1u64 << w) - 1 }
}

fn sext(v: u64, from: u32, to: u32) -> u64 {
    if from == 0 || from >= 64 {
        return v & mask(to);
    }
    let v = v & mask(from);
    if v >> (from - 1) & 1 == 1 { (v | !mask(from)) & mask(to) } else { v }
}

fn as_signed(v: u64, w: u32) -> i64 {
    sext(v, w, 64) as i64
}

impl<'d> Sim<'d> {
    pub fn new(d: &'d Design, limits: Limits) -> Self {
        let waves = d
            .signals
            .iter()
            .map(|s| {
                (!s.hidden && s.array.is_none()).then(|| WaveSignal {
                    name: s.name.clone(),
                    width: s.width,
                    times: vec![0],
                    values: vec![0],
                    by: vec![NO_ORIGIN],
                })
            })
            .collect();
        Sim {
            d,
            limits,
            vals: vec![0; d.slots],
            procs: d
                .processes
                .iter()
                .map(|_| Proc {
                    pc: 0,
                    state: State::Ready,
                    epoch: 0,
                    wait_prev: Vec::new(),
                    wait_op: 0,
                    from: NO_ORIGIN,
                })
                .collect(),
            waiters: vec![Vec::new(); d.signals.len()],
            active: (0..d.processes.len()).collect(),
            nba: Vec::new(),
            wheel: BTreeMap::new(),
            now: 0,
            steps: 0,
            steps_now: 0,
            log: Vec::new(),
            partial: String::new(),
            strobes: Vec::new(),
            monitor: None,
            finished: false,
            waves,
            changes: 0,
            warnings: d.warnings.clone(),
            cover_hits: (0..d.cover.len()).map(|_| std::cell::Cell::new(0)).collect(),
            decisions: Vec::new(),
            decisions_cut: false,
        }
    }

    pub fn run(mut self, timescale_fs: u64) -> Result<SimResult> {
        let stopped = self.main_loop()?;
        if !self.partial.is_empty() {
            let p = std::mem::take(&mut self.partial);
            self.log.push(p);
        }
        let signals = self.waves.into_iter().flatten().collect();
        Ok(SimResult {
            log: self.log,
            time: self.now,
            finished: self.finished,
            stopped,
            timescale_fs,
            signals,
            warnings: self.warnings,
            steps: self.steps,
            coverage: self.d.cover.iter().cloned().zip(self.cover_hits.iter().map(|c| c.get())).collect(),
            origins: self.d.origins.clone(),
            decisions: std::mem::take(&mut self.decisions),
            cover_decision: self.d.cover_decision.clone(),
            decisions_cut: self.decisions_cut,
        })
    }

    fn main_loop(&mut self) -> Result<Option<String>> {
        loop {
            // Active and NBA regions until quiet.
            loop {
                while let Some(p) = self.active.pop_front() {
                    self.exec(p)?;
                    if self.finished {
                        self.end_of_step();
                        return Ok(None);
                    }
                }
                if self.nba.is_empty() {
                    break;
                }
                for w in std::mem::take(&mut self.nba) {
                    self.write(&w);
                }
            }
            self.end_of_step();
            let Some((t, events)) = self.wheel.pop_first() else {
                return Ok(None);
            };
            if t > self.limits.max_time {
                return Ok(Some(format!(
                    "stopped at the time limit ({} units); add $finish to your testbench",
                    self.limits.max_time
                )));
            }
            self.now = t;
            self.steps_now = 0;
            for ev in events {
                match ev {
                    Scheduled::Resume(p) => self.active.push_back(p),
                    Scheduled::Nba(ws) => self.nba.extend(ws),
                }
            }
            if self.steps > self.limits.max_steps {
                return Ok(Some(format!("stopped after {} steps", self.limits.max_steps)));
            }
        }
    }

    /// Postponed region: `$monitor` first, then `$strobe` (Icarus Verilog's order).
    fn end_of_step(&mut self) {
        if let Some((args, scope, last)) = self.monitor.take() {
            let key: Vec<u64> =
                args.iter().filter(|a| !matches!(a, CExpr::Time | CExpr::Str(_))).map(|a| self.eval(a)).collect();
            if last.as_ref() != Some(&key) {
                let s = self.format(&args, &scope);
                self.emit(s, true);
            }
            self.monitor = Some((args, scope, Some(key)));
        }
        for (args, scope) in std::mem::take(&mut self.strobes) {
            let s = self.format(&args, &scope);
            self.emit(s, true);
        }
    }

    fn emit(&mut self, s: String, newline: bool) {
        if self.log.len() >= self.limits.max_log_lines {
            return;
        }
        self.partial.push_str(&s);
        if newline {
            let line = std::mem::take(&mut self.partial);
            for l in line.split('\n') {
                self.log.push(l.to_string());
            }
            if self.log.len() >= self.limits.max_log_lines {
                self.log.push(format!("[output truncated after {} lines]", self.limits.max_log_lines));
            }
        }
    }

    fn runtime_err(&self, p: usize, msg: String) -> Error {
        let pr = &self.d.processes[p];
        Error::Runtime { file: pr.file.clone(), line: pr.line, time: self.now, msg }
    }

    // ---- process execution ----

    fn exec(&mut self, p: usize) -> Result<()> {
        if self.procs[p].state == State::Done {
            return Ok(());
        }
        self.procs[p].state = State::Ready;
        let ops = &self.d.processes[p].ops;
        loop {
            self.steps += 1;
            self.steps_now += 1;
            if self.steps_now > self.limits.max_steps_per_time {
                return Err(self.runtime_err(
                    p,
                    "no progress in time: a zero-delay loop or combinational feedback (check always blocks without delays)".into(),
                ));
            }
            let pc = self.procs[p].pc;
            let Some(op) = ops.get(pc) else {
                self.procs[p].state = State::Done;
                return Ok(());
            };
            self.procs[p].pc += 1;
            match op {
                Op::From(n) => self.procs[p].from = *n,
                Op::Assign(lv, e) => {
                    let w = lv.width(self.d);
                    let v = self.eval_ctx(e, w);
                    let writes = self.resolve(lv, v, self.procs[p].from);
                    for wr in &writes {
                        self.write(wr);
                    }
                }
                Op::NbAssign(lv, e, delay) => {
                    let w = lv.width(self.d);
                    let v = self.eval_ctx(e, w);
                    let writes = self.resolve(lv, v, self.procs[p].from);
                    match delay {
                        Some(d) => {
                            let d = self.eval(d);
                            self.wheel.entry(self.now + d).or_default().push(Scheduled::Nba(writes));
                        }
                        None => self.nba.extend(writes),
                    }
                }
                Op::Jump(t) => self.procs[p].pc = *t,
                Op::JumpIfZero(c, t) => {
                    if self.eval(c) == 0 {
                        self.procs[p].pc = *t;
                    }
                }
                Op::Delay(d) => {
                    let d = self.eval(d);
                    if d == 0 {
                        // #0: after everything else currently active.
                        self.active.push_back(p);
                    } else {
                        self.wheel.entry(self.now + d).or_default().push(Scheduled::Resume(p));
                    }
                    self.procs[p].state = State::Waiting;
                    return Ok(());
                }
                Op::Wait(items) => {
                    let prev: Vec<u64> = items.iter().map(|it| self.eval(&it.expr)).collect();
                    let pr = &mut self.procs[p];
                    pr.epoch += 1;
                    pr.state = State::Waiting;
                    pr.wait_prev = prev;
                    pr.wait_op = pc;
                    let g = pr.epoch;
                    for it in items {
                        for &s in &it.sigs {
                            self.waiters[s].push((p, g));
                        }
                    }
                    return Ok(());
                }
                Op::WaitCond(c, sigs) => {
                    if self.eval(c) != 0 {
                        continue;
                    }
                    // Retest this op when an input changes.
                    let pr = &mut self.procs[p];
                    pr.pc -= 1;
                    pr.epoch += 1;
                    pr.state = State::Waiting;
                    pr.wait_prev = Vec::new();
                    pr.wait_op = pc;
                    let g = pr.epoch;
                    for &s in sigs {
                        self.waiters[s].push((p, g));
                    }
                    return Ok(());
                }
                Op::Display { kind, args, scope, .. } => match kind {
                    DisplayKind::Display => {
                        let s = self.format(args, scope);
                        self.emit(s, true);
                    }
                    DisplayKind::Write => {
                        let s = self.format(args, scope);
                        self.emit(s, false);
                    }
                    DisplayKind::Strobe => self.strobes.push((args.clone(), scope.clone())),
                    DisplayKind::Monitor => self.monitor = Some((args.clone(), scope.clone(), None)),
                },
                Op::Finish => {
                    self.finished = true;
                    self.procs[p].state = State::Done;
                    return Ok(());
                }
                Op::Halt => {
                    self.procs[p].state = State::Done;
                    return Ok(());
                }
                Op::Cover(n) => {
                    self.hit(*n);
                    if self.d.cover_decision[*n] != crate::elab::NO_DECISION {
                        if self.decisions.len() < MAX_DECISIONS {
                            self.decisions.push((self.now, *n as u32));
                        } else {
                            self.decisions_cut = true;
                        }
                    }
                }
                Op::CoverParts(parts) => {
                    for (e, id) in parts {
                        let v = self.eval(e) != 0;
                        self.hit(id + usize::from(!v));
                    }
                }
            }
        }
    }

    /// Wakes processes waiting on `sig` whose wait condition is now met.
    fn notify(&mut self, sig: usize) {
        if self.waiters[sig].is_empty() {
            return;
        }
        let list = std::mem::take(&mut self.waiters[sig]);
        let mut keep = Vec::with_capacity(list.len());
        for (p, g) in list {
            let pr = &self.procs[p];
            if pr.epoch != g || pr.state != State::Waiting {
                continue; // stale
            }
            let op = &self.d.processes[p].ops[pr.wait_op];
            let fire = match op {
                Op::Wait(items) => {
                    let mut fire = false;
                    for (i, it) in items.iter().enumerate() {
                        if !it.sigs.contains(&sig) {
                            continue;
                        }
                        let now = self.eval(&it.expr);
                        let before = self.procs[p].wait_prev[i];
                        let hit = match it.edge {
                            Edge::Any => now != before || self.d.signals[sig].array.is_some(),
                            Edge::Pos => before & 1 == 0 && now & 1 == 1,
                            Edge::Neg => before & 1 == 1 && now & 1 == 0,
                        };
                        self.procs[p].wait_prev[i] = now;
                        fire |= hit;
                    }
                    fire
                }
                Op::WaitCond(c, _) => self.eval(c) != 0,
                _ => false,
            };
            if fire {
                let pr = &mut self.procs[p];
                pr.epoch += 1;
                pr.state = State::Ready;
                self.active.push_back(p);
            } else {
                keep.push((p, g));
            }
        }
        // New waiters may have been added while we iterated.
        keep.append(&mut self.waiters[sig]);
        self.waiters[sig] = keep;
    }

    // ---- values ----

    fn write(&mut self, w: &Write) {
        let old = self.vals[w.slot];
        let m = mask(w.w) << w.lo;
        let new = ((old & !m) | ((w.value & mask(w.w)) << w.lo)) & mask(self.d.signals[w.sig].width);
        if new == old {
            // A value the design confirms without changing (its starting value) is credited to
            // the first assignment that writes it, so it can be explained too.
            if let Some(wave) = self.waves[w.sig].as_mut()
                && wave.by.last() == Some(&NO_ORIGIN)
            {
                *wave.by.last_mut().unwrap() = w.by;
            }
            return;
        }
        self.vals[w.slot] = new;
        if let Some(wave) = self.waves[w.sig].as_mut()
            && self.changes < self.limits.max_changes
        {
            if wave.times.last() == Some(&self.now) {
                *wave.values.last_mut().unwrap() = new;
                *wave.by.last_mut().unwrap() = w.by;
            } else {
                wave.times.push(self.now);
                wave.values.push(new);
                wave.by.push(w.by);
                self.changes += 1;
                if self.changes == self.limits.max_changes {
                    self.warnings.push(format!("waveform recording stopped after {} changes", self.changes));
                }
            }
        }
        self.notify(w.sig);
    }

    /// An index or part-select start: unsigned unless the expression is signed, so a sized
    /// index such as `4'd8` (or a 4-bit `b + 4'd1` that reaches 8) isn't read as negative.
    fn index(&self, e: &CExpr) -> i64 {
        let v = self.eval(e);
        if e.signed() { as_signed(v, e.width(self.d)) } else { i64::try_from(v).unwrap_or(i64::MAX / 2) }
    }

    fn word_slot(&self, sig: usize, word: Option<&CExpr>) -> Option<usize> {
        let s = &self.d.signals[sig];
        match (s.array, word) {
            (Some((lo, hi)), Some(idx)) => {
                let i = self.index(idx);
                (i >= lo && i <= hi).then(|| s.slot + (i - lo) as usize)
            }
            _ => Some(s.slot),
        }
    }

    /// Turns an assignment of `v` to `lv` into slot writes, evaluating indices now.
    fn resolve(&self, lv: &LVal, v: u64, by: u32) -> Vec<Write> {
        let mut out = Vec::new();
        self.resolve_into(lv, v, by, &mut out);
        out
    }

    fn resolve_into(&self, lv: &LVal, v: u64, by: u32, out: &mut Vec<Write>) {
        match lv {
            LVal::Sig(sig) => {
                let s = &self.d.signals[*sig];
                out.push(Write { slot: s.slot, sig: *sig, lo: 0, w: s.width, value: v, by });
            }
            LVal::Word { sig, idx } => {
                if let Some(slot) = self.word_slot(*sig, Some(idx)) {
                    out.push(Write { slot, sig: *sig, lo: 0, w: self.d.signals[*sig].width, value: v, by });
                }
            }
            LVal::Bit { sig, word, idx } => {
                let s = &self.d.signals[*sig];
                let i = self.index(idx);
                if let (Some(off), Some(slot)) = (s.offset(i), self.word_slot(*sig, word.as_ref())) {
                    out.push(Write { slot, sig: *sig, lo: off, w: 1, value: v, by });
                }
            }
            LVal::Part { sig, word, lo, w } => {
                if let Some(slot) = self.word_slot(*sig, word.as_ref()) {
                    out.push(Write { slot, sig: *sig, lo: *lo, w: *w, value: v, by });
                }
            }
            LVal::IPart { sig, start, w, up } => {
                if let Some(lo) = self.ipart_lo(*sig, start, *w, *up) {
                    out.push(Write { slot: self.d.signals[*sig].slot, sig: *sig, lo, w: *w, value: v, by });
                }
            }
            LVal::Concat(parts) => {
                // The last part takes the least significant bits.
                let mut shift = 0;
                for part in parts.iter().rev() {
                    let w = part.width(self.d);
                    self.resolve_into(part, if shift >= 64 { 0 } else { v >> shift }, by, out);
                    shift += w;
                }
            }
        }
    }

    fn ipart_lo(&self, sig: usize, start: &CExpr, w: u32, up: bool) -> Option<u32> {
        let s = &self.d.signals[sig];
        let st = self.index(start).clamp(i64::MIN / 2, i64::MAX / 2);
        let (a, b) = if up { (st, st + w as i64 - 1) } else { (st - w as i64 + 1, st) };
        let (oa, ob) = (s.offset(a)?, s.offset(b)?);
        Some(oa.min(ob))
    }

    fn hit(&self, n: usize) {
        let c = &self.cover_hits[n];
        c.set(c.get() + 1);
    }

    pub fn eval(&self, e: &CExpr) -> u64 {
        let w = e.width(self.d);
        self.eval_ctx(e, w)
    }

    /// Evaluates `e` with operands extended to `ctx` bits (Verilog context-determined width).
    fn eval_ctx(&self, e: &CExpr, ctx: u32) -> u64 {
        let own = e.width(self.d);
        let w = own.max(ctx).min(64);
        let m = mask(w);
        match e {
            CExpr::Const { v, w: cw, signed, fill } => {
                if *fill {
                    if *v != 0 { m } else { 0 }
                } else if *signed {
                    sext(*v, *cw, w)
                } else {
                    *v & m
                }
            }
            CExpr::Str(s) => s.bytes().fold(0u64, |acc, b| (acc << 8) | b as u64) & m,
            CExpr::Sig { sig, w: sw, signed } => {
                let v = self.vals[self.d.signals[*sig].slot];
                if *signed { sext(v, *sw, w) } else { v }
            }
            CExpr::Word { sig, idx } => self.word_slot(*sig, Some(idx)).map(|s| self.vals[s]).unwrap_or(0),
            CExpr::Bit { sig, word, idx } => {
                let s = &self.d.signals[*sig];
                let i = self.index(idx);
                match (s.offset(i), self.word_slot(*sig, word.as_deref())) {
                    (Some(off), Some(slot)) => (self.vals[slot] >> off) & 1,
                    _ => 0,
                }
            }
            CExpr::Part { sig, word, lo, w: pw } => {
                self.word_slot(*sig, word.as_deref()).map(|slot| (self.vals[slot] >> lo) & mask(*pw)).unwrap_or(0)
            }
            CExpr::IPart { sig, start, w: pw, up } => self
                .ipart_lo(*sig, start, *pw, *up)
                .map(|lo| (self.vals[self.d.signals[*sig].slot] >> lo) & mask(*pw))
                .unwrap_or(0),
            CExpr::Concat(parts) => {
                let mut acc = 0u64;
                for p in parts {
                    let pw = p.width(self.d);
                    acc = if pw >= 64 { self.eval(p) } else { (acc << pw) | (self.eval(p) & mask(pw)) };
                }
                acc & m
            }
            CExpr::Repl(n, inner) => {
                let iw = inner.width(self.d);
                let v = self.eval(inner) & mask(iw);
                let mut acc = 0u64;
                for _ in 0..*n {
                    acc = if iw >= 64 { v } else { (acc << iw) | v };
                }
                acc & m
            }
            CExpr::Unary(op, a) => match op {
                UOp::Not => !self.eval_ctx(a, w) & m,
                UOp::Neg => self.eval_ctx(a, w).wrapping_neg() & m,
                UOp::LNot => (self.eval(a) == 0) as u64,
                _ => {
                    let aw = a.width(self.d);
                    let v = self.eval(a) & mask(aw);
                    match op {
                        UOp::RedAnd => (v == mask(aw)) as u64,
                        UOp::RedOr => (v != 0) as u64,
                        UOp::RedXor => (v.count_ones() & 1) as u64,
                        UOp::RedNand => (v != mask(aw)) as u64,
                        UOp::RedNor => (v == 0) as u64,
                        _ => (v.count_ones() & 1 == 0) as u64,
                    }
                }
            },
            CExpr::Binary(op, a, b) => self.binary(*op, a, b, w),
            CExpr::Ternary(c, t, f, cover) => {
                let v = self.eval(c) != 0;
                if let Some(id) = cover {
                    self.hit(id + usize::from(!v));
                }
                if v { self.eval_ctx(t, w) } else { self.eval_ctx(f, w) }
            }
            CExpr::Time => self.now & m,
            CExpr::Random(_) => {
                // Deterministic per run; not IEEE's $random sequence.
                let s = self.seed_next();
                sext(s as u64, 32, w)
            }
            CExpr::Signed(a) => {
                let aw = a.width(self.d);
                sext(self.eval(a), aw, w)
            }
            CExpr::Unsigned(a) => self.eval(a) & m,
        }
    }

    fn seed_next(&self) -> u32 {
        // xorshift of the step counter: deterministic, varies between calls.
        let mut x = (self.steps as u32) ^ 0x9e37_79b9;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }

    fn binary(&self, op: BOp, a: &CExpr, b: &CExpr, w: u32) -> u64 {
        let m = mask(w);
        let signed = a.signed() && b.signed();
        match op {
            BOp::Add => self.eval_ctx(a, w).wrapping_add(self.eval_ctx(b, w)) & m,
            BOp::Sub => self.eval_ctx(a, w).wrapping_sub(self.eval_ctx(b, w)) & m,
            BOp::Mul => self.eval_ctx(a, w).wrapping_mul(self.eval_ctx(b, w)) & m,
            BOp::Div | BOp::Mod => {
                let (x, y) = (self.eval_ctx(a, w), self.eval_ctx(b, w));
                if y == 0 {
                    return 0;
                }
                if signed {
                    let (x, y) = (as_signed(x, w), as_signed(y, w));
                    (if op == BOp::Div { x.wrapping_div(y) } else { x.wrapping_rem(y) }) as u64 & m
                } else if op == BOp::Div {
                    x / y
                } else {
                    x % y
                }
            }
            BOp::Pow => {
                let x = self.eval_ctx(a, w);
                let y = self.eval(b);
                x.wrapping_pow(y.min(u32::MAX as u64) as u32) & m
            }
            BOp::And => self.eval_ctx(a, w) & self.eval_ctx(b, w),
            BOp::Or => self.eval_ctx(a, w) | self.eval_ctx(b, w),
            BOp::Xor => self.eval_ctx(a, w) ^ self.eval_ctx(b, w),
            BOp::Xnor => !(self.eval_ctx(a, w) ^ self.eval_ctx(b, w)) & m,
            BOp::Shl => {
                let s = self.eval(b);
                if s >= 64 { 0 } else { (self.eval_ctx(a, w) << s) & m }
            }
            BOp::Shr => {
                let s = self.eval(b);
                if s >= 64 { 0 } else { self.eval_ctx(a, w) >> s }
            }
            BOp::AShr => {
                let s = self.eval(b);
                let x = self.eval_ctx(a, w);
                if a.signed() {
                    let v = as_signed(x, w);
                    (v >> s.min(63)) as u64 & m
                } else if s >= 64 {
                    0
                } else {
                    x >> s
                }
            }
            BOp::LAnd => (self.eval(a) != 0 && self.eval(b) != 0) as u64,
            BOp::LOr => (self.eval(a) != 0 || self.eval(b) != 0) as u64,
            BOp::CaseMatch(xz) => {
                let cw = a.width(self.d).max(b.width(self.d));
                (((self.eval_ctx(a, cw) ^ self.eval_ctx(b, cw)) & !xz & mask(cw)) == 0) as u64
            }
            BOp::Eq | BOp::Ne | BOp::Lt | BOp::Le | BOp::Gt | BOp::Ge => {
                let cw = a.width(self.d).max(b.width(self.d));
                let (x, y) = (self.eval_ctx(a, cw), self.eval_ctx(b, cw));
                let ord = if signed { as_signed(x, cw).cmp(&as_signed(y, cw)) } else { x.cmp(&y) };
                use std::cmp::Ordering::*;
                (match op {
                    BOp::Eq => ord == Equal,
                    BOp::Ne => ord != Equal,
                    BOp::Lt => ord == Less,
                    BOp::Le => ord != Greater,
                    BOp::Gt => ord == Greater,
                    _ => ord != Less,
                }) as u64
            }
        }
    }

    // ---- $display formatting ----

    fn format(&self, args: &[CExpr], scope: &str) -> String {
        let mut out = String::new();
        let mut i = 0;
        while i < args.len() {
            if let CExpr::Str(fmt) = &args[i] {
                i += 1;
                let chars: Vec<char> = fmt.chars().collect();
                let mut k = 0;
                while k < chars.len() {
                    if chars[k] != '%' {
                        out.push(chars[k]);
                        k += 1;
                        continue;
                    }
                    k += 1;
                    let mut width = String::new();
                    while k < chars.len() && chars[k].is_ascii_digit() {
                        width.push(chars[k]);
                        k += 1;
                    }
                    let Some(&spec) = chars.get(k) else { break };
                    k += 1;
                    let spec = spec.to_ascii_lowercase();
                    if spec == '%' {
                        out.push('%');
                        continue;
                    }
                    if spec == 'm' {
                        out.push_str(scope);
                        continue;
                    }
                    let Some(arg) = args.get(i) else {
                        out.push_str("<missing argument>");
                        continue;
                    };
                    i += 1;
                    out.push_str(&self.format_value(
                        arg,
                        spec,
                        if width.is_empty() { None } else { width.parse().ok() },
                    ));
                }
            } else {
                out.push_str(&self.format_value(&args[i], 'd', None));
                i += 1;
            }
        }
        out
    }

    fn format_value(&self, e: &CExpr, spec: char, width: Option<usize>) -> String {
        if let CExpr::Str(s) = e {
            return s.clone();
        }
        let w = e.width(self.d);
        let v = self.eval(e) & mask(w);
        let pad = |s: String, n: usize, c: char| {
            if s.len() >= n { s } else { std::iter::repeat_n(c, n - s.len()).collect::<String>() + &s }
        };
        match spec {
            'd' | 'i' => {
                let s = if e.signed() { as_signed(v, w).to_string() } else { v.to_string() };
                // Default width: digits of the largest value of this width.
                let natural = if e.signed() {
                    as_signed(mask(w - 1).max(1), w).to_string().len() + 1
                } else {
                    mask(w).to_string().len()
                };
                pad(s, width.unwrap_or(natural), ' ')
            }
            'h' | 'x' => pad(format!("{v:x}"), width.unwrap_or(w.div_ceil(4) as usize), '0'),
            'b' => pad(format!("{v:b}"), width.unwrap_or(w as usize), '0'),
            'o' => pad(format!("{v:o}"), width.unwrap_or(w.div_ceil(3) as usize), '0'),
            'c' => ((v & 0xff) as u8 as char).to_string(),
            's' => {
                // Leading zero bytes print as spaces, like Icarus Verilog.
                let bytes: Vec<u8> = (0..w.div_ceil(8))
                    .rev()
                    .map(|i| (v >> (i * 8)) as u8)
                    .map(|b| if b == 0 { b' ' } else { b })
                    .collect();
                pad(String::from_utf8_lossy(&bytes).into_owned(), width.unwrap_or(0), ' ')
            }
            't' => pad(v.to_string(), width.unwrap_or(20), ' '),
            _ => format!("%{spec}"),
        }
    }
}
