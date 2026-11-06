//! Public data types: ledger rows, their enums, and ids as validated newtypes.
//!
//! Replaces the `dict[str, Any]` rows of the Python code (`sqlite3.Row` → dict) and the
//! value tuples of `arboria.db` (`CONFIDENCE`, `RUN_STATUS`, …). Column names and enum
//! spellings are the ledger's, so a row maps 1:1 to a struct and `serde_json` output
//! matches Python's `--json` keys.
//!
//! Invariants:
//! - Text ids are `<prefix><n>` with at least three digits (`r001`, `d012`, `t1000`);
//!   new ids come from `db::WriteTx::next_id` (`{prefix}{n:03}`, n = max + 1), exactly
//!   like Python `Ledger.next_id`. Parsing is lenient about extra leading zeros because
//!   Python never validated ids it read back.
//! - Names (`IdeaName`, also seeds) match `^[a-z0-9][a-z0-9-]{0,62}$` (Python
//!   `db.NAME_RE`); project ids `^[a-z0-9][a-z0-9-]{0,30}$` (`slurm.PROJECT_RE`); Slurm job
//!   ids `^[0-9]+(?:_[0-9]+)?$` (`backends.base.JOB_ID_RE`).
//! - Enum strings are the ledger's CHECK lists (schema v8); `ALL` keeps their order.
//! - All of these match the whole string: a trailing `\n` is refused (Python uses
//!   `re.fullmatch`; its `$` alone would let one through).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

// ---------------------------------------------------------------- string enums

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident ($field:literal) { $($(#[$vm:meta])* $var:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vm])* #[serde(rename = $s)] $var),+
        }

        impl $name {
            /// Every value, in the ledger's CHECK order.
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            /// The ledger spelling.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }

            /// The ledger spellings, in order (for messages and CHECK lists).
            pub fn names() -> Vec<&'static str> {
                Self::ALL.iter().map(|v| v.as_str()).collect()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;
            /// Python `db.validate_choice`: `invalid <field> '<v>'; choose one of: a, b`.
            fn from_str(s: &str) -> Result<Self> {
                match s {
                    $($s => Ok($name::$var),)+
                    _ => Err(Error::Arb(format!(
                        "invalid {} {}; choose one of: {}",
                        $field, crate::records::py_repr(s),
                        Self::names().join(", ")
                    ))),
                }
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                let s = v.as_str()?;
                s.parse().map_err(|e: Error| rusqlite::types::FromSqlError::Other(Box::new(e)))
            }
        }
    };
}

str_enum!(
    /// `ideas.confidence`, `notes.confidence` (Python `db.CONFIDENCE`).
    Confidence("confidence") { Guess = "guess", Prelim = "prelim", Confirmed = "confirmed", Rejected = "rejected" }
);

str_enum!(
    /// `ideas.stage` (v8): the one lifecycle of an idea. `seed` = no repo yet (was the
    /// v6–v7 `seeds` table); `open` = branch + worktree; `merged` / `dropped` = closed.
    /// A dropped seed is `dropped` with no branch.
    IdeaStage("stage") { Seed = "seed", Open = "open", Merged = "merged", Dropped = "dropped" }
);

str_enum!(
    /// `ideas.status` (v1–v7, kept in v8 for Python read-only readers):
    /// `status = CASE stage WHEN 'seed' THEN 'open' ELSE stage END` (a CHECK enforces it).
    IdeaStatus("idea status") { Open = "open", Merged = "merged", Dropped = "dropped" }
);

str_enum!(
    /// `runs.status` (Python `db.RUN_STATUS`).
    RunStatus("run status") {
        Draft = "draft",
        AwaitingApproval = "awaiting_approval",
        Approved = "approved",
        Queued = "queued",
        Running = "running",
        Succeeded = "succeeded",
        Failed = "failed",
        Cancelled = "cancelled",
        Rejected = "rejected",
    }
);

impl RunStatus {
    /// Runs whose jobs may still be on the machine (`db.RUN_ACTIVE`).
    pub const ACTIVE: &'static [RunStatus] =
        &[RunStatus::Approved, RunStatus::Queued, RunStatus::Running];
    /// Finished runs (`db.RUN_FINAL`).
    pub const FINAL: &'static [RunStatus] = &[
        RunStatus::Succeeded,
        RunStatus::Failed,
        RunStatus::Cancelled,
    ];
    /// Runs an identical re-submit does not return (`db.RUN_NOT_REUSABLE`).
    pub const NOT_REUSABLE: &'static [RunStatus] =
        &[RunStatus::Rejected, RunStatus::Cancelled, RunStatus::Failed];
}

