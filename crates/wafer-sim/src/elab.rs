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
    /// `c ? t : f`, with the coverage point counting its `t` side (the `f` side is the next one).
    Ternary(Box<CExpr>, Box<CExpr>, Box<CExpr>, Option<usize>),
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
            CExpr::Ternary(_, t, f, _) => t.width(d).max(f.width(d)),
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
            CExpr::Ternary(_, t, f, _) => t.signed() && f.signed(),
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
            CExpr::Ternary(c, t, f, _) => {
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
    /// Counts one visit of coverage point `n` (`Design::cover`).
    Cover(usize),
    /// The assignments that follow come from `Design::origins[n]` (until the next `From`).
    From(u32),
    /// Counts each part of a combined condition as true (its point) or false (the next point).
    CoverParts(Vec<(CExpr, usize)>),
}

/// A decision in the design whose branches the simulation can take or not: each `if`/`else`
/// branch and `case` arm, so a testbench's coverage can be reported in words.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CoverPoint {
    pub file: String,
    pub line: u32,
    /// The instance (or generate block, function) it is in, e.g. `tb.dut`.
    pub scope: String,
    pub kind: CoverKind,
    /// The condition or the `case` selector, as written (`cnt == N - 1`).
    pub what: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CoverKind {
    /// The `if` branch: the condition was true.
    Then,
    /// The `else` branch (written, or the implicit one): the condition was false.
    Else { written: bool },
    /// A `case` arm, with its labels as written.
    Arm { labels: String },
    /// The `default` arm, or no arm matching when there is none.
    Default { written: bool },
    /// One part of a combined condition (`full` in `wr && !full`) was `value` when the decision was
    /// made; `what` is the whole condition.
    Part { atom: String, value: bool },
    /// A `?:` choice took its true (`value`) or false side; `what` is its condition.
    Side { value: bool },
}

/// Where a value written in the simulation came from: an assignment in the design (or a port
/// connection), with what it reads and the decisions that led to it, so a value can be explained.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub file: String,
    pub line: u32,
    /// The instance it is in, e.g. `tb.dut`.
    pub scope: String,
    /// As written: `count <= count + 1`, `assign busy = state != 0`, or a port connection
    /// (`output led of tb.dut`).
    pub text: String,
    /// The signals its value is computed from (full names, e.g. `tb.dut.count`).
    pub reads: Vec<String>,
    /// The `if` and `case` decisions it is inside, outermost first.
    pub conds: Vec<Cond>,
}

/// A decision an assignment is inside, and which way it went for the assignment to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Cond {
    pub line: u32,
    /// The condition or `case` selector, as written.
    pub text: String,
    pub reads: Vec<String>,
    /// `true` or `false` for an `if`; the arm's labels for a `case`, or `default`.
    pub taken: String,
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
    /// Branch coverage points, numbered by `Op::Cover`.
    pub cover: Vec<CoverPoint>,
    /// Where assignments come from, numbered by `Op::From`.
    pub origins: Vec<Origin>,
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
    let mut e = Elab {
        by_name: &by_name,
        d: Design { top: top.to_string(), ..Default::default() },
        tmp: 0,
        hoist: None,
        rets: Vec::new(),
        calling: Vec::new(),
        made: BTreeSet::new(),
        const_depth: std::cell::Cell::new(0),
        conds: Vec::new(),
    };
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
    /// While compiling an expression in a statement: the operations its function calls need first.
    hoist: Option<Vec<Op>>,
    /// The `if`/`case` decisions around the statement being compiled.
    conds: Vec<Cond>,
    /// Functions and tasks being compiled: return variable, and `return` jumps to patch.
    rets: Vec<(Option<usize>, Vec<usize>)>,
    /// Function and task calls being expanded (`inst.name`), to refuse recursion.
    calling: Vec<String>,
    /// Functions and tasks whose variables exist (`inst.name`).
    made: BTreeSet<String>,
    /// Nesting of constant-function evaluation.
    const_depth: std::cell::Cell<usize>,
}

/// A name scope: an instance, a generate block in it, or a function or task.
#[derive(Clone)]
struct Scope<'m> {
    /// Where new names go and what `%m` prints (`tb.dut.g[2]`).
    path: String,
    /// The module instance (`tb.dut`): functions and tasks belong to it.
    inst: String,
    /// Enclosing scopes to search after `path`, innermost first.
    outer: Vec<String>,
    module: &'m Module,
    params: HashMap<String, i64>,
}

/// A generate block after expansion: its names live under `prefix` in the instance.
struct GenCtx {
    /// `g[2].inner.` (relative to the instance, with a trailing dot); empty for the module itself.
    prefix: String,
    /// Prefixes to search for names, innermost first, ending with "" (the module).
    chain: Vec<String>,
    params: HashMap<String, i64>,
}

