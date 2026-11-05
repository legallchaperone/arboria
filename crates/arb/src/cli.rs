//! The `arb` command tree. Mirrors the Python CLI (`src/arboria/cli.py`) exactly: command
//! names, positional arguments, flags, defaults, hidden commands (`arb run new`, the
//! `arb seed …` alias) and the help panels of `cli.HELP_PANELS`
//! (`tests/cli_tree.rs` compares the tree with `tests/python_commands.txt`, generated from
//! the Python CLI). Differences on purpose:
//! - one global `--json` instead of a `--json` per command (commands without JSON output
//!   ignore it);
//! - the hidden `arb probe enable|run|check|guide` of ADR 0003.
//!
//! Every subcommand dispatches to one core function and prints what the Python CLI prints
//! (`tests/cli_parity.rs` runs both on the same steps); commands whose core module is not
//! ported yet (the rest of `arb probe`) still reach `todo!()`; `arb ui` runs the Python
//! dashboard until the app replaces it.
//! Exit status: 0, or 1 with `error: <message>` on stderr (Python `handle_errors`);
//! `arb hook session-start` always exits 0 silently.

// The help texts are the Python CLI's verbatim (`lab.<name>(...)`, `<home>`), not HTML.
#![allow(rustdoc::invalid_html_tags)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{bail, Context as _};
use arboria_core::model::{Confidence, IdeaName, LessonId, RunId, TaskId};
use arboria_core::paths::RepoContext;
use arboria_core::{claudemd, report, slurm, state, submission, tasks, ui};
use arboria_core::{
    commands, db, hook, human, ideas, inspection, install, lessons, machine, monitor, paths, probe,
    records,
};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use serde::Serialize;

/// `arb --help` panels (Python `cli.HELP_PANELS`): every visible top-level command is in
/// exactly one; the order is the help's order.
pub const HELP_PANELS: &[(&str, &[&str])] = &[
    (
        "你做的",
        &[
            "idea", "submit", "card", "approve", "reject", "note", "status", "ui",
        ],
    ),
    (
        "集群",
        &[
            "watch", "poll", "cancel", "logs", "remote", "machine", "doctor",
        ],
    ),
    (
        "agent 做的",
        &["todo", "commands", "inspect", "run", "lesson"],
    ),
    (
        "安装与内部",
        &["init", "install-claude", "hook", "claudemd", "export"],
    ),
];

/// `arb note` inside Claude Code.
pub const NOTE_REFUSED: &str = "arb note 只由人来写。Claude 的结论请写进 `arb todo done tNNN --result \"...\"`（做完一个待办时）或 `arb run summary RUN \"...\"`（一个 run 的结果）。";
/// `arb idea set|revive` inside a repo.
pub const SEED_ONLY: &str = "arb idea {verb} 只用于种子（还没有仓库的 idea），这里是仓库：请在仓库外运行，或者用 arb seed {verb} {name}";
/// After `arb idea new` outside a repo.
pub const SEED_RECORDED: &str =
    "不在仓库里，所以记成了种子：{path}；以后在仓库里用 arb idea plant {name} 把它种下";
/// Printed after closing an idea.
pub const IDEA_CLOSE_HINT: &str = "如果这次有可复用的教训，用 arb lesson propose \"...\" 提出。";

#[derive(Debug, Parser)]
#[command(
    name = "arb",
    about = "Arboria: local ledger of ML research ideas, runs and notes.",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    // ------------------------------------------------ 你做的
    /// Research ideas: in a repo, a branch + worktree each; outside any repo, seeds (ideas with no repo yet, in ~/arboria).
    #[command(subcommand, arg_required_else_help = true)]
    Idea(IdeaCmd),
    /// Ask the human to approve a cluster run (local only: snapshot + approval card).
    ///
    /// Snapshots the idea worktree (or reuses --from's). An identical earlier run is
    /// returned instead of a new one, unless --repeat.
    Submit(SubmitArgs),
    /// Show a submitted run's approval card, recomputed now (local only).
    Card {
        /// Run id, e.g. r001.
        run_id: String,
    },
    /// (Human only, in your own terminal) approve a run's card and submit it to the
    /// cluster; you type the run id to confirm. Re-run to resume.
    Approve {
        /// Run id, e.g. r001.
        run_id: String,
    },
    /// Reject a run awaiting approval (nothing is sent to the cluster).
    Reject {
        /// Run id, e.g. r001.
        run_id: String,
        /// Why (saved as a note, ≤1000 chars).
        #[arg(long)]
        reason: Option<String>,
    },
    /// (Human only) add a note to the current idea (--confidence also updates the idea).
    Note {
        /// The note (≤1000 chars).
        text: String,
        /// Attach to this run id.
        #[arg(long)]
        run: Option<String>,
        /// guess | prelim | confirmed | rejected
        #[arg(long)]
        confidence: Option<String>,
    },
    /// Where things stand: the current idea, the open ideas, or the seeds.
    ///
    /// Inside an idea worktree, that idea; elsewhere in a repo, an overview of its open
    /// ideas; in the seed home, the seeds.
    Status {
        /// Compact text (as used by the hook).
        #[arg(long)]
        brief: bool,
    },
    /// Open the read-only dashboard (看板).
    ///
    /// In a repo: what needs you, the idea tree, SU budget; outside any repo (or with
    /// --home), the seed home (种子园). Runs `marimo run` on 127.0.0.1; it reads the ledger and never changes anything.
    Ui {
        /// Idea to select (default: the idea of the current worktree, else the most recently active open one).
        #[arg(long)]
        idea: Option<String>,
        /// Port (default: marimo picks a free one).
        #[arg(long)]
        port: Option<u16>,
        /// The seed home (种子园) instead; the default outside any repo.
        #[arg(long)]
        home: bool,
    },

    // ------------------------------------------------ 集群
    /// Keep polling active runs; cancel a run that crashes or logs a failure early on.
    ///
    /// In the observation window a run is cancelled on a crash or a log line matching
    /// watch.fail_patterns in arboria.toml (default: loss NaN, CUDA OOM).
    Watch {
        /// One pass, then exit.
        #[arg(long)]
        once: bool,
        /// Seconds between passes.
        #[arg(long, default_value_t = 60)]
        interval: u64,
        /// Observation window after a job starts, in minutes (default: watch.observation_minutes in arboria.toml).
        #[arg(long)]
        window: Option<u32>,
    },
    /// One pass: update job states, run status and cost; fetch ARBORIA_OUT when done.
    Poll {
        /// Default: every active run.
        run_id: Option<String>,
    },
    /// Cancel a submitted run's jobs on the cluster.
    Cancel {
        /// Run id, e.g. r001.
        run_id: String,
    },
    /// Tail a run's Slurm log on the cluster (use this instead of ssh).
    Logs {
        /// Run id, e.g. r001.
        run_id: String,
        /// Default: the latest started segment.
        #[arg(long)]
        segment: Option<u32>,
        /// Number of lines.
        #[arg(short = 'n', default_value_t = 100)]
        n: usize,
    },
    /// Read-only views of a run's files on the cluster.
    #[command(subcommand, arg_required_else_help = true)]
    Remote(RemoteCmd),
    /// Cluster machine profiles (~/.config/arboria/clusters.toml).
    #[command(subcommand, arg_required_else_help = true)]
    Machine(MachineCmd),
    /// Read-only checks of a machine profile, the ssh setup and the cluster side.
    Doctor {
        /// Machine (default: arboria.toml default_machine).
        name: Option<String>,
    },

    // ------------------------------------------------ agent 做的
    /// Todos from the idea notebooks (written by you, done by the agent).
    #[command(subcommand, arg_required_else_help = true)]
    Todo(TodoCmd),
    /// Notebook commands (written by the agent, called as lab.<name>(...)).
    #[command(subcommand, arg_required_else_help = true)]
    Commands(CommandsCmd),
    /// 读代码 todos from lab.inspect(...): the agent gives locations and notes, arb extracts the code from git.
    #[command(subcommand, arg_required_else_help = true)]
    Inspect(InspectCmd),
    /// Runs: the result summary of a finished run.
    #[command(subcommand, arg_required_else_help = true)]
    Run(RunCmd),
    /// Reusable project lessons (proposed by the agent, accepted by you).
    #[command(subcommand, arg_required_else_help = true)]
    Lesson(LessonCmd),

    // ------------------------------------------------ 安装与内部
    /// Set up arboria for this repository (idempotent).
    Init,
    /// Add the SessionStart hook and permission rules to .claude/settings.json (the
    /// machine-specific global commands dir goes into .claude/settings.local.json).
    InstallClaude {
        /// Write into the main worktree instead.
        #[arg(long)]
        main: bool,
        /// Write into ~/.claude/settings.json instead.
        #[arg(long)]
        user: bool,
        /// Set up the seed home (~/arboria): its .claude/settings.json and CLAUDE.md.
        #[arg(long)]
        home: bool,
    },
    /// Claude Code hooks.
    #[command(subcommand, arg_required_else_help = true)]
    Hook(HookCmd),
    /// The arb-managed block in the main CLAUDE.md.
    #[command(subcommand, arg_required_else_help = true)]
    Claudemd(ClaudemdCmd),
    /// Markdown of all ideas, notes and runs (newest first).
    Export {
        /// Write to FILE instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },

    // ------------------------------------------------ hidden
    /// Alias of the seed side of `arb idea` (works from any directory): seeds (种子) are ideas without a repo yet, kept in ~/arboria (or $ARBORIA_HOME).
    #[command(subcommand, hide = true, arg_required_else_help = true)]
    Seed(SeedCmd),
    /// Probes (ADR 0003): small parsers for a machine's query commands, run on a schedule.
    #[command(subcommand, hide = true, arg_required_else_help = true)]
    Probe(ProbeCmd),
}

