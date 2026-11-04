//! `arb machine init` and `arb doctor`: measure a Slurm machine over the user's ssh
//! master and check a profile. Ports `arboria.machine` (the parsers are pure; the probe
//! script runs through [`crate::backend::SshSession`]).
//!
//! Invariants:
//! - Read-only on the cluster: one fixed script per call ([`PROBE_SCRIPT`]), plus
//!   `test -d/-w` on the profile's own paths for doctor. Nothing edits `~/.ssh/config`;
//!   a missing ControlMaster setup is reported with the block to add ([`SSH_BLOCK`]).
//! - `ssh -O check` gates every call; NeedLogin is never retried.
//! - The proposal keeps the user's own choices from an existing profile; `--write
//!   --replace` keeps a backup of `clusters.toml`.
//! - Parsers are tested against the real DeltaAI output
//!   (`tests/fixtures/deltaai_probe.txt` on the Python side).
//! - "probe" here is the machine-measuring script, unrelated to [`crate::probe`].

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::Result;

/// The fixed read-only measuring script (Python `machine.PROBE_SCRIPT`, verbatim).
pub const PROBE_SCRIPT: &str = r#"echo @@user; echo "$USER"
echo @@arch; uname -m
echo @@accounts; accounts 2>&1
echo @@partitions; scontrol show partition -o 2>&1
echo @@node; n=$(sinfo -h -N -o '%N %P' 2>/dev/null | awk '$2 ~ /\*$/ {print $1; exit}'); [ -n "$n" ] && scontrol show node -o "$n" 2>&1
echo @@flags; scontrol show config 2>/dev/null | grep -E '^PriorityFlags'
echo @@quota; quota 2>&1
echo @@tools; for t in sbatch sacct accounts jobcharge apptainer uv pixi; do command -v "$t"; done
echo @@end
"#;

/// The `~/.ssh/config` block suggested when the master setup is missing.
pub const SSH_BLOCK: &str = "Host {host}
    HostName <login node>
    User <user>
    ControlMaster auto
    ControlPath ~/.ssh/cm-%C
    ControlPersist 7d
    ServerAliveInterval 60";

/// `@@name` sections of the probe output.
pub fn split_sections(text: &str) -> BTreeMap<String, String> {
    todo!("port of arboria.machine.split_sections")
}

/// `scontrol show partition -o` → partition name → key/value.
pub fn parse_partitions(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    todo!("port of arboria.machine.parse_partitions")
}

/// `TRESBillingWeights` → weights per TRES (`cpu`, `mem` per GB, `gpu`).
pub fn parse_weights(text: &str) -> BTreeMap<String, f64> {
    todo!("port of arboria.machine.parse_weights")
}

/// `accounts` (NCSA) → `[(account, balance)]` from the rows under the dashed rule. Also
/// the built-in `balance` probe's parser (ADR 0003).
pub fn parse_accounts(text: &str) -> Vec<(String, String)> {
    todo!("port of arboria.machine.parse_accounts")
}

/// Absolute paths in `quota` output.
pub fn parse_quota_paths(text: &str) -> Vec<String> {
    todo!("port of arboria.machine.parse_quota_paths")
}

/// A proposed profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Proposal {
    /// The profile's keys and values (rendered as TOML).
    pub values: serde_json::Map<String, serde_json::Value>,
    pub warnings: Vec<String>,
    pub other_partitions: Vec<String>,
    pub accounts: Vec<(String, String)>,
    pub tools: Vec<String>,
}

/// Profile values from a probe's sections; keeps the user's choices from `existing`.
pub fn propose(
    name: &str,
    host: &str,
    sections: &BTreeMap<String, String>,
    account: Option<&str>,
    existing: Option<&toml::Table>,
) -> Result<Proposal> {
    todo!("port of arboria.machine.propose")
}

/// The `[name]` TOML table text.
pub fn render_profile(name: &str, proposal: &Proposal, note: &str) -> String {
    todo!("port of arboria.machine.render_profile")
}

/// `text` with the `[name]` table (and its subtables) replaced by `new_block`.
pub fn replace_table(text: &str, name: &str, new_block: &str) -> Result<String> {
    todo!("port of arboria.machine.replace_table")
}

/// Options of `arb machine init`.
#[derive(Debug, Clone, Default)]
pub struct InitOptions<'a> {
    pub host: Option<&'a str>,
    pub account: Option<&'a str>,
    pub write: bool,
    pub replace: bool,
}

/// What `arb machine init` did.
#[derive(Debug, Clone, Serialize)]
pub struct InitResult {
    pub name: String,
    pub text: String,
    pub proposal: Proposal,
    pub path: PathBuf,
    pub written: bool,
    pub backup: Option<PathBuf>,
    pub ssh_problems: Vec<String>,
    pub still_invalid: Option<String>,
}

/// `arb machine init NAME`.
pub fn machine_init(name: &str, opts: InitOptions<'_>) -> Result<InitResult> {
    todo!("port of arboria.machine.machine_init")
}

/// One doctor line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// `ok` | `warn` | `fail` | `info`.
    pub level: String,
    pub what: String,
}

/// `arb doctor [NAME]`: read-only checks; stops early (fail) without an ssh master.
pub fn doctor(name: &str, project_id: Option<&crate::model::ProjectId>) -> Result<Vec<Check>> {
    todo!("port of arboria.machine.doctor")
}
