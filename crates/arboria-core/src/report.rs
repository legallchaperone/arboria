//! Read-side views: `arb status` (full / `--brief`, also the hook's text), the home's
//! status, and the Markdown export. Ports `arboria.report`.
//!
//! Invariants:
//! - Pure reads; never writes the ledger.
//! - "Needs you" counts use one rule everywhere (the hook, `arb status`, the app's inbox):
//!   pending approvals and [`FINISHED_UNSEEN_SQL`] (runs that finished after the last
//!   human note of their idea).
//! - Text from the ledger is the human's or length-checked LLM text; the brief form is
//!   capped by the hook ([`crate::hook::MAX_CONTEXT_CHARS`]).
//! - v8 adds a line for probes waiting to be re-enabled ("待重新启用", ADR 0003).

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use serde::Serialize;
use serde_json::Value;

use crate::db::Ledger;
use crate::paths::{Home, RepoContext};
use crate::Result;

/// The rules line appended to the brief status (the hook's context).
pub const BRIEF_FOOTER: &str = concat!(
    "Ledger rules: experiment state lives in the arboria ledger, written only by `arb`. ",
    "Work comes from the todos: finish one with `arb todo done tNNN --result \"...\"`; ",
    "for a command todo read `arb commands guide` first, for an inspect todo ",
    "`arb inspect guide` (deliver with `arb inspect done`, not `arb todo done`); ",
    "`notebook.py` is the human's notebook: read it, never edit it; `arb note` is human-only. ",
    "Use `arb status`, `arb run summary RUN \"...\"`, ",
    "`arb submit --reason \"...\" --gpus N --time T [-- CMD]`; never edit .arboria/ by hand. ",
    "Approval happens only in the human's own terminal: you cannot run `arb approve` or ",
    "`arb lesson accept`. Read cluster logs with `arb logs RUN` / `arb remote ls RUN`, not ssh. ",
    "Propose reusable pitfalls with `arb lesson propose \"...\"`."
);
/// Section headers of the todo lists.
pub const TODOS_HEADER: &str = "来自 notebook 的待办：";
/// See [`TODOS_HEADER`].
pub const COMMAND_TODOS_HEADER: &str = "命令待办：";
/// See [`TODOS_HEADER`].
pub const INSPECT_TODOS_HEADER: &str = "读代码待办：";
/// Accepted lessons shown in status / the hook.
pub const STATUS_LESSONS: usize = 10;
/// Shown when something waits for the human.
pub const BOARD_LINE: &str = "看板：arb ui";
/// Count of finished runs of open ideas that ended (latest `run_final` event, else
/// `updated_at`) after the idea's last human note (notes without an author predate v4
/// and were the human's). Python `report.FINISHED_UNSEEN_SQL`, verbatim; the dashboard
/// and the app's inbox use the same rule.
pub const FINISHED_UNSEEN_SQL: &str = concat!(
    "SELECT COUNT(*) AS n FROM runs r JOIN ideas i ON i.id = r.idea_id",
    " WHERE i.status = 'open' AND r.status IN ('succeeded', 'failed', 'cancelled')",
    " AND COALESCE((SELECT MAX(e.ts) FROM events e WHERE e.kind = 'run_final'",
    "               AND e.subject = r.id), r.updated_at)",
    "   > COALESCE((SELECT MAX(n.created_at) FROM notes n WHERE n.idea_id = r.idea_id",
    "               AND (n.author IS NULL OR n.author = 'human')), '')"
);

/// What `arb status --json` prints (Python `status_data`): `scope` = `idea` | `overview`
/// with the keys of that scope, plus lessons, `pending_approvals`, `finished_unseen`.
/// Kept as JSON until the app's views settle its shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusData(pub Value);

/// Runs awaiting the human's approval.
pub fn pending_approvals(ledger: &Ledger) -> Result<u32> {
    todo!("port of arboria.report.pending_approvals")
}

/// See [`FINISHED_UNSEEN_SQL`].
pub fn finished_unseen(ledger: &Ledger) -> Result<u32> {
    todo!("port of arboria.report.finished_unseen")
}

/// `arb status --json`.
pub fn status_data(ctx: &RepoContext, ledger: &Ledger) -> Result<StatusData> {
    todo!("port of arboria.report.status_data")
}

/// `arb status` text (`brief`: the hook's compact form with [`BRIEF_FOOTER`]).
pub fn format_status(data: &StatusData, brief: bool) -> String {
    todo!("port of arboria.report.format_status")
}

/// `arb status` in the seed home.
pub fn home_status(home: &Home, ledger: &Ledger, brief: bool) -> Result<String> {
    todo!("port of arboria.report.home_status")
}

/// `arb export`: Markdown of all ideas, notes and runs, newest first.
pub fn export_markdown(ctx: &RepoContext, ledger: &Ledger) -> Result<String> {
    todo!("port of arboria.report.export_markdown")
}