str_enum!(
    /// `jobs.state` (Python `db.JOB_STATES`).
    JobState("job state") {
        Submitting = "submitting",
        Pending = "pending",
        Running = "running",
        Completed = "completed",
        Failed = "failed",
        Cancelled = "cancelled",
        Timeout = "timeout",
        Unknown = "unknown",
    }
);

impl JobState {
    /// `db.JOB_TERMINAL`.
    pub const TERMINAL: &'static [JobState] = &[
        JobState::Completed,
        JobState::Failed,
        JobState::Cancelled,
        JobState::Timeout,
    ];
}

str_enum!(
    /// `decisions.subject_type` (Python `db.SUBJECT_TYPES`, plus v8's `probe`: the human's
    /// hash-bound enable / disable of a probe, ADR 0003).
    SubjectType("subject_type") { Run = "run", Idea = "idea", Lesson = "lesson", Probe = "probe" }
);

str_enum!(
    /// `lessons.status` (Python `db.LESSON_STATUS`).
    LessonStatus("lesson status") { Proposed = "proposed", Accepted = "accepted", Retired = "retired" }
);

str_enum!(
    /// `lessons.source_type` (Python `db.LESSON_SOURCES`).
    LessonSource("lesson source") { Idea = "idea", Run = "run", None = "none" }
);

str_enum!(
    /// `tasks.status` (v8 agent queue; Python v7 had open / done / dropped).
    /// `open` → `queued` (handed to a background `claude -p`) → `running` → `done`, or
    /// `check_failed` (static check failed; resumed once, then a bigger model once) →
    /// `needs_human`. `dropped` from anywhere. `done_at` is set iff done or dropped.
    TaskStatus("todo status") {
        Open = "open",
        Queued = "queued",
        Running = "running",
        CheckFailed = "check_failed",
        NeedsHuman = "needs_human",
        Done = "done",
        Dropped = "dropped",
    }
);

impl TaskStatus {
    /// Still waiting for an answer: open, or somewhere in the agent queue (queued,
    /// running, check_failed, needs_human). Re-running a block reuses such a todo
    /// instead of adding another one.
    pub fn is_pending(self) -> bool {
        !matches!(self, TaskStatus::Done | TaskStatus::Dropped)
    }
}

str_enum!(
    /// `tasks.kind` (Python `db.TASK_KINDS`, plus v8's probe todos of ADR 0003). Set by
    /// code, never by an LLM.
    TaskKind("todo kind") {
        Plain = "plain",
        CommandNew = "command_new",
        CommandRevise = "command_revise",
        Inspect = "inspect",
        ProbeNew = "probe_new",
        ProbeRevise = "probe_revise",
    }
);

str_enum!(
    /// `inspections.view` (Python `db.INSPECT_VIEWS`).
    InspectView("view") { Code = "code", Pseudo = "pseudo", Simple = "simple", Map = "map" }
);

str_enum!(
    /// `seeds.status` (v6–v7; the `seeds` table is frozen in v8, see `IdeaStage`).
    SeedStatus("seed status") { Thinking = "thinking", Planted = "planted", Dropped = "dropped" }
);

// ---------------------------------------------------------------- event kinds

str_enum!(
    /// `events.kind`. Spellings are part of the cross-language contract (ADR 0001).
    EventKind("event kind") {
        IdeaNew = "idea_new",
        IdeaClose = "idea_close",
        IdeaConfidence = "idea_confidence",
        IdeaMetric = "idea_metric",
        NotebookNew = "notebook_new",
        Note = "note",
        Decision = "decision",
        RunNew = "run_new",
        RunSummary = "run_summary",
        RunSubmit = "run_submit",
        RunApprove = "run_approve",
        RunQueued = "run_queued",
        RunReject = "run_reject",
        RunCancel = "run_cancel",
        RunStatus = "run_status",
        RunFinal = "run_final",
        RunConfig = "run_config",
        JobSubmit = "job_submit",
        JobState = "job_state",
        LessonPropose = "lesson_propose",
        LessonAccept = "lesson_accept",
        LessonReject = "lesson_reject",
        LessonRetire = "lesson_retire",
        TodoNew = "todo_new",
        TodoDone = "todo_done",
        TodoDrop = "todo_drop",
        SeedNew = "seed_new",
        SeedSet = "seed_set",
        SeedDrop = "seed_drop",
        SeedRevive = "seed_revive",
        SeedPlant = "seed_plant",
        CommandCall = "command_call",
        CommandPromote = "command_promote",
        InspectDone = "inspect_done",
        // v8 (Rust core): the agent queue and probes.
        /// A todo was handed to the background agent (open → queued).
        TodoQueue = "todo_queue",
        /// One background attempt started or finished (heartbeat-like: no backup).
        TodoAttempt = "todo_attempt",
        /// The agent gave up after its retries; the todo waits for the human.
        TodoNeedsHuman = "todo_needs_human",
        /// The human enabled a probe file (hash-bound Decision).
        ProbeEnable = "probe_enable",
        /// The human disabled a probe.
        ProbeDisable = "probe_disable",
    }
);

