//! Elaboration: modules → one flat design. Every instance's nets become
//! signals with hierarchical names, ports become continuous assignments, and
//! each `initial`/`always`/`assign` becomes a process compiled to a small
//! bytecode (see [`Op`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ast::*;
use crate::{Error, Result};

pub const MAX_WIDTH: u32 = 64;

#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub width: u32,
    pub signed: bool,
    /// Declared `[msb:lsb]`.
    pub msb: i64,
    pub lsb: i64,
    /// Memory words `[lo:hi]` (inclusive, normalised so lo <= hi).
    pub array: Option<(i64, i64)>,
    /// First storage slot; memories use one slot per word.
    pub slot: usize,
    /// Compiler temporaries are not shown in waveforms.
    pub hidden: bool,
}

impl Signal {
    /// Bit offset of index `i` within the word, or `None` if out of range.
    pub fn offset(&self, i: i64) -> Option<u32> {
        let (hi, lo) = (self.msb.max(self.lsb), self.msb.min(self.lsb));
        if i < lo || i > hi {
            return None;
        }
        Some(if self.msb >= self.lsb { (i - self.lsb) as u32 } else { (self.lsb - i) as u32 })
    }

    pub fn words(&self) -> usize {
        self.array.map(|(lo, hi)| (hi - lo + 1) as usize).unwrap_or(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UOp {
    Not,
    Neg,
    LNot,
    RedAnd,
    RedOr,
    RedXor,
    RedNand,
    RedNor,
    RedXnor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    And,
    Or,
    Xor,
    Xnor,
    Shl,
    Shr,
    AShr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LAnd,
    LOr,
    /// casez/casex label match: bits set in the mask are don't-care.
    CaseMatch(u64),
}

#[derive(Debug, Clone)]
pub enum CExpr {
    Const {
        v: u64,
        w: u32,
        signed: bool,
        fill: bool,
    },
    Str(String),
    Sig {
        sig: usize,
        w: u32,
        signed: bool,
    },
    /// Memory word read.
    Word {
        sig: usize,
        idx: Box<CExpr>,
    },
    Bit {
        sig: usize,
        word: Option<Box<CExpr>>,
        idx: Box<CExpr>,
    },
    Part {
        sig: usize,
        word: Option<Box<CExpr>>,
        lo: u32,
        w: u32,
    },
    IPart {
        sig: usize,
        start: Box<CExpr>,
        w: u32,
        up: bool,
    },
    Concat(Vec<CExpr>),
    Repl(u32, Box<CExpr>),
    Unary(UOp, Box<CExpr>),
    Binary(BOp, Box<CExpr>, Box<CExpr>),
    Ternary(Box<CExpr>, Box<CExpr>, Box<CExpr>),
    Time,
    Random(Option<usize>),
    Signed(Box<CExpr>),
    Unsigned(Box<CExpr>),
}

impl CExpr {
    /// Self-determined width.
    pub fn width(&self, d: &Design) -> u32 {
        match self {
            CExpr::Const { w, fill, .. } => {
                if *fill {
                    1
                } else {
                    *w
                }
            }
            CExpr::Str(s) => (s.len() as u32 * 8).clamp(8, 64),
            CExpr::Sig { w, .. } => *w,
            CExpr::Word { sig, .. } => d.signals[*sig].width,
            CExpr::Bit { .. } => 1,
            CExpr::Part { w, .. } | CExpr::IPart { w, .. } => *w,
            CExpr::Concat(v) => v.iter().map(|e| e.width(d)).sum::<u32>().min(64),
            CExpr::Repl(n, e) => (n * e.width(d)).min(64),
            CExpr::Unary(op, a) => match op {
                UOp::Not | UOp::Neg => a.width(d),
                _ => 1,
            },
            CExpr::Binary(op, a, b) => match op {
                BOp::Add | BOp::Sub | BOp::Mul | BOp::Div | BOp::Mod | BOp::And | BOp::Or | BOp::Xor | BOp::Xnor => {
                    a.width(d).max(b.width(d))
                }
                BOp::Shl | BOp::Shr | BOp::AShr | BOp::Pow => a.width(d),
                _ => 1,
            },
            CExpr::Ternary(_, t, f) => t.width(d).max(f.width(d)),
            CExpr::Time => 64,
            CExpr::Random(_) => 32,
            CExpr::Signed(a) | CExpr::Unsigned(a) => a.width(d),
        }
    }

    pub fn signed(&self) -> bool {
        match self {
            CExpr::Const { signed, .. } | CExpr::Sig { signed, .. } => *signed,
            CExpr::Signed(_) | CExpr::Random(_) => true,
            CExpr::Unary(UOp::Not | UOp::Neg, a) => a.signed(),
            CExpr::Binary(
                BOp::Add | BOp::Sub | BOp::Mul | BOp::Div | BOp::Mod | BOp::And | BOp::Or | BOp::Xor | BOp::Xnor,
                a,
                b,
            ) => a.signed() && b.signed(),
            CExpr::Binary(BOp::Shl | BOp::Shr | BOp::AShr | BOp::Pow, a, _) => a.signed(),
            CExpr::Ternary(_, t, f) => t.signed() && f.signed(),
            _ => false,
        }
    }

    /// Signals this expression reads (for sensitivity lists).
    pub fn reads(&self, out: &mut BTreeSet<usize>) {
        match self {
            CExpr::Sig { sig, .. } => {
                out.insert(*sig);
            }
            CExpr::Word { sig, idx } => {
                out.insert(*sig);
                idx.reads(out);
            }
            CExpr::Bit { sig, word, idx } => {
                out.insert(*sig);
                idx.reads(out);
                if let Some(w) = word {
                    w.reads(out);
                }
            }
            CExpr::Part { sig, word, .. } => {
                out.insert(*sig);
                if let Some(w) = word {
                    w.reads(out);
                }
            }
            CExpr::IPart { sig, start, .. } => {
                out.insert(*sig);
                start.reads(out);
            }
            CExpr::Concat(v) => v.iter().for_each(|e| e.reads(out)),
            CExpr::Repl(_, e) | CExpr::Unary(_, e) | CExpr::Signed(e) | CExpr::Unsigned(e) => e.reads(out),
            CExpr::Binary(_, a, b) => {
                a.reads(out);
                b.reads(out);
            }
            CExpr::Ternary(c, t, f) => {
                c.reads(out);
                t.reads(out);
                f.reads(out);
            }
            CExpr::Const { .. } | CExpr::Str(_) | CExpr::Time | CExpr::Random(_) => {}
        }
    }
}

#[derive(Debug, Clone)]
pub enum LVal {
    Sig(usize),
    Word { sig: usize, idx: CExpr },
    Bit { sig: usize, word: Option<CExpr>, idx: CExpr },
    Part { sig: usize, word: Option<CExpr>, lo: u32, w: u32 },
    IPart { sig: usize, start: CExpr, w: u32, up: bool },
    Concat(Vec<LVal>),
}

impl LVal {
    pub fn width(&self, d: &Design) -> u32 {
        match self {
            LVal::Sig(s) | LVal::Word { sig: s, .. } => d.signals[*s].width,
            LVal::Bit { .. } => 1,
            LVal::Part { w, .. } | LVal::IPart { w, .. } => *w,
            LVal::Concat(v) => v.iter().map(|l| l.width(d)).sum::<u32>().min(64),
        }
    }

    fn reads(&self, out: &mut BTreeSet<usize>) {
        match self {
            LVal::Sig(_) => {}
            LVal::Word { idx, .. } => idx.reads(out),
            LVal::Bit { word, idx, .. } => {
                idx.reads(out);
                if let Some(w) = word {
                    w.reads(out);
                }
            }
            LVal::Part { word, .. } => {
                if let Some(w) = word {
                    w.reads(out);
                }
            }
            LVal::IPart { start, .. } => start.reads(out),
            LVal::Concat(v) => v.iter().for_each(|l| l.reads(out)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    Display,
    Write,
    Strobe,
    Monitor,
}

#[derive(Debug, Clone)]
pub struct WaitItem {
    pub edge: Edge,
    pub expr: CExpr,
    pub sigs: Vec<usize>,
}

#[derive(Debug, Clone)]
pub enum Op {
    Assign(LVal, CExpr),
    NbAssign(LVal, CExpr, Option<CExpr>),
    Jump(usize),
    JumpIfZero(CExpr, usize),
    Delay(CExpr),
    Wait(Vec<WaitItem>),
    /// `wait (cond)`: continue when true, else wait for one of `sigs` to change and retest.
    WaitCond(CExpr, Vec<usize>),
    Display {
        kind: DisplayKind,
        args: Vec<CExpr>,
        scope: String,
        line: u32,
    },
    Finish,
    Halt,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub ops: Vec<Op>,
    pub scope: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Default)]
pub struct Design {
    pub top: String,
    pub signals: Vec<Signal>,
    pub slots: usize,
    pub processes: Vec<Process>,
    pub names: HashMap<String, usize>,
    pub warnings: Vec<String>,
    /// Every module instance with its resolved (non-local) parameter values, top first.
    pub instances: Vec<InstanceInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstanceInfo {
    pub path: String,
    pub module: String,
    pub params: BTreeMap<String, i64>,
}

/// Bounds on what one simulation may allocate (64-bit words of signal and memory storage).
pub const MAX_SLOTS: usize = 8 << 20;
pub const MAX_INSTANCES: usize = 100_000;

pub fn elaborate(modules: &[Module], top: &str) -> Result<Design> {
    elaborate_with(modules, top, &HashMap::new())
}

/// Like [`elaborate`], with parameter overrides for the top module (as a synthesis tool's `chparam`).
pub fn elaborate_with(modules: &[Module], top: &str, overrides: &HashMap<String, i64>) -> Result<Design> {
    let by_name: HashMap<&str, &Module> = modules.iter().map(|m| (m.name.as_str(), m)).collect();
    let m = by_name.get(top).ok_or_else(|| Error::Elab {
        file: String::new(),
        line: 0,
        msg: format!("no module named {top}"),
    })?;
    let mut e = Elab { by_name: &by_name, d: Design { top: top.to_string(), ..Default::default() }, tmp: 0 };
    if let Some(name) = overrides
        .keys()
        .find(|k| !m.items.iter().any(|i| matches!(i, Item::Param { name, local: false, .. } if name == *k)))
    {
        return Err(Error::Elab {
            file: m.file.clone(),
            line: m.line,
            msg: format!("module {top} has no parameter '{name}'"),
        });
    }
    e.instance(m, top, overrides, 0)?;
    if e.d.slots > MAX_SLOTS || e.d.instances.len() > MAX_INSTANCES {
        return Err(Error::Elab {
            file: m.file.clone(),
            line: m.line,
            msg: format!(
                "the design is too large to simulate here (over {MAX_SLOTS} storage words or {MAX_INSTANCES} instances)"
            ),
        });
    }
    Ok(e.d)
}

struct Elab<'a> {
    by_name: &'a HashMap<&'a str, &'a Module>,
    d: Design,
    tmp: usize,
}

/// Per-instance name scope.
struct Scope<'m> {
    path: String,
    module: &'m Module,
    params: HashMap<String, i64>,
}

struct MergedDecl {
    dir: Option<Dir>,
    kind: Option<Kind>,
    signed: bool,
    range: Option<(Expr, Expr)>,
    array: Option<(Expr, Expr)>,
    line: u32,
}

impl<'a> Elab<'a> {
    fn err<T>(&self, s: &Scope, line: u32, msg: impl Into<String>) -> Result<T> {
        Err(Error::Elab { file: s.module.file.clone(), line, msg: msg.into() })
    }

    fn instance(&mut self, m: &'a Module, path: &str, overrides: &HashMap<String, i64>, depth: usize) -> Result<()> {
        if depth > 64 {
            return Err(Error::Elab {
                file: m.file.clone(),
                line: m.line,
                msg: "instance hierarchy too deep (recursive?)".into(),
            });
        }
        let mut s = Scope { path: path.to_string(), module: m, params: HashMap::new() };
        // Parameters in order, so later ones can use earlier ones.
        for item in &m.items {
            if let Item::Param { name, value, local, line } = item {
                let v = match overrides.get(name) {
                    Some(v) if !local => *v,
                    _ => self.const_eval(&s, value, *line)?,
                };
                s.params.insert(name.clone(), v);
            }
        }
        let params = m
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Param { name, local: false, .. } => Some((name.clone(), s.params[name])),
                _ => None,
            })
            .collect();
        self.d.instances.push(InstanceInfo { path: path.to_string(), module: m.name.clone(), params });
        // Merge declarations per name (a port may be declared as `output` and again as `reg`).
        let mut decls: Vec<(String, MergedDecl)> = Vec::new();
        for item in &m.items {
            let Item::Decl(dl) = item else { continue };
            for n in &dl.names {
                match decls.iter_mut().find(|(k, _)| *k == n.name) {
                    Some((_, md)) => {
                        md.dir = md.dir.or(dl.dir);
                        md.kind = md.kind.or(dl.kind);
                        md.signed |= dl.signed;
                        if md.range.is_none() {
                            md.range = dl.range.clone();
                        }
                        if md.array.is_none() {
                            md.array = n.array.clone();
                        }
                    }
                    None => decls.push((
                        n.name.clone(),
                        MergedDecl {
                            dir: dl.dir,
                            kind: dl.kind,
                            signed: dl.signed,
                            range: dl.range.clone(),
                            array: n.array.clone(),
                            line: dl.line,
                        },
                    )),
                }
            }
        }
        for p in &m.ports {
            if !decls.iter().any(|(k, _)| k == p) {
                return self.err(&s, m.line, format!("port '{p}' of module {} has no direction declaration", m.name));
            }
        }
        for (name, md) in &decls {
            let (msb, lsb) = match &md.range {
                Some((a, b)) => (self.const_eval(&s, a, md.line)?, self.const_eval(&s, b, md.line)?),
                None if md.kind == Some(Kind::Integer) => (31, 0),
                None => (0, 0),
            };
            let width = (msb - lsb).unsigned_abs() as u32 + 1;
            if width > MAX_WIDTH {
                return self.err(
                    &s,
                    md.line,
                    format!("'{name}' is {width} bits wide; the browser simulator supports up to {MAX_WIDTH}"),
                );
            }
            let array = match &md.array {
                Some((a, b)) => {
                    let (a, b) = (self.const_eval(&s, a, md.line)?, self.const_eval(&s, b, md.line)?);
                    if (a - b).abs() > 1 << 20 {
                        return self.err(&s, md.line, format!("memory '{name}' is too large"));
                    }
                    Some((a.min(b), a.max(b)))
                }
                None => None,
            };
            let signed = md.signed || md.kind == Some(Kind::Integer);
            self.add_signal(format!("{path}.{name}"), width, signed, msb, lsb, array, false);
        }
        // Declaration initialisers: `reg r = 1;` runs at time 0, `wire w = e;` is a continuous assign.
        for item in &m.items {
            let Item::Decl(dl) = item else { continue };
            for n in &dl.names {
                if let Some(init) = &n.init {
                    let lv = self.lval(&s, &Expr::Ident(n.name.clone(), dl.line))?;
                    if dl.kind == Some(Kind::Wire) {
                        self.continuous(&s, lv, init, dl.line)?;
                    } else {
                        let rhs = self.expr(&s, init)?;
                        self.d.processes.push(Process {
                            ops: vec![Op::Assign(lv, rhs), Op::Halt],
                            scope: s.path.clone(),
                            file: m.file.clone(),
                            line: dl.line,
                        });
                    }
                }
            }
        }
        for item in &m.items {
            match item {
                Item::Assign { lhs, rhs, line } => {
                    let lv = self.lval(&s, lhs)?;
                    self.continuous(&s, lv, rhs, *line)?;
                }
                Item::Initial { body, line } => {
                    let mut ops = Vec::new();
                    self.stmt(&s, body, &mut ops)?;
                    ops.push(Op::Halt);
                    self.d.processes.push(Process { ops, scope: s.path.clone(), file: m.file.clone(), line: *line });
                }
                Item::Always { body, comb, line } => {
                    let ops = self.always(&s, body, *comb, *line)?;
                    self.d.processes.push(Process { ops, scope: s.path.clone(), file: m.file.clone(), line: *line });
                }
                Item::Instance { module, params, name, conns, line } => {
                    self.child(&s, module, params, name, conns, *line, depth)?;
                }
                Item::Param { .. } | Item::Decl(_) => {}
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn add_signal(
        &mut self,
        name: String,
        width: u32,
        signed: bool,
        msb: i64,
        lsb: i64,
        array: Option<(i64, i64)>,
        hidden: bool,
    ) -> usize {
        let sig = Signal { name: name.clone(), width, signed, msb, lsb, array, slot: self.d.slots, hidden };
        self.d.slots += sig.words();
        let id = self.d.signals.len();
        self.d.signals.push(sig);
        self.d.names.insert(name, id);
        id
    }

    fn temp(&mut self, s: &Scope) -> usize {
        self.tmp += 1;
        self.add_signal(format!("{}.$tmp{}", s.path, self.tmp), 64, false, 63, 0, None, true)
    }

    /// `assign lhs = rhs` → loop { lhs = rhs; wait for any input change }.
    fn continuous(&mut self, s: &Scope, lv: LVal, rhs: &Expr, line: u32) -> Result<()> {
        let rhs = self.expr(s, rhs)?;
        let mut reads = BTreeSet::new();
        rhs.reads(&mut reads);
        lv.reads(&mut reads);
        let mut ops = vec![Op::Assign(lv, rhs)];
        if !reads.is_empty() {
            ops.push(Op::Wait(self.any_change(&reads)));
            ops.push(Op::Jump(0));
        } else {
            ops.push(Op::Halt);
        }
        self.d.processes.push(Process { ops, scope: s.path.clone(), file: s.module.file.clone(), line });
        Ok(())
    }

    fn any_change(&self, sigs: &BTreeSet<usize>) -> Vec<WaitItem> {
        sigs.iter()
            .map(|&sig| {
                let sg = &self.d.signals[sig];
                // Memories: wake on any word write (handled via the signal id).
                let expr = if sg.array.is_some() {
                    CExpr::Const { v: 0, w: 1, signed: false, fill: false }
                } else {
                    CExpr::Sig { sig, w: sg.width, signed: false }
                };
                WaitItem { edge: Edge::Any, expr, sigs: vec![sig] }
            })
            .collect()
    }

    fn always(&mut self, s: &Scope, body: &Stmt, comb: bool, line: u32) -> Result<Vec<Op>> {
        let mut ops = Vec::new();
        // always @(*) / always_comb: run once, then rerun on any input change.
        let star_body = match body {
            Stmt::Event(EventCtl::Star, inner, _) => Some(inner.as_ref()),
            other if comb => Some(other),
            _ => None,
        };
        if let Some(inner) = star_body {
            self.stmt(s, inner, &mut ops)?;
            let mut reads = BTreeSet::new();
            for op in &ops {
                op_reads(op, &mut reads);
            }
            reads.retain(|&sig| !self.d.signals[sig].hidden);
            if reads.is_empty() {
                return self.err(s, line, "always @(*) block reads no signals");
            }
            ops.push(Op::Wait(self.any_change(&reads)));
            ops.push(Op::Jump(0));
            return Ok(ops);
        }
        self.stmt(s, body, &mut ops)?;
        if !ops.iter().any(|o| matches!(o, Op::Wait(_) | Op::Delay(_) | Op::WaitCond(..))) {
            return self.err(s, line, "always block without a timing control (@ or #) would loop forever");
        }
        ops.push(Op::Jump(0));
        Ok(ops)
    }

    #[allow(clippy::too_many_arguments)]
    fn child(
        &mut self,
        s: &Scope,
        module: &str,
        params: &[(Option<String>, Expr)],
        name: &str,
        conns: &[(Option<String>, Option<Expr>)],
        line: u32,
        depth: usize,
    ) -> Result<()> {
        let Some(&cm) = self.by_name.get(module) else {
            return self.err(s, line, format!("unknown module '{module}' (add its file to the simulation)"));
        };
        let declared: Vec<&String> = cm
            .items
            .iter()
            .filter_map(|i| if let Item::Param { name, local: false, .. } = i { Some(name) } else { None })
            .collect();
        let mut overrides = HashMap::new();
        for (i, (pname, value)) in params.iter().enumerate() {
            let key = match pname {
                Some(n) => n.clone(),
                None => declared.get(i).map(|n| n.to_string()).ok_or_else(|| Error::Elab {
                    file: s.module.file.clone(),
                    line,
                    msg: format!("too many parameters for {module}"),
                })?,
            };
            if !declared.iter().any(|d| **d == key) {
                return self.err(s, line, format!("module {module} has no parameter '{key}'"));
            }
            overrides.insert(key, self.const_eval(s, value, line)?);
        }
        let path = format!("{}.{name}", s.path);
        self.instance(cm, &path, &overrides, depth + 1)?;

        let cs = Scope { path: path.clone(), module: cm, params: HashMap::new() };
        for (i, (pname, expr)) in conns.iter().enumerate() {
            let port = match pname {
                Some(n) => {
                    if !cm.ports.contains(n) {
                        return self.err(s, line, format!("module {module} has no port '{n}'"));
                    }
                    n.clone()
                }
                None => cm.ports.get(i).cloned().ok_or_else(|| Error::Elab {
                    file: s.module.file.clone(),
                    line,
                    msg: format!("too many port connections for {module}"),
                })?,
            };
            let Some(expr) = expr else { continue };
            let dir = cm
                .items
                .iter()
                .find_map(|it| match it {
                    Item::Decl(d) if d.dir.is_some() && d.names.iter().any(|n| n.name == port) => d.dir,
                    _ => None,
                })
                .unwrap_or(Dir::Input);
            let port_sig = self.d.names[&format!("{path}.{port}")];
            match dir {
                Dir::Input => self.continuous_into(LVal::Sig(port_sig), s, expr, line)?,
                Dir::Output => {
                    let lv = self.lval(s, expr).map_err(|_| Error::Elab {
                        file: s.module.file.clone(),
                        line,
                        msg: format!("output port '{port}' of {name} must connect to a net or variable"),
                    })?;
                    let pw = self.d.signals[port_sig].width;
                    let ps = self.d.signals[port_sig].signed;
                    let rhs = CExpr::Sig { sig: port_sig, w: pw, signed: ps };
                    let mut reads = BTreeSet::new();
                    reads.insert(port_sig);
                    let ops = vec![Op::Assign(lv, rhs), Op::Wait(self.any_change(&reads)), Op::Jump(0)];
                    self.d.processes.push(Process { ops, scope: s.path.clone(), file: s.module.file.clone(), line });
                }
                Dir::Inout => return self.err(&cs, line, format!("inout port '{port}' is not supported")),
            }
        }
        Ok(())
    }

    fn continuous_into(&mut self, lv: LVal, s: &Scope, rhs: &Expr, line: u32) -> Result<()> {
        self.continuous(s, lv, rhs, line)
    }

    // ---- name resolution ----

    fn lookup(&self, s: &Scope, name: &str, line: u32) -> Result<usize> {
        if let Some(&id) = self.d.names.get(&format!("{}.{name}", s.path)) {
            return Ok(id);
        }
        // Absolute hierarchical name (top.a.b).
        if let Some(&id) = self.d.names.get(name) {
            return Ok(id);
        }
        self.err(s, line, format!("'{name}' is not declared in {}", s.module.name))
    }

    fn const_eval(&self, s: &Scope, e: &Expr, line: u32) -> Result<i64> {
        let ce = |e: &Expr| self.const_eval(s, e, line);
        Ok(match e {
            Expr::Num(n) => n.value as i64,
            Expr::Ident(name, l) => match s.params.get(name) {
                Some(v) => *v,
                None => return self.err(s, *l, format!("'{name}' is not a constant (parameter) here")),
            },
            Expr::Unary(op, a) => {
                let a = ce(a)?;
                match *op {
                    "-" => -a,
                    "~" => !a,
                    "!" => (a == 0) as i64,
                    _ => return self.err(s, line, format!("unsupported operator {op} in a constant")),
                }
            }
            Expr::Binary(op, a, b) => {
                let (a, b) = (ce(a)?, ce(b)?);
                match *op {
                    "+" => a.wrapping_add(b),
                    "-" => a.wrapping_sub(b),
                    "*" => a.wrapping_mul(b),
                    "/" if b != 0 => a / b,
                    "%" if b != 0 => a % b,
                    "**" => a.wrapping_pow(b.clamp(0, 63) as u32),
                    "<<" => a.wrapping_shl(b as u32),
                    ">>" | ">>>" => a.wrapping_shr(b as u32),
                    "&" => a & b,
                    "|" => a | b,
                    "^" => a ^ b,
                    "==" => (a == b) as i64,
                    "!=" => (a != b) as i64,
                    "<" => (a < b) as i64,
                    "<=" => (a <= b) as i64,
                    ">" => (a > b) as i64,
                    ">=" => (a >= b) as i64,
                    "&&" => (a != 0 && b != 0) as i64,
                    "||" => (a != 0 || b != 0) as i64,
                    _ => return self.err(s, line, format!("unsupported operator {op} in a constant")),
                }
            }
            Expr::Ternary(c, t, f) => {
                if ce(c)? != 0 {
                    ce(t)?
                } else {
                    ce(f)?
                }
            }
            Expr::Call(name, args, l) if name == "$clog2" && args.len() == 1 => {
                let v = ce(&args[0])?;
                if v <= 1 { 0 } else { 64 - ((v - 1) as u64).leading_zeros() as i64 }
            }
            Expr::Call(name, _, l) => return self.err(s, *l, format!("{name} is not allowed in a constant")),
            _ => return self.err(s, line, "expected a constant expression"),
        })
    }

    // ---- expressions ----

    fn expr(&mut self, s: &Scope, e: &Expr) -> Result<CExpr> {
        Ok(match e {
            Expr::Num(n) => {
                if n.real {
                    self.d.warnings.push(format!("{}: real number {} rounded", s.module.file, n.value));
                }
                CExpr::Const { v: n.value, w: n.width.unwrap_or(32), signed: n.signed, fill: n.fill }
            }
            Expr::Str(st) => CExpr::Str(st.clone()),
            Expr::Ident(name, line) => {
                if let Some(v) = s.params.get(name) {
                    return Ok(CExpr::Const { v: *v as u64, w: 32, signed: true, fill: false });
                }
                let sig = self.lookup(s, name, *line)?;
                let sg = &self.d.signals[sig];
                if sg.array.is_some() {
                    return self.err(s, *line, format!("memory '{name}' must be indexed"));
                }
                CExpr::Sig { sig, w: sg.width, signed: sg.signed }
            }
            Expr::Index(base, idx) if self.param_base(s, base).is_some() => {
                let v = self.param_base(s, base).unwrap();
                let line = e.line().unwrap_or(0);
                let i = self.const_eval(s, idx, line)?.clamp(0, 63);
                CExpr::Const { v: (v as u64 >> i) & 1, w: 1, signed: false, fill: false }
            }
            Expr::Range(base, a, b) if self.param_base(s, base).is_some() => {
                // Bit-select of a parameter (parameters are 32-bit, [31:0]).
                let v = self.param_base(s, base).unwrap() as u64;
                let line = e.line().unwrap_or(0);
                let (a, b) = (self.const_eval(s, a, line)?, self.const_eval(s, b, line)?);
                let (lo, hi) = (a.min(b).clamp(0, 63) as u32, a.max(b).clamp(0, 63) as u32);
                let w = hi - lo + 1;
                CExpr::Const {
                    v: (v >> lo) & if w >= 64 { u64::MAX } else { (1 << w) - 1 },
                    w,
                    signed: false,
                    fill: false,
                }
            }
            Expr::Index(base, idx) => {
                let (sig, word) = self.base(s, base)?;
                let sg = self.d.signals[sig].clone();
                let idx = self.expr(s, idx)?;
                match (&sg.array, word) {
                    (Some(_), None) => CExpr::Word { sig, idx: Box::new(idx) },
                    (_, word) => CExpr::Bit { sig, word: word.map(Box::new), idx: Box::new(idx) },
                }
            }
            Expr::Range(base, a, b) => {
                let (sig, word) = self.base(s, base)?;
                let line = e.line().unwrap_or(0);
                let (lo, w) = self.const_range(s, sig, a, b, line)?;
                CExpr::Part { sig, word: word.map(Box::new), lo, w }
            }
            Expr::IndexedPart { base, start, width, up } => {
                let (sig, word) = self.base(s, base)?;
                let line = e.line().unwrap_or(0);
                if word.is_some() {
                    return self.err(s, line, "indexed part-select of a memory word is not supported");
                }
                let w = self.const_eval(s, width, line)?;
                if !(1..=64).contains(&w) {
                    return self.err(s, line, "bad part-select width");
                }
                CExpr::IPart { sig, start: Box::new(self.expr(s, start)?), w: w as u32, up: *up }
            }
            Expr::Concat(v) => CExpr::Concat(v.iter().map(|e| self.expr(s, e)).collect::<Result<_>>()?),
            Expr::Repl(n, v) => {
                let line = e.line().unwrap_or(0);
                let n = self.const_eval(s, n, line)?;
                if !(0..=64).contains(&n) {
                    return self.err(s, line, "bad replication count");
                }
                let inner = CExpr::Concat(v.iter().map(|e| self.expr(s, e)).collect::<Result<_>>()?);
                CExpr::Repl(n as u32, Box::new(inner))
            }
            Expr::Unary(op, a) => {
                let a = Box::new(self.expr(s, a)?);
                let op = match *op {
                    "~" => UOp::Not,
                    "-" => UOp::Neg,
                    "!" => UOp::LNot,
                    "&" => UOp::RedAnd,
                    "|" => UOp::RedOr,
                    "^" => UOp::RedXor,
                    "~&" => UOp::RedNand,
                    "~|" => UOp::RedNor,
                    _ => UOp::RedXnor,
                };
                CExpr::Unary(op, a)
            }
            Expr::Binary(op, a, b) => {
                let (a, b) = (Box::new(self.expr(s, a)?), Box::new(self.expr(s, b)?));
                let op = match *op {
                    "+" => BOp::Add,
                    "-" => BOp::Sub,
                    "*" => BOp::Mul,
                    "/" => BOp::Div,
                    "%" => BOp::Mod,
                    "**" => BOp::Pow,
                    "&" => BOp::And,
                    "|" => BOp::Or,
                    "^" => BOp::Xor,
                    "~^" => BOp::Xnor,
                    "<<" | "<<<" => BOp::Shl,
                    ">>" => BOp::Shr,
                    ">>>" => BOp::AShr,
                    "==" => BOp::Eq,
                    "!=" => BOp::Ne,
                    "<" => BOp::Lt,
                    "<=" => BOp::Le,
                    ">" => BOp::Gt,
                    ">=" => BOp::Ge,
                    "&&" => BOp::LAnd,
                    _ => BOp::LOr,
                };
                CExpr::Binary(op, a, b)
            }
            Expr::Ternary(c, t, f) => {
                CExpr::Ternary(Box::new(self.expr(s, c)?), Box::new(self.expr(s, t)?), Box::new(self.expr(s, f)?))
            }
            Expr::Call(name, args, line) => match (name.as_str(), args.len()) {
                ("$time" | "$stime" | "$realtime", 0) => CExpr::Time,
                ("$random" | "$urandom", 0) => CExpr::Random(None),
                ("$random" | "$urandom", 1) => {
                    let Expr::Ident(n, l) = &args[0] else {
                        return self.err(s, *line, "$random(seed) needs a variable");
                    };
                    CExpr::Random(Some(self.lookup(s, n, *l)?))
                }
                ("$signed", 1) => CExpr::Signed(Box::new(self.expr(s, &args[0])?)),
                ("$unsigned", 1) => CExpr::Unsigned(Box::new(self.expr(s, &args[0])?)),
                ("$clog2", 1) => {
                    let v = self.const_eval(s, e, *line)?;
                    CExpr::Const { v: v as u64, w: 32, signed: true, fill: false }
                }
                _ => return self.err(s, *line, format!("{name} is not supported in expressions")),
            },
        })
    }

    fn param_base(&self, s: &Scope, base: &Expr) -> Option<i64> {
        match base {
            Expr::Ident(name, _) => s.params.get(name).copied(),
            _ => None,
        }
    }

    /// Resolves the target of a select: a signal, or a memory word (`mem[i][3:0]`).
    fn base(&mut self, s: &Scope, base: &Expr) -> Result<(usize, Option<CExpr>)> {
        match base {
            Expr::Ident(name, line) => Ok((self.lookup(s, name, *line)?, None)),
            Expr::Index(inner, idx) => match inner.as_ref() {
                Expr::Ident(name, line) => {
                    let sig = self.lookup(s, name, *line)?;
                    if self.d.signals[sig].array.is_none() {
                        return self.err(s, *line, format!("'{name}' is not a memory"));
                    }
                    Ok((sig, Some(self.expr(s, idx)?)))
                }
                _ => self.err(s, base.line().unwrap_or(0), "unsupported select"),
            },
            _ => self.err(s, base.line().unwrap_or(0), "only names can be indexed"),
        }
    }

    fn const_range(&self, s: &Scope, sig: usize, a: &Expr, b: &Expr, line: u32) -> Result<(u32, u32)> {
        let sg = &self.d.signals[sig];
        let (a, b) = (self.const_eval(s, a, line)?, self.const_eval(s, b, line)?);
        let (Some(oa), Some(ob)) = (sg.offset(a), sg.offset(b)) else {
            return self.err(
                s,
                line,
                format!("part-select [{a}:{b}] is outside '{}' [{}:{}]", sg.name, sg.msb, sg.lsb),
            );
        };
        Ok((oa.min(ob), oa.abs_diff(ob) + 1))
    }

    fn lval(&mut self, s: &Scope, e: &Expr) -> Result<LVal> {
        Ok(match e {
            Expr::Ident(name, line) => {
                if s.params.contains_key(name) {
                    return self.err(s, *line, format!("cannot assign to parameter '{name}'"));
                }
                let sig = self.lookup(s, name, *line)?;
                if self.d.signals[sig].array.is_some() {
                    return self.err(s, *line, format!("memory '{name}' must be indexed"));
                }
                LVal::Sig(sig)
            }
            Expr::Index(base, idx) => {
                let (sig, word) = self.base(s, base)?;
                let idx = self.expr(s, idx)?;
                if self.d.signals[sig].array.is_some() && word.is_none() {
                    LVal::Word { sig, idx }
                } else {
                    LVal::Bit { sig, word, idx }
                }
            }
            Expr::Range(base, a, b) => {
                let (sig, word) = self.base(s, base)?;
                let (lo, w) = self.const_range(s, sig, a, b, e.line().unwrap_or(0))?;
                LVal::Part { sig, word, lo, w }
            }
            Expr::IndexedPart { base, start, width, up } => {
                let (sig, _) = self.base(s, base)?;
                let w = self.const_eval(s, width, e.line().unwrap_or(0))?;
                LVal::IPart { sig, start: self.expr(s, start)?, w: w.clamp(1, 64) as u32, up: *up }
            }
            Expr::Concat(v) => LVal::Concat(v.iter().map(|e| self.lval(s, e)).collect::<Result<_>>()?),
            _ => return self.err(s, e.line().unwrap_or(0), "not something that can be assigned"),
        })
    }

    // ---- statements ----

    fn stmt(&mut self, s: &Scope, st: &Stmt, ops: &mut Vec<Op>) -> Result<()> {
        match st {
            Stmt::Null => {}
            Stmt::Block(v) => {
                for x in v {
                    self.stmt(s, x, ops)?;
                }
            }
            Stmt::Blocking { lhs, rhs, .. } => {
                let lv = self.lval(s, lhs)?;
                let rhs = self.expr(s, rhs)?;
                ops.push(Op::Assign(lv, rhs));
            }
            Stmt::NonBlocking { lhs, rhs, delay, .. } => {
                let lv = self.lval(s, lhs)?;
                let rhs = self.expr(s, rhs)?;
                let delay = delay.as_ref().map(|d| self.expr(s, d)).transpose()?;
                ops.push(Op::NbAssign(lv, rhs, delay));
            }
            Stmt::If(c, t, f) => {
                let c = self.expr(s, c)?;
                let jz = ops.len();
                ops.push(Op::Halt); // patched
                self.stmt(s, t, ops)?;
                match f {
                    Some(f) => {
                        let jend = ops.len();
                        ops.push(Op::Halt);
                        ops[jz] = Op::JumpIfZero(c, ops.len());
                        self.stmt(s, f, ops)?;
                        ops[jend] = Op::Jump(ops.len());
                    }
                    None => ops[jz] = Op::JumpIfZero(c, ops.len()),
                }
            }
            Stmt::Case { kind, sel, items, default, line } => {
                // Evaluate the selector once into a temporary.
                let sel_c = self.expr(s, sel)?;
                let tmp = self.temp(s);
                let sel_w = sel_c.width(&self.d);
                let label_w = items
                    .iter()
                    .flat_map(|(ls, _)| ls)
                    .map(|l| self.expr(s, l).map(|c| c.width(&self.d)))
                    .collect::<Result<Vec<_>>>()?;
                let w = label_w.into_iter().fold(sel_w, u32::max);
                // Store at the comparison width; width of the temp is 64 so no truncation.
                ops.push(Op::Assign(LVal::Sig(tmp), sel_c));
                let tmp_e = || CExpr::Sig { sig: tmp, w, signed: false };
                let mut end_jumps = Vec::new();
                for (labels, body) in items {
                    let mut cond: Option<CExpr> = None;
                    for l in labels {
                        let mask = match (kind, l) {
                            (CaseKind::Casez | CaseKind::Casex, Expr::Num(n)) => n.xz,
                            (CaseKind::Case, Expr::Num(n)) if n.xz != 0 => {
                                self.d.warnings.push(format!(
                                    "{}:{line}: x/z bits in case labels never match in 2-state simulation",
                                    s.module.file
                                ));
                                0
                            }
                            _ => 0,
                        };
                        let lc = self.expr(s, l)?;
                        let c = if mask != 0 {
                            CExpr::Binary(BOp::CaseMatch(mask), Box::new(tmp_e()), Box::new(lc))
                        } else {
                            CExpr::Binary(BOp::Eq, Box::new(tmp_e()), Box::new(lc))
                        };
                        cond = Some(match cond {
                            None => c,
                            Some(prev) => CExpr::Binary(BOp::LOr, Box::new(prev), Box::new(c)),
                        });
                    }
                    let jz = ops.len();
                    ops.push(Op::Halt);
                    self.stmt(s, body, ops)?;
                    end_jumps.push(ops.len());
                    ops.push(Op::Halt);
                    ops[jz] = Op::JumpIfZero(cond.expect("case item has labels"), ops.len());
                }
                if let Some(d) = default {
                    self.stmt(s, d, ops)?;
                }
                let end = ops.len();
                for j in end_jumps {
                    ops[j] = Op::Jump(end);
                }
            }
            Stmt::For { init, cond, step, body } => {
                self.stmt(s, init, ops)?;
                let top = ops.len();
                let c = self.expr(s, cond)?;
                let jz = ops.len();
                ops.push(Op::Halt);
                self.stmt(s, body, ops)?;
                self.stmt(s, step, ops)?;
                ops.push(Op::Jump(top));
                ops[jz] = Op::JumpIfZero(c, ops.len());
            }
            Stmt::While(c, body) => {
                let top = ops.len();
                let c = self.expr(s, c)?;
                let jz = ops.len();
                ops.push(Op::Halt);
                self.stmt(s, body, ops)?;
                ops.push(Op::Jump(top));
                ops[jz] = Op::JumpIfZero(c, ops.len());
            }
            Stmt::Repeat(n, body) => {
                let tmp = self.temp(s);
                let n = self.expr(s, n)?;
                ops.push(Op::Assign(LVal::Sig(tmp), n));
                let top = ops.len();
                let t = CExpr::Sig { sig: tmp, w: 64, signed: false };
                let jz = ops.len();
                ops.push(Op::Halt);
                ops.push(Op::Assign(
                    LVal::Sig(tmp),
                    CExpr::Binary(
                        BOp::Sub,
                        Box::new(t.clone()),
                        Box::new(CExpr::Const { v: 1, w: 64, signed: false, fill: false }),
                    ),
                ));
                self.stmt(s, body, ops)?;
                ops.push(Op::Jump(top));
                ops[jz] = Op::JumpIfZero(t, ops.len());
            }
            Stmt::Forever(body) => {
                let top = ops.len();
                self.stmt(s, body, ops)?;
                ops.push(Op::Jump(top));
            }
            Stmt::Delay(d, body) => {
                let d = self.expr(s, d)?;
                ops.push(Op::Delay(d));
                self.stmt(s, body, ops)?;
            }
            Stmt::Event(ctl, body, line) => {
                let items = match ctl {
                    EventCtl::Star => {
                        return self.err(s, *line, "@(*) is only supported at the start of an always block");
                    }
                    EventCtl::List(list) => {
                        let mut items = Vec::new();
                        for (edge, e) in list {
                            let expr = self.expr(s, e)?;
                            let mut sigs = BTreeSet::new();
                            expr.reads(&mut sigs);
                            if sigs.is_empty() {
                                return self.err(s, *line, "event expression reads no signals");
                            }
                            items.push(WaitItem { edge: *edge, expr, sigs: sigs.into_iter().collect() });
                        }
                        items
                    }
                };
                ops.push(Op::Wait(items));
                self.stmt(s, body, ops)?;
            }
            Stmt::Wait(c, body) => {
                let c = self.expr(s, c)?;
                let mut sigs = BTreeSet::new();
                c.reads(&mut sigs);
                ops.push(Op::WaitCond(c, sigs.into_iter().collect()));
                self.stmt(s, body, ops)?;
            }
            Stmt::SysTask { name, args, line } => match name.as_str() {
                "$display" | "$displayb" | "$displayh" | "$write" | "$strobe" | "$monitor" => {
                    let kind = match name.as_str() {
                        "$write" => DisplayKind::Write,
                        "$strobe" => DisplayKind::Strobe,
                        "$monitor" => DisplayKind::Monitor,
                        _ => DisplayKind::Display,
                    };
                    let args = args.iter().map(|a| self.expr(s, a)).collect::<Result<_>>()?;
                    ops.push(Op::Display { kind, args, scope: s.path.clone(), line: *line });
                }
                "$finish" | "$stop" => ops.push(Op::Finish),
                "$dumpfile" | "$dumpvars" | "$dumpon" | "$dumpoff" | "$dumpall" | "$dumpflush" | "$timeformat"
                | "$monitoron" | "$monitoroff" | "$fflush" | "$printtimescale" => {}
                "$error" | "$fatal" | "$warning" | "$info" => {
                    let mut args: Vec<CExpr> = args.iter().map(|a| self.expr(s, a)).collect::<Result<_>>()?;
                    if name == "$fatal" && !args.is_empty() && !matches!(args[0], CExpr::Str(_)) {
                        args.remove(0);
                    }
                    let prefix = match name.as_str() {
                        "$error" => "ERROR: ",
                        "$fatal" => "FATAL: ",
                        "$warning" => "WARNING: ",
                        _ => "INFO: ",
                    };
                    let mut all = vec![CExpr::Str(prefix.into())];
                    all.extend(args);
                    ops.push(Op::Display { kind: DisplayKind::Write, args: all, scope: s.path.clone(), line: *line });
                    ops.push(Op::Display {
                        kind: DisplayKind::Display,
                        args: vec![],
                        scope: s.path.clone(),
                        line: *line,
                    });
                    if name == "$fatal" {
                        ops.push(Op::Finish);
                    }
                }
                other => return self.err(s, *line, format!("{other} is not supported by the browser simulator")),
            },
        }
        Ok(())
    }
}

fn op_reads(op: &Op, out: &mut BTreeSet<usize>) {
    match op {
        Op::Assign(lv, e) | Op::NbAssign(lv, e, _) => {
            e.reads(out);
            lv.reads(out);
        }
        Op::JumpIfZero(e, _) | Op::Delay(e) | Op::WaitCond(e, _) => e.reads(out),
        Op::Display { args, .. } => args.iter().for_each(|a| a.reads(out)),
        Op::Wait(items) => items.iter().for_each(|i| i.expr.reads(out)),
        Op::Jump(_) | Op::Finish | Op::Halt => {}
    }
}
