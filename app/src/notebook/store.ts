// Per-block state of an open notebook, shared by the editor (which runs blocks) and the
// output views under each block (React roots inside ProseMirror node views).

import type { AgentRow, NbCached, NbOutput } from "../types";

export interface BlockState {
  /** The last finished result (the outputs cache). */
  cached: NbCached | null;
  /** What the block has said so far while it runs. */
  live: NbOutput[] | null;
  running: boolean;
  /** Waiting for the kernel (another block runs). */
  queued: boolean;
  /** The run could not even start (kernel would not start, …). */
  error: string | null;
  /** A block above ran later and changed a name this one uses. */
  stale: boolean;
}

const EMPTY: BlockState = { cached: null, live: null, running: false, queued: false, error: null, stale: false };

/** What the output views can ask of the page. */
export interface Actions {
  run(block: string): void;
  /** The run's card in the inspector (signing happens there, ADR 0004). */
  openRun(run: string): void;
  /** The agent panel at this todo. */
  openTask(task: string): void;
}

export class NbStore {
  private states = new Map<string, BlockState>();
  private listeners = new Map<string, Set<() => void>>();
  private any = new Set<() => void>();
  /** The agent's todos by id (pending outputs show how far the agent got). */
  tasks = new Map<string, AgentRow>();
  actions: Actions = { run: () => {}, openRun: () => {}, openTask: () => {} };

  get(id: string): BlockState {
    return this.states.get(id) ?? EMPTY;
  }

  set(id: string, patch: Partial<BlockState>) {
    this.states.set(id, { ...this.get(id), ...patch });
    this.emit(id);
  }

  /** Append a live event: streams of the same name merge. */
  push(id: string, out: NbOutput) {
    const live = [...(this.get(id).live ?? [])];
    const last = live[live.length - 1];
    if (out.kind === "stream" && last?.kind === "stream" && last.stderr === out.stderr) {
      live[live.length - 1] = { ...last, text: last.text + out.text };
    } else live.push(out);
    this.set(id, { live });
  }

  setTasks(rows: AgentRow[]) {
    this.tasks = new Map(rows.map((r) => [r.id, r]));
    for (const id of this.states.keys()) this.emit(id);
  }

  ids(): string[] {
    return [...this.states.keys()];
  }

  subscribe(id: string, f: () => void): () => void {
    let set = this.listeners.get(id);
    if (!set) this.listeners.set(id, (set = new Set()));
    set.add(f);
    return () => set!.delete(f);
  }

  /** Called on any block's change (the kernel bar counts running blocks). */
  subscribeAll(f: () => void): () => void {
    this.any.add(f);
    return () => this.any.delete(f);
  }

  private emit(id: string) {
    for (const f of this.listeners.get(id) ?? []) f();
    for (const f of this.any) f();
  }
}

/** The todo a block's outputs wait for (a missing command, an inspect being written). */
export function waitingTask(c: NbCached | null): string | null {
  for (const o of c?.outputs ?? []) {
    if (o.kind === "pending" && o.task) return o.task;
    if (o.kind === "inspect" && o.status === "pending" && o.task) return o.task;
  }
  return null;
}

/**
 * Blocks whose result is older than a block above that defines a name they use (ADR
 * 0005: marked, not re-run). `order` is the blocks top to bottom.
 */
export function staleBlocks(order: string[], cached: (id: string) => NbCached | null): Set<string> {
  const out = new Set<string>();
  for (let i = 0; i < order.length; i++) {
    const b = cached(order[i]);
    if (!b || !b.uses.length) continue;
    for (let j = 0; j < i; j++) {
      const a = cached(order[j]);
      if (a && a.ran_at > b.ran_at && a.defines.some((n) => b.uses.includes(n))) {
        out.add(order[i]);
        break;
      }
    }
  }
  return out;
}

/** sha256 hex of a block's code (`nbout::code_sha`). */
export async function codeSha(code: string): Promise<string> {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(code));
  return [...new Uint8Array(d)].map((b) => b.toString(16).padStart(2, "0")).join("");
}
