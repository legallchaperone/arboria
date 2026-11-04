//! `arb install-claude`: merge arboria's SessionStart hook and permission rules into a
//! Claude Code `settings.json`. Ports `arboria.install`.
//!
//! Invariants:
//! - Merging is additive and idempotent: existing user settings are kept; a malformed
//!   section is an error naming the file, never overwritten.
//! - Deny rules are an extra layer, not the guarantee (that is the guarded submit and the
//!   human-only checks in code): cluster commands, approvals, the human's files, seed
//!   writes, notebook restyling, and (v8, ADR 0003) `arb probe enable`.
//! - The global commands dir is machine-specific: in a project it goes into the
//!   uncommitted `.claude/settings.local.json` (excluded via `info/exclude`).

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::Result;

/// The hook command.
pub const HOOK_COMMAND: &str = "arb hook session-start";
/// Approvals happen only in the human's own terminal.
pub const APPROVAL_RULES: &[&str] = &["Bash(arb approve:*)", "Bash(arb lesson accept:*)"];
/// Keep the agent off the cluster directly.
pub const CLUSTER_RULES: &[&str] = &[
    "Bash(sbatch:*)",
    "Bash(srun:*)",
    "Bash(salloc:*)",
    "Bash(scancel:*)",
    "Bash(ssh:*)",
    "Bash(scp:*)",
    "Bash(rsync:*)",
];
/// Files only `arb` or the human write.
pub const PROTECTED_PATHS: &[&str] = &[
    "**/.arboria/notebooks/**",
    "**/.arboria/ledger.db*",
    "**/.arboria/backup/**",
    "**/.arboria/*.toml",
    "**/.arboria/sessions/**",
    "**/notebook.py",
];
/// The tools denied on [`PROTECTED_PATHS`].
pub const EDIT_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];
/// Commands that only ever write seeds.
pub const SEED_RULES: &[&str] = &[
    "Bash(arb seed new:*)",
    "Bash(arb seed set:*)",
    "Bash(arb seed drop:*)",
    "Bash(arb seed revive:*)",
    "Bash(arb seed plant:*)",
    "Bash(arb idea set:*)",
    "Bash(arb idea revive:*)",
    "Bash(arb idea plant:*)",
];
/// `arb idea style` rewrites the human's notebooks.
pub const NOTEBOOK_RULES: &[&str] = &["Bash(arb idea style:*)"];
/// v8 (ADR 0003): enabling a probe is the human's Decision.
pub const PROBE_RULES: &[&str] = &["Bash(arb probe enable:*)"];
/// In the seed home, `arb idea new/drop` write seeds too.
pub const HOME_IDEA_RULES: &[&str] = &["Bash(arb idea new:*)", "Bash(arb idea drop:*)"];
/// The seed home's protected paths.
pub const HOME_PROTECTED_PATHS: &[&str] = &["**/seeds/**", "**/.arboria/**"];
/// `.claude/settings.json`.
pub const SETTINGS_NAME: &str = "settings.json";
/// `.claude/settings.local.json` (per machine, not committed).
pub const LOCAL_NAME: &str = "settings.local.json";

/// Every deny rule of a project, in Python's order (`DENY_RULES`) plus [`PROBE_RULES`].
pub fn deny_rules() -> Vec<String> {
    todo!("port of arboria.install.DENY_RULES")
}

/// The seed home's deny rules (`HOME_DENY_RULES`) plus [`PROBE_RULES`].
pub fn home_deny_rules() -> Vec<String> {
    todo!("port of arboria.install.HOME_DENY_RULES")
}

/// Merge the hook, deny rules and (without `local`) the commands dir into `settings`;
/// returns the changes made.
pub fn merge_settings(
    settings: &mut Value,
    where_: &str,
    add_dirs: Option<&[String]>,
    deny: &[String],
) -> Result<Vec<String>> {
    todo!("port of arboria.install.merge_settings")
}

/// Where `arb install-claude` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The current worktree (default).
    Worktree,
    /// `--main`: the main worktree.
    Main,
    /// `--user`: `~/.claude/settings.json`.
    User,
    /// `--home`: the seed home.
    Home,
}

/// Merge into `<root>/.claude/settings.json` (and `settings.local.json` with `local`);
/// returns the settings path and the changes.
pub fn install(
    root: &Path,
    add_dirs: Option<&[String]>,
    local: bool,
    deny: &[String],
) -> Result<(PathBuf, Vec<String>)> {
    todo!("port of arboria.install.install")
}
