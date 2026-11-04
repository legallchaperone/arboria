//! `arb init`: create the state dir, ledger, backup repo and `arboria.toml` template.
//! Ports `arboria.state`.
//!
//! Invariants:
//! - Idempotent: a second run changes nothing and says "already initialized".
//! - `.arboria/` is excluded through `<common dir>/info/exclude`, never `.gitignore`;
//!   `arboria.toml` is the user's to commit.
//! - The project id is generated once (`<slug ≤26>-<4 hex>`) and only added to a file
//!   without one, keeping its comments and layout ([`crate::config::add_project_id`]).

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use crate::model::ProjectId;
use crate::paths::RepoContext;
use crate::{Result, Warn};

/// What `arb init` prints after a first init.
pub const NEXT_STEPS: &str = "账本现在是空的，所以 arb ui 里还什么都没有。下一步：";

/// The `arboria.toml` written by `arb init` for a new project id.
pub fn config_template(project_id: &ProjectId) -> String {
    todo!("port of arboria.state.config_template")
}

/// Add a project id to an `arboria.toml` that lacks one; returns it when added.
pub fn ensure_project_id(ctx: &RepoContext) -> Result<Option<ProjectId>> {
    todo!("port of arboria.state.ensure_project_id")
}

/// `arb init`: the actions taken (empty: already initialized).
pub fn init_state(ctx: &RepoContext, warn: Warn<'_>) -> Result<Vec<String>> {
    todo!("port of arboria.state.init_state")
}
