//! `arb submit` / `arb approve` / `arb reject`: the human approval gate in front of every
//! machine. Ports `arboria.submission`.
//!
//! Invariants:
//! - [`submit`] is purely local: snapshot (or the `--from` run's), request, card, an open
//!   Decision. It never builds a backend. An identical usable run (same card hash in the
//!   same idea) is returned with `existed = true` and nothing is written, unless
//!   `--repeat` (a random nonce goes into the request and so into the card hash).
//! - [`approve`] is the only path to [`crate::backend::Backend::submit`]: the backend
//!   method needs a [`SubmitPermit`], whose fields are private to this module, and the
//!   only constructor (`guarded_permit`) re-reads the ledger for the run's human
//!   `approve` Decision right before every submit (Python `_guarded_submit`). There is no
//!   automatic approval anywhere.
//! - The approval binds to the card hash: a changed request or profile is refused
//!   ([`crate::cards::CARD_CHANGED`]). Approving happens only in the human's terminal
//!   ([`crate::human::require_human`]) or, later, the app's Touch ID panel.
//! - Resumable: segments already submitted (job id recorded) are never submitted again;
//!   an existing job with the same name is adopted. A per-run lock
//!   (`.arboria/locks/<run>.lock`) is held across remote calls; the ledger lock never is.
//! - Runs of another project id, or from before project ids, are refused.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::PathBuf;

use serde::Serialize;

use crate::backend::Backend;
use crate::cards::Card;
use crate::db::Ledger;
use crate::model::{Decision, DecisionId, Idea, Job, ProjectId, Run, RunId};
use crate::paths::RepoContext;
use crate::slurm::Plan;
use crate::{Result, Warn};

/// The approval Decision's question.
pub const QUESTION: &str = "批准这个 run 上集群吗？";
/// Its options.
pub const OPTIONS: &[&str] = &["approve", "reject", "revise"];
/// `arb submit` returned an existing run.
pub const EXISTS: &str = "已存在：{run}";
/// No command after `--` and none in `arboria.toml`.
pub const NO_COMMAND: &str =
    "no command: put it after `--` (arb submit ... -- python train.py) or set [run] command in arboria.toml";

/// Proof that the ledger holds a human approval of this run, checked just now. Only this
/// module can make one; [`Backend::submit`] requires it.
#[derive(Debug)]
pub struct SubmitPermit<'a> {
    run_id: &'a RunId,
    decision_id: DecisionId,
    card_hash: String,
}

impl SubmitPermit<'_> {
    /// The approved run.
    pub fn run_id(&self) -> &RunId {
        self.run_id
    }
    /// The human's approve Decision.
    pub fn decision_id(&self) -> &DecisionId {
        &self.decision_id
    }
    /// The card hash that Decision approved.
    pub fn card_hash(&self) -> &str {
        &self.card_hash
    }
}

/// Re-read the ledger: the run is `approved` and has a human `approve` Decision. The only
/// constructor of [`SubmitPermit`] (Python `_guarded_submit`'s check).
#[allow(dead_code)] // called by submit_segments once it is ported
fn guarded_permit<'a>(ledger: &Ledger, run_id: &'a RunId) -> Result<SubmitPermit<'a>> {
    todo!("port of arboria.submission._guarded_submit")
}

/// The run's latest human `approve` Decision.
pub fn approval_decision(ledger: &Ledger, run_id: &RunId) -> Result<Option<Decision>> {
    todo!("port of arboria.submission.approval_decision")
}

/// The run's jobs, by segment.
pub fn jobs_of(ledger: &Ledger, run_id: &RunId) -> Result<Vec<Job>> {
    todo!("port of arboria.submission.jobs_of")
}

/// The current project id, refusing runs submitted under another one (or none).
pub fn check_same_project(ctx: &RepoContext, run: &Run) -> Result<ProjectId> {
    todo!("port of arboria.submission.check_same_project")
}

/// `arb card`: the card recomputed now, and whether it still matches the stored hash.
pub fn card_for(ctx: &RepoContext, ledger: &Ledger, run_id: &RunId) -> Result<(Card, bool)> {
    todo!("port of arboria.submission.card_for")
}