impl EventKind {
    /// Events after which the ledger is dumped and committed to the backup repo
    /// (Python `db.BACKUP_EVENTS` plus v8's; heartbeat-like updates — job states, run
    /// status, todo attempts, probe results — are deliberately absent).
    pub const BACKUP: &'static [EventKind] = &[
        EventKind::IdeaNew,
        EventKind::IdeaClose,
        EventKind::Note,
        EventKind::RunNew,
        EventKind::Decision,
        EventKind::LessonPropose,
        EventKind::LessonAccept,
        EventKind::LessonReject,
        EventKind::LessonRetire,
        EventKind::RunSubmit,
        EventKind::RunApprove,
        EventKind::RunQueued,
        EventKind::RunReject,
        EventKind::RunCancel,
        EventKind::RunFinal,
        EventKind::RunConfig,
        EventKind::TodoNew,
        EventKind::TodoDone,
        EventKind::TodoDrop,
        EventKind::IdeaConfidence,
        EventKind::IdeaMetric,
        EventKind::NotebookNew,
        EventKind::SeedNew,
        EventKind::SeedSet,
        EventKind::SeedDrop,
        EventKind::SeedRevive,
        EventKind::SeedPlant,
        EventKind::CommandPromote,
        EventKind::InspectDone,
        // v8
        EventKind::TodoQueue,
        EventKind::TodoNeedsHuman,
        EventKind::ProbeEnable,
        EventKind::ProbeDisable,
    ];

    /// Whether this event triggers a backup commit.
    pub fn triggers_backup(self) -> bool {
        Self::BACKUP.contains(&self)
    }
}

// ---------------------------------------------------------------- id newtypes

fn invalid(what: &str, s: &str, rule: &str) -> Error {
    Error::Arb(format!(
        "invalid {what} {}: {rule}",
        crate::records::py_repr(s)
    ))
}

macro_rules! seq_id {
    ($(#[$m:meta])* $name:ident, $prefix:literal, $what:literal) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// The id prefix (`r`, `d`, `t`, `l`).
            pub const PREFIX: char = $prefix;

            /// Validate `<prefix><3+ digits>`.
            pub fn parse(s: &str) -> Result<Self> {
                let digits = s.strip_prefix($prefix).unwrap_or("");
                if s.starts_with($prefix) && digits.len() >= 3 && digits.bytes().all(|b| b.is_ascii_digit()) {
                    Ok($name(s.to_string()))
                } else {
                    Err(invalid($what, s, concat!("expected '", $prefix, "' and at least 3 digits, e.g. ", $prefix, "001")))
                }
            }

            /// The id with number `n` as Python formats new ones (`f"{prefix}{n:03d}"`).
            pub fn from_number(n: u64) -> Self {
                $name(format!("{}{:03}", $prefix, n))
            }

            /// The numeric part (ids order by this, not by their text: `r999` < `r1000`).
            pub fn number(&self) -> u64 {
                self.0[1..].parse().expect("validated digits")
            }

            /// The id text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(s: String) -> Result<Self> {
                Self::parse(&s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.0.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                Self::parse(v.as_str()?).map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
            }
        }
    };
}

seq_id!(
    /// `runs.id`: `r001`.
    RunId, 'r', "run id"
);
seq_id!(
    /// `decisions.id`: `d001`.
    DecisionId, 'd', "decision id"
);
seq_id!(
    /// `tasks.id` (todos): `t001`.
    TaskId, 't', "todo id"
);
seq_id!(
    /// `lessons.id`: `l001`.
    LessonId, 'l', "lesson id"
);

