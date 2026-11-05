// Layout renderers: WebGPU when available, Canvas 2D otherwise. Both draw
// per-layer triangle fills and outline edges in µm under a pan/zoom view.

export interface RenderLayer {
  key: number;
  color: [number, number, number];
  visible: boolean;
  /// Draw the fill (false: outline only).
  fill: boolean;
  order: number;
  triangles: Float32Array;
  edges: Float32Array;
}

/** View: the µm point at the canvas centre and the zoom in CSS pixels per µm. */
export interface View {
  cx: number;
  cy: number;
  scale: number;
}

export interface Markers {
  triangles: Float32Array;
  edges: Float32Array;
  /// Index ranges (in vertices) of the selected marker, drawn highlighted.
  selected: { tri: [number, number]; edge: [number, number] } | null;
}

export interface Renderer {
  readonly kind: "WebGPU" | "Canvas 2D";
  setLayers(layers: RenderLayer[]): void;
  setMarkers(markers: Markers | null): void;
  render(view: View): void;
  destroy(): void;
}

const FILL_ALPHA = 0.38;
const EDGE_ALPHA = 0.95;
const BACKGROUND = { r: 0.07, g: 0.08, b: 0.1, a: 1 };
const MARKER: [number, number, number] = [1, 0.17, 0.84];
const MARKER_SELECTED: [number, number, number] = [1, 0.92, 0.2];

export async function createRenderer(canvas: HTMLCanvasElement): Promise<Renderer> {
  const gpu = await GpuRenderer.create(canvas).catch(() => null);
  return gpu ?? new CanvasRenderer(canvas);
}

/** Clip-space transform (x' = x·sx + tx) for a view on a canvas of `w`×`h` CSS pixels. */
function clipTransform(view: View, w: number, h: number): [number, number, number, number] {
  const sx = (2 * view.scale) / w;
  const sy = (2 * view.scale) / h;
  return [sx, sy, -view.cx * sx, -view.cy * sy];
}

const SHADER = /* wgsl */ `
struct U { xform: vec4f, color: vec4f };
@group(0) @binding(0) var<uniform> u: U;

@vertex fn vs(@location(0) p: vec2f) -> @builtin(position) vec4f {
  return vec4f(p * u.xform.xy + u.xform.zw, 0.0, 1.0);
}

@fragment fn fs() -> @location(0) vec4f {
  return vec4f(u.color.rgb * u.color.a, u.color.a);
}
`;

interface GpuBatch {
  vertices: GPUBuffer | null;
  count: number;
  uniform: GPUBuffer;
  bind: GPUBindGroup;
  color: [number, number, number, number];
}

class GpuRenderer implements Renderer {
  readonly kind = "WebGPU" as const;
  private layers: { info: RenderLayer; fill: GpuBatch; edge: GpuBatch }[] = [];
  private markers: GpuBatch[] = [];

  private constructor(
    private canvas: HTMLCanvasElement,
    private device: GPUDevice,
    private context: GPUCanvasContext,
    private fillPipeline: GPURenderPipeline,
    private linePipeline: GPURenderPipeline,
  ) {}

