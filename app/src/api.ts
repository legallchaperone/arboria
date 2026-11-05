// Typed calls into the app's Rust side (src-tauri/src/lib.rs). Every command name here is
// in src-tauri/build.rs and capabilities/main.json.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Approved,
  ClustersCheck,
  Board,
  CardView,
  HelperStatus,
  Platform,
  ProjectSummary,
} from "./types";

export const api = {
  platform: () => invoke<Platform>("platform"),
  projects: () => invoke<ProjectSummary[]>("projects"),
  board: (root: string) => invoke<Board>("board", { root }),
  addProject: (path: string) => invoke<{ path: string; added_at: string }>("add_project", { path }),
  cardShown: (root: string, run: string) => invoke<CardView>("card_shown", { root, run }),
  approve: (root: string, run: string, cardHash: string) =>
    invoke<Approved>("approve", { root, run, cardHash }),
  helperStatus: () => invoke<HelperStatus>("helper_status"),
  helperInstall: () => invoke<string>("helper_install"),
  openNotebook: (root: string, idea: string) => invoke<void>("open_notebook", { root, idea }),
  pickProject: () => invoke<{ path: string; added_at: string } | null>("pick_project"),
  clustersRead: () => invoke<ClustersCheck>("clusters_read"),
  clustersCheck: (text: string) => invoke<ClustersCheck>("clusters_check", { text }),
  clustersSave: (text: string) => invoke<ClustersCheck>("clusters_save", { text }),
  openInEditor: (path: string) => invoke<void>("open_in_editor", { path }),
  ptyOpen: (
    cwd: string,
    program: string | null,
    cols: number,
    rows: number,
    onData: (bytes: Uint8Array) => void,
    onExit: () => void,
  ) => {
    const data = new Channel<ArrayBuffer | number[]>();
    data.onmessage = (m) => onData(m instanceof ArrayBuffer ? new Uint8Array(m) : Uint8Array.from(m));
    const exit = new Channel<null>();
    exit.onmessage = () => onExit();
    return invoke<number>("pty_open", { cwd, program, cols, rows, onData: data, onExit: exit });
  },
  ptyWrite: (id: number, data: string) => invoke<void>("pty_write", { id, data }),
  ptyResize: (id: number, cols: number, rows: number) => invoke<void>("pty_resize", { id, cols, rows }),
  ptyClose: (id: number) => invoke<void>("pty_close", { id }),
};

/** `ledger-changed` (a root) and `projects-changed`, from src-tauri/src/watch.rs. */
export function onChange(f: (root: string | null) => void): Promise<UnlistenFn[]> {
  return Promise.all([
    listen<string>("ledger-changed", (e) => f(e.payload)),
    listen("projects-changed", () => f(null)),
  ]);
}

/** `clusters-changed`: clusters.toml was written (by the app, an editor, arb machine init). */
export function onClustersChange(f: () => void): Promise<UnlistenFn> {
  return listen("clusters-changed", () => f());
}

/** A message for an error thrown by `invoke` (the Rust side returns strings). */
export function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
