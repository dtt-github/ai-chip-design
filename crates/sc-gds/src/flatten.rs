//! Hierarchy → flat, render-ready geometry: per (layer, datatype) a triangle
//! list for fills and a line list for outlines, in micrometres.

use std::collections::{BTreeMap, HashMap};

use crate::parse::{Cell, Library, Path, Point, Reference, Strans};
use crate::{GdsError, Result};

const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Upper bound on output floats (triangles + edges) before giving up.
    pub max_floats: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_floats: 120_000_000 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FlatLayer {
    pub layer: i16,
    pub datatype: i16,
    /// x, y pairs; every three vertices form a triangle.
    pub triangles: Vec<f32>,
    /// x, y pairs; every two vertices form a line segment.
    pub edges: Vec<f32>,
    pub shapes: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlatText {
    pub layer: i16,
    pub texttype: i16,
    pub x: f32,
    pub y: f32,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Flat {
    pub top: String,
    pub layers: Vec<FlatLayer>,
    pub texts: Vec<FlatText>,
    /// [x0, y0, x1, y1] in µm; all zeros for an empty layout.
    pub bbox: [f64; 4],
    pub instances: u64,
}

/// Row-major 2×3 affine transform: x' = a x + b y + tx, y' = c x + d y + ty.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl Affine {
    fn scale(s: f64) -> Self {
        Self { a: s, b: 0.0, c: 0.0, d: s, tx: 0.0, ty: 0.0 }
    }

    /// GDS placement: reflect about x, magnify, rotate, then translate to `origin`.
    fn placement(s: &Strans, origin: (f64, f64)) -> Self {
        let (sin, cos) = s.angle.to_radians().sin_cos();
        // Snap to exact values for multiples of 90° (the common case) to avoid drift.
        let snap = |v: f64| if (v - v.round()).abs() < 1e-12 { v.round() } else { v };
        let (sin, cos) = (snap(sin), snap(cos));
        let r = if s.reflect { -1.0 } else { 1.0 };
        Self { a: s.mag * cos, b: -s.mag * sin * r, c: s.mag * sin, d: s.mag * cos * r, tx: origin.0, ty: origin.1 }
    }

    /// `self ∘ other`: apply `other` first.
    fn then(&self, other: &Affine) -> Affine {
        Affine {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            tx: self.a * other.tx + self.b * other.ty + self.tx,
            ty: self.c * other.tx + self.d * other.ty + self.ty,
        }
    }

    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.b * y + self.tx, self.c * x + self.d * y + self.ty)
    }
}

/// Triangles, outline edges and shape count of one layer, in cell coordinates (database units).
type LocalLayer = (Vec<f64>, Vec<f64>, u32);

/// A cell's own shapes, tessellated once.
#[derive(Default)]
struct Local {
    layers: BTreeMap<(i16, i16), LocalLayer>,
}

pub fn flatten(lib: &Library, top: &str, limits: Limits) -> Result<Flat> {
    let top_cell = lib.cell(top).ok_or_else(|| GdsError::Format(format!("no cell named {top}")))?;
    let mut ctx =
        Ctx { lib, locals: HashMap::new(), out: BTreeMap::new(), texts: Vec::new(), floats: 0, limits, instances: 0 };
    let to_um = Affine::scale(lib.db_in_meters * 1e6);
    ctx.emit(top_cell, &to_um, 0)?;

    let mut bbox = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut layers: Vec<FlatLayer> = ctx.out.into_values().collect();
    for l in &layers {
        for p in l.triangles.as_chunks::<2>().0.iter().chain(l.edges.as_chunks::<2>().0) {
            bbox[0] = bbox[0].min(p[0] as f64);
            bbox[1] = bbox[1].min(p[1] as f64);
            bbox[2] = bbox[2].max(p[0] as f64);
            bbox[3] = bbox[3].max(p[1] as f64);
        }
    }
    if !bbox[0].is_finite() {
        bbox = [0.0; 4];
    }
    layers.retain(|l| l.shapes > 0);
    Ok(Flat { top: top.to_string(), layers, texts: ctx.texts, bbox, instances: ctx.instances })
}

