//! Round-trip tests with a minimal GDSII writer.

use crate::parse::real8;
use crate::*;

/// Encodes a GDSII 8-byte real.
fn to_real8(v: f64) -> [u8; 8] {
    if v == 0.0 {
        return [0; 8];
    }
    let sign = if v < 0.0 { 0x80u8 } else { 0 };
    let mut m = v.abs();
    let mut exp = 64i32;
    while m >= 1.0 {
        m /= 16.0;
        exp += 1;
    }
    while m < 1.0 / 16.0 {
        m *= 16.0;
        exp -= 1;
    }
    let mantissa = (m * 2f64.powi(56)).round() as u64;
    let mut out = [0u8; 8];
    out[0] = sign | exp as u8;
    out[1..].copy_from_slice(&mantissa.to_be_bytes()[1..]);
    out
}

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn rec(&mut self, kind: u8, dtype: u8, data: &[u8]) -> &mut Self {
        let mut data = data.to_vec();
        if data.len() % 2 == 1 {
            data.push(0);
        }
        self.0.extend_from_slice(&((data.len() + 4) as u16).to_be_bytes());
        self.0.extend_from_slice(&[kind, dtype]);
        self.0.extend_from_slice(&data);
        self
    }
    fn i16s(&mut self, kind: u8, v: &[i16]) -> &mut Self {
        self.rec(kind, 2, &v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<_>>())
    }
    fn i32s(&mut self, kind: u8, v: &[i32]) -> &mut Self {
        self.rec(kind, 3, &v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<_>>())
    }
    fn s(&mut self, kind: u8, v: &str) -> &mut Self {
        self.rec(kind, 6, v.as_bytes())
    }
    fn begin(units_db_in_user: f64, db_in_m: f64) -> Self {
        let mut w = Writer::default();
        w.i16s(0x00, &[600]).i16s(0x01, &[0; 12]).s(0x02, "LIB");
        let mut u = to_real8(units_db_in_user).to_vec();
        u.extend_from_slice(&to_real8(db_in_m));
        w.rec(0x03, 5, &u);
        w
    }
    fn cell(&mut self, name: &str) -> &mut Self {
        self.i16s(0x05, &[0; 12]).s(0x06, name)
    }
    fn end_cell(&mut self) -> &mut Self {
        self.rec(0x07, 0, &[])
    }
    fn boundary(&mut self, layer: i16, dt: i16, pts: &[(i32, i32)]) -> &mut Self {
        let mut xy: Vec<i32> = pts.iter().flat_map(|&(x, y)| [x, y]).collect();
        xy.extend_from_slice(&[pts[0].0, pts[0].1]);
        self.rec(0x08, 0, &[]).i16s(0x0D, &[layer]).i16s(0x0E, &[dt]).i32s(0x10, &xy).rec(0x11, 0, &[])
    }
    fn path(&mut self, layer: i16, width: i32, pathtype: i16, pts: &[(i32, i32)]) -> &mut Self {
        let xy: Vec<i32> = pts.iter().flat_map(|&(x, y)| [x, y]).collect();
        self.rec(0x09, 0, &[])
            .i16s(0x0D, &[layer])
            .i16s(0x0E, &[0])
            .i16s(0x21, &[pathtype])
            .i32s(0x0F, &[width])
            .i32s(0x10, &xy)
            .rec(0x11, 0, &[])
    }
    fn sref(&mut self, cell: &str, angle: f64, reflect: bool, at: (i32, i32)) -> &mut Self {
        self.rec(0x0A, 0, &[]).s(0x12, cell);
        self.rec(0x1A, 1, &[if reflect { 0x80 } else { 0 }, 0]);
        if angle != 0.0 {
            self.rec(0x1C, 5, &to_real8(angle));
        }
        self.i32s(0x10, &[at.0, at.1]).rec(0x11, 0, &[])
    }
    fn aref(&mut self, cell: &str, cols: i16, rows: i16, origin: (i32, i32), pitch: (i32, i32)) -> &mut Self {
        let c = (origin.0 + cols as i32 * pitch.0, origin.1);
        let r = (origin.0, origin.1 + rows as i32 * pitch.1);
        self.rec(0x0B, 0, &[])
            .s(0x12, cell)
            .i16s(0x13, &[cols, rows])
            .i32s(0x10, &[origin.0, origin.1, c.0, c.1, r.0, r.1])
            .rec(0x11, 0, &[])
    }
    fn text(&mut self, layer: i16, at: (i32, i32), s: &str) -> &mut Self {
        self.rec(0x0C, 0, &[]).i16s(0x0D, &[layer]).i16s(0x16, &[25]).i32s(0x10, &[at.0, at.1]).s(0x19, s).rec(
            0x11,
            0,
            &[],
        )
    }
    fn finish(&mut self) -> Vec<u8> {
        self.rec(0x04, 0, &[]);
        std::mem::take(&mut self.0)
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn real8_round_trip() {
    for v in [1e-3, 1e-9, 90.0, -2.5, 0.0, 1.0, 270.0] {
        let back = real8(&to_real8(v));
        assert!((back - v).abs() <= v.abs() * 1e-12, "{v} -> {back}");
    }
    // Known encoding of 1e-3 from the GDSII spec examples.
    assert_eq!(to_real8(1e-3)[0], 0x3e);
}

fn sample() -> Vec<u8> {
    let mut w = Writer::begin(1e-3, 1e-9);
    // Leaf: 100 x 50 nm rectangle on Metal1 and a label.
    w.cell("LEAF").boundary(8, 0, &[(0, 0), (100, 0), (100, 50), (0, 50)]).text(8, (10, 10), "A").end_cell();
    // Top: one plain instance, one rotated by 90° at (1000, 0), a 3x2 array, an L-shaped polygon and a path.
    w.cell("TOP")
        .sref("LEAF", 0.0, false, (0, 0))
        .sref("LEAF", 90.0, false, (1000, 0))
        .aref("LEAF", 3, 2, (0, 2000), (200, 100))
        .boundary(10, 0, &[(0, 0), (300, 0), (300, 100), (100, 100), (100, 300), (0, 300)])
        .path(30, 20, 0, &[(0, -500), (500, -500), (500, -200)])
        .end_cell();
    w.finish()
}

#[test]
fn parses_library() {
    let lib = parse(&sample()).unwrap();
    assert_eq!(lib.name, "LIB");
    assert!(close(lib.db_in_meters, 1e-9) && close(lib.db_in_user, 1e-3));
    assert_eq!(lib.cells.len(), 2);
    assert_eq!(lib.top_cells(), vec!["TOP"]);
    let top = lib.cell("TOP").unwrap();
    assert_eq!(top.refs.len(), 3);
    assert_eq!(top.refs[1].strans.angle, 90.0);
    assert_eq!(top.refs[2].array.map(|a| (a.0, a.1)), Some((3, 2)));
    assert_eq!(top.paths[0].width, 20);
}

#[test]
fn flattens_hierarchy_into_micrometres() {
    let flat = load(&sample(), None, Limits::default()).unwrap();
    assert_eq!(flat.top, "TOP");
    assert_eq!(flat.instances, 1 + 1 + 1 + 6);
    let m1 = flat.layers.iter().find(|l| l.layer == 8).unwrap();
    // 8 leaf instances, 2 triangles (12 floats) and 4 edges (16 floats) each.
    assert_eq!(m1.shapes, 8);
    assert_eq!(m1.triangles.len(), 8 * 12);
    assert_eq!(m1.edges.len(), 8 * 16);
    // The rotated instance spans x 0.95..1.0 µm, y 0..0.1 µm.
    let has = |x: f32, y: f32| {
        m1.triangles.as_chunks::<2>().0.iter().any(|p| (p[0] - x).abs() < 1e-6 && (p[1] - y).abs() < 1e-6)
    };
    assert!(has(0.95, 0.1) && has(1.0, 0.0));
    // Last array element: column 2, row 1 → origin (0.4, 2.1) µm.
    assert!(has(0.5, 2.15));

    // L-shape: 6 vertices → 4 triangles.
    let m2 = flat.layers.iter().find(|l| l.layer == 10).unwrap();
    assert_eq!(m2.triangles.len(), 4 * 6);

    // Path with two segments → two quads.
    let m3 = flat.layers.iter().find(|l| l.layer == 30).unwrap();
    assert_eq!(m3.triangles.len(), 2 * 12);

    assert_eq!(flat.texts.len(), 8);
    assert!(close(flat.bbox[0], 0.0) && close(flat.bbox[1], -0.51) && close(flat.bbox[3], 2.15));
}

#[test]
fn rejects_bad_input() {
    assert!(matches!(parse(b"not a gds file at all"), Err(GdsError::Format(_))));
    let mut truncated = sample();
    truncated.truncate(truncated.len() / 2 + 1);
    // Either a format error or a partial library, never a panic.
    let _ = parse(&truncated);
    let lib = parse(&sample()).unwrap();
    assert!(flatten(&lib, "NOPE", Limits::default()).is_err());
    assert!(matches!(load(&sample(), None, Limits { max_floats: 100 }), Err(GdsError::TooLarge)));
}

#[test]
fn recursive_references_are_reported() {
    let mut w = Writer::begin(1e-3, 1e-9);
    w.cell("A").boundary(1, 0, &[(0, 0), (1, 0), (1, 1), (0, 1)]).sref("A", 0.0, false, (1, 0)).end_cell();
    let lib = parse(&w.finish()).unwrap();
    assert!(matches!(flatten(&lib, "A", Limits::default()), Err(GdsError::Format(_))));
}

/// Optional: SC_GDS_FIXTURE=/path/to/real.gds cargo test -p sc-gds -- --nocapture
#[test]
fn real_file_if_given() {
    let Ok(path) = std::env::var("SC_GDS_FIXTURE") else { return };
    let bytes = std::fs::read(&path).unwrap();
    let t = std::time::Instant::now();
    let flat = load(&bytes, None, Limits::default()).unwrap();
    let tris: usize = flat.layers.iter().map(|l| l.triangles.len() / 6).sum();
    eprintln!(
        "{path}: top {} with {} layers, {} instances, {tris} triangles, bbox {:?} in {:?}",
        flat.top,
        flat.layers.len(),
        flat.instances,
        flat.bbox,
        t.elapsed()
    );
    assert!(tris > 0);
}
