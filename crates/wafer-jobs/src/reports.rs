//! Reports a LibreLane run leaves behind, turned into JSON for the viewer:
//! DRC violations with geometry (KLayout, Magic and router DRC all write the
//! KLayout report-database format) and timing/area by corner.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

/// Most violations returned per report; real runs with thousands are summarized by rule counts.
pub const MAX_VIOLATIONS: usize = 2000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Violation {
    /// `klayout`, `magic` or `route`.
    pub source: String,
    pub rule: String,
    pub description: String,
    /// Polygons in top-cell µm. Edge pairs become the quadrilateral between the two edges.
    pub shapes: Vec<Vec<[f64; 2]>>,
    /// `[x0, y0, x1, y1]` over all shapes, or `None` if the location could not be resolved.
    pub bbox: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct DrcReport {
    pub violations: Vec<Violation>,
    /// Count per (source, rule), including violations beyond `MAX_VIOLATIONS`.
    pub counts: Vec<RuleCount>,
    pub truncated: bool,
    /// Report files that were read, relative to the run directory.
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RuleCount {
    pub source: String,
    pub rule: String,
    pub description: String,
    pub count: usize,
}

/// The newest step directory whose name ends with `suffix` (step dirs are `NN-name`).
fn latest_step(run: &Path, suffix: &str) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for e in std::fs::read_dir(run).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((num, rest)) = name.split_once('-') else { continue };
        let Ok(num) = num.parse::<u32>() else { continue };
        // `openroad-detailedrouting` but also `openroad-detailedrouting-1` etc.
        if (rest == suffix || rest.strip_prefix(suffix).is_some_and(|r| r.starts_with('-')))
            && best.as_ref().is_none_or(|(n, _)| num > *n)
        {
            best = Some((num, e.path()));
        }
    }
    best.map(|(_, p)| p)
}

pub fn drc_report(run: &Path) -> DrcReport {
    let mut out = DrcReport::default();
    let mut sources: Vec<(&str, PathBuf)> = Vec::new();
    if let Some(d) = latest_step(run, "klayout-drc") {
        sources.push(("klayout", d.join("reports/drc.klayout.lyrdb")));
    }
    if let Some(d) = latest_step(run, "magic-drc") {
        sources.push(("magic", d.join("reports/drc.magic.lyrdb")));
    }
    if let Some(d) = latest_step(run, "openroad-detailedrouting")
        && let Ok(entries) = std::fs::read_dir(&d)
    {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".drc.xml") {
                sources.push(("route", e.path()));
            }
        }
    }
    for (source, path) in sources {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        out.sources.push(path.strip_prefix(run).unwrap_or(&path).to_string_lossy().into_owned());
        let Ok(parsed) = parse_lyrdb(source, &text) else { continue };
        for v in parsed {
            match out.counts.iter_mut().find(|c| c.source == v.source && c.rule == v.rule) {
                Some(c) => c.count += 1,
                None => out.counts.push(RuleCount {
                    source: v.source.clone(),
                    rule: v.rule.clone(),
                    description: v.description.clone(),
                    count: 1,
                }),
            }
            if out.violations.len() < MAX_VIOLATIONS {
                out.violations.push(v);
            } else {
                out.truncated = true;
            }
        }
    }
    out
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text(n: Option<roxmltree::Node<'_, '_>>) -> String {
    n.and_then(|n| n.text()).unwrap_or("").trim().to_string()
}