/// Generate loops may not run more often than this in one instance.
const MAX_GENERATE: usize = 100_000;

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
        let mut s = Scope {
            path: path.to_string(),
            inst: path.to_string(),
            outer: Vec::new(),
            module: m,
            params: HashMap::new(),
        };
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
        // Generate blocks expanded: every item with the generate scope it belongs to.
        let mut ctxs = vec![GenCtx { prefix: String::new(), chain: vec![String::new()], params: s.params.clone() }];
        let mut flat = Vec::new();
        let (mut genblk, mut budget) = (0, MAX_GENERATE);
        self.flatten(&s, &m.items, 0, &mut ctxs, &mut flat, &mut genblk, &mut budget)?;
        let scopes: Vec<Scope> = ctxs.iter().map(|c| self.ctx_scope(&s, c)).collect();
        // Merge declarations per name (a port may be declared as `output` and again as `reg`).
        let mut decls: Vec<(String, usize, MergedDecl)> = Vec::new();
        for &(ctx, item) in &flat {
            let Item::Decl(dl) = item else { continue };
            for n in &dl.names {
                let full = format!("{}{}", ctxs[ctx].prefix, n.name);
                match decls.iter_mut().find(|(k, _, _)| *k == full) {
                    Some((_, _, md)) => {
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
                        full,
                        ctx,
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
            if !decls.iter().any(|(k, _, _)| k == p) {
                return self.err(&s, m.line, format!("port '{p}' of module {} has no direction declaration", m.name));
            }
        }
        for (name, ctx, md) in &decls {
            let ds = &scopes[*ctx];
            let (msb, lsb) = match &md.range {
                Some((a, b)) => (self.const_eval(ds, a, md.line)?, self.const_eval(ds, b, md.line)?),
                None if md.kind == Some(Kind::Integer) => (31, 0),
                None => (0, 0),
            };
            let width = (msb - lsb).unsigned_abs() as u32 + 1;
            if width > MAX_WIDTH {
                return self.err(
                    ds,
                    md.line,
                    format!("'{name}' is {width} bits wide; the browser simulator supports up to {MAX_WIDTH}"),
                );
            }
            let array = match &md.array {
                Some((a, b)) => {
                    let (a, b) = (self.const_eval(ds, a, md.line)?, self.const_eval(ds, b, md.line)?);
                    if (a - b).abs() > 1 << 20 {
                        return self.err(ds, md.line, format!("memory '{name}' is too large"));
                    }
                    Some((a.min(b), a.max(b)))
                }
                None => None,
            };
            let signed = md.signed || md.kind == Some(Kind::Integer);
            self.add_signal(format!("{path}.{name}"), width, signed, msb, lsb, array, false);
        }
        // Declaration initialisers: `reg r = 1;` runs at time 0, `wire w = e;` is a continuous assign.
        for &(ctx, item) in &flat {
            let Item::Decl(dl) = item else { continue };
            let ds = &scopes[ctx];
            for n in &dl.names {
                if let Some(init) = &n.init {
                    let lv = self.lval(ds, &Expr::Ident(n.name.clone(), dl.line))?;
                    if dl.kind == Some(Kind::Wire) {
                        self.continuous(ds, lv, init, dl.line, format!("wire {} = {init}", n.name))?;
                    } else {
                        let mut ops = Vec::new();
                        let rhs = self.sexpr(ds, init, &mut ops)?;
                        let from = self.origin(ds, dl.line, format!("{} = {init} (its starting value)", n.name), &rhs);
                        ops.push(from);
                        ops.push(Op::Assign(lv, rhs));
                        ops.push(Op::Halt);
                        self.d.processes.push(Process {
                            ops,
                            scope: ds.path.clone(),
                            file: m.file.clone(),
                            line: dl.line,
                        });
                    }
                }
            }
        }
        for &(ctx, item) in &flat {
            let ds = &scopes[ctx];
            match item {
                Item::Assign { lhs, rhs, line } => {
                    let lv = self.lval(ds, lhs)?;
                    self.continuous(ds, lv, rhs, *line, format!("assign {lhs} = {rhs}"))?;
                }
                Item::Initial { body, line } => {
                    let mut ops = Vec::new();
                    self.stmt(ds, body, &mut ops)?;
                    ops.push(Op::Halt);
                    self.d.processes.push(Process { ops, scope: ds.path.clone(), file: m.file.clone(), line: *line });
                }
                Item::Always { body, comb, line } => {
                    let ops = self.always(ds, body, *comb, *line)?;
                    self.d.processes.push(Process { ops, scope: ds.path.clone(), file: m.file.clone(), line: *line });
                }
                Item::Instance { module, params, name, conns, line } => {
                    self.child(ds, module, params, name, conns, *line, depth)?;
                }
                // Generate constructs were expanded by `flatten`; functions and tasks are compiled where called.
                Item::Param { .. }
                | Item::Decl(_)
                | Item::Function(_)
                | Item::Task(_)
                | Item::GenFor { .. }
                | Item::GenIf { .. }
                | Item::GenCase { .. }
                | Item::GenBlock(_) => {}
            }
        }
        Ok(())
    }

    /// The scope of a generate block (or of the module itself, for the first one).
    fn ctx_scope(&self, s: &Scope<'a>, c: &GenCtx) -> Scope<'a> {
        let full = |p: &str| {
            if p.is_empty() { s.inst.clone() } else { format!("{}.{}", s.inst, p.trim_end_matches('.')) }
        };
        Scope {
            path: full(&c.prefix),
            inst: s.inst.clone(),
            outer: c.chain[1..].iter().map(|p| full(p)).collect(),
            module: s.module,
            params: c.params.clone(),
        }
    }

    /// Expands generate loops, conditions and blocks into `(scope, item)` pairs.
    #[allow(clippy::too_many_arguments)]
    fn flatten(
        &self,
        s: &Scope<'a>,
        items: &'a [Item],
        ctx: usize,
        ctxs: &mut Vec<GenCtx>,
        out: &mut Vec<(usize, &'a Item)>,
        genblk: &mut usize,
        budget: &mut usize,
    ) -> Result<()> {
        for item in items {
            match item {
                Item::GenFor { var, init, cond, step, block, line } => {
                    *genblk += 1;
                    let name = block.label.clone().unwrap_or_else(|| format!("genblk{genblk}"));
                    let mut params = ctxs[ctx].params.clone();
                    let mut v = self.const_eval(&self.ctx_scope(s, &ctxs[ctx]), init, *line)?;
                    loop {
                        params.insert(var.clone(), v);
                        let here = Scope { params: params.clone(), ..self.ctx_scope(s, &ctxs[ctx]) };
                        if self.const_eval(&here, cond, *line)? == 0 {
                            break;
                        }
                        if *budget == 0 {
                            return self.err(s, *line, format!("generate loops run more than {MAX_GENERATE} times"));
                        }
                        *budget -= 1;
                        self.enter(s, block, format!("{name}[{v}]"), params.clone(), ctx, ctxs, out, genblk, budget)?;
                        v = self.const_eval(&here, step, *line)?;
                    }
                }
                Item::GenIf { cond, then, els, line } => {
                    *genblk += 1;
                    let n = *genblk;
                    let here = self.ctx_scope(s, &ctxs[ctx]);
                    let chosen = if self.const_eval(&here, cond, *line)? != 0 { Some(then) } else { els.as_ref() };
                    if let Some(b) = chosen {
                        let name = b.label.clone().unwrap_or_else(|| format!("genblk{n}"));
                        self.enter(s, b, name, here.params, ctx, ctxs, out, genblk, budget)?;
                    }
                }
                Item::GenCase { sel, arms, default, line } => {
                    *genblk += 1;
                    let n = *genblk;
                    let here = self.ctx_scope(s, &ctxs[ctx]);
                    let v = self.const_eval(&here, sel, *line)?;
                    let mut chosen = None;
                    'arms: for (labels, b) in arms {
                        for l in labels {
                            if self.const_eval(&here, l, *line)? == v {
                                chosen = Some(b);
                                break 'arms;
                            }
                        }
                    }
                    if let Some(b) = chosen.or(default.as_ref()) {
                        let name = b.label.clone().unwrap_or_else(|| format!("genblk{n}"));
                        self.enter(s, b, name, here.params, ctx, ctxs, out, genblk, budget)?;
                    }
                }
                Item::GenBlock(b) => {
                    *genblk += 1;
                    let name = b.label.clone().unwrap_or_else(|| format!("genblk{genblk}"));
                    let params = ctxs[ctx].params.clone();
                    self.enter(s, b, name, params, ctx, ctxs, out, genblk, budget)?;
                }
                // Parameters in generate blocks were evaluated by `enter`.
                Item::Param { .. } if ctx != 0 => {}
                _ => out.push((ctx, item)),
            }
        }
        Ok(())
    }

    /// One generate block named `name` inside `parent`, with its own parameters evaluated in order.
    #[allow(clippy::too_many_arguments)]
    fn enter(
        &self,
        s: &Scope<'a>,
        b: &'a GenBlock,
        name: String,
        mut params: HashMap<String, i64>,
        parent: usize,
        ctxs: &mut Vec<GenCtx>,
        out: &mut Vec<(usize, &'a Item)>,
        genblk: &mut usize,
        budget: &mut usize,
    ) -> Result<()> {
        let prefix = format!("{}{name}.", ctxs[parent].prefix);
        let mut chain = vec![prefix.clone()];
        chain.extend(ctxs[parent].chain.iter().cloned());
        let mut c = GenCtx { prefix, chain, params: HashMap::new() };
        for it in &b.items {
            if let Item::Param { name, value, line, .. } = it {
                c.params = params.clone();
                let v = self.const_eval(&self.ctx_scope(s, &c), value, *line)?;
                params.insert(name.clone(), v);
            }
        }
        c.params = params;
        ctxs.push(c);
        let id = ctxs.len() - 1;
        self.flatten(s, &b.items, id, ctxs, out, genblk, budget)
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

    /// A new coverage point in `s`, and the op that counts it.
    fn cover(&mut self, s: &Scope, line: u32, kind: CoverKind, what: String) -> Op {
        Op::Cover(self.cover_point(s, line, kind, what))
    }

    /// A new coverage point in `s`, by number.
    fn cover_point(&mut self, s: &Scope, line: u32, kind: CoverKind, what: String) -> usize {
        self.d.cover.push(CoverPoint { file: s.module.file.clone(), line, scope: s.path.clone(), kind, what });
        self.d.cover.len() - 1
    }

    /// A new origin in `s` for an assignment written `text` computing `from`, inside the current
    /// decisions; returns the op that marks it.
    fn origin(&mut self, s: &Scope, line: u32, text: String, from: &CExpr) -> Op {
        let reads = self.read_names(from);
        self.d.origins.push(Origin {
            file: s.module.file.clone(),
            line,
            scope: s.path.clone(),
            text,
            reads,
            conds: self.conds.clone(),
        });
        Op::From((self.d.origins.len() - 1) as u32)
    }

    /// The names of the (visible) signals `e` reads.
    fn read_names(&self, e: &CExpr) -> Vec<String> {
        let mut sigs = BTreeSet::new();
        e.reads(&mut sigs);
        sigs.into_iter().filter(|&i| !self.d.signals[i].hidden).map(|i| self.d.signals[i].name.clone()).collect()
    }

    fn temp(&mut self, s: &Scope) -> usize {
        self.tmp += 1;
        self.add_signal(format!("{}.$tmp{}", s.path, self.tmp), 64, false, 63, 0, None, true)
    }

    /// `assign lhs = rhs` → loop { lhs = rhs; wait for any input change }. `text` says what it is
    /// for explanations (`assign busy = state != 0`).
    fn continuous(&mut self, s: &Scope, lv: LVal, rhs: &Expr, line: u32, text: String) -> Result<()> {
        // Function calls in the expression run first, every time it is recomputed.
        let mut ops = Vec::new();
        let rhs = self.sexpr(s, rhs, &mut ops)?;
        let saved = std::mem::take(&mut self.conds);
        let from = self.origin(s, line, text, &rhs);
        self.conds = saved;
        ops.push(from);
        ops.push(Op::Assign(lv, rhs));
        let mut reads = BTreeSet::new();
        for op in &ops {
            op_reads(op, &mut reads);
        }
        reads.retain(|&sig| !self.d.signals[sig].hidden);
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

        let cs =
            Scope { path: path.clone(), inst: path.clone(), outer: Vec::new(), module: cm, params: HashMap::new() };
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
                Dir::Input => {
                    self.continuous_into(LVal::Sig(port_sig), s, expr, line, format!("input {port} of {path}"))?
                }
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
                    let from = self.origin(s, line, format!("output {port} of {path}"), &rhs);
                    let ops = vec![from, Op::Assign(lv, rhs), Op::Wait(self.any_change(&reads)), Op::Jump(0)];
                    self.d.processes.push(Process { ops, scope: s.path.clone(), file: s.module.file.clone(), line });
                }
                Dir::Inout => return self.err(&cs, line, format!("inout port '{port}' is not supported")),
            }
        }
        Ok(())
    }

    fn continuous_into(&mut self, lv: LVal, s: &Scope, rhs: &Expr, line: u32, text: String) -> Result<()> {
        self.continuous(s, lv, rhs, line, text)
    }

    // ---- name resolution ----

    fn lookup(&self, s: &Scope, name: &str, line: u32) -> Result<usize> {
        for p in std::iter::once(&s.path).chain(&s.outer) {
            if let Some(&id) = self.d.names.get(&format!("{p}.{name}")) {
                return Ok(id);
            }
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
            Expr::Call(name, args, _) if name == "$clog2" && args.len() == 1 => {
                let v = ce(&args[0])?;
                if v <= 1 { 0 } else { 64 - ((v - 1) as u64).leading_zeros() as i64 }
            }
            Expr::Call(name, args, l) if !name.starts_with('$') => self.const_call(s, name, args, *l)?,
            Expr::Index(base, i) if self.param_base(s, base).is_some() => {
                (self.param_base(s, base).unwrap() >> ce(i)?.clamp(0, 63)) & 1
            }
            Expr::Range(base, a, b) if self.param_base(s, base).is_some() => {
                let (a, b) = (ce(a)?, ce(b)?);
                let (lo, hi) = (a.min(b).clamp(0, 63), a.max(b).clamp(0, 63));
                let w = hi - lo + 1;
                let v = (self.param_base(s, base).unwrap() as u64) >> lo;
                (if w >= 64 { v } else { v & ((1 << w) - 1) }) as i64
            }
            Expr::Call(name, _, l) => return self.err(s, *l, format!("{name} is not allowed in a constant")),
            _ => return self.err(s, line, "expected a constant expression"),
        })
    }

    /// A function evaluated at elaboration time (a "constant function", e.g. a hand-written clog2
    /// used in a parameter or a range).
    fn const_call(&self, s: &Scope, name: &str, args: &[Expr], line: u32) -> Result<i64> {
        let Some(f) = s.module.subprog(name, false) else {
            return self.err(s, line, format!("unknown function '{name}' in {}", s.module.name));
        };
        if args.len() != f.args.len() {
            return self.err(s, line, format!("{name} takes {} argument(s), not {}", f.args.len(), args.len()));
        }
        if self.const_depth.get() >= 64 {
            return self.err(s, line, format!("{name} calls itself too deeply"));
        }
        let vals = args.iter().map(|a| self.const_eval(s, a, line)).collect::<Result<Vec<_>>>()?;
        let mut fscope = s.clone();
        let mut widths = HashMap::new();
        let width =
            |fscope: &Scope, range: &Option<(Expr, Expr)>, integer: bool, signed: bool| -> Result<(u32, bool)> {
                Ok(match range {
                    Some((a, b)) => (
                        (self.const_eval(fscope, a, line)? - self.const_eval(fscope, b, line)?).unsigned_abs() as u32
                            + 1,
                        signed,
                    ),
                    None if integer => (32, true),
                    None => (1, signed),
                })
            };
        widths.insert(f.name.clone(), width(s, &f.ret_range, f.ret_integer, f.ret_signed)?);
        for d in &f.decls {
            let w = width(s, &d.range, d.kind == Some(Kind::Integer), d.signed)?;
            for n in &d.names {
                widths.insert(n.name.clone(), w);
                fscope.params.insert(n.name.clone(), 0);
            }
        }
        fscope.params.insert(f.name.clone(), 0);
        for ((n, _), v) in f.args.iter().zip(vals) {
            fscope.params.insert(n.clone(), fit(v, widths[n]));
        }
        let mut budget = 1_000_000usize;
        self.const_depth.set(self.const_depth.get() + 1);
        let r = self.const_exec(&mut fscope, &f.body, &f.name, &widths, &mut budget, line);
        self.const_depth.set(self.const_depth.get() - 1);
        r?;
        Ok(fscope.params[&f.name])
    }

    /// Runs a constant function's statement; `Ok(true)` when it executed `return`.
    fn const_exec(
        &self,
        fscope: &mut Scope,
        st: &Stmt,
        fname: &str,
        widths: &HashMap<String, (u32, bool)>,
        budget: &mut usize,
        line: u32,
    ) -> Result<bool> {
        let tick = |budget: &mut usize| {
            *budget = budget.saturating_sub(1);
            *budget > 0
        };
        match st {
            Stmt::Null | Stmt::SysTask { .. } => {}
            Stmt::Block(v) => {
                for x in v {
                    if self.const_exec(fscope, x, fname, widths, budget, line)? {
                        return Ok(true);
                    }
                }
            }
            Stmt::Blocking { lhs, rhs, line } => {
                let v = self.const_eval(fscope, rhs, *line)?;
                let (name, new) = match lhs {
                    Expr::Ident(n, _) => (n, v),
                    Expr::Index(b, i) => match b.as_ref() {
                        Expr::Ident(n, _) => {
                            let i = self.const_eval(fscope, i, *line)?.clamp(0, 63);
                            let old = fscope.params.get(n).copied().unwrap_or(0);
                            (n, (old & !(1 << i)) | ((v & 1) << i))
                        }
                        _ => return self.err(fscope, *line, "a constant function can only assign its own variables"),
                    },
                    _ => return self.err(fscope, *line, "a constant function can only assign whole variables or bits"),
                };
                let Some(w) = widths.get(name) else {
                    return self.err(fscope, *line, format!("a constant function can't assign '{name}'"));
                };
                fscope.params.insert(name.clone(), fit(new, *w));
            }
            Stmt::If(c, t, f) => {
                if self.const_eval(fscope, c, line)? != 0 {
                    return self.const_exec(fscope, t, fname, widths, budget, line);
                } else if let Some(f) = f {
                    return self.const_exec(fscope, f, fname, widths, budget, line);
                }
            }
            Stmt::Case { sel, items, default, line, .. } => {
                let v = self.const_eval(fscope, sel, *line)?;
                for (labels, body) in items {
                    for l in labels {
                        if self.const_eval(fscope, l, *line)? == v {
                            return self.const_exec(fscope, body, fname, widths, budget, *line);
                        }
                    }
                }
                if let Some(d) = default {
                    return self.const_exec(fscope, d, fname, widths, budget, *line);
                }
            }
            Stmt::For { init, cond, step, body } => {
                self.const_exec(fscope, init, fname, widths, budget, line)?;
                while self.const_eval(fscope, cond, line)? != 0 {
                    if !tick(budget) {
                        return self.err(fscope, line, "a constant function's loop runs too long");
                    }
                    if self.const_exec(fscope, body, fname, widths, budget, line)? {
                        return Ok(true);
                    }
                    self.const_exec(fscope, step, fname, widths, budget, line)?;
                }
            }
            Stmt::While(c, body) => {
                while self.const_eval(fscope, c, line)? != 0 {
                    if !tick(budget) {
                        return self.err(fscope, line, "a constant function's loop runs too long");
                    }
                    if self.const_exec(fscope, body, fname, widths, budget, line)? {
                        return Ok(true);
                    }
                }
            }
            Stmt::Repeat(n, body) => {
                for _ in 0..self.const_eval(fscope, n, line)?.max(0) {
                    if !tick(budget) {
                        return self.err(fscope, line, "a constant function's loop runs too long");
                    }
                    if self.const_exec(fscope, body, fname, widths, budget, line)? {
                        return Ok(true);
                    }
                }
            }
            Stmt::Return(e, line) => {
                if let Some(e) = e {
                    let v = self.const_eval(fscope, e, *line)?;
                    fscope.params.insert(fname.to_string(), fit(v, widths[fname]));
                }
                return Ok(true);
            }
            _ => return self.err(fscope, line, "only assignments, if, case and loops can run in a constant function"),
        }
        Ok(false)
    }

    /// Compiles `e`; the operations its function calls need are added to `ops` first.
    fn sexpr(&mut self, s: &Scope, e: &Expr, ops: &mut Vec<Op>) -> Result<CExpr> {
        let saved = self.hoist.replace(Vec::new());
        let r = self.expr(s, e);
        let pre = std::mem::replace(&mut self.hoist, saved).unwrap_or_default();
        let c = r?;
        append(ops, pre);
        Ok(c)
    }

    /// The variables of a function or task in this instance (static: one copy, hidden from waveforms).
    fn storage(&mut self, s: &Scope, sp: &Subprog, task: bool) -> Result<()> {
        let key = format!("{}.{}", s.inst, sp.name);
        if !self.made.insert(key.clone()) {
            return Ok(());
        }
        let width = |e: &Self, range: &Option<(Expr, Expr)>, integer: bool| -> Result<(i64, i64)> {
            Ok(match range {
                Some((a, b)) => (e.const_eval(s, a, sp.line)?, e.const_eval(s, b, sp.line)?),
                None if integer => (31, 0),
                None => (0, 0),
            })
        };
        let mut vars = Vec::new();
        for d in &sp.decls {
            let (msb, lsb) = width(self, &d.range, d.kind == Some(Kind::Integer))?;
            for n in &d.names {
                let array = match &n.array {
                    Some((a, b)) => {
                        let (a, b) = (self.const_eval(s, a, d.line)?, self.const_eval(s, b, d.line)?);
                        Some((a.min(b), a.max(b)))
                    }
                    None => None,
                };
                vars.push((n.name.clone(), msb, lsb, d.signed || d.kind == Some(Kind::Integer), array));
            }
        }
        if let Some((n, _)) = sp.args.iter().find(|(n, _)| !vars.iter().any(|v| v.0 == *n)) {
            return self.err(s, sp.line, format!("argument '{n}' of {} has no declaration", sp.name));
        }
        if !task {
            let (msb, lsb) = width(self, &sp.ret_range, sp.ret_integer)?;
            vars.push((sp.name.clone(), msb, lsb, sp.ret_signed || sp.ret_integer, None));
        }
        for (n, msb, lsb, signed, array) in vars {
            let w = (msb - lsb).unsigned_abs() as u32 + 1;
            if w > MAX_WIDTH {
                return self.err(
                    s,
                    sp.line,
                    format!("'{n}' in {} is {w} bits wide; the limit is {MAX_WIDTH}", sp.name),
                );
            }
            self.add_signal(format!("{key}.{n}"), w, signed, msb, lsb, array, true);
        }
        Ok(())
    }

    /// Expands a call of a function (`task` false; returns where the result is) or a task into
    /// `ops`: arguments in, the body, outputs back.
    fn call(
        &mut self,
        s: &Scope,
        name: &str,
        args: &[Expr],
        line: u32,
        task: bool,
        ops: &mut Vec<Op>,
    ) -> Result<Option<(usize, u32, bool)>> {
        let module = s.module;
        let Some(sp) = module.subprog(name, task) else {
            let what = if task { "task" } else { "function" };
            return self.err(s, line, format!("unknown {what} '{name}' in {}", module.name));
        };
        if args.len() != sp.args.len() {
            return self.err(s, line, format!("{name} takes {} argument(s), not {}", sp.args.len(), args.len()));
        }
        let key = format!("{}.{name}", s.inst);
        if self.calling.contains(&key) {
            return self.err(s, line, format!("{name} calls itself; recursive functions and tasks are not supported"));
        }
        self.storage(s, sp, task)?;
        let var = |e: &Self, n: &str| e.d.names[&format!("{key}.{n}")];
        for ((n, dir), a) in sp.args.iter().zip(args) {
            if *dir != Dir::Output {
                let v = self.sexpr(s, a, ops)?;
                ops.push(Op::Assign(LVal::Sig(var(self, n)), v));
            }
        }
        // The body sees its own variables, then the module's (not the caller's).
        let fs = Scope {
            path: key.clone(),
            inst: s.inst.clone(),
            outer: vec![s.inst.clone()],
            module,
            params: s.params.clone(),
        };
        let ret = (!task).then(|| var(self, name));
        let saved = self.hoist.take();
        self.calling.push(key.clone());
        self.rets.push((ret, Vec::new()));
        let mut body = Vec::new();
        let r = self.stmt(&fs, &sp.body, &mut body);
        let (_, returns) = self.rets.pop().expect("pushed above");
        self.calling.pop();
        self.hoist = saved;
        r?;
        let end = body.len();
        for i in returns {
            body[i] = Op::Jump(end);
        }
        append(ops, body);
        for ((n, dir), a) in sp.args.iter().zip(args) {
            if *dir != Dir::Input {
                let lv = self.lval(s, a).map_err(|_| Error::Elab {
                    file: module.file.clone(),
                    line,
                    msg: format!("output argument '{n}' of {name} must be a variable"),
                })?;
                let sg = &self.d.signals[var(self, n)];
                ops.push(Op::Assign(lv, CExpr::Sig { sig: var(self, n), w: sg.width, signed: sg.signed }));
            }
        }
        let Some(ret) = ret else { return Ok(None) };
        // A copy per call, so f(a) + f(b) keeps both results.
        let (w, signed) = (self.d.signals[ret].width, self.d.signals[ret].signed);
        self.tmp += 1;
        let tmp = self.add_signal(format!("{}.$call{}", s.path, self.tmp), w, signed, w as i64 - 1, 0, None, true);
        ops.push(Op::Assign(LVal::Sig(tmp), CExpr::Sig { sig: ret, w, signed }));
        Ok(Some((tmp, w, signed)))
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
                let (line, what) = (c.line().unwrap_or(0), c.to_string());
                let id = self.cover_point(s, line, CoverKind::Side { value: true }, what.clone());
                self.cover_point(s, line, CoverKind::Side { value: false }, what);
                CExpr::Ternary(
                    Box::new(self.expr(s, c)?),
                    Box::new(self.expr(s, t)?),
                    Box::new(self.expr(s, f)?),
                    Some(id),
                )
            }
            Expr::Call(name, args, line) if !name.starts_with('$') => {
                if self.hoist.is_none() {
                    return self.err(
                        s,
                        *line,
                        format!("{name}(...) can't be called here (an event control, a wait condition or the index of an assignment target); call it in a statement and use a variable"),
                    );
                }
                let mut ops = Vec::new();
                let (sig, w, signed) =
                    self.call(s, name, args, *line, false, &mut ops)?.expect("functions return a value");
                append(self.hoist.as_mut().expect("checked above"), ops);
                CExpr::Sig { sig, w, signed }
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
            Stmt::Blocking { lhs, rhs: r, line } => {
                let lv = self.lval(s, lhs)?;
                let rhs = self.sexpr(s, r, ops)?;
                let from = self.origin(s, *line, format!("{lhs} = {r}"), &rhs);
                ops.push(from);
                ops.push(Op::Assign(lv, rhs));
            }
            Stmt::NonBlocking { lhs, rhs: r, delay, line } => {
                let lv = self.lval(s, lhs)?;
                let rhs = self.sexpr(s, r, ops)?;
                let from = self.origin(s, *line, format!("{lhs} <= {r}"), &rhs);
                ops.push(from);
                let delay = match delay {
                    Some(d) => Some(self.sexpr(s, d, ops)?),
                    None => None,
                };
                ops.push(Op::NbAssign(lv, rhs, delay));
            }
            Stmt::If(cond, t, f) => {
                let line = cond.line().unwrap_or(0);
                let what = cond.to_string();
                let c = self.sexpr(s, cond, ops)?;
                // A combined condition: count each part both ways at the moment of the decision.
                let (mut ea, mut ca) = (Vec::new(), Vec::new());
                ast_parts(cond, &mut ea);
                c_parts(&c, &mut ca);
                if ea.len() > 1 && ea.len() == ca.len() {
                    let mut parts = Vec::new();
                    for (e, ce) in ea.iter().zip(ca) {
                        let id = self.cover_point(
                            s,
                            line,
                            CoverKind::Part { atom: e.to_string(), value: true },
                            what.clone(),
                        );
                        self.cover_point(s, line, CoverKind::Part { atom: e.to_string(), value: false }, what.clone());
                        parts.push((ce, id));
                    }
                    ops.push(Op::CoverParts(parts));
                }
                let jz = ops.len();
                ops.push(Op::Halt); // patched
                let cov = self.cover(s, line, CoverKind::Then, what.clone());
                ops.push(cov);
                let reads = self.read_names(&c);
                self.conds.push(Cond { line, text: what.clone(), reads: reads.clone(), taken: "true".into() });
                let r = self.stmt(s, t, ops);
                self.conds.pop();
                r?;
                // An implicit else is a branch too: "the condition was never false".
                let jend = ops.len();
                ops.push(Op::Halt);
                ops[jz] = Op::JumpIfZero(c, ops.len());
                let cov = self.cover(s, line, CoverKind::Else { written: f.is_some() }, what.clone());
                ops.push(cov);
                if let Some(f) = f {
                    self.conds.push(Cond { line, text: what, reads, taken: "false".into() });
                    let r = self.stmt(s, f, ops);
                    self.conds.pop();
                    r?;
                }
                ops[jend] = Op::Jump(ops.len());
            }
            Stmt::Case { kind, sel, items, default, line } => {
                // Evaluate the selector once into a temporary.
                let sel_c = self.sexpr(s, sel, ops)?;
                let tmp = self.temp(s);
                let sel_w = sel_c.width(&self.d);
                let label_w = items
                    .iter()
                    .flat_map(|(ls, _)| ls)
                    .map(|l| self.expr(s, l).map(|c| c.width(&self.d)))
                    .collect::<Result<Vec<_>>>()?;
                let w = label_w.into_iter().fold(sel_w, u32::max);
                let sel_reads = self.read_names(&sel_c);
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
                    let labels = labels.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
                    let cov = self.cover(s, *line, CoverKind::Arm { labels: labels.clone() }, sel.to_string());
                    ops.push(cov);
                    self.conds.push(Cond {
                        line: *line,
                        text: sel.to_string(),
                        reads: sel_reads.clone(),
                        taken: labels,
                    });
                    let r = self.stmt(s, body, ops);
                    self.conds.pop();
                    r?;
                    end_jumps.push(ops.len());
                    ops.push(Op::Halt);
                    ops[jz] = Op::JumpIfZero(cond.expect("case item has labels"), ops.len());
                }
                let cov = self.cover(s, *line, CoverKind::Default { written: default.is_some() }, sel.to_string());
                ops.push(cov);
                if let Some(d) = default {
                    self.conds.push(Cond {
                        line: *line,
                        text: sel.to_string(),
                        reads: sel_reads,
                        taken: "default".into(),
                    });
                    let r = self.stmt(s, d, ops);
                    self.conds.pop();
                    r?;
                }
                let end = ops.len();
                for j in end_jumps {
                    ops[j] = Op::Jump(end);
                }
            }
            Stmt::For { init, cond, step, body } => {
                self.stmt(s, init, ops)?;
                let top = ops.len();
                let c = self.sexpr(s, cond, ops)?;
                let jz = ops.len();
                ops.push(Op::Halt);
                self.stmt(s, body, ops)?;
                self.stmt(s, step, ops)?;
                ops.push(Op::Jump(top));
                ops[jz] = Op::JumpIfZero(c, ops.len());
            }
            Stmt::While(c, body) => {
                let top = ops.len();
                let c = self.sexpr(s, c, ops)?;
                let jz = ops.len();
                ops.push(Op::Halt);
                self.stmt(s, body, ops)?;
                ops.push(Op::Jump(top));
                ops[jz] = Op::JumpIfZero(c, ops.len());
            }
            Stmt::Repeat(n, body) => {
                let tmp = self.temp(s);
                let n = self.sexpr(s, n, ops)?;
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
                let d = self.sexpr(s, d, ops)?;
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
                    let mut cargs = Vec::new();
                    for a in args {
                        cargs.push(self.sexpr(s, a, ops)?);
                    }
                    ops.push(Op::Display { kind, args: cargs, scope: s.path.clone(), line: *line });
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
            // A function called as a statement discards its result.
            Stmt::TaskCall { name, args, line } => {
                let task = s.module.subprog(name, true).is_some();
                self.call(s, name, args, *line, task, ops)?;
            }
            Stmt::Return(e, line) => {
                let Some(ret) = self.rets.last().map(|r| r.0) else {
                    return self.err(s, *line, "return is only allowed in a function or a task");
                };
                if let Some(e) = e {
                    let Some(ret) = ret else {
                        return self.err(s, *line, "a task's return can't have a value");
                    };
                    let v = self.sexpr(s, e, ops)?;
                    ops.push(Op::Assign(LVal::Sig(ret), v));
                }
                // Jumps to the end of the body (patched by `call`).
                self.rets.last_mut().expect("checked above").1.push(ops.len());
                ops.push(Op::Halt);
            }
        }
        Ok(())
    }
}

