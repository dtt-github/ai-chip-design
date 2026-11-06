//! Licence compatibility for IP imports: can a block under SPDX expression
//! `block` go into a project licensed `project`?
//!
//! A deliberately small, conservative rule set over common software and
//! hardware licences, not legal advice. Each licence is permissive, weak
//! copyleft (file-level: the block's files keep their licence) or strong
//! copyleft (the whole design must use it). Unknown identifiers are never
//! assumed compatible: importing one needs an owner to accept it explicitly.
//! `A OR B` is as good as its best option, `A AND B` as bad as its worst;
//! `WITH` exceptions are kept in the text but judged by the base licence.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Permissive,
    Weak,
    Strong,
}

const KNOWN: &[(&str, Kind)] = &[
    ("0BSD", Kind::Permissive),
    ("Apache-2.0", Kind::Permissive),
    ("BSD-2-Clause", Kind::Permissive),
    ("BSD-3-Clause", Kind::Permissive),
    ("BSL-1.0", Kind::Permissive),
    ("CC0-1.0", Kind::Permissive),
    ("CERN-OHL-P-2.0", Kind::Permissive),
    ("ISC", Kind::Permissive),
    ("MIT", Kind::Permissive),
    ("MIT-0", Kind::Permissive),
    ("SHL-0.5", Kind::Permissive),
    ("SHL-0.51", Kind::Permissive),
    ("SHL-2.0", Kind::Permissive),
    ("SHL-2.1", Kind::Permissive),
    ("Unlicense", Kind::Permissive),
    ("Zlib", Kind::Permissive),
    ("CERN-OHL-W-2.0", Kind::Weak),
    ("EPL-2.0", Kind::Weak),
    ("LGPL-2.1-only", Kind::Weak),
    ("LGPL-2.1-or-later", Kind::Weak),
    ("LGPL-3.0-only", Kind::Weak),
    ("LGPL-3.0-or-later", Kind::Weak),
    ("MPL-2.0", Kind::Weak),
    ("AGPL-3.0-only", Kind::Strong),
    ("AGPL-3.0-or-later", Kind::Strong),
    ("CERN-OHL-S-2.0", Kind::Strong),
    ("GPL-2.0-only", Kind::Strong),
    ("GPL-2.0-or-later", Kind::Strong),
    ("GPL-3.0-only", Kind::Strong),
    ("GPL-3.0-or-later", Kind::Strong),
];

fn kind(id: &str) -> Option<Kind> {
    KNOWN.iter().find(|(k, _)| *k == id).map(|(_, kind)| *kind)
}

/// Licences a project can declare (every known identifier, permissive first).
pub fn project_licences() -> Vec<&'static str> {
    KNOWN.iter().map(|(k, _)| *k).collect()
}

pub fn is_known(id: &str) -> bool {
    kind(id).is_some()
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Id(String),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// Parses an SPDX licence expression (`MIT`, `Apache-2.0 OR MIT`, `(A AND B) OR C`, `GPL-2.0-only WITH X`).
fn parse(text: &str) -> Result<Expr, String> {
    let spaced = text.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    if tokens.is_empty() {
        return Err("empty licence expression".into());
    }
    let mut pos = 0;
    let e = or_expr(&tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err(format!("unexpected '{}' in licence expression", tokens[pos]));
    }
    Ok(e)
}

fn or_expr(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    let mut left = and_expr(t, pos)?;
    while t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("OR")) {
        *pos += 1;
        left = Expr::Or(Box::new(left), Box::new(and_expr(t, pos)?));
    }
    Ok(left)
}

fn and_expr(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    let mut left = atom(t, pos)?;
    while t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("AND")) {
        *pos += 1;
        left = Expr::And(Box::new(left), Box::new(atom(t, pos)?));
    }
    Ok(left)
}

