//! GDSII for the SiliconCommons layout viewer.
//!
//! [`parse`] reads a GDSII stream into cells; [`flatten`] expands the
//! hierarchy under a top cell into per-layer triangle and outline buffers in
//! micrometres, ready to upload to the GPU. Pure Rust, so it compiles to
//! WebAssembly unchanged (see `sc-gds-wasm`).

mod flatten;
mod parse;

pub use flatten::{Flat, FlatLayer, FlatText, Limits, flatten};
pub use parse::{Cell, Library, Path, Point, Polygon, Reference, Strans, Text, parse};

#[derive(Debug, thiserror::Error)]
pub enum GdsError {
    #[error("invalid GDSII: {0}")]
    Format(String),
    #[error("layout too large to flatten in the browser")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, GdsError>;

/// Parses `bytes` and flattens `top` (or the largest top-level cell).
pub fn load(bytes: &[u8], top: Option<&str>, limits: Limits) -> Result<Flat> {
    let lib = parse(bytes)?;
    let top = match top {
        Some(t) => t.to_string(),
        None => lib
            .top_cells()
            .first()
            .map(|s| s.to_string())
            .ok_or_else(|| GdsError::Format("library has no cells".into()))?,
    };
    flatten(&lib, &top, limits)
}

#[cfg(test)]
mod tests;
