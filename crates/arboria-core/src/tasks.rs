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
//! - Live owner (Python `tasks.LIVE`): stage `open` or `seed`. A seed row is one without a
//!   branch (stage `seed`, or `dropped` before planting); its todos carry the owner's name
//!   as `seed`, an idea's as `idea`.
//! - Retries: at most [`MAX_ATTEMPTS`] background attempts (the first, one `--resume`, one
//!   with a bigger model); a failed check after the last one is `needs_human`. `done` and
//!   `drop` work from every state that is not final.
//! - Only the user's own logged-in `claude` is used; the core never handles an OAuth
//!   token (ADR 0001).

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::db::{self, now, Ledger};
use crate::model::{EventKind, Idea, IdeaId, IdeaStage, Task, TaskId, TaskKind, TaskStatus};
use crate::records::{py_repr, require_text, validate_text, TextField, AUTHOR_HUMAN};
use crate::{bail, Error, Result};

/// `arb todo done` on an inspect todo.
pub const INSPECT_DONE: &str = "{id} 是读代码待办：用 `arb inspect done {id} --file …` 交付答案（代码由 arb 从 git 取），约定见 `arb inspect guide`；做不了就 `arb todo drop {id}`";

/// Background attempts per todo: the first, one `--resume`, one with a bigger model.
pub const MAX_ATTEMPTS: u32 = 3;

/// `SELECT t.*` with the owner's name as `idea` or `seed`.
const SELECT: &str = "SELECT t.*, CASE WHEN i.branch IS NULL THEN NULL ELSE i.name END AS idea, \
     CASE WHEN i.branch IS NULL THEN i.name END AS seed \
     FROM tasks t LEFT JOIN ideas i ON i.id = t.idea_id";
/// The todo's owner is an open idea or a live seed.
const LIVE: &str = "i.stage IN ('open', 'seed')";

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

impl db::LedgerRow for TaskView {
    fn from_row(row: &rusqlite::Row<'_>) -> Result<Self> {
        let mut map = db::row_map(row)?;
        let mut name = |k: &str| match map.remove(k) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        };
        let (idea, seed) = (name("idea"), name("seed"));
        Ok(TaskView {
            task: db::from_map(map)?,
            idea,
            seed,
        })
    }
}

/// A seed row (no branch: live or dropped before planting) rather than an idea.
pub fn is_seed(owner: &Idea) -> bool {
    owner.stage == IdeaStage::Seed || owner.branch.is_none()
}

fn get_task_in(conn: &rusqlite::Connection, task_id: &TaskId) -> Result<TaskView> {
    db::one(conn, &format!("{SELECT} WHERE t.id = ?1"), [task_id])?.ok_or_else(|| {
        Error::Arb(format!(
            "no todo {} (see `arb todo list`)",
            py_repr(task_id.as_str())
        ))
    })
}

/// A todo by id (`no todo 't009' (see `arb todo list`)`).
pub fn get_task(ledger: &Ledger, task_id: &TaskId) -> Result<TaskView> {
    get_task_in(ledger.conn(), task_id)
}

/// `payload_json` decoded (`{}` for plain todos).
pub fn payload(task: &Task) -> Result<Value> {
    match task.payload_json.as_deref() {
        Some(raw) if !raw.is_empty() => Ok(serde_json::from_str(raw)?),
        _ => Ok(Value::Object(Map::new())),
    }
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
    let text = require_text(TextField::Todo, text)?;
    let seed = is_seed(owner);
    if seed && owner.stage == IdeaStage::Dropped {
        bail!("seed {} is dropped", py_repr(owner.name.as_str()));
    }
    if !seed && owner.stage != IdeaStage::Open {
        bail!("idea {} is {}", py_repr(owner.name.as_str()), owner.stage);
    }
    let raw = data.map(crate::canonical::ledger_json);
    let task_id = ledger.write(|tx| {
        let task_id: TaskId = tx.next_id()?;
        tx.execute(
            "INSERT INTO tasks (id, idea_id, text, status, created_at, kind, payload_json, \
             block_id) VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7)",
            rusqlite::params![
                task_id,
                owner.id.0,
                text,
                now(),
                kind.as_str(),
                raw,
                block_id
            ],
        )?;
        let mut event = Map::new();
        event.insert(
            (if seed { "seed" } else { "idea" }).into(),
            owner.name.as_str().into(),
        );
        event.insert("author".into(), AUTHOR_HUMAN.into());
        event.insert("kind".into(), kind.as_str().into());
        tx.event(EventKind::TodoNew, task_id.as_str(), &Value::Object(event))?;
        Ok(task_id)
    })?;
    get_task(ledger, &task_id)
}

