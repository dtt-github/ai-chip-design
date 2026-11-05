//! Verilog tokens. Handles comments, sized/based numbers (with x/z/? masks for
//! `casez`), strings, and the compiler directives that matter here: `define
//! (object-like macros), `timescale; others are skipped.

use std::collections::HashMap;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// `$name`
    System(String),
    Number(Num),
    Str(String),
    Op(&'static str),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Num {
    pub value: u64,
    /// Bits written as x, z or ?.
    pub xz: u64,
    pub width: Option<u32>,
    pub signed: bool,
    /// `'0` / `'1`: fill to the context width.
    pub fill: bool,
    /// Had a fractional part (delays); value is rounded.
    pub real: bool,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
}

/// Longest first.
const OPS: &[&str] = &[
    "<<<=", ">>>=", "===", "!==", "<<<", ">>>", "<<=", ">>=", "**", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>",
    "~&", "~|", "~^", "^~", "+:", "-:", "->", "(*", "*)", "::", "++", "--", "+=", "-=", "+", "-", "*", "/", "%", "<",
    ">", "!", "~", "&", "|", "^", "=", "?", ":", ";", ",", ".", "(", ")", "[", "]", "{", "}", "#", "@", "'",
];

pub struct Lexed {
    pub tokens: Vec<Token>,
    /// Time unit in femtoseconds from the last `timescale (default 1 ns).
    pub timescale_fs: u64,
}

/// Expands object-like macros and handles directives, keeping line numbers:
/// directive lines become blank lines.
fn preprocess(src: &str, file: &str, defines: &mut HashMap<String, String>, timescale_fs: &mut u64) -> Result<String> {
    let mut out = String::with_capacity(src.len());
    for (n, raw) in src.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let line = n as u32 + 1;
        let err = |msg: String| Error::Syntax { file: file.to_string(), line, msg };
        let trimmed = raw.trim_start();
        if let Some(d) = trimmed.strip_prefix('`') {
            let name: String = d.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let rest = &d[name.len()..];
            let handled = match name.as_str() {
                "define" => {
                    let rest = rest.trim();
                    let (m, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if m.contains('(') {
                        return Err(err(format!("macros with arguments are not supported (`define {m})")));
                    }
                    let body = body.split("//").next().unwrap_or("").trim().to_string();
                    defines.insert(m.to_string(), body);
                    true
                }
                "timescale" => {
                    if let Some(fs) = parse_time_unit(rest.split('/').next().unwrap_or("")) {
                        *timescale_fs = fs;
                    }
                    true
                }
                "include" => {
                    return Err(err("`include is not supported; add the file to the simulation instead".into()));
                }
                "ifdef" | "ifndef" | "else" | "elsif" | "endif" => {
                    return Err(err(format!("`{name} is not supported yet")));
                }
                "default_nettype"
                | "resetall"
                | "celldefine"
                | "endcelldefine"
                | "undef"
                | "unconnected_drive"
                | "nounconnected_drive" => true,
                _ => false,
            };
            if handled {
                continue;
            }
        }
        // Expand `NAME uses (repeatedly, for macros that use macros).
        let mut text = raw.to_string();
        for _ in 0..32 {
            let Some(pos) = text.find('`') else { break };
            let name: String = text[pos + 1..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let body = defines.get(&name).ok_or_else(|| err(format!("undefined macro `{name}")))?;
            text = format!("{}{} {}", &text[..pos], body, &text[pos + 1 + name.len()..]);
        }
        if text.contains('`') {
            return Err(err("macro expansion too deep".into()));
        }
        out.push_str(&text);
    }
    Ok(out)
}

pub fn lex(src: &str, file: &str, defines: &mut HashMap<String, String>) -> Result<Lexed> {
    let mut timescale_fs = 1_000_000;
    let src = preprocess(src, file, defines, &mut timescale_fs)?;
    let mut out = Lexed { tokens: Vec::new(), timescale_fs };
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1u32;
    let err = |line: u32, msg: String| Error::Syntax { file: file.to_string(), line, msg };

    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                if chars[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        // Attributes (* ... *) are ignored.
        if c == '(' && chars.get(i + 1) == Some(&'*') && chars.get(i + 2) != Some(&')') {
            let mut j = i + 2;
            while j + 1 < chars.len() && !(chars[j] == '*' && chars[j + 1] == ')') {
                j += 1;
            }
            if j + 1 < chars.len() {
                line += chars[i..j].iter().filter(|&&c| c == '\n').count() as u32;
                i = j + 2;
                continue;
            }
        }
        let tok_line = line;
        // Identifiers and keywords.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            out.tokens.push(Token { tok: Tok::Ident(chars[start..i].iter().collect()), line: tok_line });
            continue;
        }
        if c == '\\' {
            // Escaped identifier: up to whitespace.
            let start = i + 1;
            i += 1;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            out.tokens.push(Token { tok: Tok::Ident(chars[start..i].iter().collect()), line: tok_line });
            continue;
        }
        if c == '$' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            out.tokens.push(Token { tok: Tok::System(chars[start..i].iter().collect()), line: tok_line });
            continue;
        }
        if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        other => other,
                    });
                } else {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    s.push(chars[i]);
                }
                i += 1;
            }
            i += 1;
            out.tokens.push(Token { tok: Tok::Str(s), line: tok_line });
            continue;
        }
        // Numbers: [size]'[s]base digits | decimal | real.
        if c.is_ascii_digit() || (c == '\'' && is_base_start(&chars, i + 1)) {
            let (num, next) = lex_number(&chars, i).map_err(|m| err(line, m))?;
            out.tokens.push(Token { tok: Tok::Number(num), line: tok_line });
            i = next;
            continue;
        }
        if let Some(op) = OPS.iter().find(|op| starts_with(&chars, i, op)) {
            out.tokens.push(Token { tok: Tok::Op(op), line: tok_line });
            i += op.len();
            continue;
        }
        return Err(err(line, format!("unexpected character '{c}'")));
    }
    out.tokens.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