struct Ctx<'a> {
    lib: &'a Library,
    locals: HashMap<&'a str, Local>,
    out: BTreeMap<(i16, i16), FlatLayer>,
    texts: Vec<FlatText>,
    floats: usize,
    limits: Limits,
    instances: u64,
}

impl<'a> Ctx<'a> {
    fn emit(&mut self, cell: &'a Cell, t: &Affine, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(GdsError::Format(format!(
                "hierarchy deeper than {MAX_DEPTH} (recursive reference to {}?)",
                cell.name
            )));
        }
        self.instances += 1;
        if !self.locals.contains_key(cell.name.as_str()) {
            self.locals.insert(&cell.name, tessellate(cell));
        }
        let local = &self.locals[cell.name.as_str()];
        for (&(layer, datatype), (tris, edges, shapes)) in &local.layers {
            self.floats += tris.len() + edges.len();
            if self.floats > self.limits.max_floats {
                return Err(GdsError::TooLarge);
            }
            let dst = self.out.entry((layer, datatype)).or_insert_with(|| FlatLayer {
                layer,
                datatype,
                ..Default::default()
            });
            dst.shapes += shapes;
            push_transformed(&mut dst.triangles, tris, t);
            push_transformed(&mut dst.edges, edges, t);
        }
        for text in &cell.texts {
            let (x, y) = t.apply(text.position.x as f64, text.position.y as f64);
            self.texts.push(FlatText {
                layer: text.layer,
                texttype: text.texttype,
                x: x as f32,
                y: y as f32,
                text: text.text.clone(),
            });
        }
        for r in &cell.refs {
            let Some(child) = self.lib.cell(&r.cell) else { continue };
            for placement in placements(r) {
                self.emit(child, &t.then(&placement), depth + 1)?;
            }
        }
        Ok(())
    }
}

fn push_transformed(dst: &mut Vec<f32>, src: &[f64], t: &Affine) {
    dst.reserve(src.len());
    for &[px, py] in src.as_chunks::<2>().0 {
        let (x, y) = t.apply(px, py);
        dst.push(x as f32);
        dst.push(y as f32);
    }
}

fn placements(r: &Reference) -> Vec<Affine> {
    let o = (r.origin.x as f64, r.origin.y as f64);
    match r.array {
        None => vec![Affine::placement(&r.strans, o)],
        Some((cols, rows, pc, pr)) => {
            let (cols, rows) = (cols.max(1) as f64, rows.max(1) as f64);
            let col = ((pc.x as f64 - o.0) / cols, (pc.y as f64 - o.1) / cols);
            let row = ((pr.x as f64 - o.0) / rows, (pr.y as f64 - o.1) / rows);
            let mut out = Vec::with_capacity((cols * rows) as usize);
            for j in 0..rows as usize {
                for i in 0..cols as usize {
                    let (i, j) = (i as f64, j as f64);
                    let origin = (o.0 + i * col.0 + j * row.0, o.1 + i * col.1 + j * row.1);
                    out.push(Affine::placement(&r.strans, origin));
                }
            }
            out
        }
    }
}

fn tessellate(cell: &Cell) -> Local {
    let mut local = Local::default();
    for p in &cell.polygons {
        let (tris, edges, shapes) = local.layers.entry((p.layer, p.datatype)).or_default();
        polygon(&p.points, tris, edges);
        *shapes += 1;
    }
    for path in &cell.paths {
        let (tris, edges, shapes) = local.layers.entry((path.layer, path.datatype)).or_default();
        for quad in path_quads(path) {
            polygon(&quad, tris, edges);
        }
        *shapes += 1;
    }
    local
}

fn polygon(pts: &[Point], tris: &mut Vec<f64>, edges: &mut Vec<f64>) {
    let n = pts.len();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        edges.extend_from_slice(&[a.x as f64, a.y as f64, b.x as f64, b.y as f64]);
    }
    // Fast path: axis-aligned rectangles are most of a digital layout.
    if n == 4 && is_rect(pts) {
        let [a, b, c, d] = [pts[0], pts[1], pts[2], pts[3]];
        for p in [a, b, c, a, c, d] {
            tris.extend_from_slice(&[p.x as f64, p.y as f64]);
        }
        return;
    }
    let flat: Vec<f64> = pts.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
    if let Ok(idx) = earcutr::earcut(&flat, &[], 2) {
        for i in idx {
            tris.extend_from_slice(&[flat[2 * i], flat[2 * i + 1]]);
        }
    }
}

