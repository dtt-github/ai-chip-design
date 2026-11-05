import { useEffect, useRef, useState } from "react";
import { SimClient } from "../sim/client";
import type { SimResult } from "../sim/protocol";
import { Waveform } from "./Waveform";

export interface SimFile {
  name: string;
  source: string;
}

interface Props {
  /** Current text of every HDL file in the project (editor buffers win over the last commit). */
  gatherFiles: () => Promise<SimFile[]>;
  onOpenFile: (path: string, line: number) => void;
  onClose: () => void;
  maximized: boolean;
  onMaximize: (max: boolean) => void;
}

const ERROR_LOCATION = /^([^\s:]+\.s?vh?):(\d+):/;

/** Browser RTL simulation of the working copy, uncommitted edits included. */
export function SimPanel({ gatherFiles, onOpenFile, onClose, maximized, onMaximize }: Props) {
  const client = useRef<SimClient | null>(null);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<SimResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tops, setTops] = useState<string[]>([]);
  const [top, setTop] = useState<string>("");
  const [tab, setTab] = useState<"output" | "waves">("output");
  const [fileCount, setFileCount] = useState(0);

  useEffect(() => {
    client.current = new SimClient();
    void run();
    return () => client.current?.dispose();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function run() {
    const c = client.current;
    if (!c) return;
    setRunning(true);
    setError(null);
    try {
      const files = await gatherFiles();
      setFileCount(files.length);
      if (!files.length) throw new Error("No Verilog files (.v, .sv) in this project.");
      const candidates = await c.tops(files);
      setTops(candidates);
      const chosen = top && candidates.includes(top) ? top : (candidates[0] ?? null);
      if (chosen) setTop(chosen);
      const r = await c.run(files, chosen);
      setResult(r);
      if (r.signals.length && tab === "output" && !r.log.length) setTab("waves");
    } catch (e) {
      setResult(null);
      setError((e as Error).message);
    } finally {
      setRunning(false);
    }
  }

  async function downloadVcd() {
    const vcd = await client.current?.vcd();
    if (!vcd) return;
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([vcd], { type: "text/plain" }));
    a.download = `${top || "simulation"}.vcd`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }

  const errLoc = error?.match(ERROR_LOCATION);
  const status = running ? (
    <span className="status status-running">running</span>
  ) : error ? (
    <span className="status status-failed">error</span>
  ) : result ? (
    <span className={`status ${result.stopped ? "status-failed" : "status-succeeded"}`}>
      {result.finished ? "finished" : result.stopped ? "stopped" : "idle"}
    </span>
  ) : null;

  return (
    <section className="run-view" aria-label="Simulation">
      <header className="run-view-head">
        <strong>Simulation</strong>
        {status}
        {result && !running && (
          <span className="muted small">
            t={result.time} · {result.steps.toLocaleString()} steps · {result.ms.toFixed(0)} ms · {fileCount} files
          </span>
        )}
        <span className="spacer" />
        {tops.length > 1 && (
          <select className="small" value={top} onChange={(e) => setTop(e.target.value)} aria-label="Top module">
            {tops.map((t) => (
              <option key={t}>{t}</option>
            ))}
          </select>
        )}
        {running ? (
          <button className="small" onClick={() => client.current?.cancel()}>
            Stop
          </button>
        ) : (
          <button className="small primary" onClick={run}>
            Run
          </button>
        )}
        <div className="run-tabs" role="tablist">
          {(["output", "waves"] as const).map((t) => (
            <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "active" : ""} onClick={() => setTab(t)}>
              {t}
            </button>
          ))}
        </div>
        <button className="small" onClick={downloadVcd} disabled={!result || running} title="Download VCD for GTKWave/Surfer">
          VCD
        </button>
        <button className="ghost small" onClick={() => onMaximize(!maximized)} aria-label={maximized ? "Restore panel" : "Maximize panel"}>
          {maximized ? "▾" : "▴"}
        </button>
        <button className="ghost small" onClick={onClose} aria-label="Close simulation">
          ×
        </button>
      </header>
      <div className="run-body">
        {tab === "output" && (
          <pre className="run-log">
            {error && (
              <span className="error">
                {errLoc ? (
                  <button className="link error" onClick={() => onOpenFile(errLoc[1], Number(errLoc[2]))}>
                    {error}
                  </button>
                ) : (
                  error
                )}
                {"\n"}
              </span>
            )}
            {result?.log.join("\n")}
            {result?.stopped && <span className="warn">{`\n${result.stopped}`}</span>}
            {result?.warnings.map((w) => <span key={w} className="warn">{`\nwarning: ${w}`}</span>)}
            {result && !result.log.length && !result.stopped && <span className="muted">(no output; see the waves tab)</span>}
            {running && !result && "Simulating…"}
          </pre>
        )}
        {tab === "waves" && result && <Waveform signals={result.signals} endTime={result.time} timescaleFs={result.timescaleFs} />}
        {tab === "waves" && !result && <p className="muted pad">{running ? "Simulating…" : "No waveforms."}</p>}
      </div>
    </section>
  );
}
