//! Recursive-descent parser for the supported Verilog subset.

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::{Error, Result};

/// Kind (if written), signedness and packed range of a declaration.
type TypeSpec = (Option<Kind>, bool, Option<(Expr, Expr)>);

pub struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
    file: String,
}

const NOT_SUPPORTED: &[&str] = &[
    "function",
    "task",
    "generate",
    "genvar",
    "specify",
    "primitive",
    "interface",
    "class",
    "package",
    "fork",
    "real",
    "realtime",
    "time",
    "event",
    "defparam",
    "tri",
    "supply0",
    "supply1",
    "wand",
    "wor",
    "struct",
    "typedef",
];

impl<'a> Parser<'a> {
    pub fn new(toks: &'a [Token], file: &str) -> Self {
        Parser { toks, pos: 0, file: file.to_string() }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(Error::Syntax { file: self.file.clone(), line: self.line(), msg: msg.into() })
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Tok::Op(o) if *o == op)
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(i) if i == kw)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<()> {
        if self.eat_op(op) { Ok(()) } else { self.err(format!("expected '{op}', found {}", describe(self.peek()))) }
    }

    fn ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Tok::Ident(s) if !is_keyword(&s) => {
                self.bump();
                Ok(s)
            }
            t => self.err(format!("expected an identifier, found {}", describe(&t))),
        }
    }

    pub fn source(&mut self) -> Result<Vec<Module>> {
        let mut mods = Vec::new();
        while *self.peek() != Tok::Eof {
            if self.eat_kw("module") || self.eat_kw("macromodule") {
                mods.push(self.module()?);
            } else {
                return self.err(format!("expected 'module', found {}", describe(self.peek())));
            }
        }
        Ok(mods)
    }

    fn module(&mut self) -> Result<Module> {
        let line = self.line();
        let name = self.ident()?;
        let mut m = Module { name, file: self.file.clone(), line, ports: Vec::new(), items: Vec::new() };
        if self.eat_op("#") {
            self.expect_op("(")?;
            loop {
                self.eat_kw("parameter");
                self.skip_type_words();
                let line = self.line();
                let name = self.ident()?;
                self.expect_op("=")?;
                let value = self.expr()?;
                m.items.push(Item::Param { name, value, local: false, line });
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        if self.eat_op("(") {
            if !self.is_op(")") {
                // ANSI style if the first token is a direction.
                if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") {
                    let mut last: Option<Decl> = None;
                    loop {
                        let line = self.line();
                        let decl = if self.is_kw("input") || self.is_kw("output") || self.is_kw("inout") {
                            let dir = self.direction()?;
                            let (kind, signed, range) = self.type_spec()?;
                            Decl { dir: Some(dir), kind, signed, range, names: Vec::new(), line }
                        } else {
                            let mut d = last.clone().ok_or_else(|| self.syntax("port direction expected"))?;
                            d.names.clear();
                            d
                        };
                        let name = self.ident()?;
                        m.ports.push(name.clone());
                        let mut d = decl.clone();
                        d.names.push(DeclName { name, array: None, init: None });
                        m.items.push(Item::Decl(d));
                        last = Some(decl);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                } else {
                    loop {
                        m.ports.push(self.ident()?);
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
            }
            self.expect_op(")")?;
        }
        self.expect_op(";")?;
        while !self.eat_kw("endmodule") {
            if *self.peek() == Tok::Eof {
                return self.err(format!("missing 'endmodule' for module {}", m.name));
            }
            self.item(&mut m.items)?;
        }
        Ok(m)
    }

    fn syntax(&self, msg: &str) -> Error {
        Error::Syntax { file: self.file.clone(), line: self.line(), msg: msg.to_string() }
    }

    fn direction(&mut self) -> Result<Dir> {
        match self.bump() {
            Tok::Ident(s) if s == "input" => Ok(Dir::Input),
            Tok::Ident(s) if s == "output" => Ok(Dir::Output),
            Tok::Ident(s) if s == "inout" => Ok(Dir::Inout),
            t => self.err(format!("expected a port direction, found {}", describe(&t))),
        }
    }

    /// `[wire|reg|logic|integer] [signed|unsigned] [range]`
    fn type_spec(&mut self) -> Result<TypeSpec> {
        let mut kind = None;
        if self.eat_kw("wire") {
            kind = Some(Kind::Wire);
        } else if self.eat_kw("reg") || self.eat_kw("logic") || self.eat_kw("bit") {
            kind = Some(Kind::Reg);
        } else if self.eat_kw("integer") || self.eat_kw("int") {
            kind = Some(Kind::Integer);
        }
        let mut signed = kind == Some(Kind::Integer);
        if self.eat_kw("signed") {
            signed = true;
        }
        self.eat_kw("unsigned");
        let range = self.opt_range()?;
        Ok((kind, signed, range))
    }

    fn skip_type_words(&mut self) {
        while self.is_kw("integer") || self.is_kw("signed") || self.is_kw("unsigned") || self.is_kw("int") {
            self.bump();
        }
        if self.is_op("[") {
            // Parameter ranges don't change our 64-bit evaluation.
            let mut depth = 0;
            loop {
                match self.bump() {
                    Tok::Op("[") => depth += 1,
                    Tok::Op("]") => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    Tok::Eof => break,
                    _ => {}
                }
            }
        }
    }

    fn opt_range(&mut self) -> Result<Option<(Expr, Expr)>> {
        if self.eat_op("[") {
            let msb = self.expr()?;
            self.expect_op(":")?;
            let lsb = self.expr()?;
            self.expect_op("]")?;
            Ok(Some((msb, lsb)))
        } else {
            Ok(None)
        }
    }

    fn item(&mut self, items: &mut Vec<Item>) -> Result<()> {
        let line = self.line();
        if let Tok::Ident(kw) = self.peek().clone() {
            if NOT_SUPPORTED.contains(&kw.as_str()) {
                return self.err(format!("'{kw}' is not supported by the browser simulator yet"));
            }
            match kw.as_str() {
                "parameter" | "localparam" => {
                    self.bump();
                    self.skip_type_words();
                    loop {
                        let line = self.line();
                        let name = self.ident()?;
                        self.expect_op("=")?;
                        let value = self.expr()?;
                        items.push(Item::Param { name, value, local: kw == "localparam", line });
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    return self.expect_op(";");
                }
                "input" | "output" | "inout" => {
                    let dir = self.direction()?;
                    let (kind, signed, range) = self.type_spec()?;
                    let names = self.decl_names()?;
                    items.push(Item::Decl(Decl { dir: Some(dir), kind, signed, range, names, line }));
                    return Ok(());
                }
                "wire" | "reg" | "logic" | "bit" | "integer" | "int" => {
                    let (kind, signed, range) = self.type_spec()?;
                    let names = self.decl_names()?;
                    items.push(Item::Decl(Decl { dir: None, kind, signed, range, names, line }));
                    return Ok(());
                }
                "assign" => {
                    self.bump();
                    loop {
                        let lhs = self.lvalue()?;
                        self.expect_op("=")?;
                        let rhs = self.expr()?;
                        items.push(Item::Assign { lhs, rhs, line });
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                    return self.expect_op(";");
                }
                "always" | "always_ff" | "always_comb" | "always_latch" => {
                    self.bump();
                    let body = self.stmt()?;
                    items.push(Item::Always { body, comb: kw == "always_comb" || kw == "always_latch", line });
                    return Ok(());
                }
                "initial" => {
                    self.bump();
                    let body = self.stmt()?;
                    items.push(Item::Initial { body, line });
                    return Ok(());
                }
                _ => {}
            }
            // Module instance: `name [#(...)] inst (...);`
            if !is_keyword(&kw) {
                return self.instance(items);
            }
        }
        if self.eat_op(";") {
            return Ok(());
        }
        self.err(format!("unexpected {} in module body", describe(self.peek())))
    }

    fn decl_names(&mut self) -> Result<Vec<DeclName>> {
        let mut names = Vec::new();
        loop {
            let name = self.ident()?;
            let array = self.opt_range()?;
            let init = if self.eat_op("=") { Some(self.expr()?) } else { None };
            names.push(DeclName { name, array, init });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")?;
        Ok(names)
    }

    fn instance(&mut self, items: &mut Vec<Item>) -> Result<()> {
        let line = self.line();
        let module = self.ident()?;
        let mut params = Vec::new();
        if self.eat_op("#") {
            self.expect_op("(")?;
            if !self.is_op(")") {
                loop {
                    if self.eat_op(".") {
                        let n = self.ident()?;
                        self.expect_op("(")?;
                        let v = self.expr()?;
                        self.expect_op(")")?;
                        params.push((Some(n), v));
                    } else {
                        params.push((None, self.expr()?));
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")")?;
        }
        loop {
            let name = self.ident()?;
            if self.is_op("[") {
                return self.err("arrays of instances are not supported");
            }
            self.expect_op("(")?;
            let mut conns = Vec::new();
            if !self.is_op(")") {
                loop {
                    if self.eat_op(".") {
                        if self.eat_op("*") {
                            return self.err(".* port connections are not supported");
                        }
                        let n = self.ident()?;
                        let e = if self.eat_op("(") {
                            let e = if self.is_op(")") { None } else { Some(self.expr()?) };
                            self.expect_op(")")?;
                            e
                        } else {
                            // `.name` shorthand
                            Some(Expr::Ident(n.clone(), self.line()))
                        };
                        conns.push((Some(n), e));
                    } else {
                        conns.push((None, Some(self.expr()?)));
                    }
                    if !self.eat_op(",") {
                        break;
                    }
                }
            }
            self.expect_op(")")?;
            items.push(Item::Instance { module: module.clone(), params: params.clone(), name, conns, line });
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op(";")
    }

    // ---- statements ----

    fn stmt(&mut self) -> Result<Stmt> {
        let line = self.line();
        if self.eat_op(";") {
            return Ok(Stmt::Null);
        }
        if self.eat_op("#") {
            let d = self.delay_value()?;
            let s = self.stmt()?;
            return Ok(Stmt::Delay(d, Box::new(s)));
        }
        if self.eat_op("@") {
            let ctl = self.event_ctl()?;
            let s = self.stmt()?;
            return Ok(Stmt::Event(ctl, Box::new(s), line));
        }
        if let Tok::System(name) = self.peek().clone() {
            self.bump();
            let mut args = Vec::new();
            if self.eat_op("(") {
                if !self.is_op(")") {
                    loop {
                        // Empty arguments ($display("a",,b)) print nothing.
                        if self.is_op(",") {
                            args.push(Expr::Str(String::new()));
                        } else {
                            args.push(self.expr()?);
                        }
                        if !self.eat_op(",") {
                            break;
                        }
                    }
                }
                self.expect_op(")")?;
            }
            self.expect_op(";")?;
            return Ok(Stmt::SysTask { name, args, line });
        }
        let kw = match self.peek() {
            Tok::Ident(s) => s.clone(),
            Tok::Op("{") => String::new(),
            t => return self.err(format!("expected a statement, found {}", describe(t))),
        };
        match kw.as_str() {
            "begin" => {
                self.bump();
                if self.eat_op(":") {
                    self.ident()?;
                }
                let mut v = Vec::new();
                while !self.eat_kw("end") {
                    if *self.peek() == Tok::Eof {
                        return self.err("missing 'end'");
                    }
                    // Local declarations inside named blocks are not supported.
                    if self.is_kw("reg") || self.is_kw("integer") || self.is_kw("logic") {
                        return self
                            .err("declarations inside begin/end blocks are not supported; declare them in the module");
                    }
                    v.push(self.stmt()?);
                }
                if self.eat_op(":") {
                    self.ident()?;
                }
                Ok(Stmt::Block(v))
            }
            "if" => {
                self.bump();
                self.expect_op("(")?;
                let c = self.expr()?;
                self.expect_op(")")?;
                let t = self.stmt()?;
                let e = if self.eat_kw("else") { Some(Box::new(self.stmt()?)) } else { None };
                Ok(Stmt::If(c, Box::new(t), e))
            }
            "unique" | "priority" => {
                self.bump();
                self.stmt()
            }
            "case" | "casez" | "casex" => {
                self.bump();
                let kind = match kw.as_str() {
                    "casez" => CaseKind::Casez,
                    "casex" => CaseKind::Casex,
                    _ => CaseKind::Case,
                };
                self.expect_op("(")?;
                let sel = self.expr()?;
                self.expect_op(")")?;
                let mut items = Vec::new();
                let mut default = None;
                while !self.eat_kw("endcase") {
                    if *self.peek() == Tok::Eof {
                        return self.err("missing 'endcase'");
                    }
                    if self.eat_kw("default") {
                        self.eat_op(":");
                        default = Some(Box::new(self.stmt()?));
                        continue;
                    }
                    let mut labels = vec![self.expr()?];
                    while self.eat_op(",") {
                        labels.push(self.expr()?);
                    }
                    self.expect_op(":")?;
                    items.push((labels, self.stmt()?));
                }
                Ok(Stmt::Case { kind, sel, items, default, line })
            }
            "for" => {
                self.bump();
                self.expect_op("(")?;
                if self.is_kw("integer") || self.is_kw("int") || self.is_kw("genvar") {
                    return self
                        .err("declare the loop variable in the module (for (i = 0; ...)), not in the for header");
                }
                let init = self.assignment_stmt(false)?;
                self.expect_op(";")?;
                let cond = self.expr()?;
                self.expect_op(";")?;
                let step = self.assignment_stmt(false)?;
                self.expect_op(")")?;
                let body = self.stmt()?;
                Ok(Stmt::For { init: Box::new(init), cond, step: Box::new(step), body: Box::new(body) })
            }
            "while" => {
                self.bump();
                self.expect_op("(")?;
                let c = self.expr()?;
                self.expect_op(")")?;
                Ok(Stmt::While(c, Box::new(self.stmt()?)))
            }
            "repeat" => {
                self.bump();
                self.expect_op("(")?;
                let c = self.expr()?;
                self.expect_op(")")?;
                Ok(Stmt::Repeat(c, Box::new(self.stmt()?)))
            }
            "forever" => {
                self.bump();
                Ok(Stmt::Forever(Box::new(self.stmt()?)))
            }
            "wait" => {
                self.bump();
                self.expect_op("(")?;
                let c = self.expr()?;
                self.expect_op(")")?;
                Ok(Stmt::Wait(c, Box::new(self.stmt()?)))
            }
            "disable" | "fork" | "assert" | "assume" | "cover" | "force" | "release" | "deassign" => {
                self.err(format!("'{kw}' is not supported by the browser simulator yet"))
            }
            _ => {
                let s = self.assignment_stmt(true)?;
                self.expect_op(";")?;
                Ok(s)
            }
        }
    }

    /// `lhs = expr`, `lhs <= [#d] expr`, `i++`, `i += e`.
    fn assignment_stmt(&mut self, allow_nb: bool) -> Result<Stmt> {
        let line = self.line();
        let lhs = self.lvalue()?;
        if self.eat_op("++") || self.eat_op("--") {
            let op = if matches!(self.toks[self.pos - 1].tok, Tok::Op("++")) { "+" } else { "-" };
            let one =
                Expr::Num(crate::lexer::Num { value: 1, xz: 0, width: None, signed: true, fill: false, real: false });
            return Ok(Stmt::Blocking { rhs: Expr::Binary(op, Box::new(lhs.clone()), Box::new(one)), lhs, line });
        }
        for (tok, op) in [("+=", "+"), ("-=", "-")] {
            if self.eat_op(tok) {
                let rhs = self.expr()?;
                return Ok(Stmt::Blocking { rhs: Expr::Binary(op, Box::new(lhs.clone()), Box::new(rhs)), lhs, line });
            }
        }
        if self.eat_op("=") {
            if self.is_op("#") || self.is_op("@") {
                return self.err("intra-assignment delays (a = #d b) are not supported");
            }
            let rhs = self.expr()?;
            return Ok(Stmt::Blocking { lhs, rhs, line });
        }
        if allow_nb && self.eat_op("<=") {
            let delay = if self.eat_op("#") { Some(self.delay_value()?) } else { None };
            let rhs = self.expr()?;
            return Ok(Stmt::NonBlocking { lhs, rhs, delay, line });
        }
        self.err(format!("expected an assignment, found {}", describe(self.peek())))
    }

    fn delay_value(&mut self) -> Result<Expr> {
        if self.eat_op("(") {
            let e = self.expr()?;
            self.expect_op(")")?;
            Ok(e)
        } else {
            self.primary()
        }
    }

    fn event_ctl(&mut self) -> Result<EventCtl> {
        if self.eat_op("*") {
            return Ok(EventCtl::Star);
        }
        if self.is_op("(") && matches!(self.peek_at(1), Tok::Op("*")) && matches!(self.peek_at(2), Tok::Op(")")) {
            self.bump();
            self.bump();
            self.bump();
            return Ok(EventCtl::Star);
        }
        if self.is_op("(*") {
            // Lexed as the attribute opener.
            self.bump();
            self.expect_op(")")?;
            return Ok(EventCtl::Star);
        }
        if !self.eat_op("(") {
            let e = self.primary()?;
            return Ok(EventCtl::List(vec![(Edge::Any, e)]));
        }
        let mut list = Vec::new();
        loop {
            let edge = if self.eat_kw("posedge") {
                Edge::Pos
            } else if self.eat_kw("negedge") {
                Edge::Neg
            } else {
                Edge::Any
            };
            list.push((edge, self.expr()?));
            if !(self.eat_kw("or") || self.eat_op(",")) {
                break;
            }
        }
        self.expect_op(")")?;
        Ok(EventCtl::List(list))
    }

    fn lvalue(&mut self) -> Result<Expr> {
        if self.eat_op("{") {
            let mut v = vec![self.lvalue()?];
            while self.eat_op(",") {
                v.push(self.lvalue()?);
            }
            self.expect_op("}")?;
            return Ok(Expr::Concat(v));
        }
        let line = self.line();
        let name = self.hier_ident()?;
        self.selects(Expr::Ident(name, line))
    }

    /// `a.b.c` hierarchical names (read-only use in testbenches).
    fn hier_ident(&mut self) -> Result<String> {
        let mut name = self.ident()?;
        while self.is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.bump();
            name.push('.');
            name.push_str(&self.ident()?);
        }
        Ok(name)
    }

    fn selects(&mut self, mut e: Expr) -> Result<Expr> {
        while self.eat_op("[") {
            let a = self.expr()?;
            if self.eat_op(":") {
                let b = self.expr()?;
                self.expect_op("]")?;
                e = Expr::Range(Box::new(e), Box::new(a), Box::new(b));
            } else if self.eat_op("+:") || self.eat_op("-:") {
                let up = matches!(self.toks[self.pos - 1].tok, Tok::Op("+:"));
                let w = self.expr()?;
                self.expect_op("]")?;
                e = Expr::IndexedPart { base: Box::new(e), start: Box::new(a), width: Box::new(w), up };
            } else {
                self.expect_op("]")?;
                e = Expr::Index(Box::new(e), Box::new(a));
            }
        }
        Ok(e)
    }

    // ---- expressions (lowest to highest precedence) ----

    pub fn expr(&mut self) -> Result<Expr> {
        let c = self.binary(0)?;
        if self.eat_op("?") {
            let t = self.expr()?;
            self.expect_op(":")?;
            let f = self.expr()?;
            return Ok(Expr::Ternary(Box::new(c), Box::new(t), Box::new(f)));
        }
        Ok(c)
    }

    fn binary(&mut self, level: usize) -> Result<Expr> {
        const LEVELS: &[&[&str]] = &[
            &["||"],
            &["&&"],
            &["|"],
            &["^", "~^", "^~"],
            &["&"],
            &["==", "!=", "===", "!=="],
            &["<", "<=", ">", ">="],
            &["<<", ">>", "<<<", ">>>"],
            &["+", "-"],
            &["*", "/", "%"],
            &["**"],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        loop {
            let op = match self.peek() {
                Tok::Op(o) if LEVELS[level].contains(o) => *o,
                _ => break,
            };
            self.bump();
            let rhs = self.binary(level + 1)?;
            let op = match op {
                "===" => "==",
                "!==" => "!=",
                "^~" => "~^",
                o => o,
            };
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        for op in ["!", "~", "-", "+", "&", "|", "^", "~&", "~|", "~^"] {
            if self.is_op(op) {
                self.bump();
                let e = self.unary()?;
                return Ok(if op == "+" { e } else { Expr::Unary(op, Box::new(e)) });
            }
        }
        if self.is_op("^~") {
            self.bump();
            return Ok(Expr::Unary("~^", Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Number(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::System(name) => {
                self.bump();
                let mut args = Vec::new();
                if self.eat_op("(") {
                    if !self.is_op(")") {
                        loop {
                            args.push(self.expr()?);
                            if !self.eat_op(",") {
                                break;
                            }
                        }
                    }
                    self.expect_op(")")?;
                }
                Ok(Expr::Call(name, args, line))
            }
            Tok::Op("(") => {
                self.bump();
                let e = self.expr()?;
                // min:typ:max delays: take typ.
                if self.eat_op(":") {
                    let typ = self.expr()?;
                    self.expect_op(":")?;
                    self.expr()?;
                    self.expect_op(")")?;
                    return Ok(typ);
                }
                self.expect_op(")")?;
                Ok(e)
            }
            Tok::Op("{") => {
                self.bump();
                let first = self.expr()?;
                if self.eat_op("{") {
                    let mut v = vec![self.expr()?];
                    while self.eat_op(",") {
                        v.push(self.expr()?);
                    }
                    self.expect_op("}")?;
                    self.expect_op("}")?;
                    return Ok(Expr::Repl(Box::new(first), v));
                }
                let mut v = vec![first];
                while self.eat_op(",") {
                    v.push(self.expr()?);
                }
                self.expect_op("}")?;
                Ok(Expr::Concat(v))
            }
            Tok::Ident(_) => {
                let name = self.hier_ident()?;
                if self.is_op("(") {
                    return self.err(format!("function calls ({name}(...)) are not supported yet"));
                }
                self.selects(Expr::Ident(name, line))
            }
            t => self.err(format!("expected an expression, found {}", describe(&t))),
        }
    }
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s,
        "module"
            | "endmodule"
            | "input"
            | "output"
            | "inout"
            | "wire"
            | "reg"
            | "logic"
            | "integer"
            | "assign"
            | "always"
            | "always_ff"
            | "always_comb"
            | "always_latch"
            | "initial"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "case"
            | "casez"
            | "casex"
            | "endcase"
            | "default"
            | "for"
            | "while"
            | "repeat"
            | "forever"
            | "posedge"
            | "negedge"
            | "or"
            | "parameter"
            | "localparam"
            | "signed"
            | "unsigned"
            | "wait"
    ) || NOT_SUPPORTED.contains(&s)
}

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("'{s}'"),
        Tok::System(s) => format!("'{s}'"),
        Tok::Number(n) => format!("number {}", n.value),
        Tok::Str(_) => "a string".into(),
        Tok::Op(o) => format!("'{o}'"),
        Tok::Eof => "end of file".into(),
    }
}