#[derive(Debug, Args)]
pub struct SubmitArgs {
    /// The training command, after `--` (default: run.command in arboria.toml, or the --from run's command).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
    /// What changed vs. the previous run and why (≤300 chars).
    #[arg(long)]
    pub reason: String,
    /// Reuse this run's snapshot, resources and overrides (no new snapshot).
    #[arg(long = "from")]
    pub from_run: Option<String>,
    /// Override key=value (repeatable), rendered per run.override_style.
    #[arg(long)]
    pub set: Vec<String>,
    /// Total GPUs (default with --from: the parent's).
    #[arg(long)]
    pub gpus: Option<u32>,
    /// Walltime: 90m, 12h, 1d6h or HH:MM:SS (default with --from: the parent's).
    #[arg(long)]
    pub time: Option<String>,
    /// Nodes (GPUs are split evenly; default 1).
    #[arg(long)]
    pub nodes: Option<u32>,
    /// Default: the machine's default_partition.
    #[arg(long)]
    pub partition: Option<String>,
    /// Default: arboria.toml default_machine.
    #[arg(long)]
    pub machine: Option<String>,
    /// Memory per node in GB (default: GPU share; more is billed as extra GPUs).
    #[arg(long)]
    pub mem: Option<u32>,
    /// CPUs per node (default: GPU share; more is billed as extra GPUs).
    #[arg(long)]
    pub cpus: Option<u32>,
    /// Snapshot large untracked files anyway.
    #[arg(long)]
    pub allow_large: bool,
    /// Always a new run, even if identical to an earlier one (e.g. seed variance).
    #[arg(long)]
    pub repeat: bool,
}

#[derive(Debug, Subcommand)]
pub enum IdeaCmd {
    /// New idea: in a repo, branch idea/NAME and its worktree folder (prints its path).
    ///
    /// Outside any repo: (human only) a seed in ~/arboria and its notebook
    /// seeds/NAME.py, to plant into a repo later with `arb idea plant NAME`.
    New {
        /// Slug: lowercase letters, digits, '-'.
        name: String,
        /// Seeds only: the idea in one line (≤300 chars).
        gist: Option<String>,
        /// What you expect and why (≤300 chars); required in a repo.
        #[arg(long)]
        hypothesis: Option<String>,
        /// How success is measured (≤200 chars); required in a repo.
        #[arg(long)]
        metric: Option<String>,
        /// Branch off this open idea (in a repo).
        #[arg(long)]
        parent: Option<String>,
    },
    /// List all ideas; outside any repo, the seeds (most recently edited first).
    List {
        /// Seeds: include dropped ones.
        #[arg(long = "all")]
        show_all: bool,
    },
    /// One idea, or (outside any repo) one seed.
    ///
    /// An idea: hypothesis, metric, status, branch, notebook and runs. A seed: gist,
    /// hypothesis, metric, status, notebook, where it was planted.
    Show {
        /// Default (in a repo): the idea of the current directory.
        name: Option<String>,
    },
    /// (Human only) change a seed's gist, hypothesis or metric (outside any repo).
    Set(SeedSetArgs),
    /// Drop an idea (= `close --drop`), or (outside any repo) a seed.
    ///
    /// An idea: archive a final snapshot, remove its worktree and branch. A seed (human
    /// only): give up on it; its notebook is kept.
    Drop {
        /// Default (in a repo): the idea of the current directory.
        name: Option<String>,
        /// guess | prelim | confirmed | rejected
        #[arg(long)]
        confidence: Option<String>,
        /// Snapshot large untracked files anyway.
        #[arg(long)]
        allow_large: bool,
    },
    /// (Human only) bring a dropped seed back to thinking (outside any repo).
    Revive {
        /// Seed name.
        name: String,
    },
    /// (Human only) plant a seed into this repo as a new idea, whose notebook links back.
    ///
    /// Run it inside the target repo (or pass --repo).
    Plant(PlantArgs),
    /// Create the notebook (and notebook.py link) of an idea that has none.
    Notebook {
        /// Defaults to the idea of the current directory.
        name: Option<String>,
    },
    /// Give older notebooks the Arboria font (Maple Mono CN).
    ///
    /// In a repo: every idea notebook; outside any repo: every seed notebook. Refreshes
    /// the arboria.css next to them and adds css_file="arboria.css" to marimo.App(...)
    /// where it is missing; no cell is touched. Idempotent. Yours only: refused inside
    /// Claude Code (it writes the notebooks).
    Style,
    /// Close an idea with --merge or --drop; removes its worktree and branch.
    Close {
        /// Defaults to the idea of the current directory.
        name: Option<String>,
        /// Merge idea/NAME into main's branch.
        #[arg(long)]
        merge: bool,
        /// Archive a final snapshot and delete the branch.
        #[arg(long)]
        drop: bool,
        /// guess | prelim | confirmed | rejected
        #[arg(long)]
        confidence: Option<String>,
        /// Snapshot large untracked files anyway.
        #[arg(long)]
        allow_large: bool,
    },
}

#[derive(Debug, Args)]
pub struct SeedSetArgs {
    /// Seed name.
    pub name: String,
    /// One line (≤300 chars); "" clears it.
    #[arg(long)]
    pub gist: Option<String>,
    /// ≤300 chars; "" clears it.
    #[arg(long)]
    pub hypothesis: Option<String>,
    /// ≤200 chars; "" clears it.
    #[arg(long)]
    pub metric: Option<String>,
}

#[derive(Debug, Args)]
pub struct PlantArgs {
    /// Seed name.
    pub name: String,
    /// Target repo (default: the repo of the current directory); must be arb-initialized.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Idea name (default: the seed's name).
    #[arg(long = "as")]
    pub as_: Option<String>,
    /// Default: the seed's.
    #[arg(long)]
    pub hypothesis: Option<String>,
    /// Default: the seed's.
    #[arg(long)]
    pub metric: Option<String>,
    /// Plant a seed that is already planted (into another repo, or under another name).
    #[arg(long)]
    pub again: bool,
}

/// `arb seed …`: the seed side of `arb idea`, from any directory (hidden alias).
#[derive(Debug, Subcommand)]
pub enum SeedCmd {
    /// (Human only) new seed and its notebook <home>/seeds/NAME.py (prints its path).
    New {
        /// Slug: lowercase letters, digits, '-'.
        name: String,
        /// The idea in one line (≤300 chars).
        gist: Option<String>,
        /// What you expect and why (≤300 chars).
        #[arg(long)]
        hypothesis: Option<String>,
        /// How success would be measured (≤200 chars).
        #[arg(long)]
        metric: Option<String>,
    },
    /// Seeds, most recently edited first (dropped ones only with --all).
    List {
        /// Include dropped seeds.
        #[arg(long = "all")]
        show_all: bool,
    },
    /// One seed: gist, hypothesis, metric, status, notebook, where it was planted.
    Show {
        /// Seed name.
        name: String,
    },
    /// (Human only) change a seed's gist, hypothesis or metric.
    Set(SeedSetArgs),
    /// (Human only) give up on a seed; its notebook is kept.
    Drop {
        /// Seed name.
        name: String,
    },
    /// (Human only) bring a dropped seed back to thinking.
    Revive {
        /// Seed name.
        name: String,
    },
    /// (Human only) plant a seed into a repo as a new idea, whose notebook links back.
    Plant(PlantArgs),
}

