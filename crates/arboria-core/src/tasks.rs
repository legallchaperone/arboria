//! Todos and the agent queue: written by the human (a notebook block), done by the agent
//! (`arb todo done`, or `arb inspect done` / `arb probe` for those kinds). Ports
//! `arboria.tasks`; the queue states are new in v8 (ADR 0001).
//!
//! Invariants:
//! - The todo text is the human's (≤1000); the result is LLM-writable (≤600). `kind` and
//!   `payload_json` are set by code, never by an LLM.
//! - A todo belongs to one idea row (since v8 also a seed-stage idea); a todo of a closed
//!   idea or a dropped seed is not live.
//! - Queue: `open` → `queued` (handed to a background `claude -p`, one process per todo)
//!   → `running` → `done`; a failed static check is `check_failed`, resumed once
//!   (`--resume <session_id>`), then retried once with a bigger model, then
//!   `needs_human`. `dropped` from any state. `done_at` is set iff done or dropped
//!   (a CHECK). Attempts, session, model, usage and times are recorded on the row.
//! - `arb todo done` refuses inspect todos ([`INSPECT_DONE`]).
//! - Only the user's own logged-in `claude` is used; the core never handles an OAuth
//!   token (ADR 0001).

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use serde::Serialize;
use serde_json::Value;

use crate::db::Ledger;
use crate::model::{Idea, IdeaId, Task, TaskId, TaskKind, TaskStatus};
use crate::Result;

/// `arb todo done` on an inspect todo.
pub const INSPECT_DONE: &str = "{id} 是读代码待办：用 `arb inspect done {id} --file …` 交付答案（代码由 arb 从 git 取），约定见 `arb inspect guide`；做不了就 `arb todo drop {id}`";

/// A todo with its owner's name (`tasks.SELECT`: `idea` / `seed`).
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    #[serde(flatten)]
    pub task: Task,
    /// The owner's name when it is an idea in a repo.
    pub idea: Option<String>,
    /// The owner's name when it is a seed.
    pub seed: Option<String>,
}

impl TaskView {
    /// The owner's name (`?` when unknown).
    pub fn owner_name(&self) -> &str {
        self.idea.as_deref().or(self.seed.as_deref()).unwrap_or("?")
    }
}

/// A todo by id (`no todo 't009' (see `arb todo list`)`).
pub fn get_task(ledger: &Ledger, task_id: &TaskId) -> Result<TaskView> {
    todo!("port of arboria.tasks.get_task")
}

/// `payload_json` decoded (`{}` for plain todos).
pub fn payload(task: &Task) -> Result<Value> {
    todo!("port of arboria.tasks.payload")
}

/// A new open todo of an idea or seed (`todo_new` event). `kind` / `data` from code only.
pub fn add(
    ledger: &mut Ledger,
    owner: &Idea,
    text: &str,
    kind: TaskKind,
    data: Option<&Value>,
    block_id: Option<&str>,
) -> Result<TaskView> {
    todo!("port of arboria.tasks.add")
}

/// The oldest open plain todo of a live owner with exactly this text (re-running a block
/// does not duplicate it).
pub fn open_plain_task(ledger: &Ledger, owner: IdeaId, text: &str) -> Result<Option<TaskView>> {
    todo!("port of arboria.tasks.open_plain_task")
}

/// Open command todos about command `name`, oldest first.
pub fn open_command_tasks(
    ledger: &Ledger,
    name: &str,
    kind: Option<TaskKind>,
) -> Result<Vec<TaskView>> {
    todo!("port of arboria.tasks.open_command_tasks")
}

/// `arb todo list`: oldest first; open only unless `include_closed`; all live owners when
/// `owner` is None.
pub fn list_tasks(
    ledger: &Ledger,
    owner: Option<IdeaId>,
    include_closed: bool,
) -> Result<Vec<TaskView>> {
    todo!("port of arboria.tasks.list_tasks")
}

/// `arb todo done` (result ≤600; refuses inspect todos).
pub fn done(ledger: &mut Ledger, task_id: &TaskId, result: &str) -> Result<TaskView> {
    todo!("port of arboria.tasks.done")
}

/// `arb todo drop` (reason ≤600, optional).
pub fn drop_task(ledger: &mut Ledger, task_id: &TaskId, reason: Option<&str>) -> Result<TaskView> {
    todo!("port of arboria.tasks.drop")
}

// ---------------------------------------------------------------- agent queue (v8)

/// What one background attempt used (summed into `usage_json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub turns: u32,
    pub cost_usd: f64,
}

/// open → queued (`todo_queue` event).
pub fn enqueue(ledger: &mut Ledger, task_id: &TaskId) -> Result<TaskView> {
    todo!("agent queue (ADR 0001): enqueue")
}

/// The oldest queued (or check_failed and retryable) todo, marked running with a new
/// attempt (`todo_attempt` event, no backup).
pub fn claim_next(ledger: &mut Ledger, model: &str) -> Result<Option<TaskView>> {
    todo!("agent queue (ADR 0001): claim_next")
}

/// Record an attempt's end: its session id, usage and the new status (running →
/// done / check_failed / needs_human).
pub fn finish_attempt(
    ledger: &mut Ledger,
    task_id: &TaskId,
    session_id: &str,
    usage: &Usage,
    status: TaskStatus,
) -> Result<TaskView> {
    todo!("agent queue (ADR 0001): finish_attempt")
}