/// How the run's command was built (stored in `request_json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub command: Vec<String>,
    pub base: Vec<String>,
    /// `explicit` (`-- CMD`) | `config` (`[run] command`).
    pub source: String,
    pub overrides: Vec<(String, String)>,
    pub style: String,
}

/// The command: `-- CMD`, else the parent's base or `[run] command`, plus overrides (the
/// parent's, updated by the new ones) rendered in the configured style.
pub fn build_command(
    cfg: &crate::config::ProjectConfig,
    explicit: Option<&[String]>,
    overrides: &[(String, String)],
    parent_request: Option<&serde_json::Value>,
) -> Result<CommandSpec> {
    todo!("port of arboria.submission.build_command")
}

/// What `arb submit` was asked (CLI flags / `lab.run` arguments).
#[derive(Debug, Clone, Default)]
pub struct SubmitArgs<'a> {
    pub reason: &'a str,
    /// The command after `--` (None: `[run] command` or the parent's).
    pub command: Option<&'a [String]>,
    pub gpus: Option<u32>,
    /// Walltime text (`12h`).
    pub time: Option<&'a str>,
    pub nodes: Option<u32>,
    pub partition: Option<&'a str>,
    pub machine: Option<&'a str>,
    pub mem_gb: Option<u32>,
    pub cpus: Option<u32>,
    /// `--from rNNN`: reuse that run's snapshot, resources and overrides.
    pub from_run: Option<&'a RunId>,
    /// `--set k=v`, parsed.
    pub overrides: Vec<(String, String)>,
    pub allow_large: bool,
    pub repeat: bool,
    /// The notebook block that asked (v8 `runs.block_id`).
    pub block_id: Option<&'a str>,
}

/// What [`submit`] returns (`--json`: the run row plus these keys).
#[derive(Debug, Clone, Serialize)]
pub struct Submitted {
    #[serde(flatten)]
    pub run: Run,
    /// An identical earlier run was returned; nothing was written.
    pub existed: bool,
    pub decision_id: Option<DecisionId>,
    pub request: serde_json::Value,
    #[serde(skip)]
    pub card: Card,
}

/// `arb submit`: snapshot + request + card + open Decision (see module docs).
pub fn submit(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    idea: &Idea,
    args: SubmitArgs<'_>,
    warn: Warn<'_>,
) -> Result<Submitted> {
    todo!("port of arboria.submission.submit")
}

/// One submitted segment: `(segment, job id, adopted an existing job)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmittedSegment {
    pub segment: u32,
    pub job_id: String,
    pub adopted: bool,
}

/// What [`approve`] did.
#[derive(Debug, Clone, Serialize)]
pub struct ApproveResult {
    pub run: Run,
    /// None when resuming an earlier approval.
    pub decision_id: Option<DecisionId>,
    pub submitted: Vec<SubmittedSegment>,
}

/// Ship the code once, then submit each missing segment idempotently (each through a
/// fresh `guarded_permit`).
pub fn submit_segments(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run: &Run,
    plan: &Plan,
    backend: &dyn Backend,
) -> Result<Vec<SubmittedSegment>> {
    todo!("port of arboria.submission.submit_segments")
}

/// `arb approve` (human only): show the card (`show`), the human types the run id, record
/// the Decision bound to the card hash, then submit. Re-run to resume.
pub fn approve(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    show: &mut dyn FnMut(&str),
) -> Result<ApproveResult> {
    todo!("port of arboria.submission.approve")
}

/// `arb reject`: decide the open Decision as reject (reason saved as a note). No backend.
pub fn reject(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    reason: Option<&str>,
) -> Result<Run> {
    todo!("port of arboria.submission.reject")
}

/// `.arboria/runs/<run>`: where `ARBORIA_OUT` is fetched to.
pub fn local_run_dir(ctx: &RepoContext, run_id: &RunId) -> PathBuf {
    ctx.state_dir().join("runs").join(run_id.as_str())
}
