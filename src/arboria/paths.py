"""Locate the main worktree and the shared `.arboria/` state directory."""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from . import gitutil
from .errors import NotARepoError, NotInitializedError

STATE_DIR_NAME = ".arboria"
DB_NAME = "ledger.db"
CONFIG_NAME = "arboria.toml"


@dataclass(frozen=True)
class RepoContext:
    """Where we are: the current worktree and the repo-wide state location."""

    cwd: Path
    common_dir: Path  # shared git dir (`.git` of the main worktree)
    main: Path  # main worktree root
    toplevel: Path | None  # root of the worktree containing cwd (None inside .git etc.)

    @property
    def state_dir(self) -> Path:
        return self.main / STATE_DIR_NAME

    @property
    def db_path(self) -> Path:
        return self.state_dir / DB_NAME

    @property
    def config_path(self) -> Path:
        return self.main / CONFIG_NAME

    @property
    def initialized(self) -> bool:
        return self.db_path.is_file()

    @property
    def in_main(self) -> bool:
        return self.toplevel == self.main

    def require_initialized(self) -> "RepoContext":
        if not self.initialized:
            raise NotInitializedError(
                f"arboria is not initialized for {self.main}; run `arb init` there first"
            )
        return self


def locate(cwd: Path | str | None = None) -> RepoContext:
    """Resolve the repository context for `cwd` (default: process cwd)."""
    cwd = Path(cwd if cwd is not None else os.getcwd()).resolve()
    proc = gitutil.run(
        ["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd, check=False
    )
    if proc.returncode != 0:
        raise NotARepoError(f"{cwd} is not inside a git repository")
    common_dir = Path(proc.stdout).resolve()
    top = gitutil.run(["rev-parse", "--show-toplevel"], cwd, check=False)
    toplevel = Path(top.stdout).resolve() if top.returncode == 0 and top.stdout else None
    return RepoContext(cwd=cwd, common_dir=common_dir, main=common_dir.parent, toplevel=toplevel)


def ensure_excluded(common_dir: Path, pattern: str = f"/{STATE_DIR_NAME}/") -> bool:
    """Add `pattern` to `<common git dir>/info/exclude` once. Returns True if added."""
    exclude = common_dir / "info" / "exclude"
    exclude.parent.mkdir(parents=True, exist_ok=True)
    text = exclude.read_text() if exclude.exists() else ""
    if pattern in (line.strip() for line in text.splitlines()):
        return False
    if text and not text.endswith("\n"):
        text += "\n"
    exclude.write_text(text + f"# arboria state (local only)\n{pattern}\n")
    return True