/// The oldest open plain todo of a live owner with exactly this text (re-running a block
/// does not duplicate it).
pub fn open_plain_task(ledger: &Ledger, owner: IdeaId, text: &str) -> Result<Option<TaskView>> {
    db::one(
        ledger.conn(),
        &format!(
            "{SELECT} WHERE t.idea_id = ?1 AND {LIVE} AND t.status = 'open' \
             AND t.kind = 'plain' AND t.text = ?2 ORDER BY t.id LIMIT 1"
        ),
        rusqlite::params![owner.0, crate::records::py_strip(text)],
    )
}

/// Open command todos about command `name`, oldest first.
pub fn open_command_tasks(
    ledger: &Ledger,
    name: &str,
    kind: Option<TaskKind>,
) -> Result<Vec<TaskView>> {
    let kinds: Vec<&str> = match kind {
        Some(k) => vec![k.as_str()],
        None => vec![
            TaskKind::CommandNew.as_str(),
            TaskKind::CommandRevise.as_str(),
        ],
    };
    let marks = vec!["?"; kinds.len()].join(", ");
    let rows: Vec<TaskView> = db::all(
        ledger.conn(),
        &format!(
            "{SELECT} WHERE t.status = 'open' AND {LIVE} AND t.kind IN ({marks}) ORDER BY t.id"
        ),
        rusqlite::params_from_iter(kinds),
    )?;
    let mut out = Vec::new();
    for r in rows {
        if payload(&r.task)?.get("name").and_then(Value::as_str) == Some(name) {
            out.push(r);
        }
    }
    Ok(out)
}

/// `arb todo list`: oldest first; open only unless `include_closed`; all live owners when
/// `owner` is None.
pub fn list_tasks(
    ledger: &Ledger,
    owner: Option<IdeaId>,
    include_closed: bool,
) -> Result<Vec<TaskView>> {
    let mut sql = format!("{SELECT} WHERE ");
    sql.push_str(if owner.is_some() {
        "t.idea_id = ?1"
    } else {
        LIVE
    });
    if !include_closed {
        sql.push_str(" AND t.status = 'open'");
    }
    sql.push_str(" ORDER BY t.id");
    match owner {
        Some(id) => db::all(ledger.conn(), &sql, [id.0]),
        None => db::all(ledger.conn(), &sql, []),
    }
}

fn is_final(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Done | TaskStatus::Dropped)
}

fn close(
    ledger: &mut Ledger,
    task_id: &TaskId,
    status: TaskStatus,
    result: Option<String>,
) -> Result<TaskView> {
    let task = get_task(ledger, task_id)?;
    if is_final(task.task.status) {
        bail!("todo {task_id} is already {}", task.task.status);
    }
    ledger.write(|tx| {
        tx.execute(
            "UPDATE tasks SET status = ?1, done_at = ?2, result = ?3 WHERE id = ?4",
            rusqlite::params![status.as_str(), now(), result, task_id],
        )?;
        let kind = if status == TaskStatus::Done {
            EventKind::TodoDone
        } else {
            EventKind::TodoDrop
        };
        let mut event = Map::new();
        match &task.seed {
            Some(s) => event.insert("seed".into(), s.as_str().into()),
            None => event.insert("idea".into(), json!(task.idea)),
        };
        event.insert(
            "result_chars".into(),
            result.as_ref().map_or(0, |r| r.chars().count()).into(),
        );
        tx.event(kind, task_id.as_str(), &Value::Object(event))
    })?;
    get_task(ledger, task_id)
}

/// `arb todo done` (result ≤600; refuses inspect todos).
pub fn done(ledger: &mut Ledger, task_id: &TaskId, result: &str) -> Result<TaskView> {
    if get_task(ledger, task_id)?.task.kind == TaskKind::Inspect {
        return Err(Error::Arb(INSPECT_DONE.replace("{id}", task_id.as_str())));
    }
    let result = require_text(TextField::Result, result)?;
    close(ledger, task_id, TaskStatus::Done, Some(result))
}

