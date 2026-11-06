//! "Try a small fix": when an example fails, tries the small repairs that undo the most common
//! mistakes (a comparison turned around, a constant one off, a condition reversed, a `!` too many,
//! `&&` for `||`, `-` for `+`, an assigned value one off), one at a time, and keeps those after
//! which every example passes. No AI: each repair is checked by simulating all the examples.
//!
//! Repairs are made in the parsed design, like planted mistakes (`crate::mistakes`), lines the
//! person points at (from "Why not?") first. A repair found is turned back into an edit of one
//! line of the file, keeping the rest of the line as written.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use wafer_sim::ast::Module;

use crate::mistakes::Walker;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Focus {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fixes {
    pub fixes: Vec<Fix>,
    /// Repairs tried, of `sites` possible.
    pub tried: usize,
    pub sites: usize,
    /// The examples failing before any repair.
    pub failing: Vec<String>,
    pub elapsed_ms: u64,
    /// In words: what was found, or why nothing was.
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fix {
    pub file: String,
    pub line: u32,
    /// As `crate::mistakes::Slipped::kind`, plus `value` (an assigned constant).
    pub kind: String,
    pub original: String,
    pub replacement: String,
    /// "Line 18 of blinker.v: `cnt == N - 1` → `cnt == N`; every example passes."
    pub text: String,
    /// The line as it is and as it would be; None when the change can't be placed in the text
    /// (it spans lines, or is written very differently from how it prints).
    pub edit: Option<Edit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Edit {
    pub before: String,
    pub after: String,
    /// The whole file with the change made.
    pub content: String,
}

/// Fixes kept, at most.
const MAX_FIXES: usize = 3;

/// Repairs that make every example in `examples` (testbench tops) pass, within `budget`. Fails (in
/// words) if the design doesn't parse or every example already passes. Call on a big stack.
pub fn find(
    files: &BTreeMap<String, String>,
    examples: &[String],
    focus: &[Focus],
    budget: Duration,
) -> Result<Fixes, String> {
    let started = Instant::now();
    let hdl: Vec<(&str, &str)> =
        files.iter().filter(|(p, _)| crate::gate::is_hdl(p)).map(|(p, c)| (p.as_str(), c.as_str())).collect();
    let (modules, timescale) = wafer_sim::parse(&hdl).map_err(|e| format!("the design doesn't parse: {e}"))?;
    let limits = crate::gate::limits();
    let passes = |ms: &[Module], top: &str| match wafer_sim::run_parsed(ms, timescale, top, limits) {
        Ok(r) => r.finished && crate::gate::verdict(&r.log).0 == "pass",
        Err(_) => false,
    };
    let failing: Vec<String> = examples.iter().filter(|t| !passes(&modules, t)).cloned().collect();
    if failing.is_empty() {
        return Err("every example passes already".into());
    }
    // The others, after the failing ones: a repair has to fix those first.
    let order: Vec<&String> = failing.iter().chain(examples.iter().filter(|t| !failing.contains(t))).collect();

    let mine =
        |m: &Module| !m.file.starts_with("tb/") && files.get(&m.file).is_some_and(|c| !crate::blocks::is_generated(c));
    let mut probe = modules.clone();
    let mut w = Walker::repairs(None);
    for m in probe.iter_mut().filter(|m| mine(m)) {
        w.module(m);
    }
    let sites = w.sites;
    // The lines pointed at first, then the rest in the order of the design.
    let near = |i: &usize| {
        let s = &sites[*i];
        !focus.iter().any(|f| f.file == s.file && f.line == s.line)
    };
    let mut chosen: Vec<usize> = (0..sites.len()).collect();
    chosen.sort_by_key(|i| near(i));

    let (mut fixes, mut tried) = (Vec::new(), 0);
    for i in chosen {
        if started.elapsed() > budget || fixes.len() >= MAX_FIXES {
            break;
        }
        let mut ms = modules.clone();
        let mut w = Walker::repairs(Some(i));
        for m in ms.iter_mut().filter(|m| mine(m)) {
            w.module(m);
        }
        tried += 1;
        if order.iter().all(|t| passes(&ms, t)) {
            let s = &sites[i];
            let name = s.file.rsplit('/').next().unwrap_or(&s.file);
            fixes.push(Fix {
                file: s.file.clone(),
                line: s.line,
                kind: s.kind.clone(),
                text: format!(
                    "Line {} of {name}: `{}` → `{}`; {} pass{}.",
                    s.line,
                    s.original,
                    s.mistake,
                    if examples.len() == 1 {
                        "the example".to_string()
                    } else {
                        format!("all {} examples", examples.len())
                    },
                    if examples.len() == 1 { "es" } else { "" }
                ),
                edit: files.get(&s.file).and_then(|src| edit(src, s.line, &s.original, &s.mistake)),
                original: s.original.clone(),
                replacement: s.mistake.clone(),
            });
        }
    }
    let message = if !fixes.is_empty() {
        format!(
            "Found {} small change{} after which every example passes ({tried} tried).",
            fixes.len(),
            if fixes.len() == 1 { "" } else { "s" }
        )
    } else if tried < sites.len() {
        format!(
            "No small change tried makes every example pass ({tried} of {} tried before the time ran out).",
            sites.len()
        )
    } else {
        format!("None of the {tried} small changes there are makes every example pass.")
    };
    Ok(Fixes {
        fixes,
        tried,
        sites: sites.len(),
        failing: failing.iter().map(|t| t.trim_start_matches("tb_ex_").to_string()).collect(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        message,
    })
}

/// A token of a line of Verilog, with where it is in the line.
#[derive(Debug, Clone, PartialEq)]
struct Tok {
    text: String,
    /// For a number: its value, so `4'd7` matches `7`.
    value: Option<u64>,
    start: usize,
    end: usize,
}

fn tokens(s: &str) -> Vec<Tok> {
    const OPS: [&str; 22] = [
        "<<<", ">>>", "===", "!==", "<=", ">=", "==", "!=", "&&", "||", "<<", ">>", "~^", "^~", "~&", "~|", "+:", "-:",
        "**", "->", "::", "'{",
    ];
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_alphabetic() || c == '_' || c == '$' || c == '`' {
            i += 1;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push(Tok { text: s[start..i].into(), value: None, start, end: i });
            continue;
        }
        if c.is_ascii_digit() || (c == '\'' && i + 1 < b.len() && b"sSbBoOdDhH".contains(&b[i + 1])) {
            while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if j < b.len() && b[j] == b'\'' {
                i = j + 1;
                if i < b.len() && (b[i] == b's' || b[i] == b'S') {
                    i += 1;
                }
                i += 1; // the base
                while i < b.len() && b[i] == b' ' {
                    i += 1;
                }
                while i < b.len() && ((b[i] as char).is_ascii_hexdigit() || b"_xXzZ?".contains(&b[i])) {
                    i += 1;
                }
            }
            let text = s[start..i].to_string();
            let value = number(&text);
            out.push(Tok { text, value, start, end: i });
            continue;
        }
        let op = OPS.iter().find(|o| s[i..].starts_with(*o)).map_or(1, |o| o.len());
        i += op;
        out.push(Tok { text: s[start..i].into(), value: None, start, end: i });
    }
    out
}

/// The value of a Verilog number (`12`, `4'd7`, `8'hff`, `1'b1`); None with x/z bits.
fn number(t: &str) -> Option<u64> {
    let t: String = t.chars().filter(|c| *c != '_' && *c != ' ').collect();
    match t.split_once('\'') {
        None => t.parse().ok(),
        Some((_, rest)) => {
            let rest = rest.trim_start_matches(['s', 'S']);
            let (base, digits) = rest.split_at(1);
            let radix = match base.to_ascii_lowercase().as_str() {
                "b" => 2,
                "o" => 8,
                "d" => 10,
                "h" => 16,
                _ => return None,
            };
            u64::from_str_radix(digits, radix).ok()
        }
    }
}

fn same(a: &Tok, b: &Tok) -> bool {
    match (a.value, b.value) {
        (Some(x), Some(y)) => x == y,
        _ => a.text == b.text,
    }
}

/// `src` with `original` (as the syntax tree prints it) on line `line` replaced by `replacement`:
/// numbers as written in the line where they are unchanged, and in its style (`4'd8`) where the
/// change is to a number.
fn edit(src: &str, line: u32, original: &str, replacement: &str) -> Option<Edit> {
    let lines: Vec<&str> = src.split_inclusive('\n').collect();
    let raw = *lines.get(line.checked_sub(1)? as usize)?;
    let text = raw.trim_end_matches(['\n', '\r']);
    let have = tokens(text);
    let want = tokens(original);
    if want.is_empty() {
        return None;
    }
    let at = (0..have.len()).find_map(|k| matched(&have, &want, k))?;
    let span: Vec<&Tok> = at.iter().map(|&i| &have[i]).collect();
    let written: Vec<&str> = span.iter().map(|t| &text[t.start..t.end]).collect();

    // The replacement, its numbers written the way the line writes them.
    let new = tokens(replacement);
    let aligned = new.len() == want.len();
    let mut out = String::new();
    let mut last = 0;
    for (k, t) in new.iter().enumerate() {
        out.push_str(&replacement[last..t.start]);
        last = t.end;
        let Some(v) = t.value else {
            out.push_str(&t.text);
            continue;
        };
        let spelled = if aligned && span[k].value.is_some() {
            // The number at the same place: as written, or changed in its style.
            if span[k].value == Some(v) { Some(written[k].to_string()) } else { restyle(written[k], v) }
        } else {
            // A number of the original, as written.
            span.iter().zip(&written).find(|(o, _)| o.value == Some(v)).map(|(_, w)| w.to_string())
        };
        out.push_str(&spelled.unwrap_or_else(|| t.text.clone()));
    }
    out.push_str(&replacement[last..]);

    let after = format!("{}{}{}", &text[..span[0].start], out, &text[span[span.len() - 1].end..]);
    let mut content = String::with_capacity(src.len() + 8);
    for (i, l) in lines.iter().enumerate() {
        if i + 1 == line as usize {
            content.push_str(&after);
            content.push_str(&l[text.len()..]);
        } else {
            content.push_str(l);
        }
    }
    Some(Edit { before: text.to_string(), after, content })
}

/// Where `want` appears in `have` from token `from` on: the token matching each, allowing
/// parentheses the printed form leaves out (`!(en)` for `!en`) as long as they pair up.
fn matched(have: &[Tok], want: &[Tok], from: usize) -> Option<Vec<usize>> {
    let mut at = Vec::with_capacity(want.len());
    let (mut i, mut depth) = (from, 0i32);
    for w in want {
        loop {
            let h = have.get(i)?;
            if same(w, h) {
                break;
            }
            // Only inside the match, and only parentheses.
            match h.text.as_str() {
                "(" if !at.is_empty() => depth += 1,
                ")" if !at.is_empty() && depth > 0 => depth -= 1,
                _ => return None,
            }
            i += 1;
        }
        at.push(i);
        i += 1;
    }
    // Close what was skipped open.
    while depth > 0 {
        if have.get(i)?.text != ")" {
            return None;
        }
        depth -= 1;
        at.push(i);
        i += 1;
    }
    Some(at)
}

/// `v` written like `like` (`4'd7` → `4'd8`, `1'b0` → `1'b1`); None if `like` isn't a sized number.
fn restyle(like: &str, v: u64) -> Option<String> {
    let (w, rest) = like.split_once('\'')?;
    let signed = rest.starts_with(['s', 'S']);
    let base = rest.trim_start_matches(['s', 'S']).chars().next()?;
    let digits = match base.to_ascii_lowercase() {
        'b' => format!("{v:b}"),
        'o' => format!("{v:o}"),
        'h' => format!("{v:x}"),
        _ => v.to_string(),
    };
    Some(format!("{}'{}{base}{digits}", w.trim(), if signed { "s" } else { "" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(id: &str) -> (BTreeMap<String, String>, Vec<String>) {
        let st = crate::starters::get(id).expect("starter");
        let files = st
            .project_files(&st.title)
            .into_iter()
            .filter_map(|(p, c)| String::from_utf8(c).ok().map(|c| (p, c)))
            .collect();
        (files, st.spec.examples.iter().map(|e| crate::spec::tb_name(&e.id)).collect())
    }

    fn run(files: &BTreeMap<String, String>, examples: &[String], focus: &[Focus]) -> Result<Fixes, String> {
        wafer_sim::on_big_stack(|| find(files, examples, focus, Duration::from_secs(60)))
    }

    /// Breaks a starter in one place and expects the repair (and the line as it was) back.
    fn repairs(id: &str, file: &str, good: &str, bad: &str) -> Fixes {
        let (mut files, examples) = project(id);
        let src = files[file].clone();
        assert_eq!(src.matches(good).count(), 1, "{good}");
        let line = src[..src.find(good).unwrap()].matches('\n').count() as u32 + 1;
        files.insert(file.into(), src.replacen(good, bad, 1));
        let r = run(&files, &examples, &[Focus { file: file.into(), line }]).expect("searches");
        let back = r.fixes.iter().find(|f| f.edit.as_ref().is_some_and(|e| e.content == src));
        assert!(back.is_some(), "{id}: {bad} → {good}: {r:#?}");
        r
    }

    #[test]
    fn typical_mistakes_are_repaired_back_to_the_line_as_written() {
        // Off by one: the edit keeps the line, only the condition changes.
        let r = repairs("blinker", "src/blinker.v", "cnt == N - 1", "cnt == N");
        let f = &r.fixes[0];
        assert_eq!((f.original.as_str(), f.replacement.as_str()), ("cnt == N", "cnt == N - 1"));
        assert_eq!(f.text, format!("Line {} of blinker.v: `cnt == N` → `cnt == N - 1`; all 2 examples pass.", f.line));
        let e = f.edit.as_ref().unwrap();
        assert!(e.before.contains("cnt == N)") && e.after.contains("cnt == N - 1)"), "{e:?}");
        assert_eq!(r.failing.len(), 1);

        // A comparison turned around, `-` for `+`, and a sized constant one off (written as before).
        repairs("updown", "src/updown.v", "up && !down) count <= count + 4'd1", "up && !down) count <= count - 4'd1");
        repairs("watchdog", "src/watchdog.v", "bark <= 1'b1", "bark <= 1'b0");
    }

    /// Every mistake "Would your examples catch a mistake?" plants in a starter (a spread of them;
    /// leaving lines out aside), written into the file, is repaired back to the line as it was.
    #[test]
    fn planted_mistakes_in_the_starters_are_repaired_exactly() {
        let mut repaired = 0;
        for st in crate::starters::all() {
            let (files, examples) = project(&st.id);
            let hdl: Vec<(&str, &str)> =
                files.iter().filter(|(p, _)| p.ends_with(".v")).map(|(p, c)| (p.as_str(), c.as_str())).collect();
            let (mut ms, _) = wafer_sim::parse(&hdl).unwrap();
            let mut w = Walker::new(None);
            ms.iter_mut().filter(|m| !m.file.starts_with("tb/")).for_each(|m| w.module(m));
            let every = (w.sites.len() / 6).max(1);
            for s in w.sites.iter().step_by(every).filter(|s| s.kind != "forget") {
                let planted = edit(&files[&s.file], s.line, &s.original, &s.mistake).expect("placed in the text");
                let mut broken = files.clone();
                broken.insert(s.file.clone(), planted.content);
                let Ok(r) = run(&broken, &examples, &[Focus { file: s.file.clone(), line: s.line }]) else {
                    continue; // no example notices it
                };
                assert!(
                    r.fixes.iter().any(|f| f.edit.as_ref().is_some_and(|e| e.content == files[&s.file])),
                    "{}: {} → {}: {r:#?}",
                    st.id,
                    s.original,
                    s.mistake
                );
                repaired += 1;
            }
        }
        assert!(repaired > 60, "{repaired}");
    }

    #[test]
    fn passing_designs_are_refused() {
        let (files, examples) = project("blinker");
        assert_eq!(run(&files, &examples, &[]).unwrap_err(), "every example passes already");
    }

    #[test]
    fn edits_keep_how_the_line_is_written() {
        let src = "a\n    else if (cnt == 16'd7) begin // seven\nb\n";
        let e = edit(src, 2, "cnt == 7", "cnt == 8").unwrap();
        assert_eq!(e.after, "    else if (cnt == 16'd8) begin // seven");
        assert_eq!(e.content, "a\n    else if (cnt == 16'd8) begin // seven\nb\n");
        let e = edit("  if (!rst_n) x <= 1'b0;\n", 1, "!rst_n", "rst_n").unwrap();
        assert_eq!(e.after, "  if (rst_n) x <= 1'b0;");
        let e = edit("  led <= 1'b0;\n", 1, "led <= 0", "led <= 1").unwrap();
        assert_eq!(e.after, "  led <= 1'b1;");
        let e = edit("  if (a&&b) y = 0;\n", 1, "a && b", "a || b").unwrap();
        assert_eq!(e.after, "  if (a || b) y = 0;");
        let e = edit("  if (!(en)) y = 0;\n", 1, "!en", "en").unwrap();
        assert_eq!(e.after, "  if (en) y = 0;", "parentheses the printed form leaves out");
        assert!(edit("  x = 1;\n", 1, "y == 2", "y == 3").is_none(), "not on that line");
    }
}