#[derive(Debug, Subcommand)]
pub enum RemoteCmd {
    /// List a run's directory (or its ARBORIA_OUT) on the cluster.
    Ls {
        /// Run id, e.g. r001.
        run_id: String,
        /// Relative path inside the run dir.
        subpath: Option<String>,
        /// List ARBORIA_OUT instead of the run dir.
        #[arg(long)]
        out: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MachineCmd {
    /// Measure a Slurm machine over your ssh master (read-only) and print its profile.
    Init {
        /// Machine name, e.g. deltaai.
        name: String,
        /// ~/.ssh/config alias (default: the profile's host, else NAME).
        #[arg(long)]
        host: Option<String>,
        /// Slurm account, if you have several.
        #[arg(long)]
        account: Option<String>,
        /// Write the profile into clusters.toml.
        #[arg(long)]
        write: bool,
        /// With --write: rewrite an existing profile (keeps a backup).
        #[arg(long)]
        replace: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TodoCmd {
    /// Todos of the current idea (in an idea worktree), of every open idea, or (in the
    /// seed home) of the seeds.
    List {
        /// Include done and dropped todos.
        #[arg(long = "all")]
        show_all: bool,
    },
    /// Mark a todo done with a short result for the human.
    Done {
        /// Todo id, e.g. t001.
        task_id: String,
        /// What was done / found (≤600 chars).
        #[arg(long)]
        result: String,
    },
    /// Drop a todo that will not be done.
    Drop {
        /// Todo id, e.g. t001.
        task_id: String,
        /// Why (≤600 chars).
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum CommandsCmd {
    /// The conventions for writing a notebook command (for the agent).
    Guide,
    /// Commands of this project and the global library (project wins on a clash).
    List,
    /// Copy a project command into the global library (~/.config/arboria/commands).
    Promote {
        /// The project command to copy.
        name: String,
        /// Replace a different global one.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum InspectCmd {
    /// The conventions for answering a lab.inspect todo (for the agent).
    Guide,
    /// The code a 读代码 todo is about, at its code_ref (read from git; nothing is changed).
    Source {
        /// Todo id, e.g. t007.
        task_id: String,
        /// File to print with line numbers (omit to list the files).
        path: Option<String>,
    },
    /// Validate an answer and print what the human will see; saves nothing.
    Check {
        /// Todo id, e.g. t007.
        task_id: String,
        /// The answer as JSON: a file, or - for stdin.
        #[arg(long)]
        file: String,
    },
    /// Deliver the answer to a 读代码 todo: arb extracts the code, stores it, closes the todo.
    Done {
        /// Todo id, e.g. t007.
        task_id: String,
        /// The answer as JSON: a file, or - for stdin.
        #[arg(long)]
        file: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RunCmd {
    /// Snapshot the current idea worktree as a draft run; prints the run id.
    #[command(hide = true)]
    New {
        /// What changed vs. the previous run and why (≤300 chars).
        #[arg(long)]
        reason: String,
        /// Snapshot large untracked files anyway.
        #[arg(long)]
        allow_large: bool,
    },
    /// Set a run's result summary (run must belong to the current idea).
    Summary {
        /// Run id, e.g. r001.
        run_id: String,
        /// Result summary (≤10 lines, ≤1500 chars).
        text: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum LessonCmd {
    /// Propose a lesson; it only goes into CLAUDE.md after the human accepts it.
    Propose {
        /// One line, ≤200 chars: a reusable project pitfall.
        text: String,
        /// Source idea (default: current idea).
        #[arg(long)]
        idea: Option<String>,
        /// Source run id.
        #[arg(long)]
        run: Option<String>,
    },
    /// List lessons (default: proposed + accepted), newest first.
    List {
        /// Include retired/rejected lessons.
        #[arg(long = "all")]
        show_all: bool,
    },
    /// (Human only, in your own terminal) accept a proposed lesson and update CLAUDE.md;
    /// you type the lesson id to confirm.
    Accept {
        /// Lesson id, e.g. l001.
        lesson_id: String,
    },
    /// Reject a proposed lesson.
    Reject {
        /// Lesson id, e.g. l001.
        lesson_id: String,
    },
    /// Retire an accepted lesson and remove it from CLAUDE.md.
    Retire {
        /// Lesson id, e.g. l001.
        lesson_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum HookCmd {
    /// SessionStart hook: reads hook JSON on stdin, prints additionalContext JSON.
    SessionStart,
}

#[derive(Debug, Subcommand)]
pub enum ClaudemdCmd {
    /// Write the arboria block (rules + accepted lessons) into the main worktree's CLAUDE.md.
    Sync {
        /// Write CLAUDE.md but do not commit it.
        #[arg(long)]
        no_commit: bool,
    },
}

/// `arb probe …` (hidden until probes ship; ADR 0003).
#[derive(Debug, Subcommand)]
pub enum ProbeCmd {
    /// (Human only, in your own terminal) enable a probe: binds the Decision to the
    /// file's hash; you type its name to confirm.
    Enable {
        /// Probe name.
        name: String,
    },
    /// (Human only) run an enabled probe once now.
    Run {
        /// Probe name.
        name: String,
    },
    /// Run only a probe's parser on a saved input (never the remote command).
    Check {
        /// Probe name.
        name: String,
        /// The command output to parse: a file, or - for stdin.
        #[arg(long)]
        input: String,
    },
    /// The conventions for writing a probe (for the agent).
    Guide,
}

// ---------------------------------------------------------------- the clap command

/// The root command with the help panels in its help text (clap has no per-subcommand
/// headings, so the root's help lists them from [`HELP_PANELS`]).
pub fn command() -> clap::Command {
    let cmd = Cli::command();
    let about = |name: &str| {
        cmd.find_subcommand(name)
            .and_then(|c| c.get_about())
            .map(|a| a.to_string())
            .unwrap_or_default()
    };
    let width = HELP_PANELS
        .iter()
        .flat_map(|(_, n)| n.iter())
        .map(|n| n.len())
        .max()
        .unwrap_or(0);
    let mut panels = String::new();
    for (panel, names) in HELP_PANELS {
        panels.push_str(&format!("{panel}:\n"));
        for name in *names {
            panels.push_str(&format!("  {name:<width$}  {}\n", about(name)));
        }
        panels.push('\n');
    }
    let template = format!("{{about-with-newline}}\n{{usage-heading}} {{usage}}\n\n{panels}Options:\n{{options}}{{after-help}}");
    cmd.help_template(template)
}

/// Parse the process arguments with [`command`].
pub fn parse() -> Cli {
    let matches = command().get_matches();
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

// ---------------------------------------------------------------- dispatch

fn warn(msg: &str) {
    eprintln!("warning: {msg}");
}

fn emit<T: Serialize>(
    json: bool,
    value: &T,
    text: impl FnOnce(&T) -> String,
) -> anyhow::Result<()> {
    let out = if json { pretty(value)? } else { text(value) };
    println!("{out}");
    Ok(())
}

/// Python `_echo_json`: `json.dumps(obj, indent=2, ensure_ascii=False)`.
fn pretty<T: Serialize + ?Sized>(value: &T) -> anyhow::Result<String> {
    Ok(arboria_core::canonical::python_pretty(
        &serde_json::to_value(value)?,
    ))
}

/// `由你批准：arb approve {run}` (Python `cli.APPROVE_LINE`).
fn approve_line(run: &str) -> String {
    format!("由你批准：arb approve {run}")
}

fn lesson_out(r: &arboria_core::lessons::LessonResult, verb: &str) {
    let (out, warning) = lines::lesson_result(r, verb);
    println!("{out}");
    if let Some(w) = warning {
        warn(&w);
    }
}

fn open() -> anyhow::Result<(RepoContext, db::Ledger)> {
    let ctx = paths::locate(None)?.require_initialized()?;
    let ledger = db::Ledger::open(&ctx.state_dir())?;
    Ok((ctx, ledger))
}

fn cwd() -> anyhow::Result<PathBuf> {
    std::env::current_dir().context("cannot read the current directory")
}

fn idea_name(s: &str) -> anyhow::Result<IdeaName> {
    records::validate_name(s, "idea")?;
    Ok(IdeaName::parse(s)?)
}

fn seed_name(s: &str) -> anyhow::Result<IdeaName> {
    records::validate_name(s, "seed")?;
    Ok(IdeaName::parse(s)?)
}

fn confidence(s: Option<&str>) -> anyhow::Result<Option<Confidence>> {
    Ok(s.map(Confidence::from_str).transpose()?)
}

/// Python `cli._answer`: the `--file` text (`-` = stdin).
fn read_answer(file: &str) -> anyhow::Result<String> {
    if file == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        return Ok(s);
    }
    std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("cannot read {file}: {e}"))
}

fn read_file_arg(file: &str) -> anyhow::Result<String> {
    if file == "-" {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(file).with_context(|| format!("cannot read {file}"))
    }
}

/// The text lines of the Python CLI's printers, to be ported with it.
mod lines {
    use arboria_core::{ideas, lessons, tasks};

    /// `_seed_line`.
    pub fn seed(s: &ideas::SeedView) -> String {
        let date: String = s.last_edited.chars().take(10).collect();
        let mut line = format!(
            "{:<24} {:<4} {date}  {}",
            s.idea.name.as_str(),
            s.seed_status.label(),
            s.idea.gist.as_deref().unwrap_or_default()
        );
        for p in &s.planted_to {
            let repo = std::path::Path::new(&p.repo)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            line.push_str(&format!("\n{:<25}→ {repo} · {}", "", p.idea));
        }
        line
    }
    /// `_seed_show` (text).
    pub fn seed_detail(s: &ideas::SeedView) -> String {
        let mut out = vec![format!("{} · {}", s.idea.name, s.seed_status.label())];
        for (label, value) in [
            ("想法", &s.idea.gist),
            ("假设", &s.idea.hypothesis),
            ("指标", &s.idea.metric),
        ] {
            if let Some(v) = value.as_deref().filter(|v| !v.is_empty()) {
                out.push(format!("{label}：{v}"));
            }
        }
        out.push(format!("notebook：{}", s.notebook));
        let created: String = s.idea.created_at.chars().take(10).collect();
        let edited: String = s
            .last_edited
            .chars()
            .take(16)
            .collect::<String>()
            .replace('T', " ");
        out.push(format!("创建 {created} · 最近编辑 {edited}"));
        for p in &s.planted_to {
            let when: String = p.created_at.chars().take(10).collect();
            out.push(format!("种在 {} · idea {}（{when}）", p.repo, p.idea));
        }
        out.join("\n")
    }
    /// `_todo_line`.
    pub fn todo(t: &tasks::TaskView) -> String {
        let kind = match t.task.kind.as_str() {
            "plain" => String::new(),
            k => format!("[{k}] "),
        };
        let mut line = format!(
            "{:<5} {:<7} {:<20} {kind}{}",
            t.task.id.as_str(),
            t.task.status.as_str(),
            t.owner_name(),
            t.task.text
        );
        if let Some(r) = t.task.result.as_deref().filter(|r| !r.is_empty()) {
            line.push_str(&format!("\n      → {r}"));
        }
        line
    }
    /// `_lesson_line`.
    pub fn lesson(l: &arboria_core::model::Lesson) -> String {
        let source = l
            .source_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-");
        let date: String = l.created_at.chars().take(10).collect();
        format!(
            "{:<5} {:<9} {date}  {source:<16} {}",
            l.id.as_str(),
            l.status.as_str(),
            l.text
        )
    }
    /// `_echo_lesson_result`: stdout lines, then the warning for stderr.
    pub fn lesson_result(r: &lessons::LessonResult, verb: &str) -> (String, Option<String>) {
        let mut out = vec![format!("{verb} {}", r.lesson.id)];
        if let Some(sync) = &r.sync {
            out.extend(sync.messages());
        }
        let warning = r
            .warning
            .as_ref()
            .map(|w| format!("{w}; then run `arb claudemd sync`"));
        (out.join("\n"), warning)
    }
    /// One `arb idea list` line.
    pub fn idea(r: &ideas::IdeaSummary) -> String {
        let parent = match r.parent_name.as_deref() {
            Some(p) if !p.is_empty() => format!(" (from {p})"),
            _ => String::new(),
        };
        let hyp: String = r
            .idea
            .hypothesis
            .as_deref()
            .unwrap_or_default()
            .chars()
            .take(60)
            .collect();
        format!(
            "{:<24} {:<7} {:<9} runs:{:<3} {hyp}{parent}",
            r.idea.name.as_str(),
            r.idea.status.as_str(),
            r.idea.confidence.as_str(),
            r.run_count
        )
    }
    /// `arb idea show` (text).
    pub fn idea_detail(i: &ideas::IdeaDetail) -> String {
        let d = &i.idea;
        let mut out = vec![
            format!("{} · {} · {}", d.name, d.status, d.confidence),
            format!("假设：{}", d.hypothesis.as_deref().unwrap_or("None")),
            format!("指标：{}", d.metric.as_deref().unwrap_or("None")),
        ];
        if let Some(p) = i.parent_name.as_deref().filter(|p| !p.is_empty()) {
            out.push(format!("来自：{p}"));
        }
        let folder = if d.status.as_str() == "open" {
            format!(
                " · 文件夹：{}",
                d.worktree_path.as_deref().unwrap_or("None")
            )
        } else {
            String::new()
        };
        out.push(format!(
            "分支：{}{folder}",
            d.branch.as_deref().unwrap_or("None")
        ));
        out.push(format!("notebook：{}", i.notebook));
        let created: String = d.created_at.chars().take(10).collect();
        let closed = match d.closed_at.as_deref().filter(|c| !c.is_empty()) {
            Some(c) => format!(" · 结束 {}", c.chars().take(10).collect::<String>()),
            None => String::new(),
        };
        out.push(format!("创建 {created}{closed}"));
        for r in &i.runs {
            let why: String = r.change_reason.chars().take(70).collect();
            out.push(format!(
                "  {:<5} {:<18} {why}",
                r.id.as_str(),
                r.status.as_str()
            ));
        }
        out.join("\n")
    }
}

/// Run one parsed command; returns the exit status.
pub fn run(cli: Cli) -> anyhow::Result<i32> {
    let json = cli.json;
    match cli.command {
        Cmd::Idea(cmd) => idea(cmd, json)?,
        Cmd::Seed(cmd) => seed(cmd, json)?,
        Cmd::Submit(a) => {
            let (ctx, mut ledger) = open()?;
            let idea = ideas::require_current_idea(&ctx, &ledger)?;
            let overrides = a
                .set
                .iter()
                .map(|s| slurm::parse_override(s))
                .collect::<Result<Vec<_>, _>>()?;
            let from = a.from_run.as_deref().map(RunId::parse).transpose()?;
            let args = submission::SubmitArgs {
                reason: &a.reason,
                command: (!a.command.is_empty()).then_some(a.command.as_slice()),
                gpus: a.gpus,
                time: a.time.as_deref(),
                nodes: a.nodes,
                partition: a.partition.as_deref(),
                machine: a.machine.as_deref(),
                mem_gb: a.mem,
                cpus: a.cpus,
                from_run: from.as_ref(),
                overrides,
                allow_large: a.allow_large,
                repeat: a.repeat,
                block_id: None,
            };
            let s = submission::submit(&ctx, &mut ledger, &idea, args, &mut warn)?;
            if json {
                let out = serde_json::json!({
                    "run": &s, "card": &s.card, "card_text": s.card.text(), "existed": s.existed,
                });
                println!("{}", pretty(&out)?);
                return Ok(0);
            }
            if s.existed {
                println!("{}", submission::EXISTS.replace("{run}", s.run.id.as_str()));
                if s.run.status != arboria_core::model::RunStatus::AwaitingApproval {
                    println!("状态：{}", s.run.status);
                    return Ok(0);
                }
            }
            println!("{}", s.card.text());
            println!("{}", approve_line(s.run.id.as_str()));
        }
        Cmd::Card { run_id } => {
            let (ctx, ledger) = open()?;
            let id = RunId::parse(&run_id)?;
            let (card, matches) = submission::card_for(&ctx, &ledger, &id)?;
            let status = records::get_run(&ledger, &id)?.status;
            if json {
                let mut out = serde_json::to_value(&card)?;
                out["matches_submitted"] = matches.into();
                out["status"] = status.as_str().into();
                println!("{}", pretty(&out)?);
                return Ok(0);
            }
            println!("{}", card.text());
            if !matches {
                println!("⚠ {}", arboria_core::cards::CARD_CHANGED);
            } else if status == arboria_core::model::RunStatus::AwaitingApproval {
                println!("{}", approve_line(&run_id));
            } else {
                println!("状态：{status}");
            }
        }
        Cmd::Approve { run_id } => {
            let (ctx, mut ledger) = open()?;
            let r = submission::approve(&ctx, &mut ledger, &RunId::parse(&run_id)?, &mut |t| {
                println!("{t}")
            })?;
            match &r.decision_id {
                Some(d) => println!("approved {run_id} (decision {d})"),
                None => println!("resuming approved {run_id}"),
            }
            for s in &r.submitted {
                let adopted = if s.adopted {
                    " (already on the cluster, adopted)"
                } else {
                    ""
                };
                println!("  s{}: job {}{adopted}", s.segment, s.job_id);
            }
            println!(
                "{run_id} is {}; follow it with `arb poll {run_id}` or `arb watch`",
                r.run.status
            );
        }
        Cmd::Reject { run_id, reason } => {
            let (ctx, mut ledger) = open()?;
            submission::reject(
                &ctx,
                &mut ledger,
                &RunId::parse(&run_id)?,
                reason.as_deref(),
            )?;
            println!("rejected {run_id}");
        }
        Cmd::Note {
            text,
            run,
            confidence: conf,
        } => {
            if human::in_claude_code().is_some() {
                bail!(NOTE_REFUSED);
            }
            let (ctx, mut ledger) = open()?;
            let idea = ideas::require_current_idea(&ctx, &ledger)?;
            let run = run.as_deref().map(RunId::parse).transpose()?;
            let conf = confidence(conf.as_deref())?;
            let n = records::add_note(&mut ledger, &idea, &text, run.as_ref(), conf)?;
            let suffix = conf
                .map(|c| format!("; confidence now {c}"))
                .unwrap_or_default();
            println!("note {} added to {}{suffix}", n.id.0, idea.name);
        }
        Cmd::Status { brief } => {
            if let Some(home) = ideas::home_of(None).filter(|h| h.initialized()) {
                let ledger = db::Ledger::open(&home.state_dir())?;
                if json {
                    let out = serde_json::json!({
                        "scope": "home",
                        "home": home.root.display().to_string(),
                        "seeds": ideas::list_seeds(&home, &ledger, true)?,
                        "todos": tasks::list_tasks(&ledger, None, false)?,
                    });
                    println!("{}", pretty(&out)?);
                } else {
                    println!("{}", report::home_status(&home, &ledger, brief)?);
                }
            } else {
                let (ctx, ledger) = open()?;
                let data = report::status_data(&ctx, &ledger)?;
                emit(json, &data, |d| report::format_status(d, brief))?;
            }
        }
        Cmd::Ui { idea, port, home } => {
            let home = home
                || match paths::locate(None) {
                    Err(arboria_core::Error::NotARepo(_)) => true,
                    Err(e) => return Err(e.into()),
                    Ok(_) => ideas::home_of(None).is_some(),
                };
            let code = if home {
                if idea.is_some() {
                    bail!("--idea is for a project's dashboard, not the seed home");
                }
                let h = ideas::ensure_home(&mut info)?;
                ui::launch_home(&h.root, port)?
            } else {
                let ctx = paths::locate(None)?.require_initialized()?;
                ui::launch(&ctx.main, ctx.toplevel.as_deref(), idea.as_deref(), port)?
            };
            return Ok(code);
        }
        Cmd::Watch {
            once,
            interval,
            window,
        } => {
            if interval < 10 {
                bail!("--interval must be at least 10 seconds");
            }
            if window.is_some_and(|w| w < 1) {
                bail!("--window must be at least 1 minute");
            }
            let (ctx, mut ledger) = open()?;
            let opts = monitor::WatchOptions {
                once,
                interval_s: interval,
                window_minutes: window,
                max_iterations: None,
            };
            return Ok(monitor::watch(&ctx, &mut ledger, opts, &mut |l| {
                println!("{l}")
            })?);
        }
        Cmd::Poll { run_id } => {
            let (ctx, mut ledger) = open()?;
            arboria_core::config::require_project_id(&ctx)?;
            let ids = match run_id {
                Some(r) => vec![RunId::parse(&r)?],
                None => monitor::active_runs(&ledger)?
                    .into_iter()
                    .map(|r| r.id)
                    .collect(),
            };
            let mut outcomes = Vec::new();
            for id in &ids {
                outcomes.push(monitor::poll_run(&ctx, &mut ledger, id, None)?);
            }
            emit(json, &outcomes, |o| {
                if o.is_empty() {
                    return "no active runs".into();
                }
                let mut out = Vec::new();
                for p in o {
                    let was = if p.previous != p.status {
                        format!(" (was {})", p.previous)
                    } else {
                        String::new()
                    };
                    out.push(format!("{}: {}{was}", p.run_id, p.status));
                    out.extend(p.messages.iter().map(|m| format!("  {m}")));
                }
                out.join("\n")
            })?;
        }
        Cmd::Cancel { run_id } => {
            let (ctx, mut ledger) = open()?;
            let (_, ids) = monitor::cancel(&ctx, &mut ledger, &RunId::parse(&run_id)?)?;
            if ids.is_empty() {
                println!("cancelled {run_id} (no live jobs)");
            } else {
                println!("cancelled {run_id} (scancel {})", ids.join(" "));
            }
        }
        Cmd::Logs { run_id, segment, n } => {
            let (ctx, ledger) = open()?;
            let text = monitor::logs(&ctx, &ledger, &RunId::parse(&run_id)?, segment, n)?;
            if text.ends_with('\n') {
                print!("{text}");
            } else {
                println!("{text}");
            }
        }
        Cmd::Remote(RemoteCmd::Ls {
            run_id,
            subpath,
            out,
        }) => {
            let (ctx, ledger) = open()?;
            let (base, entries) = monitor::remote_ls(
                &ctx,
                &ledger,
                &RunId::parse(&run_id)?,
                subpath.as_deref(),
                out,
            )?;
            println!("{base}:");
            for e in entries {
                println!("  {e}");
            }
        }
        Cmd::Machine(MachineCmd::Init {
            name,
            host,
            account,
            write,
            replace,
        }) => {
            let opts = machine::InitOptions {
                host: host.as_deref(),
                account: account.as_deref(),
                write,
                replace,
            };
            let r = machine::machine_init(&name, opts)?;
            if json {
                println!("{}", pretty(&r)?);
                return Ok(0);
            }
            let host = r.proposal.values["host"].as_str().unwrap_or_default();
            for problem in &r.ssh_problems {
                warn(&format!(
                    "ssh: {problem}; add to ~/.ssh/config:\n{}",
                    machine::SSH_BLOCK.replace("{host}", host)
                ));
            }
            for w in &r.proposal.warnings {
                warn(w);
            }
            print!("{}", r.text);
            if !r.proposal.other_partitions.is_empty() {
                println!(
                    "# also usable, left out: {}",
                    r.proposal.other_partitions.join(", ")
                );
            }
            if r.written {
                let backup = r
                    .backup
                    .as_ref()
                    .and_then(|b| b.file_name())
                    .map(|b| format!(" (backup {})", b.to_string_lossy()))
                    .unwrap_or_default();
                println!("wrote [{name}] to {}{backup}", r.path.display());
            } else {
                println!(
                    "(not written; rerun with --write to save it to {})",
                    r.path.display()
                );
            }
            if let Some(s) = &r.still_invalid {
                warn(&format!("still to fill in: {s}"));
            } else if r.proposal.values["env_activate"]
                .as_array()
                .is_none_or(Vec::is_empty)
            {
                warn("env_activate is empty: decide the job environment, then set it");
            }
        }
        Cmd::Doctor { name } => {
            let ctx = match paths::locate(None) {
                Ok(c) => Some(c),
                Err(arboria_core::Error::NotARepo(_)) => None,
                Err(e) => return Err(e.into()),
            };
            let mut name = name;
            let mut project_id = None;
            if let Some(ctx) = ctx.as_ref().filter(|c| c.initialized()) {
                let cfg = arboria_core::config::project_config(ctx)?;
                if name.as_deref().is_none_or(str::is_empty) && !cfg.default_machine.is_empty() {
                    name = Some(cfg.default_machine.clone());
                }
                project_id = cfg.project_id;
            }
            let Some(name) = name.filter(|n| !n.is_empty()) else {
                bail!("which machine? pass a name or set default_machine in arboria.toml");
            };
            let checks = machine::doctor(&name, project_id.as_ref())?;
            let failed = checks.iter().filter(|c| c.level == "fail").count();
            if json {
                println!("{}", pretty(&checks)?);
            } else {
                for c in &checks {
                    let mark = match c.level.as_str() {
                        "ok" => "ok  ",
                        "warn" => "WARN",
                        "fail" => "FAIL",
                        _ => "    ",
                    };
                    println!("{mark} {}", c.what);
                }
                if failed > 0 {
                    println!("{failed} problem(s)");
                } else {
                    println!("all checks passed");
                }
            }
            return Ok(i32::from(failed > 0));
        }
        Cmd::Todo(cmd) => todo_cmd(cmd, json)?,
        Cmd::Commands(cmd) => match cmd {
            CommandsCmd::Guide => print!("{}", commands::guide()),
            CommandsCmd::List => {
                let state = paths::locate(None)
                    .ok()
                    .filter(RepoContext::initialized)
                    .map(|c| c.state_dir());
                let rows = commands::discover(state.as_deref())?;
                emit(json, &rows, |rows| {
                    let names: Vec<&str> = commands::BUILTINS.iter().map(|(n, _)| *n).collect();
                    let builtins = format!(
                        "built-in (内置, signatures in lab.commands()): {}",
                        names.join(", ")
                    );
                    if rows.is_empty() {
                        return format!(
                            "no commands yet (a notebook call lab.<name>(..., ask=\"...\") asks Claude to write one)\n{builtins}"
                        );
                    }
                    let width = rows
                        .iter()
                        .map(|r| r.signature.chars().count())
                        .max()
                        .unwrap_or(0)
                        .min(48);
                    let mut out: Vec<String> = rows
                        .iter()
                        .map(|r| {
                            let mark = if r.active {
                                ""
                            } else {
                                "  (shadowed by the project's)"
                            };
                            let problem = r
                                .problem
                                .as_ref()
                                .map(|p| format!("  ⚠ {p}"))
                                .unwrap_or_default();
                            format!(
                                "{:<width$}  {}  v{}  {}{mark}{problem}",
                                r.signature,
                                r.scope.label(),
                                r.version,
                                r.description
                            )
                        })
                        .collect();
                    out.push(builtins);
                    out.join("\n")
                })?;
            }
            CommandsCmd::Promote { name, force } => {
                let (_ctx, mut ledger) = open()?;
                let res = commands::promote(&mut ledger, &name, force)?;
                if !res.changed {
                    println!(
                        "{name} v{} 已经是全局命令库里的版本（{}）",
                        res.version, res.path
                    );
                } else {
                    let replaced = res
                        .replaced
                        .as_ref()
                        .map(|r| format!("，替换了 v{r}"))
                        .unwrap_or_default();
                    println!(
                        "promoted {name} v{} → {}{replaced}\n项目里的 {name} 仍然优先；别的项目和种子里 lab.{name}(...) 用全局的这份。",
                        res.version, res.path
                    );
                }
            }
        },
        Cmd::Inspect(InspectCmd::Guide) => print!("{}", inspection::guide()),
        Cmd::Inspect(cmd) => {
            let (ctx, mut ledger) = open()?;
            match cmd {
                InspectCmd::Guide => unreachable!("handled above"),
                InspectCmd::Source { task_id, path } => print!(
                    "{}",
                    inspection::source_text(
                        &ledger,
                        &ctx.main,
                        &TaskId::parse(&task_id)?,
                        path.as_deref()
                    )?
                ),
                InspectCmd::Check { task_id, file } => {
                    let data = inspection::load_answer(&read_answer(&file)?)?;
                    println!(
                        "{}",
                        inspection::check(&ledger, &ctx.main, &TaskId::parse(&task_id)?, &data)?
                    );
                }
                InspectCmd::Done { task_id, file } => {
                    let data = inspection::load_answer(&read_answer(&file)?)?;
                    let r = inspection::deliver(
                        &mut ledger,
                        &ctx.main,
                        &TaskId::parse(&task_id)?,
                        &data,
                    )?;
                    let spans = r["spans"].as_array().cloned().unwrap_or_default();
                    let lines: usize = spans
                        .iter()
                        .map(|s| s["lines"].as_array().map_or(0, Vec::len))
                        .sum();
                    println!(
                        "done {task_id}: {} ({} spans, {lines} lines @ {}); the notebook cell re-runs by itself",
                        r["title"].as_str().unwrap_or_default(),
                        spans.len(),
                        inspection::short(r["code_ref"].as_str().unwrap_or_default())
                    );
                }
            }
        }
        Cmd::Run(cmd) => {
            let (ctx, mut ledger) = open()?;
            let idea = ideas::require_current_idea(&ctx, &ledger)?;
            match cmd {
                RunCmd::New {
                    reason,
                    allow_large,
                } => {
                    let opts = records::SnapshotOptions {
                        allow_large,
                        ..Default::default()
                    };
                    let r = records::new_run(&mut ledger, &idea, &reason, opts, &mut warn)?;
                    emit(json, &r, |r| r.run.id.to_string())?;
                }
                RunCmd::Summary { run_id, text } => {
                    records::set_run_summary(&mut ledger, &idea, &RunId::parse(&run_id)?, &text)?;
                    println!("summary saved for {run_id}");
                }
            }
        }
        Cmd::Lesson(cmd) => {
            let (ctx, mut ledger) = open()?;
            match cmd {
                LessonCmd::Propose { text, idea, run } => {
                    let run = run.as_deref().map(RunId::parse).transpose()?;
                    let (lesson, dec) =
                        lessons::propose(&ctx, &mut ledger, &text, idea.as_deref(), run.as_ref())?;
                    println!(
                        "proposed {id} (decision {dec}); the human accepts it with \
                         `arb lesson accept {id}`",
                        id = lesson.id
                    );
                }
                LessonCmd::List { show_all } => {
                    let ls = lessons::list_lessons(&ledger, show_all)?;
                    emit(json, &ls, |ls| {
                        if ls.is_empty() {
                            return "no lessons yet".into();
                        }
                        ls.iter().map(lines::lesson).collect::<Vec<_>>().join("\n")
                    })?;
                }
                LessonCmd::Accept { lesson_id } => {
                    let r = lessons::accept(
                        &ctx,
                        &mut ledger,
                        &LessonId::parse(&lesson_id)?,
                        &mut |t| println!("{t}"),
                    )?;
                    lesson_out(&r, "accepted");
                }
                LessonCmd::Reject { lesson_id } => {
                    let r = lessons::reject(&ctx, &mut ledger, &LessonId::parse(&lesson_id)?)?;
                    lesson_out(&r, "rejected");
                }
                LessonCmd::Retire { lesson_id } => {
                    let r = lessons::retire(&ctx, &mut ledger, &LessonId::parse(&lesson_id)?)?;
                    lesson_out(&r, "retired");
                }
            }
        }
        Cmd::Init => {
            let ctx = paths::locate(None)?;
            let actions = state::init_state(&ctx, &mut warn)?;
            for a in &actions {
                println!("{a}");
            }
            if actions.is_empty() {
                println!("already initialized");
            } else {
                println!("arboria ready: state in {}", ctx.state_dir().display());
                println!("{}", state::NEXT_STEPS);
            }
        }
        Cmd::InstallClaude { main, user, home } => install_claude(main, user, home)?,
        Cmd::Hook(HookCmd::SessionStart) => {
            // Never fails, never prints anything extra (see main.rs, which also catches panics).
            let stdin = std::io::stdin();
            let code = if std::io::IsTerminal::is_terminal(&stdin) {
                hook::session_start(&mut std::io::empty(), &mut std::io::stdout())
            } else {
                hook::session_start(&mut stdin.lock(), &mut std::io::stdout())
            };
            return Ok(code);
        }
        Cmd::Claudemd(ClaudemdCmd::Sync { no_commit }) => {
            let (ctx, ledger) = open()?;
            let r = claudemd::sync(&ctx, &ledger, !no_commit)?;
            emit(json, &r, |r| r.messages().join("\n"))?;
        }
        Cmd::Export { out } => {
            let (ctx, ledger) = open()?;
            let md = report::export_markdown(&ctx, &ledger)?;
            match out {
                None => {
                    print!("{md}");
                    std::io::stdout().flush()?;
                }
                Some(p) => {
                    std::fs::write(&p, md)?;
                    println!("wrote {}", p.display());
                }
            }
        }
        Cmd::Probe(cmd) => probe_cmd(cmd, json)?,
    }
    Ok(0)
}

/// `arb install-claude [--main|--user|--home]`.
fn install_claude(main: bool, user: bool, home: bool) -> anyhow::Result<()> {
    if u8::from(main) + u8::from(user) + u8::from(home) > 1 {
        bail!("use only one of --main / --user / --home");
    }
    let report = |path: &Path, changes: &[String]| {
        for c in changes {
            println!("{c}");
        }
        if changes.is_empty() {
            println!("{} already up to date", path.display());
        } else {
            println!("updated {}", path.display());
        }
    };
    if home {
        let h = ideas::ensure_home(&mut |m| eprintln!("{m}"))?;
        let (path, changes) = install::install(&h.root, None, false, &install::home_deny_rules())?;
        report(&path, &changes);
        let r = claudemd::sync_home(&h.root)?;
        if let Some(action) = &r.action {
            println!("{action} arboria block in {}", r.path.display());
        }
        return Ok(());
    }
    let (root, ctx) = if user {
        let root = std::env::home_dir().context("cannot find the home directory")?;
        (root, paths::locate(None).ok()) // --user works outside any repository
    } else {
        let ctx = paths::locate(None)?;
        let root = if main {
            ctx.main.clone()
        } else {
            ctx.toplevel.clone().context("not inside a worktree")?
        };
        (root, Some(ctx))
    };
    let (path, mut changes) = install::install(&root, None, !user, &install::deny_rules())?;
    if let Some(ctx) = ctx.as_ref().filter(|_| !user) {
        // the local file is per machine: keep it out of git
        if paths::ensure_excluded(
            &ctx.common_dir,
            install::LOCAL_EXCLUDE,
            install::LOCAL_COMMENT,
        )? {
            changes.push(format!(
                "added {} to {}",
                install::LOCAL_EXCLUDE,
                ctx.common_dir.join("info").join("exclude").display()
            ));
        }
    }
    if changes.is_empty() {
        println!("{} already up to date", path.display());
    } else {
        report(&path, &changes);
    }
    sync_claudemd_after(ctx.as_ref(), true);
    Ok(())
}

/// Sync CLAUDE.md as a follow-up step; a failure only warns (the main action succeeded).
fn sync_claudemd_after(ctx: Option<&RepoContext>, quiet_if_unchanged: bool) {
    let Some(ctx) = ctx.filter(|c| c.initialized()) else {
        return;
    };
    let result = db::Ledger::open(&ctx.state_dir()).and_then(|l| claudemd::sync(ctx, &l, true));
    match result {
        Err(e) => warn(&format!("CLAUDE.md not updated: {e}")),
        Ok(r) => {
            if r.changed() || !quiet_if_unchanged {
                for line in r.messages() {
                    println!("{line}");
                }
            }
        }
    }
}

fn idea(cmd: IdeaCmd, json: bool) -> anyhow::Result<()> {
    let ctx = match ideas::route(&cwd()?)? {
        ideas::Route::Repo(ctx) => Some(ctx),
        ideas::Route::Seeds(_) => None,
    };
    let seed_only = |verb: &str, name: &str| -> anyhow::Result<()> {
        if ctx.is_some() {
            bail!(SEED_ONLY.replace("{verb}", verb).replace("{name}", name));
        }
        Ok(())
    };
    match cmd {
        IdeaCmd::New {
            name,
            gist,
            hypothesis,
            metric,
            parent,
        } => {
            let Some(ctx) = &ctx else {
                if parent.is_some() {
                    bail!("--parent is for ideas in a repo; a seed has no parent");
                }
                let fields = ideas::SeedFields {
                    gist: gist.as_deref(),
                    hypothesis: hypothesis.as_deref(),
                    metric: metric.as_deref(),
                };
                let s = seed_new(&name, fields, &format!("arb idea new {name}"), json)?;
                info(
                    &SEED_RECORDED
                        .replace("{path}", &report::display_path(Path::new(&s.notebook)))
                        .replace("{name}", &name),
                );
                return Ok(());
            };
            if let Some(g) = gist {
                bail!(
                    "unexpected argument {}: in a repo an idea takes --hypothesis and --metric (a one-line gist is for seeds, outside any repo)",
                    records::py_repr(&g)
                );
            }
            let missing: Vec<&str> = [("--hypothesis", &hypothesis), ("--metric", &metric)]
                .into_iter()
                .filter(|(_, v)| v.is_none())
                .map(|(o, _)| o)
                .collect();
            if !missing.is_empty() {
                bail!(
                    "missing {}: an idea in a repo needs both",
                    missing.join(" and ")
                );
            }
            let mut ledger = db::Ledger::open(&ctx.state_dir())?;
            let name = idea_name(&name)?;
            let parent = parent.as_deref().map(idea_name).transpose()?;
            let new = ideas::NewIdea {
                hypothesis: hypothesis.as_deref().unwrap_or_default(),
                metric: metric.as_deref().unwrap_or_default(),
                parent: parent.as_ref(),
                ..Default::default()
            };
            let idea = ideas::new_idea(ctx, &mut ledger, &name, new, &mut warn)?;
            emit(json, &idea, |i| i.worktree_path.clone().unwrap_or_default())?;
        }
        IdeaCmd::List { show_all } => {
            let Some(ctx) = &ctx else {
                return seed(SeedCmd::List { show_all }, json);
            };
            let ledger = db::Ledger::open(&ctx.state_dir())?;
            let ideas = ideas::list_ideas(&ledger)?;
            emit(json, &ideas, |is| {
                if is.is_empty() {
                    return "no ideas yet".into();
                }
                is.iter().map(lines::idea).collect::<Vec<_>>().join("\n")
            })?;
        }
        IdeaCmd::Show { name } => {
            let Some(ctx) = &ctx else {
                let name = name.context("which seed? pass its NAME (see `arb idea list`)")?;
                return seed(SeedCmd::Show { name }, json);
            };
            let ledger = db::Ledger::open(&ctx.state_dir())?;
            let name = name.as_deref().map(idea_name).transpose()?;
            let d = ideas::show_idea(ctx, &ledger, name.as_ref())?;
            emit(json, &d, lines::idea_detail)?;
        }
        IdeaCmd::Set(a) => {
            ideas::require_human_for_seeds(&format!("arb idea set {}", a.name), "改种子")?;
            seed_only("set", &a.name)?;
            seed(SeedCmd::Set(a), json)?;
        }
        IdeaCmd::Drop {
            name,
            confidence: conf,
            allow_large,
        } => {
            let Some(ctx) = &ctx else {
                let shown = name.as_deref().unwrap_or("NAME");
                ideas::require_human_for_seeds(&format!("arb idea drop {shown}"), "放弃一个种子")?;
                let name = name.context("which seed? pass its NAME (see `arb idea list`)")?;
                if conf.is_some() || allow_large {
                    bail!("--confidence and --allow-large are for ideas in a repo, not seeds");
                }
                return seed(SeedCmd::Drop { name }, json);
            };
            close(
                ctx,
                name.as_deref(),
                ideas::CloseMode::Drop,
                conf.as_deref(),
                allow_large,
                json,
            )?;
        }
        IdeaCmd::Revive { name } => {
            ideas::require_human_for_seeds(
                &format!("arb idea revive {name}"),
                "恢复一个已放弃的种子",
            )?;
            seed_only("revive", &name)?;
            seed(SeedCmd::Revive { name }, json)?;
        }
        IdeaCmd::Plant(a) => seed(SeedCmd::Plant(a), json)?,
        IdeaCmd::Notebook { name } => {
            let (ctx, mut ledger) = open()?;
            let name = name.as_deref().map(idea_name).transpose()?;
            let (idea, path, created) =
                ideas::ensure_notebook(&ctx, &mut ledger, name.as_ref(), &mut warn)?;
            if created {
                println!(
                    "created {} (open it with `marimo edit {}`)",
                    path.display(),
                    path.display()
                );
            } else {
                println!(
                    "{} already has a notebook: {} (left as is)",
                    idea.name,
                    path.display()
                );
            }
        }
        IdeaCmd::Style => {
            ideas::require_human_for_seeds("arb idea style", "会改 notebook 文件")?;
            let done = match &ctx {
                Some(ctx) => {
                    let ledger = db::Ledger::open(&ctx.state_dir())?;
                    ideas::restyle(ctx, &ledger, &mut warn)?
                }
                None => ideas::restyle_seeds(&ideas::ensure_home(&mut info)?)?,
            };
            if done.is_empty() {
                println!("没有 notebook");
            }
            for (path, how) in done {
                let what = match how {
                    ideas::Restyled::Added => "加上了 Arboria 字体",
                    ideas::Restyled::Already => "已经有了",
                    ideas::Restyled::Own => "有自己的 css_file，没动",
                    ideas::Restyled::Unknown => "找不到 marimo.App(...)，没动",
                };
                println!(
                    "{}: {what}",
                    path.file_stem().unwrap_or_default().to_string_lossy()
                );
            }
        }
        IdeaCmd::Close {
            name,
            merge,
            drop,
            confidence: conf,
            allow_large,
        } => {
            if merge == drop {
                bail!("choose exactly one of --merge or --drop");
            }
            let (ctx, _) = open()?;
            let mode = if merge {
                ideas::CloseMode::Merge
            } else {
                ideas::CloseMode::Drop
            };
            close(
                &ctx,
                name.as_deref(),
                mode,
                conf.as_deref(),
                allow_large,
                json,
            )?;
        }
    }
    Ok(())
}

/// Python `_info`: a message for the human on stderr.
fn info(msg: &str) {
    eprintln!("{msg}");
}

fn close(
    ctx: &RepoContext,
    name: Option<&str>,
    mode: ideas::CloseMode,
    conf: Option<&str>,
    allow_large: bool,
    json: bool,
) -> anyhow::Result<()> {
    let mut ledger = db::Ledger::open(&ctx.state_dir())?;
    let name = name.map(idea_name).transpose()?;
    let closed = ideas::close_idea(
        ctx,
        &mut ledger,
        name.as_ref(),
        mode,
        confidence(conf)?,
        allow_large,
        &mut warn,
    )?;
    if json {
        println!("{}", pretty(&closed)?);
        return Ok(());
    }
    let (i, c) = (&closed.idea, &closed.close);
    let worktree = i.worktree_path.as_deref().unwrap_or_default();
    match mode {
        ideas::CloseMode::Merge => println!(
            "merged {} into {}; removed {worktree}",
            i.branch.as_deref().unwrap_or_default(),
            c.merged_into.as_deref().unwrap_or_default()
        ),
        ideas::CloseMode::Drop => {
            let commit: String = c
                .final_commit
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(12)
                .collect();
            println!(
                "dropped {}: final snapshot {commit} tagged {}; removed {worktree}",
                i.name,
                c.tag.as_deref().unwrap_or_default()
            );
        }
    }
    println!("{IDEA_CLOSE_HINT}");
    Ok(())
}

/// Python `_seed_new` (the human check included).
fn seed_new(
    name: &str,
    fields: ideas::SeedFields<'_>,
    command: &str,
    json: bool,
) -> anyhow::Result<ideas::SeedView> {
    ideas::require_human_for_seeds(command, "写一个种子")?;
    let home = ideas::ensure_home(&mut info)?;
    let mut ledger = db::Ledger::open(&home.state_dir())?;
    let s = ideas::new_seed(&home, &mut ledger, &seed_name(name)?, fields)?;
    if json {
        println!("{}", pretty(&s)?);
    } else {
        println!("{}", s.notebook);
    }
    if s.kept_existing_notebook {
        warn(&format!("{} already existed; left as is", s.notebook));
    }
    if Path::new(&s.notebook).exists() {
        info(&format!(
            "打开：VS Code（marimo 扩展），或 marimo edit {}",
            s.notebook
        ));
    }
    Ok(s)
}

fn seed(cmd: SeedCmd, json: bool) -> anyhow::Result<()> {
    match cmd {
        SeedCmd::New {
            name,
            gist,
            hypothesis,
            metric,
        } => {
            let fields = ideas::SeedFields {
                gist: gist.as_deref(),
                hypothesis: hypothesis.as_deref(),
                metric: metric.as_deref(),
            };
            seed_new(&name, fields, &format!("arb seed new {name}"), json)?;
        }
        SeedCmd::List { show_all } => {
            let home = ideas::ensure_home(&mut info)?;
            let ledger = db::Ledger::open(&home.state_dir())?;
            let seeds = ideas::list_seeds(&home, &ledger, show_all)?;
            emit(json, &seeds, |ss| {
                if ss.is_empty() {
                    return "no seeds yet (outside any repo: arb idea new NAME \"一句话想法\")"
                        .into();
                }
                ss.iter().map(lines::seed).collect::<Vec<_>>().join("\n")
            })?;
        }
        SeedCmd::Show { name } => {
            let home = ideas::ensure_home(&mut info)?;
            let ledger = db::Ledger::open(&home.state_dir())?;
            let s = ideas::show_seed(&home, &ledger, &seed_name(&name)?)?;
            emit(json, &s, lines::seed_detail)?;
        }
        SeedCmd::Set(a) => {
            ideas::require_human_for_seeds(&format!("arb seed set {}", a.name), "改种子")?;
            if a.gist.is_none() && a.hypothesis.is_none() && a.metric.is_none() {
                bail!("nothing to change: pass --gist, --hypothesis and/or --metric");
            }
            let home = ideas::ensure_home(&mut info)?;
            let mut ledger = db::Ledger::open(&home.state_dir())?;
            let fields = ideas::SeedFields {
                gist: a.gist.as_deref(),
                hypothesis: a.hypothesis.as_deref(),
                metric: a.metric.as_deref(),
            };
            ideas::set_seed_fields(&mut ledger, &seed_name(&a.name)?, fields)?;
            println!("updated {}", a.name);
        }
        SeedCmd::Drop { name } => {
            ideas::require_human_for_seeds(&format!("arb seed drop {name}"), "放弃一个种子")?;
            let home = ideas::ensure_home(&mut info)?;
            let mut ledger = db::Ledger::open(&home.state_dir())?;
            let s = ideas::drop_seed(&mut ledger, &seed_name(&name)?)?;
            emit(json, &s, |_| {
                format!(
                    "dropped {name} (notebook kept: {})",
                    home.notebook(&name).display()
                )
            })?;
        }
        SeedCmd::Revive { name } => {
            ideas::require_human_for_seeds(
                &format!("arb seed revive {name}"),
                "恢复一个已放弃的种子",
            )?;
            let home = ideas::ensure_home(&mut info)?;
            let mut ledger = db::Ledger::open(&home.state_dir())?;
            ideas::revive_seed(&mut ledger, &seed_name(&name)?)?;
            println!(
                "revived {name}: thinking again ({})",
                home.notebook(&name).display()
            );
        }
        SeedCmd::Plant(a) => {
            ideas::require_human_for_seeds(
                &format!("arb idea plant {}", a.name),
                "会建分支和 worktree",
            )?;
            let home = ideas::ensure_home(&mut info)?;
            if a.repo.is_none() && home.contains(&cwd()?) {
                bail!("which repo? run this inside the target repo, or pass --repo PATH");
            }
            let target = paths::locate(a.repo.as_deref())?.require_initialized()?;
            let mut home_ledger = db::Ledger::open(&home.state_dir())?;
            let as_name = a.as_.as_deref().map(idea_name).transpose()?;
            let opts = ideas::PlantOptions {
                idea_name: as_name.as_ref(),
                hypothesis: a.hypothesis.as_deref(),
                metric: a.metric.as_deref(),
                again: a.again,
            };
            let idea = ideas::plant(
                &home,
                &mut home_ledger,
                &seed_name(&a.name)?,
                &target,
                opts,
                &mut warn,
            )?;
            emit(json, &idea, |i| i.worktree_path.clone().unwrap_or_default())?;
            info(&format!(
                "planted {} as idea {} in {}",
                a.name,
                idea.name,
                target.main.display()
            ));
        }
    }
    Ok(())
}

fn todo_cmd(cmd: TodoCmd, json: bool) -> anyhow::Result<()> {
    // The home's ledger inside the seed home, else the project's (Python `_todo_ledger`).
    let (mut ledger, owner) = match ideas::home_of(None).filter(|h| h.initialized()) {
        Some(home) => (db::Ledger::open(&home.state_dir())?, None),
        None => {
            let (ctx, ledger) = open()?;
            let owner = ideas::current_idea(&ctx, &ledger)?.map(|i| i.id);
            (ledger, owner)
        }
    };
    match cmd {
        TodoCmd::List { show_all } => {
            let ts = tasks::list_tasks(&ledger, owner, show_all)?;
            emit(json, &ts, |ts| {
                if ts.is_empty() {
                    return if show_all {
                        "no todos"
                    } else {
                        "no open todos"
                    }
                    .into();
                }
                ts.iter().map(lines::todo).collect::<Vec<_>>().join("\n")
            })?;
        }
        TodoCmd::Done { task_id, result } => {
            tasks::done(&mut ledger, &TaskId::parse(&task_id)?, &result)?;
            println!("done {task_id}");
        }
        TodoCmd::Drop { task_id, reason } => {
            tasks::drop_task(&mut ledger, &TaskId::parse(&task_id)?, reason.as_deref())?;
            println!("dropped {task_id}");
        }
    }
    Ok(())
}

fn find_probe(name: &str) -> anyhow::Result<(Option<RepoContext>, probe::ProbeFile)> {
    probe::validate_name(name)?;
    let ctx = paths::locate(None).ok().filter(RepoContext::initialized);
    let state = ctx.as_ref().map(RepoContext::state_dir);
    let file = probe::discover(state.as_deref())?
        .into_iter()
        .find(|f| f.key.name == name)
        .with_context(|| format!("no probe named {}", records::py_repr(name)))?;
    Ok((ctx, file))
}

fn probe_ledger(ctx: Option<&RepoContext>) -> anyhow::Result<db::Ledger> {
    let state = match ctx {
        Some(c) => c.state_dir(),
        None => paths::home().state_dir(),
    };
    Ok(db::Ledger::open(Path::new(&state))?)
}

fn probe_cmd(cmd: ProbeCmd, json: bool) -> anyhow::Result<()> {
    match cmd {
        ProbeCmd::Guide => print!("{}", probe::guide()),
        ProbeCmd::Enable { name } => {
            let prov = human::require_human(&format!("arb probe enable {name}"), &name)?;
            let (ctx, file) = find_probe(&name)?;
            let mut ledger = probe_ledger(ctx.as_ref())?;
            let d = probe::enable(&mut ledger, &file, &prov)?;
            println!("{} enabled ({})", file.key, d.id);
        }
        ProbeCmd::Run { name } => {
            human::require_terminal(&format!("arb probe run {name}"))?;
            let (ctx, file) = find_probe(&name)?;
            let mut ledger = probe_ledger(ctx.as_ref())?;
            let r = probe::run_once(&mut ledger, &file)?;
            emit(json, &r, |r| r.to_ledger_json().unwrap_or_default())?;
        }
        ProbeCmd::Check { name, input } => {
            let (_ctx, file) = find_probe(&name)?;
            let r = probe::check(&file, read_file_arg(&input)?.as_bytes())?;
            emit(json, &r, |r| r.to_ledger_json().unwrap_or_default())?;
        }
    }
    Ok(())
}
