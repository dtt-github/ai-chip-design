// Shared working copy of a project (Yjs), synced with the API's collab room.
//
// Document layout (see crates/wafer-api/src/collab.rs):
//   files:   Y.Map<path, Y.Text>   every text file
//   deleted: Y.Map<path, true>     deletions staged in the working copy
//   meta:    Y.Map { head }        the commit the working copy is based on
import * as Y from "yjs";
import * as syncProtocol from "y-protocols/sync";
import * as awarenessProtocol from "y-protocols/awareness";
import * as encoding from "lib0/encoding";
import * as decoding from "lib0/decoding";
import { currentUser } from "./auth";

const MSG_SYNC = 0;
const MSG_AWARENESS = 1;
const MSG_QUERY_AWARENESS = 3;

export type CollabStatus = "connecting" | "synced" | "offline" | "refused";

/** Distinct, readable cursor colours. */
const COLORS = ["#e5484d", "#30a46c", "#0090ff", "#f76b15", "#8e4ec6", "#12a594", "#d6409f", "#ffb224"];

export interface Peer {
  clientId: number;
  name: string;
  color: string;
  file?: string;
}

export class CollabSession {
  readonly doc = new Y.Doc();
  readonly files = this.doc.getMap<Y.Text>("files");
  readonly deleted = this.doc.getMap<boolean>("deleted");
  readonly meta = this.doc.getMap<string>("meta");
  readonly awareness = new awarenessProtocol.Awareness(this.doc);
  status: CollabStatus = "connecting";
  role: "editor" | "viewer" | null = null;
  private ws: WebSocket | null = null;
  /** Handshake done: binary messages may flow. */
  private open = false;
  private retries = 0;
  private timer: number | undefined;
  private stopped = false;
  private listeners = new Set<() => void>();

  constructor(
    private projectId: string,
    user: { name: string },
  ) {
    const color = COLORS[this.doc.clientID % COLORS.length];
    this.awareness.setLocalStateField("user", { name: user.name, color, colorLight: `${color}33` });
    this.doc.on("update", (update: Uint8Array, origin: unknown) => {
      if (origin !== this) this.send((e) => {
        encoding.writeVarUint(e, MSG_SYNC);
        syncProtocol.writeUpdate(e, update);
      });
    });
    this.awareness.on("update", ({ added, updated, removed }: { added: number[]; updated: number[]; removed: number[] }, origin: unknown) => {
      if (origin === this) return;
      const changed = added.concat(updated, removed);
      this.send((e) => {
        encoding.writeVarUint(e, MSG_AWARENESS);
        encoding.writeVarUint8Array(e, awarenessProtocol.encodeAwarenessUpdate(this.awareness, changed));
      });
    });
    this.awareness.on("change", () => this.emit());
    window.addEventListener("beforeunload", this.goodbye);
    void this.connect();
  }

  /** Subscribe to status, role and presence changes. */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit() {
    this.listeners.forEach((fn) => fn());
  }

  peers(): Peer[] {
    const out: Peer[] = [];
    this.awareness.getStates().forEach((state, clientId) => {
      if (clientId === this.doc.clientID || !state.user) return;
      out.push({ clientId, name: state.user.name, color: state.user.color, file: state.file });
    });
    return out;
  }

  setActiveFile(path: string | null) {
    this.awareness.setLocalStateField("file", path ?? undefined);
  }

  /** The shared text of a file, or `undefined` if the working copy doesn't have it. */
  text(path: string): Y.Text | undefined {
    return this.files.get(path);
  }

  private send(write: (e: encoding.Encoder) => void) {
    if (this.ws?.readyState !== WebSocket.OPEN || !this.open) return;
    const e = encoding.createEncoder();
    write(e);
    this.ws.send(encoding.toUint8Array(e));
  }

  private async connect() {
    if (this.stopped) return;
    this.status = "connecting";
    this.emit();
    const user = await currentUser();
    if (!user || this.stopped) return;
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${window.location.host}/api/projects/${this.projectId}/collab`);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    let refused = false;
    ws.onopen = () => ws.send(JSON.stringify({ token: user.access_token }));
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        const msg = JSON.parse(ev.data);
        if (msg.type === "ready") {
          this.role = msg.role;
          this.retries = 0;
          this.open = true;
          // Our state vector (server answers with what we miss) and our presence.
          const e = encoding.createEncoder();
          encoding.writeVarUint(e, MSG_SYNC);
          syncProtocol.writeSyncStep1(e, this.doc);
          ws.send(encoding.toUint8Array(e));
          const a = encoding.createEncoder();
          encoding.writeVarUint(a, MSG_AWARENESS);
          encoding.writeVarUint8Array(a, awarenessProtocol.encodeAwarenessUpdate(this.awareness, [this.doc.clientID]));
          ws.send(encoding.toUint8Array(a));
        } else {
          refused = true;
          this.status = "refused";
          this.emit();
        }
        return;
      }
      this.receive(new Uint8Array(ev.data));
    };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.open = false;
      // Others' cursors are stale once we're disconnected.
      awarenessProtocol.removeAwarenessStates(
        this.awareness,
        [...this.awareness.getStates().keys()].filter((c) => c !== this.doc.clientID),
        this,
      );
      if (this.stopped || refused) return;
      this.status = "offline";
      this.emit();
      const delay = Math.min(15_000, 500 * 2 ** this.retries++);
      this.timer = window.setTimeout(() => this.connect(), delay);
    };
  }

  private receive(data: Uint8Array) {
    const d = decoding.createDecoder(data);
    while (decoding.hasContent(d)) {
      const type = decoding.readVarUint(d);
      if (type === MSG_SYNC) {
        // readSyncMessage writes a reply (sync step 2) only when one is needed.
        const reply = encoding.createEncoder();
        encoding.writeVarUint(reply, MSG_SYNC);
        const kind = syncProtocol.readSyncMessage(d, reply, this.doc, this);
        if (encoding.length(reply) > 1) this.ws?.send(encoding.toUint8Array(reply));
        if (kind === syncProtocol.messageYjsSyncStep2 && this.status !== "synced") {
          this.status = "synced";
          this.emit();
        }
      } else if (type === MSG_AWARENESS) {
        awarenessProtocol.applyAwarenessUpdate(this.awareness, decoding.readVarUint8Array(d), this);
      } else if (type === MSG_QUERY_AWARENESS) {
        // Answered by the server; nothing to do.
      } else {
        break;
      }
    }
  }

  private goodbye = () => {
    awarenessProtocol.removeAwarenessStates(this.awareness, [this.doc.clientID], "goodbye");
  };

  destroy() {
    this.stopped = true;
    window.clearTimeout(this.timer);
    this.goodbye();
    // Give the removal a moment to leave before closing.
    const ws = this.ws;
    this.ws = null;
    setTimeout(() => ws?.close(), 50);
    window.removeEventListener("beforeunload", this.goodbye);
    this.awareness.destroy();
    this.doc.destroy();
    this.listeners.clear();
  }
}