macro_rules! text_id {
    ($(#[$m:meta])* $name:ident, $check:path) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Validate (see the type's doc for the rule).
            pub fn parse(s: &str) -> Result<Self> {
                $check(s)?;
                Ok($name(s.to_string()))
            }

            /// The text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;
            fn from_str(s: &str) -> Result<Self> {
                Self::parse(s)
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(s: String) -> Result<Self> {
                Self::parse(&s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.0.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                Self::parse(v.as_str()?).map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
            }
        }
    };
}

/// `^[a-z0-9][a-z0-9-]{0,max}$` on the whole string, ASCII only.
fn slug(s: &str, max_rest: usize) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= max_rest + 1
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn check_idea_name(s: &str) -> Result<()> {
    if slug(s, 62) {
        Ok(())
    } else {
        // Python db.validate_name(name, "idea")
        Err(Error::Arb(format!(
            "invalid idea name {}: use lowercase letters, digits and '-' (must start with a letter or digit, max 63 chars)",
            crate::records::py_repr(s)
        )))
    }
}

fn check_project_id(s: &str) -> Result<()> {
    if slug(s, 30) {
        Ok(())
    } else {
        // Python slurm.check_project
        Err(Error::Arb(format!(
            "invalid project id {}; must match ^[a-z0-9][a-z0-9-]{{0,30}}$",
            crate::records::py_repr(s)
        )))
    }
}

fn check_job_id(s: &str) -> Result<()> {
    let ok_part = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let ok = match s.split_once('_') {
        Some((a, b)) => ok_part(a) && ok_part(b),
        None => ok_part(s),
    };
    if ok {
        Ok(())
    } else {
        Err(Error::Backend(format!("invalid Slurm job id(s): {s}")))
    }
}

text_id!(
    /// An idea's (or seed's) name: `^[a-z0-9][a-z0-9-]{0,62}$`. Branch `idea/<name>`.
    IdeaName, check_idea_name
);
text_id!(
    /// `arboria.toml [project] id`: `^[a-z0-9][a-z0-9-]{0,30}$`. Names jobs and remote dirs.
    ProjectId, check_project_id
);
text_id!(
    /// A Slurm job id: `^[0-9]+(?:_[0-9]+)?$` (array jobs `123_4`). Other machine kinds
    /// keep their own ids in `jobs.job_id` as text; this type is for Slurm only.
    JobId, check_job_id
);

/// `ideas.id` (INTEGER PRIMARY KEY). Seeds are ideas too since v8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdeaId(pub i64);

/// `notes.id` (INTEGER PRIMARY KEY).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(pub i64);

/// `events.id` (INTEGER PRIMARY KEY).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub i64);

/// Timestamps are ISO-8601 UTC text, second resolution: `2026-11-02T21:24:00Z`
/// (Python `db.now`; see [`crate::db::now`]).
pub type Timestamp = String;

// ---------------------------------------------------------------- rows

/// An `ideas` row (v8). A seed is an idea at stage `seed`: no branch / worktree yet, and
/// hypothesis / metric may be missing until it is planted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Idea {
    pub id: IdeaId,
    pub name: IdeaName,
    pub stage: IdeaStage,
    /// Kept in sync with `stage` for Python readers (see [`IdeaStatus`]).
    pub status: IdeaStatus,
    /// One line, seeds (≤300, human only).
    pub gist: Option<String>,
    pub hypothesis: Option<String>,
    pub metric: Option<String>,
    pub parent_id: Option<IdeaId>,
    pub confidence: Confidence,
    /// `idea/<name>`; None for seeds.
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub created_at: Timestamp,
    pub closed_at: Option<Timestamp>,
    /// Last change of gist / hypothesis / metric / stage (v8; seeds had it before).
    pub updated_at: Option<Timestamp>,
    /// The v6–v7 `seeds.id` this row was migrated from (None for everything else).
    pub legacy_seed_id: Option<i64>,
}

/// A `plantings` row: a seed planted into a repo as an idea there (v8 adds `seed_idea_id`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Planting {
    pub id: i64,
    /// The seed's row in this ledger's `ideas`.
    pub seed_idea_id: IdeaId,
    /// Legacy `seeds.id` (rows from v7 only).
    pub seed_id: Option<i64>,
    pub repo: String,
    pub project_id: Option<ProjectId>,
    /// The idea's name in the target repo.
    pub idea: IdeaName,
    pub created_at: Timestamp,
}