/// `arb todo drop` (reason ≤600, optional).
pub fn drop_task(ledger: &mut Ledger, task_id: &TaskId, reason: Option<&str>) -> Result<TaskView> {
    let reason = validate_text(TextField::Result, reason, false)?;
    close(ledger, task_id, TaskStatus::Dropped, reason)
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

impl Usage {
    fn add(&self, other: &Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens + other.input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            cache_read_tokens: self.cache_read_tokens + other.cache_read_tokens,
            turns: self.turns + other.turns,
            cost_usd: self.cost_usd + other.cost_usd,
        }
    }
}

/// open → queued (`todo_queue` event).
pub fn enqueue(ledger: &mut Ledger, task_id: &TaskId) -> Result<TaskView> {
    let task = get_task(ledger, task_id)?;
    if task.task.status != TaskStatus::Open {
        bail!("todo {task_id} is {}, not open", task.task.status);
    }
    ledger.write(|tx| {
        tx.execute(
            "UPDATE tasks SET status = 'queued' WHERE id = ?1",
            [task_id],
        )?;
        tx.event(
            EventKind::TodoQueue,
            task_id.as_str(),
            &json!({"kind": task.task.kind.as_str()}),
        )
    })?;
    get_task(ledger, task_id)
}

/// The oldest queued (or check_failed and retryable) todo, marked running with a new
/// attempt (`todo_attempt` event, no backup).
pub fn claim_next(ledger: &mut Ledger, model: &str) -> Result<Option<TaskView>> {
    let claimed = ledger.write(|tx| {
        let next: Option<TaskView> = db::one(
            tx,
            &format!(
                "{SELECT} WHERE {LIVE} AND (t.status = 'queued' OR \
                 (t.status = 'check_failed' AND t.attempts < ?1)) ORDER BY t.id LIMIT 1"
            ),
            [MAX_ATTEMPTS],
        )?;
        let Some(next) = next else {
            return Ok(None);
        };
        let attempt = next.task.attempts + 1;
        tx.execute(
            "UPDATE tasks SET status = 'running', attempts = ?1, model = ?2, started_at = ?3, \
             finished_at = NULL WHERE id = ?4",
            rusqlite::params![attempt, model, now(), next.task.id],
        )?;
        tx.event(
            EventKind::TodoAttempt,
            next.task.id.as_str(),
            &json!({"attempt": attempt, "model": model, "action": "start"}),
        )?;
        Ok(Some(next.task.id))
    })?;
    claimed.map(|id| get_task(ledger, &id)).transpose()
}

/// Record an attempt's end: its session id, usage and the new status (running →
/// check_failed / needs_human). A todo the agent already closed during the attempt
/// (`arb todo done` / `drop`, `arb inspect done`) keeps its status; `done` itself only
/// comes from those commands, which check the result.
pub fn finish_attempt(
    ledger: &mut Ledger,
    task_id: &TaskId,
    session_id: &str,
    usage: &Usage,
    status: TaskStatus,
) -> Result<TaskView> {
    let task = get_task(ledger, task_id)?.task;
    let closed = is_final(task.status);
    if !closed && task.status != TaskStatus::Running {
        bail!("todo {task_id} is {}, not running", task.status);
    }
    if !closed && !matches!(status, TaskStatus::CheckFailed | TaskStatus::NeedsHuman) {
        bail!(
            "todo {task_id} has no result: the agent closes it with `arb todo done` (or `arb inspect done`)"
        );
    }
    let new_status = if closed {
        task.status
    } else if status == TaskStatus::CheckFailed && task.attempts >= MAX_ATTEMPTS {
        TaskStatus::NeedsHuman
    } else {
        status
    };
    let total = match task.usage_json.as_deref() {
        Some(raw) if !raw.is_empty() => serde_json::from_str::<Usage>(raw)?.add(usage),
        _ => usage.clone(),
    };
    ledger.write(|tx| {
        tx.execute(
            "UPDATE tasks SET status = ?1, session_id = ?2, usage_json = ?3, finished_at = ?4 \
             WHERE id = ?5",
            rusqlite::params![
                new_status.as_str(),
                session_id,
                crate::canonical::to_ledger_json(&total)?,
                now(),
                task_id
            ],
        )?;
        tx.event(
            EventKind::TodoAttempt,
            task_id.as_str(),
            &json!({"attempt": task.attempts, "action": "finish", "status": new_status.as_str()}),
        )?;
        if new_status == TaskStatus::NeedsHuman {
            tx.event(
                EventKind::TodoNeedsHuman,
                task_id.as_str(),
                &json!({"attempts": task.attempts}),
            )?;
        }
        Ok(())
    })?;
    get_task(ledger, task_id)
}
