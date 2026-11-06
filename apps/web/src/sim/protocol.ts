export interface SimSignal {
  name: string;
  width: number;
  /** Change times (ascending), simulation time units. */
  times: Float64Array;
  /** Value from each change time on. */
  values: BigUint64Array;
  /** What wrote each value: an index into `origins`, or NO_ORIGIN (nothing in the design). */
  by: Uint32Array;
}

/** `SimSignal.by` for a value nothing in the design wrote. */
export const NO_ORIGIN = 0xffffffff;

/** Where written values come from: an assignment (or port connection) in the design. */
export interface SimOrigin {
  file: string;
  line: number;
  /** The instance it is in, e.g. `tb.dut`. */
  scope: string;
  /** As written: `count <= count + 1`, `assign busy = state != 0`, `output led of tb.dut`. */
  text: string;
  /** Full names of the signals its value is computed from. */
  reads: string[];
  /** The `if`/`case` decisions it is inside, outermost first, and which way each went. */
  conds: { line: number; text: string; reads: string[]; taken: string }[];
}

export interface SimResult {
  log: string[];
  time: number;
  finished: boolean;
  stopped: string | null;
  warnings: string[];
  timescaleFs: number;
  steps: number;
  ms: number;
  signals: SimSignal[];
  origins: SimOrigin[];
}

export type SimRequest =
  | { type: "run"; id: number; names: string[]; sources: string[]; top: string | null; maxTime: number }
  | { type: "tops"; id: number; names: string[]; sources: string[] }
  | { type: "vcd"; id: number };

export type SimResponse =
  | { type: "result"; id: number; result: SimResult }
  | { type: "tops"; id: number; tops: string[] }
  | { type: "vcd"; id: number; vcd: string }
  | { type: "error"; id: number; error: string };
