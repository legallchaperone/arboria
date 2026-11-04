//! Ideas through their whole lifecycle: seed (no repo yet) → open (branch `idea/<name>`,
//! worktree folder, notebook) → merged / dropped. Ports `arboria.ideas`,
//! `arboria.seeds` (seeds are ideas at stage `seed` since schema v8) and the routing
//! logic of `cli._idea_route`.
//!
//! Invariants:
//! - Routing is fixed by the current directory ([`route`]): in the seed home or outside
//!   any git repo → seeds; in a repo without `arb init` → an error, never a seed by
//!   accident; in an initialized repo → its ideas.
//! - Seeds (gist ≤300, hypothesis, metric) are written only by the human: every
//!   seed-writing function calls [`require_human_for_seeds`], which refuses inside
//!   Claude Code. This is enforced in code, not only by deny rules (`arb idea new/drop`
//!   are allowed for Claude in a repo).
//! - Planting runs in the target repo, only in the human's terminal (it creates a branch
//!   and a worktree), goes through [`new_idea`], and copies only the seed notebook's text
//!   blocks (parsed, never executed).
//! - Closing never loses work: a dropped idea's final state is snapshotted and tagged
//!   before its worktree and branch are removed; a dirty worktree is refused.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::Ledger;
use crate::model::{Confidence, Idea, IdeaName, Planting, Run};
use crate::paths::{Home, RepoContext};
use crate::{Result, Warn};

/// `idea/<name>`.
pub fn branch_for(name: &IdeaName) -> String {
    format!("idea/{name}")
}

/// `<main parent>/<main name>-<name>`: the idea's worktree folder.
pub fn worktree_for(ctx: &RepoContext, name: &IdeaName) -> PathBuf {
    todo!("port of arboria.ideas.worktree_for")
}

// ---------------------------------------------------------------- routing

/// Where `arb idea …` acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The seeds of this home (cwd in the home, or outside any git repo).
    Seeds(Home),
    /// The ideas of this initialized repo.
    Repo(RepoContext),
}

/// `cli._idea_route`: the home for seeds, or the initialized repo; a repo without
/// `arb init` is `Error::NotInitialized`.
pub fn route(cwd: &Path) -> Result<Route> {
    todo!("port of arboria.cli._idea_route")
}

/// The initialized home containing `cwd`, if any (`seeds.home_of`).
pub fn home_of(cwd: Option<&Path>) -> Option<Home> {
    todo!("port of arboria.seeds.home_of")
}

/// Create the home's state dir, ledger and backup repo if missing (`seeds.ensure_home`);
/// `echo` reports what was created.
pub fn ensure_home(echo: Warn<'_>) -> Result<Home> {
    todo!("port of arboria.seeds.ensure_home")
}

/// Refuse inside Claude Code: seeds are the human's (`seeds.require_human`); `what`
/// says what the command would do.
pub fn require_human_for_seeds(command: &str, what: &str) -> Result<()> {
    todo!("port of arboria.seeds.require_human")
}

// ---------------------------------------------------------------- ideas in a repo

/// An `arb idea list` row.
#[derive(Debug, Clone, Serialize)]
pub struct IdeaSummary {
    #[serde(flatten)]
    pub idea: Idea,
    pub parent_name: Option<String>,
    pub run_count: u32,
}

/// `arb idea show` of an idea.
#[derive(Debug, Clone, Serialize)]
pub struct IdeaDetail {
    #[serde(flatten)]
    pub idea: Idea,
    pub parent_name: Option<String>,
    pub notebook: String,
    pub runs: Vec<Run>,
}

/// How an idea was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseMode {
    /// Merge `idea/<name>` into the main worktree's branch.
    Merge,
    /// Archive a final snapshot (tagged) and delete the branch.
    Drop,
}

/// What closing did (`idea["close"]` in Python).
#[derive(Debug, Clone, Serialize)]
pub struct CloseInfo {
    pub merged_into: Option<String>,
    pub final_commit: Option<String>,
    pub tag: Option<String>,
}

/// `arb idea close` / `arb idea drop` of an idea.
#[derive(Debug, Clone, Serialize)]
pub struct Closed {
    #[serde(flatten)]
    pub idea: Idea,
    pub close: CloseInfo,
}

/// An idea by name (`no idea named 'x' (see `arb idea list`)`).
pub fn get_idea(ledger: &Ledger, name: &IdeaName) -> Result<Idea> {
    todo!("port of arboria.ideas.get_idea")
}

/// The idea whose worktree contains the current directory.
pub fn current_idea(ctx: &RepoContext, ledger: &Ledger) -> Result<Option<Idea>> {
    todo!("port of arboria.ideas.current_idea")
}

/// [`current_idea`], refusing when there is none or it is not open.
pub fn require_current_idea(ctx: &RepoContext, ledger: &Ledger) -> Result<Idea> {
    todo!("port of arboria.ideas.require_current_idea")
}

/// What a new idea starts from.
#[derive(Debug, Clone, Default)]
pub struct NewIdea<'a> {
    pub hypothesis: &'a str,
    pub metric: &'a str,
    /// Branch off this open idea.
    pub parent: Option<&'a IdeaName>,
    /// Fork from this run's snapshot (`lab.fork`).
    pub from_run: Option<&'a Run>,
    /// Planting: the seed's row in the home ledger.
    pub seed: Option<&'a Idea>,
}