/// A `runs` row (v8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub idea_id: IdeaId,
    pub snapshot_commit: String,
    pub tree_hash: String,
    pub config_json: Option<String>,
    /// ≤300, LLM-writable.
    pub change_reason: String,
    pub machine: Option<String>,
    pub partition: Option<String>,
    pub gpus: Option<i64>,
    pub walltime: Option<String>,
    pub est_cost: Option<f64>,
    pub actual_cost: Option<f64>,
    pub status: RunStatus,
    pub review_json: Option<String>,
    pub wandb_url: Option<String>,
    /// ≤1500 chars and ≤10 lines, LLM-writable.
    pub summary: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// What `arb submit` asked for ([`crate::slurm::Plan::request`]); `ledger_json` text.
    pub request_json: Option<String>,
    /// sha256 of the canonical card payload ([`crate::cards::card_hash`]).
    pub card_hash: Option<String>,
    pub parent_run: Option<RunId>,
    /// The notebook block (```` ```arb {#id} ```` fence) that submitted it (v8).
    pub block_id: Option<String>,
    /// The `--repeat` nonce (v8 column; also in request_json `ask.repeat`, which the card
    /// hash reads).
    pub nonce: Option<String>,
}

/// A `jobs` row: one Slurm segment (or one job on another machine kind).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub run_id: RunId,
    pub segment: u32,
    pub job_name: String,
    /// The machine's job id (Slurm: [`JobId`] text).
    pub job_id: Option<String>,
    pub state: JobState,
    pub elapsed_s: Option<i64>,
    pub billed_su: Option<f64>,
    pub submitted_at: Option<Timestamp>,
    pub updated_at: Timestamp,
}

/// A `decisions` row. Only a human decision (`decided_by = 'human'`, choice `approve`)
/// lets the core submit a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: DecisionId,
    pub subject_type: SubjectType,
    pub subject_id: String,
    pub question: String,
    /// `options_json` decoded.
    pub options: Vec<String>,
    pub choice: Option<String>,
    pub decided_by: Option<String>,
    pub created_at: Timestamp,
    /// Provenance of the decision (process chain, how it was confirmed); for a probe also
    /// what the human was shown.
    pub payload_json: Option<String>,
    /// v8: the content hash the decision is bound to (probe enable: sha256 of the file).
    pub bound_hash: Option<String>,
}

/// A `notes` row. Notes are the human's (≤1000).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub idea_id: IdeaId,
    pub run_id: Option<RunId>,
    pub text: String,
    pub confidence: Option<Confidence>,
    pub created_at: Timestamp,
    pub author: Option<String>,
}

/// A `lessons` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lesson {
    pub id: LessonId,
    /// One line ≤200, LLM-writable.
    pub text: String,
    pub source_type: LessonSource,
    pub source_id: Option<String>,
    pub status: LessonStatus,
    pub created_at: Timestamp,
    pub decided_at: Option<Timestamp>,
}

/// A `tasks` row (v8): a todo, and the agent queue entry that works on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    /// The idea (or seed-stage idea) it belongs to. Always set in v8.
    pub idea_id: IdeaId,
    /// Legacy `seeds.id` (rows from v7 only).
    pub seed_id: Option<i64>,
    /// The human's text (≤1000).
    pub text: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub created_at: Timestamp,
    pub done_at: Option<Timestamp>,
    /// The agent's result (≤600).
    pub result: Option<String>,
    pub payload_json: Option<String>,
    /// Background attempts so far (v8).
    pub attempts: u32,
    /// `claude -p` session id of the latest attempt, for `--resume` (v8).
    pub session_id: Option<String>,
    /// Model of the latest attempt (v8).
    pub model: Option<String>,
    /// Usage summed over attempts, `ledger_json` (v8).
    pub usage_json: Option<String>,
    /// Latest attempt start / end (v8).
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    /// The notebook block that asked for it (v8).
    pub block_id: Option<String>,
}

/// An `events` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub ts: Timestamp,
    /// Text, not [`EventKind`]: old ledgers may hold kinds this build does not know.
    pub kind: String,
    pub subject: Option<String>,
    pub payload_json: Option<String>,
}

/// A `probe_results` row (v8, ADR 0003). `result_json` is a validated
/// [`crate::probe::ProbeResult`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResultRow {
    pub id: i64,
    pub machine: String,
    pub probe: String,
    pub at: Timestamp,
    pub input_hash: String,
    pub result_json: String,
    pub probe_hash: Option<String>,
}

