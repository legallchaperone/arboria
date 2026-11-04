//! 读代码 (`lab.inspect`): the agent explains code for the human; the answer lives in the
//! notebook and is cached. Ports `arboria.inspection` (the ledger, git and validation
//! side; rendering moves to the app, the kernel side stays Python).
//!
//! Invariants:
//! - An answer is bound to the code it describes: a run's snapshot commit, or the idea's
//!   worktree as it is now (uncommitted and untracked files included) snapshotted
//!   read-only with [`crate::snapshot::write_tree`] (a throw-away index; never the user's
//!   branch, index or files). The commit is made only when a todo is created, anchored at
//!   `refs/arboria/inspect/<todo>`. A changed tree shows the answer as stale ("代码已变");
//!   nothing is regenerated automatically; `refresh` asks again only when it changed (or
//!   the last todo was dropped).
//! - The agent gives only locations and notes. **Source lines are never written by an
//!   LLM**: [`deliver`] extracts them with `git cat-file blob <code_ref>:<path>` (bytes,
//!   `\r` kept) and stores them in `result_json`.
//! - Limits ([`validate_result`]): title ≤80, summary and span notes ≤600, line notes
//!   ≤200 (≤40), pseudocode / simplified code ≤8000, Mermaid ≤8000, ≤6 spans, ≤400
//!   source lines, files ≤2 MiB. Mermaid: the first line is a diagram type; no
//!   `%%{init}%%`, no `click`, no links, no HTML except `<br>`. Display always escapes.
//! - `arb todo done` refuses inspect todos; `inspect_done` triggers a backup commit.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::db::Ledger;
use crate::model::{Idea, InspectView, Inspection, RunId, TaskId};
use crate::tasks::TaskView;
use crate::Result;

/// `refs/arboria/inspect/<todo>`.
pub const REF_PREFIX: &str = "refs/arboria/inspect/";
/// Spans per answer.
pub const MAX_SPANS: usize = 6;
/// Source lines over all spans.
pub const MAX_LINES: usize = 400;
/// Line notes per answer.
pub const MAX_LINE_NOTES: usize = 40;
/// Nodes of a map.
pub const MAX_NODES: usize = 30;
/// Files larger than this cannot be quoted.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Mermaid diagram types allowed on the first line (Python `MERMAID_KINDS`).
pub const MERMAID_KINDS: &[&str] = &[
    "flowchart",
    "graph",
    "sequenceDiagram",
    "classDiagram",
    "classDiagram-v2",
    "stateDiagram",
    "stateDiagram-v2",
    "erDiagram",
    "journey",
    "gantt",
    "pie",
    "mindmap",
    "timeline",
    "block-beta",
    "architecture-beta",
    "C4Context",
    "C4Container",
    "C4Component",
    "quadrantChart",
    "requirementDiagram",
    "gitGraph",
    "sankey-beta",
    "xychart-beta",
    "packet-beta",
];

/// The view's label (`代码摘录`, `伪代码`, `简化代码`, `架构图`).
pub fn view_label(view: InspectView) -> &'static str {
    match view {
        InspectView::Code => "代码摘录",
        InspectView::Pseudo => "伪代码",
        InspectView::Simple => "简化代码",
        InspectView::Map => "架构图",
    }
}

/// `refs/arboria/inspect/<todo>`.
pub fn ref_name(task_id: &TaskId) -> String {
    format!("{REF_PREFIX}{task_id}")
}

/// The code a question is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Code {
    /// `idea` or a run id.
    pub target: String,
    pub tree: String,
    /// None: the idea's worktree, no commit made yet.
    pub commit: Option<String>,
    pub head: Option<String>,
}

/// What `lab.inspect` shows.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub question: String,
    pub view: InspectView,
    pub code: Code,
    pub answer: Option<Inspection>,
    /// The open (pending) or dropped todo shown.
    pub todo: Option<TaskView>,
    pub created: bool,
}

/// `fresh` | `stale` | `pending` | `dropped`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InspectStatus {
    Fresh,
    Stale,
    Pending,
    Dropped,
}

impl State {
    /// See [`InspectStatus`].
    pub fn status(&self) -> InspectStatus {
        todo!("port of arboria.inspection.State.status")
    }
}

/// What `lab.inspect` is about right now (read-only: no commit, no ref).
pub fn current_code(ledger: &Ledger, idea: &Idea, run: Option<&RunId>) -> Result<Code> {
    todo!("port of arboria.inspection.current_code")
}

/// Look up the answer, or ask the agent (an `inspect` todo); `create = false` never writes.
pub fn ask(
    ledger: &mut Ledger,
    idea: &Idea,
    question: &str,
    view: InspectView,
    run: Option<&RunId>,
    refresh: bool,
    create: bool,
) -> Result<State> {
    todo!("port of arboria.inspection.ask")
}

/// The files at the todo's `code_ref` (`arb inspect source TASK`).
pub fn list_files(repo: &Path, commit: &str) -> Result<Vec<String>> {
    todo!("port of arboria.inspection.list_files")
}

/// One file's lines at `commit`, as bytes decoded (refuses binary and > 2 MiB).
pub fn blob_lines(repo: &Path, commit: &str, path: &str) -> Result<Vec<String>> {
    todo!("port of arboria.inspection.blob_lines")
}

/// `arb inspect source TASK PATH`: the file with line numbers.
pub fn source_text(
    ledger: &Ledger,
    repo: &Path,
    task_id: &TaskId,
    path: Option<&str>,
) -> Result<String> {
    todo!("port of arboria.inspection source (cli.inspect_source)")
}

/// Validate a Mermaid source (see module docs).
pub fn check_mermaid(src: &str) -> Result<String> {
    todo!("port of arboria.inspection.check_mermaid")
}

/// Validate an answer against the todo and extract the source lines from git; returns
/// what will be stored as `result_json`.
pub fn validate_result(repo: &Path, task: &TaskView, data: &Value) -> Result<Value> {
    todo!("port of arboria.inspection.validate_result")
}

/// Parse `--file` contents (JSON).
pub fn load_answer(text: &str) -> Result<Value> {
    todo!("port of arboria.inspection.load_answer")
}

/// `arb inspect check`: validate and render as text; saves nothing.
pub fn check(ledger: &Ledger, repo: &Path, task_id: &TaskId, data: &Value) -> Result<String> {
    todo!("port of arboria.inspection check (cli.inspect_check)")
}

/// `arb inspect done`: validate, store the `inspections` row, close the todo
/// (`inspect_done` event).
pub fn deliver(
    ledger: &mut Ledger,
    repo: &Path,
    task_id: &TaskId,
    data: &Value,
) -> Result<Inspection> {
    todo!("port of arboria.inspection.deliver")
}

/// Plain-text rendering (`arb inspect check`, the app's fallback).
pub fn to_text(state: Option<&State>, result: Option<&Value>) -> String {
    todo!("port of arboria.inspection.to_text")
}

/// `arb inspect guide`: the answer conventions (for the agent).
pub fn guide() -> &'static str {
    todo!("port of arboria.inspection.guide")
}
