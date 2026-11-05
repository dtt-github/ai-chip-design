//! `GdsLayout` for the browser: parse and flatten a GDSII file once, then hand
//! each layer's vertex buffers to the renderer.

use sc_gds::{Flat, Limits};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct GdsLayout {
    flat: Flat,
}

/// Packs (layer, datatype) into one number for JS: `layer * 65536 + datatype`.
fn key(layer: i16, datatype: i16) -> u32 {
    ((layer as u16 as u32) << 16) | datatype as u16 as u32
}

#[wasm_bindgen]
impl GdsLayout {
    /// Parses a GDSII stream and flattens `top` (default: the largest top-level cell).
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8], top: Option<String>) -> Result<GdsLayout, JsError> {
        let flat = sc_gds::load(bytes, top.as_deref(), Limits::default()).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(GdsLayout { flat })
    }

    #[wasm_bindgen(getter)]
    pub fn top(&self) -> String {
        self.flat.top.clone()
    }

    /// `[x0, y0, x1, y1]` in µm.
    #[wasm_bindgen(getter)]
    pub fn bbox(&self) -> Vec<f64> {
        self.flat.bbox.to_vec()
    }

    #[wasm_bindgen(getter)]
    pub fn instances(&self) -> f64 {
        self.flat.instances as f64
    }

    /// Layer keys present in the layout (`layer * 65536 + datatype`), ascending.
    #[wasm_bindgen(js_name = layerKeys)]
    pub fn layer_keys(&self) -> Vec<u32> {
        self.flat.layers.iter().map(|l| key(l.layer, l.datatype)).collect()
    }

    fn layer(&self, k: u32) -> Option<&sc_gds::FlatLayer> {
        self.flat.layers.iter().find(|l| key(l.layer, l.datatype) == k)
    }

    /// Triangle list (x, y pairs, µm) for a layer key.
    pub fn triangles(&self, k: u32) -> Vec<f32> {
        self.layer(k).map(|l| l.triangles.clone()).unwrap_or_default()
    }

    /// Outline line list (x, y pairs, µm) for a layer key.
    pub fn edges(&self, k: u32) -> Vec<f32> {
        self.layer(k).map(|l| l.edges.clone()).unwrap_or_default()
    }

    /// Number of shapes (polygons and paths) on a layer.
    pub fn shapes(&self, k: u32) -> u32 {
        self.layer(k).map(|l| l.shapes).unwrap_or(0)
    }

    /// Labels as parallel arrays: keys, positions (x, y pairs) and strings.
    #[wasm_bindgen(js_name = textKeys)]
    pub fn text_keys(&self) -> Vec<u32> {
        self.flat.texts.iter().map(|t| key(t.layer, t.texttype)).collect()
    }

    #[wasm_bindgen(js_name = textPositions)]
    pub fn text_positions(&self) -> Vec<f32> {
        self.flat.texts.iter().flat_map(|t| [t.x, t.y]).collect()
    }

    #[wasm_bindgen(js_name = textStrings)]
    pub fn text_strings(&self) -> Vec<String> {
        self.flat.texts.iter().map(|t| t.text.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_pack_layer_and_datatype() {
        assert_eq!(super::key(8, 0), 8 << 16);
        assert_eq!(super::key(189, 4), (189 << 16) | 4);
    }
}