fn is_rect(p: &[Point]) -> bool {
    let h = |a: Point, b: Point| a.y == b.y && a.x != b.x;
    let v = |a: Point, b: Point| a.x == b.x && a.y != b.y;
    (h(p[0], p[1]) && v(p[1], p[2]) && h(p[2], p[3]) && v(p[3], p[0]))
        || (v(p[0], p[1]) && h(p[1], p[2]) && v(p[2], p[3]) && h(p[3], p[0]))
}

/// One rectangle per path segment. Interior joints get square (half-width)
/// extensions so corners are covered; the ends follow the path type.
fn path_quads(path: &Path) -> Vec<Vec<Point>> {
    let hw = path.width as f64 / 2.0;
    let (begin, end) = match path.pathtype {
        1 | 2 => (hw, hw),
        4 => (path.begin_ext as f64, path.end_ext as f64),
        _ => (0.0, 0.0),
    };
    let n = path.points.len();
    let mut quads = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let (p, q) = (path.points[i], path.points[i + 1]);
        let (dx, dy) = ((q.x - p.x) as f64, (q.y - p.y) as f64);
        let len = (dx * dx + dy * dy).sqrt();
        if len == 0.0 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        let s = if i == 0 { begin } else { hw };
        let e = if i == n - 2 { end } else { hw };
        let (sx, sy) = (p.x as f64 - ux * s, p.y as f64 - uy * s);
        let (ex, ey) = (q.x as f64 + ux * e, q.y as f64 + uy * e);
        let (nx, ny) = (-uy * hw, ux * hw);
        let pt = |x: f64, y: f64| Point { x: x.round() as i32, y: y.round() as i32 };
        quads.push(vec![pt(sx + nx, sy + ny), pt(ex + nx, ey + ny), pt(ex - nx, ey - ny), pt(sx - nx, sy - ny)]);
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_rotates_reflects_and_scales() {
        let s = Strans { reflect: false, mag: 1.0, angle: 90.0 };
        assert_eq!(Affine::placement(&s, (10.0, 0.0)).apply(1.0, 0.0), (10.0, 1.0));
        let s = Strans { reflect: true, mag: 2.0, angle: 0.0 };
        assert_eq!(Affine::placement(&s, (0.0, 0.0)).apply(1.0, 1.0), (2.0, -2.0));
        // Reflection happens before rotation.
        let s = Strans { reflect: true, mag: 1.0, angle: 90.0 };
        assert_eq!(Affine::placement(&s, (0.0, 0.0)).apply(0.0, 1.0), (1.0, 0.0));
    }

    #[test]
    fn composition_applies_inner_first() {
        let outer = Affine::placement(&Strans::default(), (100.0, 0.0));
        let inner = Affine::placement(&Strans { angle: 180.0, ..Default::default() }, (5.0, 0.0));
        assert_eq!(outer.then(&inner).apply(1.0, 0.0), (104.0, 0.0));
    }

    #[test]
    fn path_types() {
        let path = |pathtype| Path {
            layer: 1,
            datatype: 0,
            width: 10,
            pathtype,
            begin_ext: 3,
            end_ext: 7,
            points: vec![Point { x: 0, y: 0 }, Point { x: 100, y: 0 }],
        };
        let xs = |q: &Vec<Point>| (q.iter().map(|p| p.x).min().unwrap(), q.iter().map(|p| p.x).max().unwrap());
        assert_eq!(xs(&path_quads(&path(0))[0]), (0, 100));
        assert_eq!(xs(&path_quads(&path(2))[0]), (-5, 105));
        assert_eq!(xs(&path_quads(&path(4))[0]), (-3, 107));
        let ys: Vec<i32> = path_quads(&path(0))[0].iter().map(|p| p.y).collect();
        assert!(ys.contains(&5) && ys.contains(&-5));
    }
}
