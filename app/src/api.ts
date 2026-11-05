// Typed calls into the app's Rust side (src-tauri/src/lib.rs). Every command name here is
// in src-tauri/build.rs and capabilities/main.json.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Action,
  AppSettings,
  MachineForm,
  MachinesPage,
  ProjectForm,
  ProjectSettings,
  Approved,
  ClustersCheck,
  Cloned,
  Launcher,
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
  /** The notebook server's URL (started on first use) for the embedded iframe. */
  openNotebook: (root: string, idea: string) => invoke<string>("open_notebook", { root, idea }),
  closeNotebook: (root: string, idea: string) => invoke<void>("close_notebook", { root, idea }),
  terminalLaunchers: () => invoke<Launcher[]>("terminal_launchers"),
  pickFolder: () => invoke<string | null>("pick_folder"),
  cloneDir: (set?: string) => invoke<string>("clone_dir", { set: set ?? null }),
  cloneProject: (url: string, parent: string | null) => invoke<Cloned>("clone_project", { url, parent }),
  pickProject: () => invoke<{ path: string; added_at: string } | null>("pick_project"),
  clustersRead: () => invoke<ClustersCheck>("clusters_read"),
  clustersCheck: (text: string) => invoke<ClustersCheck>("clusters_check", { text }),
  clustersSave: (text: string) => invoke<ClustersCheck>("clusters_save", { text }),
  openInEditor: (path: string) => invoke<void>("open_in_editor", { path }),
  settingsMachines: () => invoke<MachinesPage>("settings_machines"),
  blankMachine: (name: string) => invoke<MachineForm>("settings_blank_machine", { name }),
  saveMachine: (original: string | null, form: MachineForm) =>
    invoke<MachinesPage>("settings_save_machine", { original, form }),
  removeMachine: (name: string) => invoke<MachinesPage>("settings_remove_machine", { name }),
  settingsProject: (root: string) => invoke<ProjectSettings>("settings_project", { root }),
  saveProject: (root: string, form: ProjectForm) => invoke<ProjectSettings>("settings_save_project", { root, form }),
  appSettings: () => invoke<AppSettings>("app_settings"),
  setNotifications: (on: boolean) => invoke<boolean>("set_notifications", { on }),
  helperUninstall: () => invoke<void>("helper_uninstall"),
  /** An everyday action (src-tauri/src/actions.rs): a fixed `arb` command line. */
  act: (root: string, action: Action) => invoke<{ output: string }>("act", { root, action }),
  /** Off the list only: the files and the ledger stay. */
  removeProject: (path: string) => invoke<boolean>("remove_project", { path }),
  /** Finder: the project's folder, or one of its ideas' worktrees. */
  reveal: (root: string, path?: string) => invoke<void>("reveal", { root, path: path ?? null }),
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
