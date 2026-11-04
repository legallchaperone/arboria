//! `arb init`: create the state dir, ledger, backup repo and `arboria.toml` template.
//! Ports `arboria.state`.
//!
//! Invariants:
//! - Idempotent: a second run changes nothing and says "already initialized".
//! - `.arboria/` is excluded through `<common dir>/info/exclude`, never `.gitignore`;
//!   `arboria.toml` is the user's to commit.
//! - The project id is generated once (`<slug ≤26>-<4 hex>`) and only added to a file
//!   without one, keeping its comments and layout ([`crate::config::add_project_id`]).

use crate::config::{
    add_project_id, derive_project_id, project_config, project_table, DEFAULT_FAIL_PATTERNS,
    DEFAULT_OBSERVATION_MINUTES, DEFAULT_OVERRIDE_STYLE, DEFAULT_RUN_COMMAND,
};
use crate::db::{init_backup_repo, Ledger};
use crate::model::ProjectId;
use crate::paths::{ensure_state_excluded, RepoContext};
use crate::{claudemd, Result, Warn};

/// What `arb init` prints after a first init (Python `cli.NEXT_STEPS`, verbatim).
pub const NEXT_STEPS: &str = "
账本现在是空的，所以 arb ui 里还什么都没有。下一步：
  1. 编辑 arboria.toml：default_machine、[wandb] entity/project、[run] command，然后 commit
  2. arb install-claude          # hook、权限规则、CLAUDE.md 区块
  3. arb idea new NAME --hypothesis \"...\" --metric \"...\"
     # 建分支、worktree 和 notebook；之后看板上就有这个 idea 了
  4. 打开那个 worktree 里的 notebook.py（这个 idea 的 notebook）：VS Code 装 marimo 扩展，
     或者 marimo edit notebook.py
     marimo 默认打开时不运行 cell；想一打开就看到 lab.runs() 等，在 ~/.config/marimo/marimo.toml 里写
       [runtime]
       auto_instantiate = true";

const CONFIG_TEMPLATE: &str = r#"# Arboria project configuration. Commit this file with your code.
# Machine (cluster) used when a run does not name one; empty = none yet.
default_machine = ""

{project}
[budget]
# Total service units (SU) available for this project; 0 = no budget set.
total_su = 0

[review]
# Rules a run is reviewed against before it is submitted (not enforced yet).
checklist = [
    "Change a single variable relative to the parent/baseline run, and state it in the change reason.",
    "Compare against a named baseline run that uses the same data split, metric and seed policy.",
    "Evaluate at the same frequency (every N steps/epochs) as the baseline so curves are comparable.",
]

[claudemd]
# After `arb` updates its block in the main worktree's CLAUDE.md, commit that file
# (and only that file) with message "arboria: update CLAUDE.md".
autocommit = true

[watch]
# 观察期：作业开跑后这么多分钟内，arb watch 检查日志末尾；某一行匹配下面任一正则
# （Python re 语法）就取消这个 run 的全部作业并通知。默认只认 loss 为 NaN 和 CUDA OOM，
# 像 "0 NaN batches skipped" 这样的行不会触发。
observation_minutes = {minutes}
fail_patterns = [{patterns}]

[wandb]
# 训练代码里 wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)。entity 和 project 都填了，
# arb submit 就会把 run 的 wandb 链接记进账本（notebook 里 lab 读曲线也用它们）。
entity = ""
project = ""

[run]
# 默认训练命令：arb submit 不带 `-- CMD`、或 notebook 里 lab.run() 时用它。
command = "{command}"
# --set k=v（lab.run(set=...)）怎么接到命令后面：hydra → k=v；argparse / tyro → --k v
override_style = "{style}"

[claude]
# notebook 里调用一个还不存在的命令（lab.svd(...)）时，除了写待办，还把待办发进这个 idea
# worktree 里正在运行的 Claude Code 会话（本机的会话 socket，不经过网络）。false：只写待办。
push = true
"#;

/// The `arboria.toml` written by `arb init` for a new project id.
pub fn config_template(project_id: &ProjectId) -> String {
    let patterns = DEFAULT_FAIL_PATTERNS
        .iter()
        .map(|p| format!("'{p}'"))
        .collect::<Vec<_>>()
        .join(", ");
    // one pass, like str.format: a placeholder's value is never re-scanned
    let mut out = String::new();
    let mut rest = CONFIG_TEMPLATE;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find('}').expect("closed placeholder") + i;
        match &rest[i + 1..end] {
            "project" => out.push_str(&project_table(project_id)),
            "minutes" => out.push_str(&DEFAULT_OBSERVATION_MINUTES.to_string()),
            "patterns" => out.push_str(&patterns),
            "command" => out.push_str(DEFAULT_RUN_COMMAND),
            "style" => out.push_str(DEFAULT_OVERRIDE_STYLE),
            other => unreachable!("unknown placeholder {other}"),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Add a project id to an `arboria.toml` that lacks one; returns it when added.
pub fn ensure_project_id(ctx: &RepoContext) -> Result<Option<ProjectId>> {
    let path = ctx.config_path();
    if project_config(ctx)?.project_id.is_some() {
        return Ok(None); // also validates the file
    }
    let pid = derive_project_id(ctx);
    let text = std::fs::read_to_string(&path)?;
    let new = add_project_id(&text, &pid)?;
    let name = path.file_name().expect("a file").to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.arboria-tmp"));
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Some(pid))
}

/// `arb init`: the actions taken (empty: already initialized).
pub fn init_state(ctx: &RepoContext, warn: Warn<'_>) -> Result<Vec<String>> {
    let mut actions = Vec::new();
    let state = ctx.state_dir();
    if !state.is_dir() {
        std::fs::create_dir_all(&state)?;
        actions.push(format!("created {}", state.display()));
    }
    if ensure_state_excluded(&ctx.common_dir)? {
        actions.push(format!(
            "added /.arboria/ to {}",
            ctx.common_dir.join("info").join("exclude").display()
        ));
    }
    let had_db = ctx.db_path().exists();
    let had_backup_repo = state.join(".git").exists();
    init_backup_repo(&state)?;
    if !had_backup_repo {
        actions.push(format!("initialized backup repo in {}", state.display()));
    }
    {
        let ledger = Ledger::open(&state)?; // creates + migrates the db
        if !had_db {
            actions.push(format!("created ledger {}", ctx.db_path().display()));
        }
        if ledger.backup("init")? {
            actions.push("committed initial ledger backup".into());
        }
    }
    let config = ctx.config_path();
    if !config.exists() {
        std::fs::write(&config, config_template(&derive_project_id(ctx)))?;
        actions.push(format!(
            "wrote config template {} (commit it)",
            config.display()
        ));
    } else if let Some(added) = ensure_project_id(ctx)? {
        actions.push(format!(
            "added [project] id = \"{added}\" to {} (commit it)",
            config.display()
        ));
    }
    let synced = Ledger::open(&state).and_then(|ledger| claudemd::sync(ctx, &ledger, true));
    match synced {
        // e.g. malformed markers: init itself still succeeded
        Err(e) if e.is_arb() => warn(&format!("CLAUDE.md not updated: {e}")),
        Err(e) => return Err(e),
        Ok(result) => {
            if result.changed() {
                actions.extend(result.messages());
            }
        }
    }
    Ok(actions)
}