/// `arb idea new` in a repo: branch `idea/NAME`, worktree, notebook, ledger row.
pub fn new_idea(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: &IdeaName,
    new: NewIdea<'_>,
    warn: Warn<'_>,
) -> Result<Idea> {
    todo!("port of arboria.ideas.new_idea")
}

/// `arb idea notebook`: create the notebook of an idea that has none. Returns
/// `(idea, path, created)`.
pub fn ensure_notebook(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: Option<&IdeaName>,
    warn: Warn<'_>,
) -> Result<(Idea, PathBuf, bool)> {
    todo!("port of arboria.ideas.ensure_notebook")
}

/// What `arb idea style` did to one notebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Restyled {
    Added,
    Already,
    Own,
    Unknown,
}

/// `arb idea style` (human only): give older notebooks the Arboria font.
pub fn restyle(
    ctx: &RepoContext,
    ledger: &Ledger,
    warn: Warn<'_>,
) -> Result<Vec<(PathBuf, Restyled)>> {
    todo!("port of arboria.ideas.restyle")
}

/// `arb idea list`.
pub fn list_ideas(ledger: &Ledger) -> Result<Vec<IdeaSummary>> {
    todo!("port of arboria.ideas.list_ideas")
}

/// `arb idea show` (default: the current directory's idea).
pub fn show_idea(
    ctx: &RepoContext,
    ledger: &Ledger,
    name: Option<&IdeaName>,
) -> Result<IdeaDetail> {
    todo!("port of arboria.ideas.show_idea")
}

/// `arb idea close --merge|--drop` / `arb idea drop`.
pub fn close_idea(
    ctx: &RepoContext,
    ledger: &mut Ledger,
    name: Option<&IdeaName>,
    mode: CloseMode,
    confidence: Option<Confidence>,
    allow_large: bool,
    warn: Warn<'_>,
) -> Result<Closed> {
    todo!("port of arboria.ideas.close_idea")
}

// ---------------------------------------------------------------- seeds (stage = seed)

/// A seed with its notebook, last edit and plantings (`seeds._decorate`).
#[derive(Debug, Clone, Serialize)]
pub struct SeedView {
    #[serde(flatten)]
    pub idea: Idea,
    pub notebook: String,
    /// Later of the ledger's `updated_at` and the notebook's mtime.
    pub last_edited: String,
    pub planted_to: Vec<Planting>,
    /// The notebook file existed and was left as is (new only).
    pub kept_existing_notebook: bool,
}

/// The editable seed fields; `Some("")` clears one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SeedFields<'a> {
    pub gist: Option<&'a str>,
    pub hypothesis: Option<&'a str>,
    pub metric: Option<&'a str>,
}

/// `arb idea new NAME "gist"` outside a repo (human only).
pub fn new_seed(
    home: &Home,
    ledger: &mut Ledger,
    name: &IdeaName,
    fields: SeedFields<'_>,
) -> Result<SeedView> {
    todo!("port of arboria.seeds.new_seed")
}

/// `arb idea list` outside a repo: most recently edited first.
pub fn list_seeds(home: &Home, ledger: &Ledger, include_dropped: bool) -> Result<Vec<SeedView>> {
    todo!("port of arboria.seeds.list_seeds")
}

/// `arb idea show NAME` outside a repo.
pub fn show_seed(home: &Home, ledger: &Ledger, name: &IdeaName) -> Result<SeedView> {
    todo!("port of arboria.seeds.show")
}

/// `arb idea set` (human only).
pub fn set_seed_fields(
    ledger: &mut Ledger,
    name: &IdeaName,
    fields: SeedFields<'_>,
) -> Result<Idea> {
    todo!("port of arboria.seeds.set_fields")
}

/// `arb idea drop NAME` outside a repo (human only; the notebook is kept).
pub fn drop_seed(ledger: &mut Ledger, name: &IdeaName) -> Result<Idea> {
    todo!("port of arboria.seeds.drop")
}

/// `arb idea revive` (human only): dropped seed → seed.
pub fn revive_seed(ledger: &mut Ledger, name: &IdeaName) -> Result<Idea> {
    todo!("port of arboria.seeds.revive")
}

/// Options of `arb idea plant`.
#[derive(Debug, Clone, Default)]
pub struct PlantOptions<'a> {
    /// Idea name in the repo (default: the seed's).
    pub idea_name: Option<&'a IdeaName>,
    pub hypothesis: Option<&'a str>,
    pub metric: Option<&'a str>,
    /// Plant a seed that is already planted.
    pub again: bool,
}

/// `arb idea plant` (human only, in the target repo): the seed becomes an idea there.
pub fn plant(
    home: &Home,
    home_ledger: &mut Ledger,
    name: &IdeaName,
    target: &RepoContext,
    opts: PlantOptions<'_>,
    warn: Warn<'_>,
) -> Result<Idea> {
    todo!("port of arboria.seeds.plant")
}

/// `arb idea style` outside a repo: every seed notebook.
pub fn restyle_seeds(home: &Home) -> Result<Vec<(PathBuf, Restyled)>> {
    todo!("port of arboria.seeds.restyle")
}