/// An `inspections` row (a delivered `lab.inspect` answer).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inspection {
    pub task_id: TaskId,
    pub idea_id: IdeaId,
    pub question: String,
    pub view: InspectView,
    pub target: String,
    pub code_ref: String,
    pub tree: String,
    pub result_json: String,
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_ids_parse_format_and_order_numerically() {
        let r = RunId::parse("r001").unwrap();
        assert_eq!(r.number(), 1);
        assert_eq!(RunId::from_number(7).as_str(), "r007");
        assert_eq!(RunId::from_number(1000).as_str(), "r1000");
        assert!(RunId::parse("r1000").is_ok());
        for bad in [
            "", "r", "r01", "R001", "d001", "r00a", "r 001", "r-01", "r001x", "r١٢٣",
        ] {
            assert!(RunId::parse(bad).is_err(), "{bad:?}");
        }
        assert!(DecisionId::parse("d012").is_ok());
        assert!(TaskId::parse("t001").is_ok());
        assert!(LessonId::parse("l001").is_ok());
        assert!(TaskId::parse("l001").is_err());
        // text order is not id order
        assert!(RunId::parse("r999").unwrap().as_str() > RunId::parse("r1000").unwrap().as_str());
        assert!(RunId::parse("r999").unwrap().number() < RunId::parse("r1000").unwrap().number());
    }

    #[test]
    fn names_follow_python_name_re() {
        for good in ["a", "0", "wide-lr", "a-", &"a".repeat(63)] {
            assert!(IdeaName::parse(good).is_ok(), "{good:?}");
        }
        for bad in [
            "",
            "-a",
            "A",
            "a_b",
            "a.b",
            "é",
            &"a".repeat(64),
            "a b",
            "abc\n",
            "a\n\n",
            "\n",
            "a\nb",
        ] {
            assert!(IdeaName::parse(bad).is_err(), "{bad:?}");
        }
        let err = IdeaName::parse("Bad").unwrap_err().to_string();
        assert_eq!(err, "invalid idea name 'Bad': use lowercase letters, digits and '-' (must start with a letter or digit, max 63 chars)");
    }

    #[test]
    fn project_ids_and_job_ids() {
        assert!(ProjectId::parse(&"p".repeat(31)).is_ok());
        assert!(ProjectId::parse(&"p".repeat(32)).is_err());
        assert!(ProjectId::parse("wavefront-1a2b").is_ok());
        assert_eq!(
            ProjectId::parse("X").unwrap_err().to_string(),
            "invalid project id 'X'; must match ^[a-z0-9][a-z0-9-]{0,30}$"
        );
        for good in ["1", "123456", "12_3"] {
            assert!(JobId::parse(good).is_ok(), "{good:?}");
        }
        for bad in ["", "_1", "1_", "1_2_3", "a1", "1;rm", " 1"] {
            assert!(JobId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ids_round_trip_through_serde() {
        let r: RunId = serde_json::from_str("\"r042\"").unwrap();
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"r042\"");
        assert!(serde_json::from_str::<RunId>("\"x042\"").is_err());
        let n: IdeaName = serde_json::from_str("\"base\"").unwrap();
        assert_eq!(n.as_str(), "base");
    }

    #[test]
    fn enums_use_ledger_spellings() {
        assert_eq!(RunStatus::AwaitingApproval.as_str(), "awaiting_approval");
        assert_eq!(
            "check_failed".parse::<TaskStatus>().unwrap(),
            TaskStatus::CheckFailed
        );
        assert_eq!(
            "maybe".parse::<Confidence>().unwrap_err().to_string(),
            "invalid confidence 'maybe'; choose one of: guess, prelim, confirmed, rejected"
        );
        assert_eq!(serde_json::to_string(&IdeaStage::Seed).unwrap(), "\"seed\"");
        assert!(EventKind::InspectDone.triggers_backup());
        assert!(!EventKind::JobState.triggers_backup());
        assert!(!EventKind::RunSummary.triggers_backup());
        // Python's 29 plus v8's four
        assert_eq!(EventKind::BACKUP.len(), 33);
        assert!(!EventKind::TodoAttempt.triggers_backup());
        assert_eq!("probe_new".parse::<TaskKind>().unwrap(), TaskKind::ProbeNew);
        assert_eq!(SubjectType::Probe.as_str(), "probe");
    }
}
