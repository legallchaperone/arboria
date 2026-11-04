//! Pure Slurm logic: walltime, DeltaAI billing, segment plans, command overrides, job
//! names, remote paths, sbatch scripts. Ports `arboria.slurm` (the card hash moved to
//! [`crate::cards`]).
//!
//! Invariants:
//! - Nothing here touches the ledger, the file system or the network.
//! - Byte-compatible with Python (ADR 0001; test vectors `slurm_*`): walltime parsing,
//!   `slurm_time`, `human_duration`, segments, billed GPUs, the request dict, job names
//!   `arb-<project>-<run>-s<k>` (≤64 chars), remote paths, override rendering, and the
//!   sbatch text (including `shlex.quote` / `shlex.join` quoting and `$USER` → `%u` in
//!   `#SBATCH` lines).
//! - `est_su` is Python's `round(x, 2)`: round-half-even on the exact binary value — not
//!   `(x * 100).round() / 100`.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clusters::Profile;
use crate::model::{IdeaName, ProjectId, RunId};
use crate::Result;

/// `--time` must be at least this long.
pub const MIN_WALLTIME_S: u64 = 60;
/// Slurm job names longer than this are refused.
pub const MAX_JOB_NAME: usize = 64;

// ---------------------------------------------------------------- walltime

/// Seconds from `90m`, `12h`, `1d6h`, `45s`, `HH:MM:SS` or `D-HH:MM:SS`.
pub fn parse_walltime(text: &str) -> Result<u64> {
    todo!("port of arboria.slurm.parse_walltime")
}

/// `--time` value: `HH:MM:SS` (hours may exceed 24).
pub fn slurm_time(seconds: u64) -> String {
    todo!("port of arboria.slurm.slurm_time")
}

/// Compact form for cards: `1d6h`, `12h`, `1h30m`, `90s`, `0s`.
pub fn human_duration(seconds: u64) -> String {
    todo!("port of arboria.slurm.human_duration")
}

/// Split a walltime longer than the partition limit into chained segments.
pub fn segment_plan(total_s: u64, max_s: u64, max_segments: u32) -> Result<Vec<u64>> {
    todo!("port of arboria.slurm.segment_plan")
}

// ---------------------------------------------------------------- billing

/// DeltaAI bills the max of GPUs, memory and CPUs, each in GPU shares (ceil with 1e-9 slack).
pub fn billed_gpus(
    gpus: u32,
    mem_gb: f64,
    cpus: u32,
    mem_per_gpu_gb: f64,
    cpus_per_gpu: u32,
) -> u32 {
    todo!("port of arboria.slurm.billed_gpus")
}

// ---------------------------------------------------------------- request plan

/// What the submitter asked for (stored as is in `request_json` `ask`; the plan is
/// re-derived from it). Serializes like Python `dataclasses.asdict(Ask)`, `null`s included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub command: Vec<String>,
    pub gpus: u32,
    pub nodes: u32,
    pub walltime_s: u64,
    pub partition: String,
    pub machine: String,
    /// Per node; None = gpus × mem_per_gpu_gb.
    pub mem_gb: Option<u32>,
    /// Per node; None = gpus × cpus_per_gpu.
    pub cpus: Option<u32>,
    /// `--repeat` nonce (goes into the card hash).
    pub repeat: Option<String>,
}

/// `Plan.env`: what the job's environment is built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEnv {
    pub arch: String,
    pub prelude: Vec<String>,
    pub env_activate: Vec<String>,
    pub paths: BTreeMap<String, String>,
}

/// A validated request against a profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub ask: Ask,
    pub project: ProjectId,
    pub account: String,
    pub partition: String,
    pub charge: f64,
    pub max_walltime_s: u64,
    /// Requested per node.
    pub gpus_per_node: u32,
    pub cpus_per_node: u32,
    pub mem_gb_per_node: u32,
    pub billed_per_node: u32,
    /// Total over nodes.
    pub billed_gpus: u32,
    pub segments: Vec<u64>,
    pub env: PlanEnv,
}

impl Plan {
    /// Billed more GPUs than requested (memory / CPUs over the share).
    pub fn over_billed(&self) -> bool {
        self.billed_gpus > self.ask.gpus
    }

    /// Upper bound in SU: `round(Σ billed × s / 3600 × charge, 2)` (see module docs).
    pub fn est_su(&self) -> f64 {
        todo!("port of arboria.slurm.Plan.est_su")
    }

    /// What is stored in `runs.request_json` (Python `Plan.request()`; key set and value
    /// types are part of the contract).
    pub fn request(&self) -> serde_json::Value {
        todo!("port of arboria.slurm.Plan.request")
    }
}

