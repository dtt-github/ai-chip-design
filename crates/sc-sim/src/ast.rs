//! Syntax tree for the supported Verilog subset.

use crate::lexer::Num;

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub file: String,
    pub line: u32,
    /// Port names in declaration order.
    pub ports: Vec<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Input,
    Output,
    Inout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wire,
    Reg,
    Integer,
}

#[derive(Debug, Clone)]
pub struct DeclName {
    pub name: String,
    /// Unpacked array bounds (`mem [0:15]`).
    pub array: Option<(Expr, Expr)>,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub dir: Option<Dir>,
    /// `None`: only a direction was given (kind comes from another declaration or defaults to wire).
    pub kind: Option<Kind>,
    pub signed: bool,
    pub range: Option<(Expr, Expr)>,
    pub names: Vec<DeclName>,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum Item {
    Param {
        name: String,
        value: Expr,
        local: bool,
        line: u32,
    },
    Decl(Decl),
    Assign {
        lhs: Expr,
        rhs: Expr,
        line: u32,
    },
    Always {
        body: Stmt,
        comb: bool,
        line: u32,
    },
    Initial {
        body: Stmt,
        line: u32,
    },
    Instance {
        module: String,
        params: Vec<(Option<String>, Expr)>,
        name: String,
        conns: Vec<(Option<String>, Option<Expr>)>,
        line: u32,
    },
    Function(Subprog),
    Task(Subprog),
    /// `for (i = init; cond; i = step) begin : label ... end`, expanded at elaboration.
    GenFor {
        var: String,
        init: Expr,
        cond: Expr,
        /// The value `var` takes after each pass (`i + 1` for `i = i + 1` and `i++`).
        step: Expr,
        block: GenBlock,
        line: u32,
    },
    GenIf {
        cond: Expr,
        then: GenBlock,
        els: Option<GenBlock>,
        line: u32,
    },
    GenCase {
        sel: Expr,
        arms: Vec<(Vec<Expr>, GenBlock)>,
        default: Option<GenBlock>,
        line: u32,
    },
    /// A bare `begin : label ... end` in a generate region.
    GenBlock(GenBlock),
}

/// The items of a generate block, and its name if it has one (`begin : name`).
#[derive(Debug, Clone)]
pub struct GenBlock {
    pub label: Option<String>,
    pub items: Vec<Item>,
}

/// A function or a task. Functions are static (one copy of their variables per module instance),
/// as in Verilog-2001 without `automatic`.
#[derive(Debug, Clone)]
pub struct Subprog {
    pub name: String,
    pub line: u32,
    /// Function return type: signed, and `[msb:lsb]` (`None` is 1 bit, or 32 for `integer`).
    pub ret_signed: bool,
    pub ret_range: Option<(Expr, Expr)>,
    pub ret_integer: bool,
    /// Arguments in call order, with their direction.
    pub args: Vec<(String, Dir)>,
    /// Every declaration: arguments and local variables.
    pub decls: Vec<Decl>,
    pub body: Stmt,
}

impl Module {
    /// Names of the modules this one instantiates, including inside generate blocks.
    pub fn instantiated(&self) -> Vec<&str> {
        fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a str>) {
            for i in items {
                match i {
                    Item::Instance { module, .. } => out.push(module),
                    Item::GenFor { block, .. } | Item::GenBlock(block) => walk(&block.items, out),
                    Item::GenIf { then, els, .. } => {
                        walk(&then.items, out);
                        if let Some(e) = els {
                            walk(&e.items, out);
                        }
                    }
                    Item::GenCase { arms, default, .. } => {
                        for (_, b) in arms {
                            walk(&b.items, out);
                        }
                        if let Some(d) = default {
                            walk(&d.items, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// The function or task called `name`.
    pub fn subprog(&self, name: &str, task: bool) -> Option<&Subprog> {
        self.items.iter().find_map(|i| match i {
            Item::Function(f) if !task && f.name == name => Some(f),
            Item::Task(t) if task && t.name == name => Some(t),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Any,
    Pos,
    Neg,
}

#[derive(Debug, Clone)]
pub enum EventCtl {
    /// `@*` / `@(*)`
    Star,
    List(Vec<(Edge, Expr)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseKind {
    Case,
    Casez,
    Casex,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Null,
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Case {
        kind: CaseKind,
        sel: Expr,
        items: Vec<(Vec<Expr>, Stmt)>,
        default: Option<Box<Stmt>>,
        line: u32,
    },
    For {
        init: Box<Stmt>,
        cond: Expr,
        step: Box<Stmt>,
        body: Box<Stmt>,
    },
    While(Expr, Box<Stmt>),
    Repeat(Expr, Box<Stmt>),
    Forever(Box<Stmt>),
    Blocking {
        lhs: Expr,
        rhs: Expr,
        line: u32,
    },
    NonBlocking {
        lhs: Expr,
        rhs: Expr,
        delay: Option<Expr>,
        line: u32,
    },
    Delay(Expr, Box<Stmt>),
    Event(EventCtl, Box<Stmt>, u32),
    Wait(Expr, Box<Stmt>),
    SysTask {
        name: String,
        args: Vec<Expr>,
        line: u32,
    },
    /// A user task call: `name(args);`
    TaskCall {
        name: String,
        args: Vec<Expr>,
        line: u32,
    },
    /// `return [expr];` in a function or task.
    Return(Option<Expr>, u32),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(Num),
    Str(String),
    Ident(String, u32),
    /// `a[i]` (bit select or memory word)
    Index(Box<Expr>, Box<Expr>),
    /// `a[msb:lsb]`
    Range(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a[base +: width]` (`up`) or `a[base -: width]`
    IndexedPart {
        base: Box<Expr>,
        start: Box<Expr>,
        width: Box<Expr>,
        up: bool,
    },
    Concat(Vec<Expr>),
    Repl(Box<Expr>, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `$time`, `$signed(x)`, `$clog2(x)`, ... or a user function `f(x)` (no `$`).
    Call(String, Vec<Expr>, u32),
}

impl Expr {
    pub fn line(&self) -> Option<u32> {
        match self {
            Expr::Ident(_, l) | Expr::Call(_, _, l) => Some(*l),
            Expr::Index(a, _) | Expr::Range(a, _, _) | Expr::Unary(_, a) => a.line(),
            Expr::IndexedPart { base, .. } => base.line(),
            Expr::Binary(_, a, b) => a.line().or_else(|| b.line()),
            Expr::Ternary(a, _, _) => a.line(),
            Expr::Concat(v) | Expr::Repl(_, v) => v.iter().find_map(|e| e.line()),
            _ => None,
        }
    }
}
