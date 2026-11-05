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
  | { kind: "logs"; run: string }
  | { kind: "todo_drop"; task: string }
  | { kind: "seed_new"; name: string; gist: string; hypothesis: string | null; metric: string | null }
  | { kind: "seed_drop"; name: string }
  | { kind: "seed_revive"; name: string }
  | { kind: "seed_plant"; name: string; as_name: string | null; hypothesis: string | null; metric: string | null }
  | { kind: "migrate"; name: string }
  | { kind: "agent_queue"; task: string }
  | { kind: "agent_stop"; task: string };

// ---------------------------------------------------------------- the notebook (ADR 0005)

/** `nbdoc::Part`: text (restricted markdown) or a command block. */
export type NbPart = { kind: "text"; text: string } | { kind: "block"; id: string; code: string };

export interface NbDoc {
  front: [string, string][];
  parts: NbPart[];
}

/** One structured output (`arboria.output`), by `kind`. */
export type NbOutput =
  | { kind: "text"; text: string; truncated?: boolean }
  | { kind: "stream"; stderr: boolean; text: string }
  | { kind: "markdown"; text: string; truncated?: boolean }
  | { kind: "table"; columns: string[]; rows: unknown[][]; total: number; title?: string }
  | {
      kind: "plot";
      series: { name: string; x: (number | null)[]; y: (number | null)[] }[];
      title?: string;
      x?: string;
      y?: string;
      log?: boolean;
      style?: "line" | "bar";
      note?: string;
    }
  | { kind: "image"; mime: string; data: string }
  | { kind: "error"; ename: string; evalue: string; traceback: string[] }
  | { kind: "pending"; task: string | null; what?: string; name?: string; ask?: string; text?: string }
  | { kind: "run"; run: string; status?: string; text?: string }
  | {
      kind: "inspect";
      task: string | null;
      status: string;
      question: string;
      view: string;
      result: InspectResult | null;
      text?: string;
    };

export interface InspectSpan {
  id: string;
  path: string;
  start: number;
  end: number;
  note: string | null;
  line_notes: Record<string, string>;
  lines: string[];
}

export interface InspectResult {
  view: string;
  question: string;
  code_ref: string;
  title: string;
  summary: string | null;
  spans: InspectSpan[];
  text?: string;
  lang?: string;
  mermaid?: string;
}

/** `nbout::Cached`: a block's last result. */
export interface NbCached {
  code_sha: string;
  ran_at: string;
  duration_ms: number;
  ok: boolean;
  outputs: NbOutput[];
  defines: string[];
  uses: string[];
}

export interface NbView {
  notebook: { owner: string; name: string; path: string; marimo: string | null; cwd: string };
  exists: boolean;
  text: string;
  doc: NbDoc;
  outputs: Record<string, NbCached>;
  kernel: KernelInfo | null;
}

/** The kernel's `ready` event (and its pid). */
export interface KernelInfo {
  python?: string;
  lab?: boolean;
  lab_error?: string | null;
  pid?: number;
}

/** `nb-event`: what a running block says. */
export type NbEvent = { root: string; idea: string; block: string } & (
  | { event: "stream"; name: string; text: string }
  | { event: "output"; data: NbOutput }
);

export interface CommandEntry {
  name: string;
  kind: "内置" | "项目" | "全局";
  signature: string;
  description: string;
}

// ---------------------------------------------------------------- the agent queue

export interface AgentUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  turns: number;
  cost_usd: number;
}

export interface AgentRow {
  id: string;
  idea: string | null;
  seed: string | null;
  text: string;
  status: "open" | "queued" | "running" | "check_failed" | "needs_human" | "done" | "dropped";
  kind: string;
  created_at: string;
  done_at: string | null;
  result: string | null;
  attempts: number;
  session_id: string | null;
  model: string | null;
  started_at: string | null;
  finished_at: string | null;
  block_id: string | null;
  usage: AgentUsage | null;
}

export interface AgentLogLine {
  kind: "text" | "tool" | "result" | "done" | "error";
  text: string;
}

export interface AgentSettings {
  auto: boolean;
  model: string;
  max_turns: number;
  daily_limit: number;
  timeout_s: number;
  claude: string | null;
}
