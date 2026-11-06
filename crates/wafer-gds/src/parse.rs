//! GDSII stream reader: records → library of cells.

use std::collections::HashMap;

use crate::{GdsError, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Polygon {
    pub layer: i16,
    pub datatype: i16,
    /// Vertices in database units, without the repeated closing point.
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub layer: i16,
    pub datatype: i16,
    /// Negative widths are absolute (not scaled by references); treated as positive here.
    pub width: i32,
    /// 0 flush, 1 round (drawn as flush + half-width), 2 half-width extension, 4 custom.
    pub pathtype: i16,
    pub begin_ext: i32,
    pub end_ext: i32,
    pub points: Vec<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub layer: i16,
    pub texttype: i16,
    pub position: Point,
    pub text: String,
}

/// GDS `STRANS`/`MAG`/`ANGLE`: reflect about x, then scale, then rotate (degrees, counter-clockwise).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strans {
    pub reflect: bool,
    pub mag: f64,
    pub angle: f64,
}

impl Default for Strans {
    fn default() -> Self {
        Self { reflect: false, mag: 1.0, angle: 0.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub cell: String,
    pub strans: Strans,
    pub origin: Point,
    /// For arrays: columns, rows, and the two displacement corner points (as stored in GDS).
    pub array: Option<(u16, u16, Point, Point)>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cell {
    pub name: String,
    pub polygons: Vec<Polygon>,
    pub paths: Vec<Path>,
    pub refs: Vec<Reference>,
    pub texts: Vec<Text>,
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub name: String,
    /// Size of a database unit in user units (usually 1e-3: nm in µm).
    pub db_in_user: f64,
    /// Size of a database unit in metres (usually 1e-9).
    pub db_in_meters: f64,
    pub cells: Vec<Cell>,
    pub index: HashMap<String, usize>,
}

impl Library {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.index.get(name).map(|&i| &self.cells[i])
    }

    /// Cells no other cell references, largest first by element count.
    pub fn top_cells(&self) -> Vec<&str> {
        let referenced: std::collections::HashSet<&str> =
            self.cells.iter().flat_map(|c| c.refs.iter().map(|r| r.cell.as_str())).collect();
        let mut tops: Vec<&Cell> = self.cells.iter().filter(|c| !referenced.contains(c.name.as_str())).collect();
        tops.sort_by_key(|c| std::cmp::Reverse(c.polygons.len() + c.paths.len() + c.refs.len()));
        tops.into_iter().map(|c| c.name.as_str()).collect()
    }
}

// Record types we act on.
const HEADER: u8 = 0x00;
const BGNLIB: u8 = 0x01;
const LIBNAME: u8 = 0x02;
const UNITS: u8 = 0x03;
const ENDLIB: u8 = 0x04;
const BGNSTR: u8 = 0x05;
const STRNAME: u8 = 0x06;
const ENDSTR: u8 = 0x07;
const BOUNDARY: u8 = 0x08;
const PATH: u8 = 0x09;
const SREF: u8 = 0x0A;
const AREF: u8 = 0x0B;
const TEXT: u8 = 0x0C;
const LAYER: u8 = 0x0D;
const DATATYPE: u8 = 0x0E;
const WIDTH: u8 = 0x0F;
const XY: u8 = 0x10;
const ENDEL: u8 = 0x11;
const SNAME: u8 = 0x12;
const COLROW: u8 = 0x13;
const NODE: u8 = 0x15;
const TEXTTYPE: u8 = 0x16;
const STRING: u8 = 0x19;
const STRANS: u8 = 0x1A;
const MAG: u8 = 0x1B;
const ANGLE: u8 = 0x1C;
const PATHTYPE: u8 = 0x21;
const BOX: u8 = 0x2D;
const BOXTYPE: u8 = 0x2E;
const BGNEXTN: u8 = 0x30;
const ENDEXTN: u8 = 0x31;

struct Record<'a> {
    kind: u8,
    data: &'a [u8],
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn next(&mut self) -> Result<Option<Record<'a>>> {
        if self.pos + 4 > self.buf.len() {
            return Ok(None);
        }
        let len = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]) as usize;
        if len == 0 {
            // Some writers pad the end of the stream with zeros.
            return Ok(None);
        }
        if len < 4 || self.pos + len > self.buf.len() {
            return Err(GdsError::Format(format!("bad record length {len} at byte {}", self.pos)));
        }
        let rec = Record { kind: self.buf[self.pos + 2], data: &self.buf[self.pos + 4..self.pos + len] };
        self.pos += len;
        Ok(Some(rec))
    }
}

fn i16s(d: &[u8]) -> impl Iterator<Item = i16> + '_ {
    d.as_chunks::<2>().0.iter().map(|c| i16::from_be_bytes(*c))
}

fn i32s(d: &[u8]) -> impl Iterator<Item = i32> + '_ {
    d.as_chunks::<4>().0.iter().map(|c| i32::from_be_bytes(*c))
}

fn first_i16(d: &[u8]) -> Result<i16> {
    i16s(d).next().ok_or_else(|| GdsError::Format("short integer record".into()))
}

fn first_i32(d: &[u8]) -> Result<i32> {
    i32s(d).next().ok_or_else(|| GdsError::Format("short integer record".into()))
}

/// GDSII 8-byte real: sign bit, 7-bit excess-64 base-16 exponent, 56-bit mantissa.
pub(crate) fn real8(b: &[u8]) -> f64 {
    let sign = if b[0] & 0x80 != 0 { -1.0 } else { 1.0 };
    let exp = (b[0] & 0x7f) as i32 - 64;
    let mut mantissa = 0u64;
    for &byte in &b[1..8] {
        mantissa = (mantissa << 8) | byte as u64;
    }
    sign * (mantissa as f64 / 2f64.powi(56)) * 16f64.powi(exp)
}

