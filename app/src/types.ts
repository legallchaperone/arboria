// The core's JSON (crates/arboria-core/src/view.rs, helper.rs, the app commands in
// src-tauri/src/lib.rs). crates/arboria-core/tests/view.rs checks that the fixture
// src/fixtures/board.json has exactly the core's shape; `npm run check` checks the
// fixture against these types.

export type Place = "project" | "home";

export interface ProjectInfo {
  root: string;
  name: string;
  kind: Place;
  project_id: string | null;
  total_su: number;
  config_error: string | null;
  loaded_at: string;
}

export interface IdeaView {
  id: number;
  name: string;
  stage: "seed" | "open" | "merged" | "dropped" | string;
  confidence: string;
  gist: string | null;
  hypothesis: string | null;
  metric: string | null;
  parent: string | null;
  from_run: string | null;
  created_at: string;
  closed_at: string | null;
  last_active: string;
  su_used: number;
  runs: number;
  worktree: string | null;
  notebook: string | null;
}

export interface JobView {
  segment: number;
  job_id: string | null;
  state: string;
  elapsed_s: number | null;
  billed_su: number | null;
}

export interface RunView {
  id: string;
  idea: string;
  parent_run: string | null;
  status: string;
  change_reason: string;
  machine: string | null;
  partition: string | null;
  gpus: number | null;
  walltime: string | null;
  est_su: number | null;
  billed_su: number;
  summary: string | null;
  wandb_url: string | null;
  created_at: string;
  updated_at: string;
  finished_at: string | null;
  jobs: JobView[];
}

export type InboxKind =
  | "approve"
  | "failed"
  | "finished"
  | "agent_done"
  | "lesson"
  | "probe_reenable";

export interface InboxItem {
  kind: InboxKind;
  when: string;
  title: string;
  meta: string;
  idea: string | null;
  run: string | null;
  task: string | null;
  lesson: string | null;
  probe: string | null;
  command: string | null;
}

export interface CardView {
  run: string;
  text: string | null;
  card_hash: string | null;
  matches: boolean | null;
  est_su: number | null;
  resources: string[];
  estimate: string[];
  budget: string[];
  error: string | null;
}

export interface TodoView {
  id: string;
  idea: string;
  kind: string;
  status: string;
  text: string;
  created_at: string;
  done_at: string | null;
  result: string | null;
}

export interface NoteView {
  id: number;
  idea: string;
  run: string | null;
  text: string;
  author: string | null;
  created_at: string;
}

export interface LessonView {
  id: string;
  text: string;
  created_at: string;
}

export interface NotificationView {
  at: string | null;
  text: string;
}

export interface Spend {
  used: number;
  total: number;
  by_idea: { idea: string; su: number }[];
}

export interface ProbeView {
  machine: string;
  probe: string;
  at: string;
  result: ProbeResult;
}

export type ProbeResult =
  | { kind: "number"; label: string; value: number; of?: number | null; unit?: string | null }
  | { kind: "table"; label?: string | null; columns: string[]; rows: (string | number | null)[][] }
  | { kind: "text"; label?: string | null; text: string };

export interface Board {
  project: ProjectInfo;
  ideas: IdeaView[];
  runs: RunView[];
  inbox: InboxItem[];
  cards: CardView[];
  todos: TodoView[];
  notes: NoteView[];
  lessons: LessonView[];
  notifications: NotificationView[];
  spend: Spend;
  probes: ProbeView[];
}

export interface ProjectSummary {
  root: string;
  name: string;
  kind: Place;
  error: string | null;
  open_ideas: number;
  seeds: number;
  inbox: InboxItem[];
}

export interface Platform {
  os: string;
  touch_id: boolean;
  touch_id_why: string | null;
  in_claude_code: string | null;
  min_shown_ms: number;
  /** The seed home (种子园). */
  home: string;
}

export interface HelperStatus {
  service: string;
  installed: boolean;
  running: boolean;
  last_tick: string | null;
  needs_login: Record<string, string>;
  errors: string[];
}

export interface Approved {
  run: string;
  status: string;
  decision: string | null;
  jobs: string[];
}

export interface ProfileCheck {
  name: string;
  kind: string | null;
  host: string | null;
  error: string | null;
}

/** clusters::FileCheck */
export interface ClustersCheck {
  path: string;
  exists: boolean;
  text: string;
  syntax_error: string | null;
  profiles: ProfileCheck[];
}

/** src-tauri/src/pty.rs Launcher: what the terminal's "+" menu offers. */
export interface Launcher {
  id: string;
  label: string;
  installed: boolean;
  group: "agent" | "ssh" | null;
}

/** src-tauri/src/clone.rs Cloned */
export interface Cloned {
  path: string;
  actions: string[];
  warnings: string[];
}

/** core settings::PartitionForm / MachineForm / Machines */
export interface PartitionForm {
  name: string;
  max_walltime: string;
  charge: number;
}
export interface MachineForm {
  name: string;
  kind: string;
  host: string;
  account: string;
  arch: string;
  gpus_per_node: number;
  cpus_per_gpu: number;
  mem_per_gpu_gb: number;
  default_partition: string;
  max_segments: number;
  billing_per_su: number;
  balance_cmd: string;
  prelude: string[];
  env_activate: string[];
  partitions: PartitionForm[];
  paths: Record<string, string>;
  root: string | null;
}
export interface MachinesPage {
  path: string;
  machines: { form: MachineForm; error: string | null }[];
  syntax_error: string | null;
}

/** core settings::ProjectForm / ProjectSettings */
export interface ProjectForm {
  default_machine: string;
  total_su: number;
  wandb_entity: string;
  wandb_project: string;
  run_command: string;
  override_style: string;
  observation_minutes: number;
  fail_patterns: string[];
}
export interface ProjectSettings {
  root: string;
  path: string;
  project_id: string | null;
  form: ProjectForm;
  error: string | null;
  machines: string[];
  override_styles: string[];
}

export interface AppSettings {
  notifications: boolean;
  clone_dir: string;
  helper: HelperStatus;
}

/** src-tauri/src/actions.rs `Action` (serde tag `kind`). */
export type Action =
  | { kind: "idea_new"; name: string; hypothesis: string; metric: string; parent: string | null }
  | { kind: "idea_close"; name: string; merge: boolean }
  | { kind: "note"; idea: string; text: string; run: string | null }
  | { kind: "reject"; run: string; reason: string | null }
  | { kind: "cancel"; run: string }
  | { kind: "todo_drop"; task: string }
  | { kind: "seed_new"; name: string; gist: string; hypothesis: string | null; metric: string | null }
  | { kind: "seed_drop"; name: string }
  | { kind: "seed_revive"; name: string }
  | { kind: "seed_plant"; name: string; as_name: string | null; hypothesis: string | null; metric: string | null };