/// Parses a KLayout report database (`.lyrdb`) into violations in top-cell coordinates.
pub fn parse_lyrdb(source: &str, xml: &str) -> Result<Vec<Violation>, roxmltree::Error> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();

    // Category descriptions, keyed by dotted path for nested categories.
    let mut descriptions = Map::new();
    fn walk(n: roxmltree::Node<'_, '_>, prefix: &str, out: &mut Map<String, Value>) {
        for c in n.children().filter(|c| c.has_tag_name("category")) {
            let name = c.children().find(|x| x.has_tag_name("name")).and_then(|x| x.text()).unwrap_or("").trim();
            let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
            let desc = c.children().find(|x| x.has_tag_name("description")).and_then(|x| x.text()).unwrap_or("");
            out.insert(path.clone(), Value::String(desc.trim().to_string()));
            if let Some(sub) = c.children().find(|x| x.has_tag_name("categories")) {
                walk(sub, &path, out);
            }
        }
    }
    if let Some(cats) = child(root, "categories") {
        walk(cats, "", &mut descriptions);
    }

    // Cell placements in their parents, to map sub-cell items up to the top.
    let top = text(child(root, "top-cell"));
    let mut placements: Vec<(String, String, Trans)> = Vec::new();
    if let Some(cells) = child(root, "cells") {
        for cell in cells.children().filter(|c| c.has_tag_name("cell")) {
            let name = text(child(cell, "name"));
            if let Some(refs) = child(cell, "references") {
                for r in refs.children().filter(|c| c.has_tag_name("reference")) {
                    if let Some(t) = Trans::parse(&text(child(r, "trans"))) {
                        placements.push((name.clone(), text(child(r, "parent")), t));
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    let Some(items) = child(root, "items") else { return Ok(out) };
    for item in items.children().filter(|c| c.has_tag_name("item")) {
        let rule = unquote(&text(child(item, "category")));
        let cell = text(child(item, "cell"));
        let description = descriptions.get(&rule).and_then(Value::as_str).unwrap_or("").to_string();
        let local: Vec<Vec<[f64; 2]>> = child(item, "values")
            .map(|vs| {
                vs.children()
                    .filter(|c| c.has_tag_name("value"))
                    .filter_map(|v| v.text())
                    .flat_map(parse_value)
                    .collect()
            })
            .unwrap_or_default();
        let transforms = if cell.is_empty() || cell == top || top.is_empty() {
            vec![Trans::IDENTITY]
        } else {
            to_top(&cell, &top, &placements, 0)
        };
        let shapes: Vec<Vec<[f64; 2]>> = transforms
            .iter()
            .flat_map(|t| local.iter().map(move |s| s.iter().map(|p| t.apply(*p)).collect()))
            .collect();
        let bbox = bbox(&shapes);
        out.push(Violation { source: source.to_string(), rule, description, shapes, bbox });
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('\'').trim_matches('"').to_string()
}

fn bbox(shapes: &[Vec<[f64; 2]>]) -> Option<[f64; 4]> {
    let mut it = shapes.iter().flatten();
    let first = it.next()?;
    let mut b = [first[0], first[1], first[0], first[1]];
    for p in it {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    Some(b)
}

/// Transforms placing `cell` in `top` (one per instance path; capped).
fn to_top(cell: &str, top: &str, placements: &[(String, String, Trans)], depth: usize) -> Vec<Trans> {
    if cell == top {
        return vec![Trans::IDENTITY];
    }
    if depth > 32 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (_, parent, t) in placements.iter().filter(|(c, _, _)| c == cell) {
        for up in to_top(parent, top, placements, depth + 1) {
            out.push(up.then(t));
            if out.len() >= 64 {
                return out;
            }
        }
    }
    out
}

/// KLayout simple transformation string: `r90 *2 10,20`, `m45 1.5,-3`, ...
#[derive(Debug, Clone, Copy, PartialEq)]
struct Trans {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl Trans {
    const IDENTITY: Trans = Trans { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    fn parse(s: &str) -> Option<Trans> {
        let mut t = Trans::IDENTITY;
        let (mut rot, mut mirror, mut mag) = (0.0f64, false, 1.0f64);
        for tok in s.split_whitespace() {
            if let Some(r) = tok.strip_prefix('r') {
                rot = r.parse().ok()?;
            } else if let Some(m) = tok.strip_prefix('m') {
                // m<a>: mirror at the axis at angle a, i.e. reflect about x, then rotate by 2a.
                mirror = true;
                rot = 2.0 * m.parse::<f64>().ok()?;
            } else if let Some(m) = tok.strip_prefix('*') {
                mag = m.parse().ok()?;
            } else if let Some((x, y)) = tok.split_once(',') {
                t.tx = x.parse().ok()?;
                t.ty = y.parse().ok()?;
            }
        }
        let (sin, cos) = rot.to_radians().sin_cos();
        let snap = |v: f64| if (v - v.round()).abs() < 1e-12 { v.round() } else { v };
        let (sin, cos) = (snap(sin), snap(cos));
        let r = if mirror { -1.0 } else { 1.0 };
        t.a = mag * cos;
        t.b = -mag * sin * r;
        t.c = mag * sin;
        t.d = mag * cos * r;
        Some(t)
    }

    fn then(&self, o: &Trans) -> Trans {
        Trans {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            tx: self.a * o.tx + self.b * o.ty + self.tx,
            ty: self.c * o.tx + self.d * o.ty + self.ty,
        }
    }

    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.a * p[0] + self.b * p[1] + self.tx, self.c * p[0] + self.d * p[1] + self.ty]
    }
}

/// Parses one `<value>`: `polygon: (x,y;x,y;...)`, `box: (x,y;x,y)`, `edge: (x,y;x,y)`,
/// `edge-pair: (e1)|(e2)` or `(e1)/(e2)`, `path: (x,y;...) w=...`. Other kinds yield nothing.
fn parse_value(v: &str) -> Vec<Vec<[f64; 2]>> {
    let Some((kind, body)) = v.split_once(':') else { return Vec::new() };
    let groups: Vec<Vec<[f64; 2]>> = body
        .split(['(', ')'])
        .filter(|s| s.contains(','))
        .map(|g| {
            g.split(';')
                .filter_map(|p| {
                    let (x, y) = p.trim().split_once(',')?;
                    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
                })
                .collect::<Vec<_>>()
        })
        .filter(|g| !g.is_empty())
        .collect();
    match kind.trim() {
        "polygon" | "simple-polygon" | "path" => groups.into_iter().take(1).collect(),
        "box" => groups
            .first()
            .filter(|g| g.len() == 2)
            .map(|g| vec![vec![g[0], [g[1][0], g[0][1]], g[1], [g[0][0], g[1][1]]]])
            .unwrap_or_default(),
        "edge" => groups.into_iter().take(1).collect(),
        // The area between the two edges is where the problem is.
        "edge-pair" if groups.len() >= 2 && groups[0].len() == 2 && groups[1].len() == 2 => {
            vec![vec![groups[0][0], groups[0][1], groups[1][0], groups[1][1]]]
        }
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Corner {
    pub name: String,
    pub setup_ws: Option<f64>,
    pub setup_tns: Option<f64>,
    pub setup_violations: Option<f64>,
    pub hold_ws: Option<f64>,
    pub hold_tns: Option<f64>,
    pub hold_violations: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CellClass {
    pub class: String,
    pub count: Option<f64>,
    pub area_um2: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PathStage {
    pub pin: String,
    pub cell: Option<String>,
    /// `^` rising or `v` falling.
    pub edge: String,
    pub delay_ns: f64,
    pub time_ns: f64,
    pub fanout: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TimingPath {
    pub check: String,
    pub corner: String,
    pub startpoint: String,
    pub endpoint: String,
    pub group: String,
    pub arrival_ns: Option<f64>,
    pub required_ns: Option<f64>,
    pub slack_ns: Option<f64>,
    pub stages: Vec<PathStage>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunReport {
    pub corners: Vec<Corner>,
    pub die_area_um2: Option<f64>,
    pub core_area_um2: Option<f64>,
    pub stdcell_area_um2: Option<f64>,
    pub utilization: Option<f64>,
    pub classes: Vec<CellClass>,
    /// Worst setup path (worst corner) and worst hold path, when STA reports exist.
    pub paths: Vec<TimingPath>,
}

pub fn run_report(run: &Path, metrics: &Value) -> RunReport {
    let num = |k: &str| metrics.get(k).and_then(Value::as_f64);
    let mut corner_names: Vec<String> = metrics
        .as_object()
        .map(|o| o.keys().filter_map(|k| k.strip_prefix("timing__setup__ws__corner:").map(str::to_string)).collect())
        .unwrap_or_default();
    corner_names.sort();
    let corners: Vec<Corner> = corner_names
        .into_iter()
        .map(|c| Corner {
            setup_ws: num(&format!("timing__setup__ws__corner:{c}")),
            setup_tns: num(&format!("timing__setup__tns__corner:{c}")),
            setup_violations: num(&format!("timing__setup_vio__count__corner:{c}")),
            hold_ws: num(&format!("timing__hold__ws__corner:{c}")),
            hold_tns: num(&format!("timing__hold__tns__corner:{c}")),
            hold_violations: num(&format!("timing__hold_vio__count__corner:{c}")),
            name: c,
        })
        .collect();

    let mut classes: Vec<String> = metrics
        .as_object()
        .map(|o| {
            o.keys()
                .filter_map(|k| {
                    k.strip_prefix("design__instance__count__class:")
                        .or_else(|| k.strip_prefix("design__instance__area__class:"))
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    classes.sort();
    classes.dedup();
    let mut classes: Vec<CellClass> = classes
        .into_iter()
        .map(|c| CellClass {
            count: num(&format!("design__instance__count__class:{c}")),
            area_um2: num(&format!("design__instance__area__class:{c}")),
            class: c,
        })
        .collect();
    classes.sort_by(|a, b| b.area_um2.unwrap_or(0.0).total_cmp(&a.area_um2.unwrap_or(0.0)));

    let mut paths = Vec::new();
    if let Some(sta) = latest_step(run, "openroad-stapostpnr") {
        let worst = |key: fn(&Corner) -> Option<f64>| {
            corners
                .iter()
                .filter_map(|c| key(c).map(|v| (v, c.name.clone())))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, n)| n)
        };
        for (check, file, corner) in
            [("setup", "max.rpt", worst(|c| c.setup_ws)), ("hold", "min.rpt", worst(|c| c.hold_ws))]
        {
            let Some(corner) = corner else { continue };
            if let Ok(text) = std::fs::read_to_string(sta.join(&corner).join(file))
                && let Some(p) = parse_sta_path(&text, check, &corner)
            {
                paths.push(p);
            }
        }
    }

    RunReport {
        corners,
        die_area_um2: num("design__die__area"),
        core_area_um2: num("design__core__area"),
        stdcell_area_um2: num("design__instance__area__stdcell"),
        utilization: num("design__instance__utilization"),
        classes,
        paths,
    }
}

/// The first (worst) path of an OpenSTA `report_checks` output.
pub fn parse_sta_path(text: &str, check: &str, corner: &str) -> Option<TimingPath> {
    let mut lines = text.lines().skip_while(|l| !l.starts_with("Startpoint:"));
    let startpoint = lines.next()?.trim_start_matches("Startpoint:").trim().to_string();
    let mut p = TimingPath {
        check: check.into(),
        corner: corner.into(),
        startpoint,
        endpoint: String::new(),
        group: String::new(),
        arrival_ns: None,
        required_ns: None,
        slack_ns: None,
        stages: Vec::new(),
    };
    let mut in_arrival = false;
    let mut header_seen = false;
    for line in lines {
        let t = line.trim();
        if let Some(e) = t.strip_prefix("Endpoint:") {
            p.endpoint = e.trim().to_string();
        } else if let Some(g) = t.strip_prefix("Path Group:") {
            p.group = g.trim().to_string();
        } else if t.starts_with("Fanout") || t.starts_with("Delay") {
            header_seen = true;
            in_arrival = true;
        } else if t.ends_with("data arrival time") {
            if in_arrival {
                p.arrival_ns = t.split_whitespace().next().and_then(|v| v.parse().ok());
            }
            in_arrival = false;
        } else if t.ends_with("data required time") && p.required_ns.is_none() {
            p.required_ns = t.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if t.contains("slack (") {
            p.slack_ns = t.split_whitespace().next().and_then(|v| v.parse().ok());
            break;
        } else if in_arrival
            && header_seen
            && let Some(stage) = parse_stage(t)
        {
            p.stages.push(stage);
        }
    }
    Some(p)
}

/// A pin line: numbers, then `^`/`v`, then `inst/pin (cell)` or `port (in)`.
fn parse_stage(t: &str) -> Option<PathStage> {
    let edge_pos = t.find(" ^ ").or_else(|| t.find(" v "))?;
    let nums: Vec<f64> = t[..edge_pos].split_whitespace().filter_map(|n| n.parse().ok()).collect();
    if nums.len() < 2 {
        return None;
    }
    let edge = t[edge_pos + 1..edge_pos + 2].to_string();
    let desc = t[edge_pos + 3..].trim();
    let (pin, cell) = match desc.rsplit_once(" (") {
        Some((pin, cell)) => (pin.trim().to_string(), Some(cell.trim_end_matches(')').to_string())),
        None => (desc.to_string(), None),
    };
    // Columns present vary (Fanout, Cap, Slew, Delay, Time); Delay and Time are always the last two.
    let (delay, time) = (nums[nums.len() - 2], nums[nums.len() - 1]);
    let fanout = if nums.len() >= 5 { Some(nums[0] as u32) } else { None };
    Some(PathStage { pin, cell, edge, delay_ns: delay, time_ns: time, fanout })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(parse_value("polygon: (8,5;8,5.12;8.12,5.12;8.12,5)")[0].len(), 4);
        assert_eq!(parse_value("box: (1,2;3,4)"), vec![vec![[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]]]);
        assert_eq!(
            parse_value("edge-pair: (2,5;2,9)|(2.1,9;2.1,5)"),
            vec![vec![[2.0, 5.0], [2.0, 9.0], [2.1, 9.0], [2.1, 5.0]]]
        );
        assert_eq!(parse_value("edge-pair: (0,0;1,0)/(1,1;0,1)")[0].len(), 4);
        assert!(parse_value("text: ('x',r0 0,0)").is_empty());
        assert!(parse_value("garbage").is_empty());
    }

    #[test]
    fn klayout_trans() {
        let t = Trans::parse("r90 10,20").unwrap();
        assert_eq!(t.apply([1.0, 0.0]), [10.0, 21.0]);
        let m = Trans::parse("m0 *2 0,0").unwrap();
        assert_eq!(m.apply([1.0, 1.0]), [2.0, -2.0]);
    }

    #[test]
    fn sub_cell_items_map_to_top() {
        let xml = r#"<report-database><top-cell>TOP</top-cell>
          <categories><category><name>X.a</name><description>desc</description><categories/></category></categories>
          <cells>
            <cell><name>TOP</name><references/></cell>
            <cell><name>SUB</name><references>
              <reference><parent>TOP</parent><trans>r0 *1 100,0</trans></reference>
              <reference><parent>TOP</parent><trans>r0 *1 0,50</trans></reference>
            </references></cell>
          </cells>
          <items><item><category>'X.a'</category><cell>SUB</cell><values><value>box: (0,0;1,1)</value></values></item></items>
        </report-database>"#;
        let v = parse_lyrdb("klayout", xml).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].description, "desc");
        assert_eq!(v[0].shapes.len(), 2);
        assert_eq!(v[0].bbox, Some([0.0, 0.0, 101.0, 51.0]));
    }

    #[test]
    fn sta_path() {
        let rpt = "\
Startpoint: en (input port clocked by clk)
Endpoint: _42_ (rising edge-triggered flip-flop clocked by clk)
Path Group: clk
Path Type: max

Fanout         Cap        Slew       Delay        Time   Description
---------------------------------------------------------------------------------------------
                                  0.000000    0.000000   clock clk (rise edge)
                                  4.000000    4.000000 ^ input external delay
     1    0.006029    0.037040    0.008439    4.008440 ^ en (in)
                                                         en (net)
                      0.037041    0.000000    4.008440 ^ input1/A (sg13g2_buf_1)
     3    0.016475    0.116045    0.161700    4.170140 ^ input1/X (sg13g2_buf_1)
                      0.050138    0.000068    6.074377 v _42_/D (sg13g2_dfrbpq_1)
                                              6.074377   data arrival time

                                 20.000000   20.000000   clock clk (rise edge)
     1    0.017994    0.053125    0.022636   20.022636 ^ clk (in)
                                             19.774658   data required time
---------------------------------------------------------------------------------------------
                                             19.774658   data required time
                                             -6.074377   data arrival time
---------------------------------------------------------------------------------------------
                                             13.700282   slack (MET)
";
        let p = parse_sta_path(rpt, "setup", "slow").unwrap();
        assert_eq!(p.startpoint, "en (input port clocked by clk)");
        assert!(p.endpoint.starts_with("_42_"));
        assert_eq!(p.group, "clk");
        assert_eq!(p.arrival_ns, Some(6.074377));
        assert_eq!(p.required_ns, Some(19.774658));
        assert_eq!(p.slack_ns, Some(13.700282));
        let pins: Vec<&str> = p.stages.iter().map(|s| s.pin.as_str()).collect();
        assert_eq!(pins, vec!["input external delay", "en", "input1/A", "input1/X", "_42_/D"]);
        assert_eq!(p.stages[3].cell.as_deref(), Some("sg13g2_buf_1"));
        assert_eq!(p.stages[3].fanout, Some(3));
        assert_eq!(p.stages[4].edge, "v");
        assert_eq!(p.stages[3].delay_ns, 0.1617);
    }
}