fn atom(t: &[&str], pos: &mut usize) -> Result<Expr, String> {
    let Some(&tok) = t.get(*pos) else { return Err("licence expression ends early".into()) };
    *pos += 1;
    if tok == "(" {
        let e = or_expr(t, pos)?;
        if t.get(*pos) != Some(&")") {
            return Err("missing ')' in licence expression".into());
        }
        *pos += 1;
        return Ok(e);
    }
    let valid =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '+' | ':'));
    if !valid(tok) || ["AND", "OR", "WITH", ")"].iter().any(|k| tok.eq_ignore_ascii_case(k)) {
        return Err(format!("'{tok}' is not a licence identifier"));
    }
    if t.get(*pos).is_some_and(|w| w.eq_ignore_ascii_case("WITH")) {
        // The exception only relaxes the base licence; judge by the base.
        match t.get(*pos + 1) {
            Some(x) if valid(x) => *pos += 2,
            _ => return Err("WITH needs an exception identifier".into()),
        }
    }
    Ok(Expr::Id(tok.to_string()))
}

/// Checks that `text` is a well-formed SPDX expression.
pub fn validate(text: &str) -> Result<(), String> {
    parse(text).map(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    // Ordered worst to best, so AND takes the min and OR the max.
    Incompatible,
    Unknown,
    Conditions,
    Compatible,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub status: Status,
    pub block: String,
    pub project: String,
    /// Why, one line per licence that decided it.
    pub reasons: Vec<String>,
    /// What the importing project takes on.
    pub obligations: Vec<String>,
    pub note: &'static str,
}

const NOTE: &str = "An automated check over common licences, not legal advice.";

/// Can a block licensed `block` (an SPDX expression) be imported into a project licensed `project`?
pub fn check(block: &str, project: &str) -> Verdict {
    let mut v = match parse(block) {
        Ok(e) => eval(&e, project),
        Err(e) => leaf(Status::Unknown, format!("the block's licence can't be read: {e}"), None),
    };
    v.reasons.dedup();
    v.obligations.dedup();
    Verdict {
        status: v.status,
        block: block.into(),
        project: project.into(),
        reasons: v.reasons,
        obligations: v.obligations,
        note: NOTE,
    }
}

struct Partial {
    status: Status,
    reasons: Vec<String>,
    obligations: Vec<String>,
}

fn leaf(status: Status, reason: String, obligation: Option<String>) -> Partial {
    Partial { status, reasons: vec![reason], obligations: obligation.into_iter().collect() }
}

fn eval(e: &Expr, project: &str) -> Partial {
    match e {
        Expr::Id(id) => one(id, project),
        Expr::And(a, b) => {
            let (a, b) = (eval(a, project), eval(b, project));
            let status = a.status.min(b.status);
            // Every part applies, so every obligation does; reasons from the part(s) that decided.
            let mut reasons = Vec::new();
            for p in [&a, &b] {
                if p.status == status {
                    reasons.extend(p.reasons.iter().cloned());
                }
            }
            Partial { status, reasons, obligations: [a.obligations, b.obligations].concat() }
        }
        Expr::Or(a, b) => {
            let (a, b) = (eval(a, project), eval(b, project));
            // The importer may pick the better option.
            if a.status >= b.status { a } else { b }
        }
    }
}

fn one(id: &str, project: &str) -> Partial {
    use Status::*;
    let notices = format!("keep the block's {id} copyright and licence notices");
    if id == project {
        return leaf(Compatible, format!("{id} is the project's own licence"), Some(notices));
    }
    let Some(k) = kind(id) else {
        return leaf(Unknown, format!("{id} is not a licence this check knows; someone has to read it"), None);
    };
    match k {
        Kind::Permissive if id == "Apache-2.0" && project == "GPL-2.0-only" => {
            leaf(Incompatible, "Apache-2.0's patent terms are incompatible with GPL-2.0-only".into(), None)
        }
        Kind::Permissive => {
            let mut p = leaf(Compatible, format!("{id} is permissive"), Some(notices));
            if id.starts_with("Apache-2.0") || id.starts_with("SHL-") {
                p.obligations.push("pass on the block's NOTICE file, if it has one, and mark files you change".into());
            }
            p
        }
        Kind::Weak => leaf(
            Conditions,
            format!("{id} is file-level copyleft: the block's files stay under {id}"),
            Some(format!("keep the block's files under {id} and share changes to them under {id}")),
        ),
        Kind::Strong => {
            if strong_fits(id, project) {
                leaf(
                    Compatible,
                    format!("{id} is copyleft and the project's {project} satisfies it"),
                    Some(format!("release the combined design under terms compatible with {id}")),
                )
            } else {
                leaf(
                    Incompatible,
                    format!("{id} requires the whole design to be released under {id}; this project is {project}"),
                    None,
                )
            }
        }
    }
}

/// Whether a project under `project` may include a strong-copyleft block under `id`.
fn strong_fits(id: &str, project: &str) -> bool {
    match id {
        "GPL-2.0-or-later" => {
            matches!(project, "GPL-2.0-only" | "GPL-2.0-or-later" | "GPL-3.0-only" | "GPL-3.0-or-later")
        }
        "GPL-3.0-or-later" => matches!(project, "GPL-3.0-only" | "GPL-3.0-or-later"),
        "AGPL-3.0-or-later" => matches!(project, "AGPL-3.0-only" | "AGPL-3.0-or-later"),
        _ => id == project,
    }
}

/// The licence identifiers in a file's `SPDX-License-Identifier:` header (first 20 lines).
pub fn spdx_header(text: &str) -> Option<String> {
    text.lines().take(20).find_map(|l| {
        let (_, rest) = l.split_once("SPDX-License-Identifier:")?;
        let expr = rest.trim().trim_end_matches("*/").trim();
        (!expr.is_empty()).then(|| expr.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(block: &str, project: &str) -> Status {
        check(block, project).status
    }

    #[test]
    fn expressions() {
        assert!(validate("MIT").is_ok());
        assert!(validate("(Apache-2.0 OR MIT) AND BSD-3-Clause").is_ok());
        assert!(validate("GPL-2.0-only WITH Linux-syscall-note").is_ok());
        assert!(validate("MIT OR").is_err());
        assert!(validate("(MIT").is_err());
        assert!(validate("").is_err());
        assert!(validate("MIT; rm").is_err());
    }

    #[test]
    fn permissive_goes_anywhere_with_one_exception() {
        assert_eq!(status("MIT", "Apache-2.0"), Status::Compatible);
        assert_eq!(status("Apache-2.0", "GPL-3.0-only"), Status::Compatible);
        assert_eq!(status("Apache-2.0", "GPL-2.0-only"), Status::Incompatible);
        assert_eq!(status("Apache-2.0", "GPL-2.0-or-later"), Status::Compatible);
        let v = check("Apache-2.0", "MIT");
        assert!(v.obligations.iter().any(|o| o.contains("NOTICE")), "{v:?}");
    }

    #[test]
    fn copyleft() {
        assert_eq!(status("CERN-OHL-W-2.0", "Apache-2.0"), Status::Conditions);
        assert_eq!(status("CERN-OHL-S-2.0", "Apache-2.0"), Status::Incompatible);
        assert_eq!(status("CERN-OHL-S-2.0", "CERN-OHL-S-2.0"), Status::Compatible);
        assert_eq!(status("GPL-3.0-only", "MIT"), Status::Incompatible);
        assert_eq!(status("GPL-2.0-or-later", "GPL-3.0-only"), Status::Compatible);
        assert_eq!(status("GPL-3.0-only", "GPL-2.0-only"), Status::Incompatible);
        let v = check("GPL-3.0-only", "Apache-2.0");
        assert!(v.reasons[0].contains("whole design"), "{v:?}");
    }

    #[test]
    fn unknown_and_combinations() {
        assert_eq!(status("LicenseRef-Acme", "MIT"), Status::Unknown);
        assert_eq!(status("not ( valid", "MIT"), Status::Unknown);
        // OR: the better option wins; AND: the worse part decides.
        assert_eq!(status("GPL-3.0-only OR MIT", "Apache-2.0"), Status::Compatible);
        assert_eq!(status("MIT AND GPL-3.0-only", "Apache-2.0"), Status::Incompatible);
        assert_eq!(status("MIT AND MPL-2.0", "Apache-2.0"), Status::Conditions);
        assert_eq!(status("MIT AND LicenseRef-Acme", "Apache-2.0"), Status::Unknown);
        let v = check("MIT AND MPL-2.0", "Apache-2.0");
        assert_eq!(v.obligations.len(), 2, "{v:?}");
    }

    #[test]
    fn spdx_headers() {
        assert_eq!(spdx_header("// SPDX-License-Identifier: Apache-2.0\nmodule x;"), Some("Apache-2.0".into()));
        assert_eq!(spdx_header("/* SPDX-License-Identifier: MIT OR ISC */"), Some("MIT OR ISC".into()));
        assert_eq!(spdx_header("module x; endmodule"), None);
    }
}