fn string(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).into_owned()
}

fn points(d: &[u8]) -> Vec<Point> {
    let v: Vec<i32> = i32s(d).collect();
    v.as_chunks::<2>().0.iter().map(|&[x, y]| Point { x, y }).collect()
}

#[derive(Default)]
struct Element {
    kind: u8,
    layer: i16,
    datatype: i16,
    width: i32,
    pathtype: i16,
    begin_ext: i32,
    end_ext: i32,
    xy: Vec<Point>,
    sname: String,
    strans: Strans,
    colrow: (u16, u16),
    text: String,
}

pub fn parse(bytes: &[u8]) -> Result<Library> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let mut lib = Library::default();
    let mut cell: Option<Cell> = None;
    let mut el: Option<Element> = None;
    let mut saw_header = false;

    while let Some(rec) = r.next()? {
        match rec.kind {
            HEADER => saw_header = true,
            BGNLIB => {}
            LIBNAME => lib.name = string(rec.data),
            UNITS => {
                if rec.data.len() < 16 {
                    return Err(GdsError::Format("short UNITS record".into()));
                }
                lib.db_in_user = real8(&rec.data[0..8]);
                lib.db_in_meters = real8(&rec.data[8..16]);
            }
            ENDLIB => break,
            BGNSTR => cell = Some(Cell::default()),
            STRNAME => {
                if let Some(c) = cell.as_mut() {
                    c.name = string(rec.data);
                }
            }
            ENDSTR => {
                let c = cell.take().ok_or_else(|| GdsError::Format("ENDSTR outside a structure".into()))?;
                lib.index.insert(c.name.clone(), lib.cells.len());
                lib.cells.push(c);
            }
            BOUNDARY | PATH | SREF | AREF | TEXT | BOX | NODE => {
                el = Some(Element { kind: rec.kind, strans: Strans::default(), ..Default::default() });
            }
            LAYER => set(&mut el, |e| e.layer = first_i16(rec.data).unwrap_or(0)),
            DATATYPE | TEXTTYPE | BOXTYPE => set(&mut el, |e| e.datatype = first_i16(rec.data).unwrap_or(0)),
            WIDTH => set(&mut el, |e| e.width = first_i32(rec.data).unwrap_or(0).abs()),
            PATHTYPE => set(&mut el, |e| e.pathtype = first_i16(rec.data).unwrap_or(0)),
            BGNEXTN => set(&mut el, |e| e.begin_ext = first_i32(rec.data).unwrap_or(0)),
            ENDEXTN => set(&mut el, |e| e.end_ext = first_i32(rec.data).unwrap_or(0)),
            XY => set(&mut el, |e| e.xy = points(rec.data)),
            SNAME => set(&mut el, |e| e.sname = string(rec.data)),
            STRING => set(&mut el, |e| e.text = string(rec.data)),
            COLROW => set(&mut el, |e| {
                let v: Vec<i16> = i16s(rec.data).collect();
                if v.len() >= 2 {
                    e.colrow = (v[0].max(0) as u16, v[1].max(0) as u16);
                }
            }),
            STRANS => set(&mut el, |e| e.strans.reflect = rec.data.first().is_some_and(|b| b & 0x80 != 0)),
            MAG if rec.data.len() >= 8 => set(&mut el, |e| e.strans.mag = real8(rec.data)),
            ANGLE if rec.data.len() >= 8 => set(&mut el, |e| e.strans.angle = real8(rec.data)),
            ENDEL => {
                let (Some(e), Some(c)) = (el.take(), cell.as_mut()) else { continue };
                finish(e, c)?;
            }
            _ => {} // properties, presentation, plex, elflags, ...
        }
    }
    if !saw_header {
        return Err(GdsError::Format("not a GDSII stream (no HEADER record)".into()));
    }
    if lib.db_in_meters <= 0.0 {
        lib.db_in_meters = 1e-9;
        lib.db_in_user = 1e-3;
    }
    Ok(lib)
}

fn set(el: &mut Option<Element>, f: impl FnOnce(&mut Element)) {
    if let Some(e) = el.as_mut() {
        f(e);
    }
}

fn finish(e: Element, c: &mut Cell) -> Result<()> {
    match e.kind {
        BOUNDARY | BOX => {
            let mut pts = e.xy;
            if pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            if pts.len() >= 3 {
                c.polygons.push(Polygon { layer: e.layer, datatype: e.datatype, points: pts });
            }
        }
        PATH => {
            if e.xy.len() >= 2 && e.width > 0 {
                c.paths.push(Path {
                    layer: e.layer,
                    datatype: e.datatype,
                    width: e.width,
                    pathtype: e.pathtype,
                    begin_ext: e.begin_ext,
                    end_ext: e.end_ext,
                    points: e.xy,
                });
            }
        }
        SREF => {
            let origin = *e.xy.first().ok_or_else(|| GdsError::Format("SREF without XY".into()))?;
            c.refs.push(Reference { cell: e.sname, strans: e.strans, origin, array: None });
        }
        AREF => {
            if e.xy.len() < 3 {
                return Err(GdsError::Format("AREF needs three XY points".into()));
            }
            c.refs.push(Reference {
                cell: e.sname,
                strans: e.strans,
                origin: e.xy[0],
                array: Some((e.colrow.0, e.colrow.1, e.xy[1], e.xy[2])),
            });
        }
        TEXT => {
            if let Some(&position) = e.xy.first() {
                c.texts.push(Text { layer: e.layer, texttype: e.datatype, position, text: e.text });
            }
        }
        _ => {}
    }
    Ok(())
}
