// GDSII parsing in WebAssembly (crates/wafer-gds-wasm, built by scripts/build-wasm.sh).
import init, { GdsLayout } from "../wasm/wafer_gds.js";
import wasmUrl from "../wasm/wafer_gds_bg.wasm?url";

let ready: Promise<unknown> | null = null;

export interface LoadedLayout {
  top: string;
  bbox: [number, number, number, number];
  instances: number;
  layers: { key: number; triangles: Float32Array; edges: Float32Array; shapes: number }[];
  texts: { key: number; x: number; y: number; text: string }[];
  parseMs: number;
}

export async function parseGds(bytes: Uint8Array, top?: string): Promise<LoadedLayout> {
  ready ??= init({ module_or_path: wasmUrl });
  await ready;
  const t0 = performance.now();
  const layout = new GdsLayout(bytes, top);
  try {
    const keys = Array.from(layout.layerKeys());
    const tk = layout.textKeys();
    const tp = layout.textPositions();
    const ts = layout.textStrings();
    return {
      top: layout.top,
      bbox: Array.from(layout.bbox) as [number, number, number, number],
      instances: layout.instances,
      layers: keys.map((key) => ({
        key,
        triangles: layout.triangles(key),
        edges: layout.edges(key),
        shapes: layout.shapes(key),
      })),
      texts: Array.from(tk, (key, i) => ({ key, x: tp[2 * i], y: tp[2 * i + 1], text: ts[i] })),
      parseMs: performance.now() - t0,
    };
  } finally {
    layout.free();
  }
}