/// Validate `ask` against the profile; refuse anything that cannot be billed right.
pub fn build_plan(profile: &Profile, ask: &Ask, project: &ProjectId) -> Result<Plan> {
    todo!("port of arboria.slurm.build_plan")
}

// ---------------------------------------------------------------- command overrides

/// An override value before stringification (Python accepted str / int / float / bool / None).
#[derive(Debug, Clone, PartialEq)]
pub enum OverrideValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

/// Validate one `k=v` override; bools become `true` / `false`, None `null`, floats
/// Python `str(float)`.
pub fn check_override(key: &str, value: &OverrideValue) -> Result<(String, String)> {
    todo!("port of arboria.slurm.check_override")
}

/// `k=v` from `--set`.
pub fn parse_override(text: &str) -> Result<(String, String)> {
    todo!("port of arboria.slurm.parse_override")
}

/// `base` with `new` applied: same key replaced in place, new keys appended.
pub fn merge_overrides(
    base: &[(String, String)],
    new: &[(String, String)],
) -> Vec<(String, String)> {
    todo!("port of arboria.slurm.merge_overrides")
}

/// hydra → `k=v`; argparse / tyro → `--k v`.
pub fn render_overrides(style: &str, overrides: &[(String, String)]) -> Result<Vec<String>> {
    todo!("port of arboria.slurm.render_overrides")
}

// ---------------------------------------------------------------- names and paths

/// `arb-<project>-<run>-s<k>`.
pub fn job_name(project: &ProjectId, run_id: &RunId, segment: u32) -> Result<String> {
    todo!("port of arboria.slurm.job_name")
}

/// `(project, run id, segment)` of an arboria job name.
pub fn parse_job_name(name: &str) -> Result<(ProjectId, RunId, u32)> {
    todo!("port of arboria.slurm.parse_job_name")
}

/// The project of a job name, or None (e.g. a pre-project `arboria-r001-s1`).
pub fn job_project(name: &str) -> Option<ProjectId> {
    todo!("port of arboria.slurm.job_project")
}

/// `<runs>/<project>/<run>`.
pub fn run_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    todo!("port of arboria.slurm.run_dir")
}

/// `<run dir>/code`.
pub fn code_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    todo!("port of arboria.slurm.code_dir")
}

/// `<run dir>/sbatch/s<k>.sh`.
pub fn script_path(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
) -> String {
    todo!("port of arboria.slurm.script_path")
}

/// `<run dir>/logs/s<k>.%j.out` (`$USER` stays a shell variable).
pub fn log_pattern(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
) -> String {
    todo!("port of arboria.slurm.log_pattern")
}

/// [`log_pattern`] with `%j` = `job_id`.
pub fn log_path(
    paths: &BTreeMap<String, String>,
    project: &ProjectId,
    run_id: &RunId,
    segment: u32,
    job_id: &str,
) -> String {
    todo!("port of arboria.slurm.log_path")
}

/// `<out>/<project>/<run>` (`ARBORIA_OUT`).
pub fn out_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    todo!("port of arboria.slurm.out_dir")
}

/// `<ckpt>/<project>/<run>` (`ARBORIA_CKPT_DIR`).
pub fn ckpt_dir(paths: &BTreeMap<String, String>, project: &ProjectId, run_id: &RunId) -> String {
    todo!("port of arboria.slurm.ckpt_dir")
}

/// `<project>-<run>` (`ARBORIA_WANDB_ID`; the run id is not validated, as in Python).
pub fn wandb_id(project: &ProjectId, run_id: &str) -> String {
    format!("{project}-{run_id}")
}

/// `#SBATCH` lines are not shell-expanded: `$USER` / `${USER}` → `%u`.
pub fn sbatch_value(path: &str) -> String {
    todo!("port of arboria.slurm.sbatch_value")
}

/// Double-quote a validated profile path so `$USER` still expands; refuse `"` `\` `` ` `` `$(`.
pub fn dq(path: &str) -> Result<String> {
    todo!("port of arboria.slurm.dq")
}

/// Python `shlex.quote`.
pub fn shlex_quote(s: &str) -> String {
    todo!("port of shlex.quote (used by arboria.slurm.render_sbatch)")
}

/// The batch script of one segment (dependencies go to sbatch, not into the script).
pub fn render_sbatch(plan: &Plan, run_id: &RunId, idea: &IdeaName, segment: u32) -> Result<String> {
    todo!("port of arboria.slurm.render_sbatch")
}
