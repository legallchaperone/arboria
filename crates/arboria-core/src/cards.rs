//! The approval card: what the human reads before approving a run, and the hash the
//! approval binds to. Ports `arboria.cards` and `slurm.card_payload` / `card_hash`.
//!
//! Invariants:
//! - Pure reads (ledger, `arboria.toml`, the profile); no writes, no backend. The
//!   read-only dashboard / app shows the same card text.
//! - [`card_hash`] = sha256 hex of [`crate::canonical::canonical_json`] of
//!   [`card_payload`]; byte-compatible with Python (test vector `card_hash`). `repeat` is
//!   present only when set, so hashes from before `--repeat` stay valid.
//! - A changed request or profile changes the hash; approval of a stale card is refused
//!   (`审核单已变化，请重新 arb submit`).
//!
//! Porting note: sha256 needs a crate (RustCrypto `sha2`); add it pinned like the others.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use serde::Serialize;
use serde_json::Value;

use crate::clusters::Profile;
use crate::model::{Idea, IdeaId, ProjectId, Run};
use crate::paths::RepoContext;
use crate::slurm::Plan;
use crate::Result;

/// Shown when the recomputed hash differs from the stored one.
pub const CARD_CHANGED: &str = "审核单已变化，请重新 arb submit";
/// A run submitted before project ids existed.
pub const LEGACY_RUN: &str = "run {run} 是在加入项目 id 之前提交的（request 里没有 project），作业名和远程目录可能与其他仓库冲突；请重新 arb submit";

/// Everything an approval is bound to (Python `slurm.card_payload`).
pub fn card_payload(snapshot_commit: &str, plan: &Plan) -> Value {
    todo!("port of arboria.slurm.card_payload")
}

/// sha256 hex of the canonical payload (Python `slurm.card_hash`).
pub fn card_hash(snapshot_commit: &str, plan: &Plan) -> String {
    todo!("port of arboria.slurm.card_hash")
}

/// `runs.request_json` decoded (refuses runs not made by `arb submit`).
pub fn request_of(run: &Run) -> Result<Value> {
    todo!("port of arboria.cards.request_of")
}

/// The project id the run was submitted under (None before project ids).
pub fn run_project(run: &Run) -> Result<Option<ProjectId>> {
    todo!("port of arboria.cards.run_project")
}

/// Re-derive the plan from the stored request and the *current* profile.
pub fn replan(run: &Run) -> Result<(Profile, Plan)> {
    todo!("port of arboria.cards.replan")
}

/// SU billed so far (project, or one idea), rounded to 2 places.
pub fn su_used(conn: &rusqlite::Connection, idea_id: Option<IdeaId>) -> Result<f64> {
    todo!("port of arboria.cards.su_used")
}

/// One card line with its check mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub ok: bool,
    pub text: String,
}

/// The card (Python `cards.Card`; `--json` shape is `as_dict`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Card {
    #[serde(rename = "run")]
    pub run_id: String,
    pub idea: String,
    pub checks: Vec<Check>,
    pub resources: Vec<String>,
    pub estimate: Vec<String>,
    pub budget: Vec<String>,
    pub card_hash: String,
    pub est_su: f64,
}

impl Card {
    /// The text the human reads (`审核单 r001（idea …）` …).
    pub fn text(&self) -> String {
        todo!("port of arboria.cards.Card.text")
    }
}

/// Build the card for a run (`warnings` = snapshot warnings at submit time).
pub fn build_card(
    ctx: &RepoContext,
    conn: &rusqlite::Connection,
    run: &Run,
    idea: &Idea,
    profile: &Profile,
    plan: &Plan,
    warnings: &[String],
) -> Result<Card> {
    todo!("port of arboria.cards.build_card")
}
