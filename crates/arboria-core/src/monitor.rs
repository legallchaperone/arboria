//! After approval: poll jobs, derive run status and cost, cancel, watch (observation
//! window), logs, remote listings. Ports `arboria.monitor`.
//!
//! Invariants:
//! - Nothing here ever submits; only [`crate::submission::approve`] does.
//! - Job-state and run-status updates are heartbeat-like (`job_state`, `run_status`
//!   events: no backup commit); `run_final`, `run_cancel`, `run_config` do commit.
//! - Status mapping and billing stay in the core (ADR 0003): SU = sacct `billing=N` ×
//!   hours / `billing_per_su`, else the local estimate.
//! - `watch` cancels a run in its observation window on a crash or a log line matching
//!   `[watch] fail_patterns`, and the notification names the pattern and the line (≤200
//!   chars). `Error::NeedLogin` pauses that machine for the pass; never retried.
//! - Remote reads are limited to the project's `runs/` and `out/` dirs (the backend's
//!   [`crate::backend::restrict_path`]); `-n` is 1..=10000; subpaths are plain segments.
//! - `ARBORIA_OUT` is fetched once into `.arboria/runs/<run>/out` (marker `fetched`);
//!   `out/config.yaml` (≤200 KiB) is stored as `runs.config_json`.
//!
//! Porting note: Python reads `config.yaml` with PyYAML; the Rust core needs a YAML crate
//! (or keeps the raw text) — decide when porting [`config_json_from`].

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::backend::Backend;
use crate::db::Ledger;
use crate::model::{JobState, Run, RunId, RunStatus};
use crate::paths::RepoContext;
use crate::Result;

/// `.arboria/notifications.log`.
pub const NOTIFICATIONS: &str = "notifications.log";
/// Log lines read per observation pass.
pub const TAIL_LINES: usize = 50;
/// A matching log line is clipped to this in notifications.
pub const HIT_LINE_MAX: usize = 200;
/// Marker file of a fetched `ARBORIA_OUT`.
pub const FETCHED_MARKER: &str = "fetched";
/// The training code's resolved config, in `ARBORIA_OUT`.
pub const CONFIG_FILE: &str = "config.yaml";
/// Larger configs are not stored.
pub const CONFIG_MAX_BYTES: u64 = 200 * 1024;

/// Append `<now> <message>` to the notifications log; returns the line.
pub fn notify(ctx: &RepoContext, message: &str) -> Result<String> {
    todo!("port of arboria.monitor.notify")
}

/// A backend for the run's machine, bound to the current project.
pub fn backend_for(ctx: &RepoContext, run: &Run) -> Result<Box<dyn Backend>> {
    todo!("port of arboria.monitor.backend_for")
}

/// Run status from its jobs' states in segment order (`n_segments` planned).
pub fn derive_status(states: &[JobState], n_segments: usize, current: RunStatus) -> RunStatus {
    todo!("port of arboria.monitor.derive_status")
}

/// What one poll of one run did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PollOutcome {
    pub run_id: RunId,
    pub status: RunStatus,
    pub previous: RunStatus,
    pub messages: Vec<String>,
}

impl PollOutcome {
    /// The run just reached a final status.
    pub fn became_final(&self) -> bool {
        RunStatus::FINAL.contains(&self.status) && !RunStatus::FINAL.contains(&self.previous)
    }
}

/// `arb poll RUN`: update job states, run status and cost; fetch `ARBORIA_OUT` when done.
pub fn poll_run(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    backend: Option<&dyn Backend>,
) -> Result<PollOutcome> {
    todo!("port of arboria.monitor.poll_run")
}

/// Runs whose jobs may still be on a machine.
pub fn active_runs(ledger: &Ledger) -> Result<Vec<Run>> {
    todo!("port of arboria.monitor.active_runs")
}

/// Copy the run's `ARBORIA_OUT` into `.arboria/runs/<run>/out` (once).
pub fn fetch_out(ctx: &RepoContext, run: &Run, backend: &dyn Backend) -> Result<PathBuf> {
    todo!("port of arboria.monitor.fetch_out")
}

/// `out/config.yaml` as `ledger_json`, if present, small enough and parseable.
pub fn config_json_from(path: &Path) -> Result<Option<String>> {
    todo!("port of arboria.monitor.config_json_from")
}

/// Store the fetched config in `runs.config_json` (`run_config` event).
pub fn store_config(ledger: &mut Ledger, run_id: &RunId, out_dir: &Path) -> Result<bool> {
    todo!("port of arboria.monitor.store_config")
}

/// `arb cancel`: cancel the run's jobs; returns the run and the cancelled job ids.
pub fn cancel(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
) -> Result<(Run, Vec<String>)> {
    todo!("port of arboria.monitor.cancel")
}

/// The first `(pattern, line)` of `text` matching one of `patterns`.
pub fn first_hit(text: &str, patterns: &[String]) -> Result<Option<(String, String)>> {
    todo!("port of arboria.monitor.first_hit")
}

/// One observation pass over a run: cancel on a crash or a failure pattern in its window.
pub fn observe(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    run_id: &RunId,
    backend: &dyn Backend,
    window_minutes: u32,
) -> Result<Vec<String>> {
    todo!("port of arboria.monitor.observe")
}

/// Options of `arb watch`.
#[derive(Debug, Clone, Copy)]
pub struct WatchOptions {
    pub once: bool,
    pub interval_s: u64,
    /// None: `[watch] observation_minutes`.
    pub window_minutes: Option<u32>,
    /// Tests: stop after this many passes.
    pub max_iterations: Option<u32>,
}

/// `arb watch`: poll + observe active runs until none is left (or `once`). Exit status 1
/// when a machine needed a login.
pub fn watch(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    opts: WatchOptions,
    echo: &mut dyn FnMut(&str),
) -> Result<i32> {
    todo!("port of arboria.monitor.watch")
}

/// `arb logs`: the last `n` lines of a segment's log (default: the latest started one).
pub fn logs(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    segment: Option<u32>,
    n: usize,
) -> Result<String> {
    todo!("port of arboria.monitor.logs")
}

/// `arb remote ls`: `(remote dir, entries)` of the run dir or its `ARBORIA_OUT`.
pub fn remote_ls(
    ctx: &RepoContext,
    ledger: &Ledger,
    run_id: &RunId,
    subpath: Option<&str>,
    out: bool,
) -> Result<(String, Vec<String>)> {
    todo!("port of arboria.monitor.remote_ls")
}
