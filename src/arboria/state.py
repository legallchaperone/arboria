"""`arb init`: create the state dir, ledger, backup repo and config template."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from . import claudemd
from .config import (
    DEFAULT_FAIL_PATTERNS,
    DEFAULT_OBSERVATION_MINUTES,
    DEFAULT_OVERRIDE_STYLE,
    DEFAULT_RUN_COMMAND,
    add_project_id,
    derive_project_id,
    project_config,
    project_table,
)
from .db import Ledger, init_backup_repo
from .errors import ArbError
from .paths import RepoContext, ensure_excluded

CONFIG_TEMPLATE = """\
# Arboria project configuration. Commit this file with your code.
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
"""


def config_template(project_id: str) -> str:
    patterns = ", ".join(f"'{p}'" for p in DEFAULT_FAIL_PATTERNS)
    return CONFIG_TEMPLATE.format(project=project_table(project_id),
                                  minutes=DEFAULT_OBSERVATION_MINUTES, patterns=patterns,
                                  command=DEFAULT_RUN_COMMAND, style=DEFAULT_OVERRIDE_STYLE)


def ensure_project_id(ctx: RepoContext) -> str | None:
    """Add `[project] id` to an existing arboria.toml that lacks it. Returns the id if added."""
    path = ctx.config_path
    if project_config(ctx).project_id is not None:  # also validates the file
        return None
    pid = derive_project_id(ctx)
    text = path.read_text(encoding="utf-8")
    new = add_project_id(text, pid)
    tmp = path.with_name(f".{path.name}.arboria-tmp")
    tmp.write_text(new, encoding="utf-8")
    tmp.replace(path)
    return pid


def init_state(ctx: RepoContext, warn: Callable[[str], None] | None = None) -> list[str]:
    """Idempotently set up arboria for the repo. Returns a list of actions taken."""
    actions: list[str] = []
    state = ctx.state_dir
    if not state.is_dir():
        state.mkdir(parents=True)
        actions.append(f"created {state}")
    if ensure_excluded(ctx.common_dir):
        actions.append(f"added /.arboria/ to {ctx.common_dir / 'info' / 'exclude'}")
    had_db = ctx.db_path.exists()
    had_backup_repo = (state / ".git").exists()
    init_backup_repo(state)
    if not had_backup_repo:
        actions.append(f"initialized backup repo in {state}")
    with Ledger(state) as ledger:  # creates + migrates the db
        if not had_db:
            actions.append(f"created ledger {ctx.db_path}")
        if ledger.backup("init"):
            actions.append("committed initial ledger backup")
    config: Path = ctx.config_path
    if not config.exists():
        config.write_text(config_template(derive_project_id(ctx)), encoding="utf-8")
        actions.append(f"wrote config template {config} (commit it)")
    else:
        added = ensure_project_id(ctx)
        if added:
            actions.append(f"added [project] id = \"{added}\" to {config} (commit it)")
    try:
        with Ledger(state) as ledger:
            result = claudemd.sync(ctx, ledger)
    except ArbError as exc:  # e.g. malformed markers: init itself still succeeded
        if warn:
            warn(f"CLAUDE.md not updated: {exc}")
    else:
        if result.changed:
            actions.extend(result.messages())
    return actions