/// Appends `more` to `ops`, moving its jump targets (each op list numbers from its own start).
fn append(ops: &mut Vec<Op>, more: Vec<Op>) {
    let base = ops.len();
    ops.extend(more.into_iter().map(|op| match op {
        Op::Jump(t) => Op::Jump(t + base),
        Op::JumpIfZero(c, t) => Op::JumpIfZero(c, t + base),
        o => o,
    }));
}

/// `v` as a `w`-bit value, sign-extended if `signed`.
fn fit(v: i64, (w, signed): (u32, bool)) -> i64 {
    if w >= 64 {
        return v;
    }
    let m = (v as u64) & ((1u64 << w) - 1);
    if signed && (m >> (w - 1)) & 1 == 1 { (m | !((1u64 << w) - 1)) as i64 } else { m as i64 }
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
        Op::CoverParts(parts) => parts.iter().for_each(|(e, _)| e.reads(out)),
        Op::Jump(_) | Op::Finish | Op::Halt | Op::Cover(_) | Op::From(_) => {}
    }
}

/// The parts of a condition joined by `&&` and `||`.
fn ast_parts<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::Binary("&&" | "||", a, b) => {
            ast_parts(a, out);
            ast_parts(b, out);
        }
        _ => out.push(e),
    }
}

/// The same parts, compiled (`&&`/`||` compile one to one to `LAnd`/`LOr`).
fn c_parts(e: &CExpr, out: &mut Vec<CExpr>) {
    match e {
        CExpr::Binary(BOp::LAnd | BOp::LOr, a, b) => {
            c_parts(a, out);
            c_parts(b, out);
        }
        _ => out.push(e.clone()),
    }
}
