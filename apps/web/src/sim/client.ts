// Promise API over the simulator worker. A run can be cancelled by
// terminating the worker (a fresh one starts on the next call).
import type { SimRequest, SimResponse, SimResult } from "./protocol";

type Pending = { resolve: (r: SimResponse) => void; reject: (e: Error) => void };
/** `Omit` applied to each member of a union. */
type WithoutId<T> = T extends unknown ? Omit<T, "id"> : never;

export class SimClient {
  private worker: Worker | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;

  private ensure(): Worker {
    if (this.worker) return this.worker;
    const w = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (ev: MessageEvent<SimResponse>) => {
      const p = this.pending.get(ev.data.id);
      if (!p) return;
      this.pending.delete(ev.data.id);
      p.resolve(ev.data);
    };
    w.onerror = (ev) => this.fail(new Error(ev.message || "simulator crashed"));
    this.worker = w;
    return w;
  }

  private fail(e: Error) {
    for (const p of this.pending.values()) p.reject(e);
    this.pending.clear();
    this.worker?.terminate();
    this.worker = null;
  }

  private call(req: WithoutId<SimRequest>): Promise<SimResponse> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ensure().postMessage({ ...req, id });
    });
  }

  async run(files: { name: string; source: string }[], top: string | null, maxTime = 0): Promise<SimResult> {
    const r = await this.call({ type: "run", names: files.map((f) => f.name), sources: files.map((f) => f.source), top, maxTime });
    if (r.type === "error") throw new Error(r.error);
    if (r.type !== "result") throw new Error("unexpected reply");
    return r.result;
  }

  async tops(files: { name: string; source: string }[]): Promise<string[]> {
    const r = await this.call({ type: "tops", names: files.map((f) => f.name), sources: files.map((f) => f.source) });
    if (r.type === "error") throw new Error(r.error);
    return r.type === "tops" ? r.tops : [];
  }

  async vcd(): Promise<string> {
    const r = await this.call({ type: "vcd" });
    return r.type === "vcd" ? r.vcd : "";
  }

  cancel() {
    this.fail(new Error("simulation cancelled"));
  }

  dispose() {
    this.cancel();
  }
}
