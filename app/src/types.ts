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
  in_claude_code: string | null;
  min_shown_ms: number;
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