  static async create(canvas: HTMLCanvasElement): Promise<GpuRenderer | null> {
    if (!("gpu" in navigator) || !navigator.gpu) return null;
    const adapter = await navigator.gpu.requestAdapter();
    if (!adapter) return null;
    const device = await adapter.requestDevice();
    const context = canvas.getContext("webgpu");
    if (!context) return null;
    const format = navigator.gpu.getPreferredCanvasFormat();
    context.configure({ device, format, alphaMode: "premultiplied" });
    const module = device.createShaderModule({ code: SHADER });
    const layout = device.createBindGroupLayout({
      entries: [{ binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: "uniform" } }],
    });
    const pipeline = (topology: GPUPrimitiveTopology) =>
      device.createRenderPipeline({
        layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }),
        vertex: {
          module,
          entryPoint: "vs",
          buffers: [{ arrayStride: 8, attributes: [{ shaderLocation: 0, offset: 0, format: "float32x2" }] }],
        },
        fragment: {
          module,
          entryPoint: "fs",
          targets: [
            {
              format,
              blend: {
                color: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
                alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha" },
              },
            },
          ],
        },
        primitive: { topology },
      });
    return new GpuRenderer(canvas, device, context, pipeline("triangle-list"), pipeline("line-list"));
  }

  private batch(data: Float32Array, color: [number, number, number, number]): GpuBatch {
    const d = this.device;
    let vertices: GPUBuffer | null = null;
    if (data.length) {
      vertices = d.createBuffer({ size: data.byteLength, usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST });
      d.queue.writeBuffer(vertices, 0, data.buffer, data.byteOffset, data.byteLength);
    }
    const uniform = d.createBuffer({ size: 32, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
    const bind = d.createBindGroup({
      layout: this.fillPipeline.getBindGroupLayout(0),
      entries: [{ binding: 0, resource: { buffer: uniform } }],
    });
    return { vertices, count: data.length / 2, uniform, bind, color };
  }

  private free(b: GpuBatch) {
    b.vertices?.destroy();
    b.uniform.destroy();
  }

  setLayers(layers: RenderLayer[]) {
    // Re-upload only geometry that changed; visibility and colour are per-frame uniforms.
    const old = new Map(this.layers.map((l) => [l.info.key, l]));
    this.layers = [...layers]
      .sort((a, b) => a.order - b.order)
      .map((info) => {
        const prev = old.get(info.key);
        if (prev && prev.info.triangles === info.triangles) {
          old.delete(info.key);
          prev.fill.color = [...info.color, FILL_ALPHA];
          prev.edge.color = [...info.color, EDGE_ALPHA];
          return { ...prev, info };
        }
        return {
          info,
          fill: this.batch(info.triangles, [...info.color, FILL_ALPHA]),
          edge: this.batch(info.edges, [...info.color, EDGE_ALPHA]),
        };
      });
    for (const l of old.values()) {
      this.free(l.fill);
      this.free(l.edge);
    }
  }

  setMarkers(m: Markers | null) {
    this.markers.forEach((b) => this.free(b));
    this.markers = [];
    if (!m) return;
    this.markers.push(this.batch(m.triangles, [...MARKER, 0.35]), this.batch(m.edges, [...MARKER, 1]));
    if (m.selected) {
      const [t0, t1] = m.selected.tri;
      const [e0, e1] = m.selected.edge;
      this.markers.push(
        this.batch(m.triangles.slice(t0 * 2, t1 * 2), [...MARKER_SELECTED, 0.5]),
        this.batch(m.edges.slice(e0 * 2, e1 * 2), [...MARKER_SELECTED, 1]),
      );
    }
  }

  render(view: View) {
    const dpr = window.devicePixelRatio || 1;
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    if (!w || !h) return;
    if (this.canvas.width !== Math.round(w * dpr) || this.canvas.height !== Math.round(h * dpr)) {
      this.canvas.width = Math.round(w * dpr);
      this.canvas.height = Math.round(h * dpr);
    }
    const xform = clipTransform(view, w, h);
    const write = (b: GpuBatch) =>
      this.device.queue.writeBuffer(b.uniform, 0, new Float32Array([...xform, ...b.color]));
    const encoder = this.device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        { view: this.context.getCurrentTexture().createView(), clearValue: BACKGROUND, loadOp: "clear", storeOp: "store" },
      ],
    });
    const draw = (b: GpuBatch, pipeline: GPURenderPipeline) => {
      if (!b.vertices || !b.count) return;
      write(b);
      pass.setPipeline(pipeline);
      pass.setBindGroup(0, b.bind);
      pass.setVertexBuffer(0, b.vertices);
      pass.draw(b.count);
    };
    for (const l of this.layers) {
      if (!l.info.visible) continue;
      if (l.info.fill) draw(l.fill, this.fillPipeline);
      draw(l.edge, this.linePipeline);
    }
    this.markers.forEach((b, i) => draw(b, i % 2 === 0 ? this.fillPipeline : this.linePipeline));
    pass.end();
    this.device.queue.submit([encoder.finish()]);
  }

  destroy() {
    for (const l of this.layers) {
      this.free(l.fill);
      this.free(l.edge);
    }
    this.markers.forEach((b) => this.free(b));
    this.layers = [];
    this.markers = [];
    this.context.unconfigure();
    this.device.destroy();
  }
}

/** Fallback for browsers without WebGPU. Fine for small layouts. */
class CanvasRenderer implements Renderer {
  readonly kind = "Canvas 2D" as const;
  private layers: RenderLayer[] = [];
  private markers: Markers | null = null;
  private ctx: CanvasRenderingContext2D;

  constructor(private canvas: HTMLCanvasElement) {
    this.ctx = canvas.getContext("2d")!;
  }

  setLayers(layers: RenderLayer[]) {
    this.layers = [...layers].sort((a, b) => a.order - b.order);
  }

  setMarkers(m: Markers | null) {
    this.markers = m;
  }

  render(view: View) {
    const dpr = window.devicePixelRatio || 1;
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    if (!w || !h) return;
    this.canvas.width = Math.round(w * dpr);
    this.canvas.height = Math.round(h * dpr);
    const ctx = this.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.fillStyle = "rgb(18 20 26)";
    ctx.fillRect(0, 0, w, h);
    const sx = (x: number) => (x - view.cx) * view.scale + w / 2;
    const sy = (y: number) => h / 2 - (y - view.cy) * view.scale;
    const rgba = ([r, g, b]: [number, number, number], a: number) =>
      `rgba(${Math.round(r * 255)}, ${Math.round(g * 255)}, ${Math.round(b * 255)}, ${a})`;
    const fill = (t: Float32Array, color: string, from = 0, to = t.length / 2) => {
      ctx.beginPath();
      for (let i = from * 2; i + 6 <= to * 2; i += 6) {
        ctx.moveTo(sx(t[i]), sy(t[i + 1]));
        ctx.lineTo(sx(t[i + 2]), sy(t[i + 3]));
        ctx.lineTo(sx(t[i + 4]), sy(t[i + 5]));
        ctx.closePath();
      }
      ctx.fillStyle = color;
      ctx.fill();
    };
    const stroke = (e: Float32Array, color: string, from = 0, to = e.length / 2) => {
      ctx.beginPath();
      for (let i = from * 2; i + 4 <= to * 2; i += 4) {
        ctx.moveTo(sx(e[i]), sy(e[i + 1]));
        ctx.lineTo(sx(e[i + 2]), sy(e[i + 3]));
      }
      ctx.strokeStyle = color;
      ctx.lineWidth = 1;
      ctx.stroke();
    };
    for (const l of this.layers) {
      if (!l.visible) continue;
      if (l.fill) fill(l.triangles, rgba(l.color, FILL_ALPHA));
      stroke(l.edges, rgba(l.color, EDGE_ALPHA));
    }
    const m = this.markers;
    if (m) {
      fill(m.triangles, rgba(MARKER, 0.35));
      stroke(m.edges, rgba(MARKER, 1));
      if (m.selected) {
        fill(m.triangles, rgba(MARKER_SELECTED, 0.5), ...m.selected.tri);
        stroke(m.edges, rgba(MARKER_SELECTED, 1), ...m.selected.edge);
      }
    }
  }

  destroy() {}
}
