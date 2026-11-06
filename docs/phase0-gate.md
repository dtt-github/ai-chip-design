# Phase 0 gate: browser to GDS while two people co-edit

**Result: passed on 5 Nov 2026 in 4 min 30 s** (target: under 15 minutes).

Setup: local stack (Postgres, Rauthy, API, web app) on one aarch64 machine (NVIDIA GB10, 20 cores),
LibreLane 3.0.16 with the IHP SG13G2 PDK pinned by that image.

| Time    | Who        | Step |
| ------- | ---------- | ---- |
| 0:00    | Dev User   | Creates project `uart_tx` in the browser (template: RTL, testbench, `config.json`). |
| 0:10    | Dev User   | Adds Second Dev as an editor (Members box). |
| 0:20    | Second Dev | Joins in a second browser; both see each other's presence. |
| 0:35    | Dev User   | Pastes the UART 8N1 transmitter into `src/uart_tx.v`. |
| 0:38    | Second Dev | Sees it live, types a review comment into the same file (keystroke by keystroke). |
| 1:30    | Second Dev | Pastes the self-checking testbench into `tb/tb_uart_tx.v`. |
| 3:00    | Dev User   | Runs **Simulate**: the browser simulator (WASM) runs the shared, uncommitted working copy: 4 bytes sent and received, `PASS`, 8 744 steps in 9 ms; waveforms shown. |
| 3:10    | Dev User   | Commits both people's changes (one commit, `133fa85c`); Second Dev's view moves to the new head. |
| 3:15    | Dev User   | **Run RTL → GDS** on that commit. |
| 4:30    | both       | Run finished in 48 s: **sign-off clean** (Magic + KLayout DRC, LVS, antenna, setup and hold timing); 166 standard cells, 7 990 µm² die, setup slack 13.5 ns, hold slack 0.184 ns. Layout opens in the viewer. |

The design is in `crates/wafer-sim/tests/conformance/uart_8n1.v`; Icarus Verilog and the browser
simulator print identical output for it.

What the gate run exposed and what was fixed during it:

- Monaco 0.57 takes input through the EditContext API; scripted pastes have to target
  `.native-edit-context` (only relevant to test automation, not to users).
- Monaco renders spaces as U+00A0, so text checks on the rendered editor need normalising
  (test automation only).

Not covered by this run: real network latency between the two editors (both were on localhost),
and more than two concurrent editors.