fn starts_with(chars: &[char], i: usize, s: &str) -> bool {
    s.chars().enumerate().all(|(k, c)| chars.get(i + k) == Some(&c))
}

fn is_base_start(chars: &[char], i: usize) -> bool {
    let mut i = i;
    if matches!(chars.get(i), Some('s' | 'S')) {
        i += 1;
    }
    matches!(chars.get(i), Some('b' | 'B' | 'o' | 'O' | 'd' | 'D' | 'h' | 'H' | '0' | '1' | 'x' | 'X' | 'z' | 'Z'))
}

fn parse_time_unit(s: &str) -> Option<u64> {
    let s = s.trim();
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    let unit = s[digits.len()..].trim();
    let n: u64 = digits.parse().ok()?;
    let fs = match unit {
        "s" => 1_000_000_000_000_000,
        "ms" => 1_000_000_000_000,
        "us" => 1_000_000_000,
        "ns" => 1_000_000,
        "ps" => 1_000,
        "fs" => 1,
        _ => return None,
    };
    Some(n * fs)
}

fn lex_number(chars: &[char], start: usize) -> std::result::Result<(Num, usize), String> {
    let mut i = start;
    let mut size_digits = String::new();
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
        if chars[i] != '_' {
            size_digits.push(chars[i]);
        }
        i += 1;
    }
    // Skip whitespace between size and base ("8 'hFF" is legal).
    let mut j = i;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if chars.get(j) == Some(&'\'') && is_base_start(chars, j + 1) {
        i = j + 1;
        let width = if size_digits.is_empty() {
            None
        } else {
            let w: u32 = size_digits.parse().map_err(|_| "bad number size".to_string())?;
            if w == 0 {
                return Err("zero-width number".into());
            }
            Some(w)
        };
        let mut signed = false;
        if matches!(chars.get(i), Some('s' | 'S')) {
            signed = true;
            i += 1;
        }
        // '0, '1, 'x, 'z fill literals.
        if width.is_none() && matches!(chars.get(i), Some('0' | '1' | 'x' | 'X' | 'z' | 'Z')) {
            let c = chars[i];
            let all = matches!(c, '1');
            let xz = matches!(c, 'x' | 'X' | 'z' | 'Z');
            return Ok((
                Num {
                    value: if all { u64::MAX } else { 0 },
                    xz: if xz { u64::MAX } else { 0 },
                    width: None,
                    signed: false,
                    fill: true,
                    real: false,
                },
                i + 1,
            ));
        }
        let base = chars[i].to_ascii_lowercase();
        i += 1;
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        let bits_per = match base {
            'b' => 1,
            'o' => 3,
            'h' => 4,
            _ => 0,
        };
        let mut value: u128 = 0;
        let mut xz: u128 = 0;
        let mut any = false;
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '?') {
            let d = chars[i].to_ascii_lowercase();
            i += 1;
            if d == '_' {
                continue;
            }
            any = true;
            if base == 'd' {
                if matches!(d, 'x' | 'z' | '?') {
                    xz = u128::MAX;
                    continue;
                }
                let v = d.to_digit(10).ok_or_else(|| format!("bad decimal digit '{d}'"))?;
                value = value.wrapping_mul(10).wrapping_add(v as u128);
            } else {
                let mask = (1u128 << bits_per) - 1;
                value <<= bits_per;
                xz <<= bits_per;
                if matches!(d, 'x' | 'z' | '?') {
                    xz |= mask;
                } else {
                    let v =
                        d.to_digit(16).filter(|&v| (v as u128) <= mask).ok_or_else(|| format!("bad digit '{d}'"))?;
                    value |= v as u128;
                }
            }
        }
        if !any {
            return Err("number without digits".into());
        }
        let w = width.unwrap_or(32);
        if w > 64 && (value >> 64) != 0 {
            return Err("numbers wider than 64 bits are not supported".into());
        }
        let m = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        return Ok((
            Num {
                value: value as u64 & m,
                xz: xz as u64 & m,
                width: Some(w.min(64)),
                signed,
                fill: false,
                real: false,
            },
            i,
        ));
    }
    // Plain decimal, maybe real.
    let mut real = false;
    let mut frac = String::new();
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        real = true;
        i += 1;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
            frac.push(chars[i]);
            i += 1;
        }
    }
    let int: u64 = size_digits.parse().map_err(|_| "number too large".to_string())?;
    let value = if real && frac.starts_with(|c: char| c >= '5') { int + 1 } else { int };
    Ok((Num { value, xz: 0, width: None, signed: true, fill: false, real }, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s, "t.v", &mut HashMap::new()).unwrap().tokens.into_iter().map(|t| t.tok).collect()
    }

    fn num(s: &str) -> Num {
        match &toks(s)[0] {
            Tok::Number(n) => *n,
            t => panic!("{t:?}"),
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(num("8'hFF").value, 255);
        assert_eq!(num("8'hFF").width, Some(8));
        assert_eq!(num("4'b1?0z").xz, 0b0101);
        assert_eq!(num("4'b1?0z").value, 0b1000);
        assert_eq!(num("'d12").width, Some(32));
        assert_eq!(num("16'd1_000").value, 1000);
        assert_eq!(num("5").width, None);
        assert!(num("8'sd5").signed);
        assert!(num("'1").fill);
        assert_eq!(num("2.5").value, 3);
        assert_eq!(num("3'b1111").value, 0b111);
    }

    #[test]
    fn ops_comments_and_macros() {
        let mut d = HashMap::new();
        let l = lex("`define W 8\n`timescale 1ns/1ps\n// x\n a <= b >>> `W; /* c\n */ d", "t.v", &mut d).unwrap();
        let t: Vec<Tok> = l.tokens.iter().map(|t| t.tok.clone()).collect();
        assert_eq!(t[1], Tok::Op("<="));
        assert_eq!(t[3], Tok::Op(">>>"));
        assert!(matches!(t[4], Tok::Number(Num { value: 8, .. })));
        assert_eq!(l.tokens.last().unwrap().line, 5);
        assert_eq!(l.timescale_fs, 1_000_000);
    }
}
