//! Notebook commands: small analysis functions the agent writes and the human calls from a
//! notebook (`lab.<name>(...)`, `/name args`). Ports `arboria.commands` (the file side;
//! loading and running a command stays in the human's Python kernel).
//!
//! Invariants:
//! - **The core never executes command code.** Listing and describing read the file
//!   (Python: `ast`); only the human's kernel imports and runs a command.
//! - Two scopes: project `<state dir>/commands/<name>.py`, global
//!   `$ARBORIA_CONFIG_DIR/commands/<name>.py`; the project file wins on a name clash.
//!   Names match `^[a-z][a-z0-9_]{0,40}$` and must not shadow a built-in (`lab.run`, …).
//! - A call to a missing command becomes a todo (`command_new` / `command_revise`, the
//!   call in `payload_json`, written by code); the same call reuses the open todo.
//! - Project commands are versioned by the backup repo (`git -C .arboria log -p
//!   commands/`); `promote` copies one into the global library (`command_promote`
//!   event, backup commit) and refuses to replace a different global file unless `force`.
//!
//! Porting note (awkward): [`describe`] needs a Python parser (signature, defaults,
//! docstring). Options: run `python -I` with an `ast` script from the project
//! environment (as the probe host does), or parse with a vendored grammar
//! (tree-sitter-python). Never import the file.

#![allow(unused_variables)] // phase-1 stubs; remove when the module is ported

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::Ledger;
use crate::model::Idea;
use crate::tasks::TaskView;
use crate::Result;

/// `commands/` under the state dir or the config dir.
pub const COMMANDS_DIR: &str = "commands";
/// Keyword arguments `lab` consumes itself (`scope=`, `ask="…"`).
pub const RESERVED_KWARGS: &[&str] = &["scope", "ask"];
/// A long default is clipped to this many characters in signatures.
pub const DEFAULT_CLIP: usize = 24;

/// The functions `lab` itself offers, in `lab.commands()` order, with their one line.
pub const BUILTINS: &[(&str, &str)] = &[
    ("runs", "这个 idea 的 run 一览"),
    ("get", "按 id 取一个 run"),
    ("compare", "几个 run 的 wandb 曲线（长表）"),
    ("plot", "几个 run 的一条 wandb 曲线画在一起"),
    ("diff", "两个 run 哪里不同：命令、资源、代码、config"),
    ("spend", "各 idea 用了多少 SU 和 GPU 时；给 idea 就逐个 run"),
    ("run", "生成快照和审核单（不批准）"),
    ("fork", "从这个 idea 分出新 idea"),
    (
        "inspect",
        "请 agent 讲一段代码：代码摘录加批注、伪代码、简化代码或架构图",
    ),
    ("todo", "给 agent 留一条待办"),
    ("todos", "待办列表"),
    ("conf", "改置信度"),
    ("metric", "改指标"),
    ("prune", "放弃这个 idea"),
];

/// Where a command file lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This project (`项目`).
    Project,
    /// The global library (`全局`).
    Global,
}

/// Validate a command name (`check_reserved`: also refuse built-in names).
pub fn validate_name(name: &str, check_reserved: bool) -> Result<()> {
    todo!("port of arboria.commands.validate_name")
}

/// `$ARBORIA_CONFIG_DIR/commands` (default `~/.config/arboria/commands`).
pub fn global_dir() -> PathBuf {
    todo!("port of arboria.commands.global_dir")
}

/// `<state dir>/commands`.
pub fn project_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(COMMANDS_DIR)
}

/// A command file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandFile {
    pub name: String,
    pub scope: Scope,
    pub path: PathBuf,
    /// sha256 hex of the bytes; the version shown is its first 8 characters.
    pub sha256: String,
}

/// The command `name` as the notebook sees it: project first, then global.
pub fn find(name: &str, state_dir: Option<&Path>) -> Result<Option<CommandFile>> {
    todo!("port of arboria.commands.find")
}

/// One parameter after `ctx`, read from the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Param {
    pub name: String,
    /// positional | vararg | keyword | varkw
    pub kind: String,
    /// Source text of the default.
    pub default: Option<String>,
}

/// What the source says about a command, without running it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Described {
    pub description: Option<String>,
    pub problem: Option<String>,
    /// None when there is no matching `def`.
    pub params: Option<Vec<Param>>,
}

/// Read a command's signature and docstring (see the porting note).
pub fn describe(path: &Path, name: &str) -> Result<Described> {
    todo!("port of arboria.commands.describe")
}

/// `name(a, b=1, *, k=…)` clipped to `limit` characters.
pub fn format_signature(name: &str, params: &[Param], limit: Option<usize>) -> String {
    todo!("port of arboria.commands.format_signature")
}

/// An `arb commands list` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandRow {
    pub name: String,
    pub scope: Scope,
    pub version: String,
    pub path: String,
    pub signature: String,
    pub description: String,
    pub problem: Option<String>,
    /// The one `lab.<name>` uses (a project file shadows a global one).
    pub active: bool,
}

/// `arb commands list`: every command file of both scopes, by name.
pub fn discover(state_dir: Option<&Path>) -> Result<Vec<CommandRow>> {
    todo!("port of arboria.commands.discover")
}

/// `arb commands promote`: copy a project command into the global library.
pub fn promote(ledger: &mut Ledger, name: &str, force: bool) -> Result<CommandRow> {
    todo!("port of arboria.commands.promote")
}

/// Record (or reuse) the todo for a missing or to-be-revised command call.
pub fn request(
    ledger: &mut Ledger,
    idea: &Idea,
    kind: crate::model::TaskKind,
    name: &str,
    scope: Scope,
    description: Option<&str>,
    call: &str,
) -> Result<TaskView> {
    todo!("port of arboria.commands.request")
}

/// `arb commands guide`: the conventions for writing a command (for the agent).
pub fn guide() -> &'static str {
    todo!("port of arboria.commands.guide")
}
