//! The project's `arboria.toml`: project id, default machine, budget, watch patterns,
//! wandb, the run command, Claude settings. Ports `arboria.config`.
//!
//! Invariants:
//! - The project id (`[project] id`) names jobs and remote dirs and goes into the card
//!   hash; it matches [`crate::model::ProjectId`] and is generated once by `arb init`
//!   (`<slug ≤26>-<4 hex>`), never changed silently.
//! - `[watch] fail_patterns` are Python `re` syntax today (e.g. `(?i)\bloss\b…`); the
//!   port must accept the defaults below and refuse what it cannot compile, never skip a
//!   pattern silently.
//! - `[claude] push` is still parsed (old files have it) but has no effect: ADR 0001
//!   removes the session-socket push.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::ProjectId;
use crate::paths::RepoContext;
use crate::Result;

/// `[watch] observation_minutes` default.
pub const DEFAULT_OBSERVATION_MINUTES: u32 = 10;
/// `[watch] fail_patterns` default (Python `re` syntax).
pub const DEFAULT_FAIL_PATTERNS: &[&str] = &[
    r"(?i)\bloss\b[^\n]{0,40}\bnan\b",
    r"(?i)\bnan\b[^\n]{0,40}\bloss\b",
    r"CUDA out of memory",
    r"torch\.OutOfMemoryError",
];
/// `[run] override_style` values.
pub const OVERRIDE_STYLES: &[&str] = &["hydra", "argparse", "tyro"];
/// `[run] command` written by `arb init`.
pub const DEFAULT_RUN_COMMAND: &str = "python train.py";
/// `[run] override_style` default.
pub const DEFAULT_OVERRIDE_STYLE: &str = "hydra";

/// `[watch]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchConfig {
    pub observation_minutes: u32,
    /// Validated pattern sources.
    pub fail_patterns: Vec<String>,
}

/// `[wandb]` ("" = not configured).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WandbConfig {
    pub entity: String,
    pub project: String,
}

impl WandbConfig {
    /// Both entity and project set.
    pub fn configured(&self) -> bool {
        !self.entity.is_empty() && !self.project.is_empty()
    }

    /// `https://wandb.ai/<entity>/<project>/runs/<id>` when configured.
    pub fn run_url(&self, wandb_id: &str) -> Option<String> {
        self.configured().then(|| {
            format!(
                "https://wandb.ai/{}/{}/runs/{wandb_id}",
                self.entity, self.project
            )
        })
    }
}

/// `[run]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    /// `shlex.split` of `[run] command`; empty = not configured.
    pub command: Vec<String>,
    pub override_style: String,
}

/// `[claude]` (parsed for old files; `push` is ignored, see module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeConfig {
    pub push: bool,
}

/// The whole `arboria.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub default_machine: String,
    /// `[budget] total_su`; 0 = no budget set.
    pub total_su: f64,
    /// None = the file predates project ids (`arb init` adds one).
    pub project_id: Option<ProjectId>,
    pub watch: WatchConfig,
    pub wandb: WandbConfig,
    pub run: RunConfig,
    pub claude: ClaudeConfig,
}

/// Parse a TOML file (`what` names it in errors).
pub fn load_toml(path: &Path, what: &str) -> Result<toml::Table> {
    todo!("port of arboria.config.load_toml")
}

/// Validate a project id from a file or the command line.
pub fn validate_project_id(value: &toml::Value, where_: &str) -> Result<ProjectId> {
    todo!("port of arboria.config.validate_project_id")
}

/// A new project id from the main worktree's name plus 4 hex chars.
pub fn derive_project_id(ctx: &RepoContext) -> ProjectId {
    todo!("port of arboria.config.derive_project_id")
}

/// Read `arboria.toml` (defaults when absent).
pub fn project_config(ctx: &RepoContext) -> Result<ProjectConfig> {
    todo!("port of arboria.config.project_config")
}

/// The project id, refusing a file without one (`run arb init`).
pub fn require_project_id(ctx: &RepoContext) -> Result<ProjectId> {
    todo!("port of arboria.config.require_project_id")
}

/// The `[project]` table text for a new id.
pub fn project_table(project_id: &ProjectId) -> String {
    todo!("port of arboria.config.project_table")
}

/// `text` with a `[project] id` added (comments and layout kept).
pub fn add_project_id(text: &str, project_id: &ProjectId) -> Result<String> {
    todo!("port of arboria.config.add_project_id")
}
