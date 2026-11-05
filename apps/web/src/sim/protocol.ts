export interface SimSignal {
  name: string;
  width: number;
  /** Change times (ascending), simulation time units. */
  times: Float64Array;
  /** Value from each change time on. */
  values: BigUint64Array;
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
