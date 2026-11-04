//! Lessons: durable project pitfalls. The agent proposes them; only the human accepts
//! them. Accepted lessons go into the managed CLAUDE.md block ([`crate::claudemd`]) and
//! the SessionStart hook. Ports `arboria.lessons`.
//!
//! Invariants:
//! - A lesson is one line ≤200 code points without `<!--` / `-->` (LLM-writable,
//!   [`crate::records::validate_text`]).
//! - `accept` only in the human's terminal ([`crate::human::require_human`]: refused in
//!   Claude Code or without a TTY, then the human types the id); install-claude denies it.
//! - At most [`crate::claudemd::MAX_ACCEPTED_LESSONS`] accepted lessons and a block of at
//!   most [`crate::claudemd::MAX_BLOCK_LINES`] lines: a lesson that does not fit is
//!   refused with [`CAP_HINT`].
//! - Every change emits its `lesson_*` event (backup commit) and syncs CLAUDE.md; a failed
//!   sync only warns.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use serde::Serialize;

use crate::claudemd::SyncResult;
use crate::db::Ledger;
use crate::model::{Lesson, LessonId, RunId};
use crate::paths::RepoContext;
use crate::Result;

/// The lesson Decision's question.
pub const QUESTION: &str = "接受这条经验吗？";
/// Its options.
pub const OPTIONS: &[&str] = &["accept", "reject"];
/// When the block is full.
pub const CAP_HINT: &str = "retire or merge some first (`arb lesson retire ID`)";

/// What a lesson command did.
#[derive(Debug, Clone, Serialize)]
pub struct LessonResult {
    pub lesson: Lesson,
    pub sync: Option<SyncResult>,
    pub warning: Option<String>,
}

/// A lesson by id.
pub fn get_lesson(ledger: &Ledger, lesson_id: &LessonId) -> Result<Lesson> {
    todo!("port of arboria.lessons.get_lesson")
}

/// `arb lesson list`: proposed + accepted (all with `include_retired`), newest first.
pub fn list_lessons(ledger: &Ledger, include_retired: bool) -> Result<Vec<Lesson>> {
    todo!("port of arboria.lessons.list_lessons")
}

/// `arb lesson propose TEXT [--idea] [--run]`.
pub fn propose(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    text: &str,
    idea: Option<&str>,
    run: Option<&RunId>,
) -> Result<LessonResult> {
    todo!("port of arboria.lessons.propose")
}

/// `arb lesson accept` (human only).
pub fn accept(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
    show: &mut dyn FnMut(&str),
) -> Result<LessonResult> {
    todo!("port of arboria.lessons.accept")
}

/// `arb lesson reject`.
pub fn reject(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
) -> Result<LessonResult> {
    todo!("port of arboria.lessons.reject")
}

/// `arb lesson retire`: an accepted lesson leaves CLAUDE.md.
pub fn retire(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    lesson_id: &LessonId,
) -> Result<LessonResult> {
    todo!("port of arboria.lessons.retire")
}
