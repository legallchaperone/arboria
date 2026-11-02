"""`arb init`: create the state dir, ledger, backup repo and config template."""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from . import claudemd
from .db import Ledger, init_backup_repo
from .errors import ArbError
from .paths import RepoContext, ensure_excluded

CONFIG_TEMPLATE = """\
# Arboria project configuration. Commit this file with your code.
# Machine (cluster) used when a run does not name one; empty = none yet.
default_machine = ""

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
"""


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
        config.write_text(CONFIG_TEMPLATE, encoding="utf-8")
        actions.append(f"wrote config template {config} (commit it)")
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
