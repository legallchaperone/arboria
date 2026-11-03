"""`arb ui`: start `marimo run` on the dashboard shipped in this package.

The dashboard is bound to 127.0.0.1, gets the project root (and the idea to select)
as CLI args after `--` and as environment variables, and uses marimo's session token.
marimo must be importable from arb's own environment (the optional extra `ui`): the
app imports arboria, so a separately installed marimo tool could not run it.
"""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

from ..errors import ArbError
from . import APP
from .data import ReadOnlyLedger

HOST = "127.0.0.1"
NO_MARIMO = ("arb ui 需要 marimo（arboria 的可选依赖 ui），而且要装在 arb 自己的环境里："
             "在 arboria 源码目录运行 `uv tool install --force '.[ui]'`"
             "（或 `pip install 'arboria[ui]'`）")


def marimo_available() -> bool:
    return importlib.util.find_spec("marimo") is not None


def check_idea(state_dir: Path, name: str) -> None:
    with ReadOnlyLedger(state_dir) as ledger:
        if ledger.one("SELECT id FROM ideas WHERE name = ?", (name,)) is None:
            raise ArbError(f"no idea named {name!r} (see `arb idea list`)")


def idea_of_worktree(state_dir: Path, toplevel: Path | None) -> str | None:
    """The idea whose worktree is `toplevel` (where `arb ui` was started), if any."""
    if toplevel is None:
        return None
    with ReadOnlyLedger(state_dir) as ledger:
        row = ledger.one("SELECT name FROM ideas WHERE worktree_path = ? ORDER BY id DESC"
                         " LIMIT 1", (str(toplevel),))
    return row["name"] if row else None


def command(root: Path, idea: str | None = None, port: int | None = None) -> list[str]:
    cmd = [sys.executable, "-m", "marimo", "run", str(APP), "--host", HOST, "--token",
           "--no-sandbox"]
    if port is not None:
        cmd += ["--port", str(port)]
    cmd += ["--", "--root", str(root)]
    if idea:
        cmd += ["--idea", idea]
    return cmd


def run_marimo(cmd: list[str], env: dict[str, str]) -> int:
    """Run marimo in the foreground until the human stops it (tests replace this)."""
    try:
        return subprocess.call(cmd, env=env)
    except KeyboardInterrupt:
        return 0


def launch(root: Path, *, toplevel: Path | None = None, idea: str | None = None,
           port: int | None = None) -> int:
    if port is not None and not 1 <= port <= 65535:
        raise ArbError("--port must be between 1 and 65535")
    if not marimo_available():
        raise ArbError(NO_MARIMO)
    state_dir = Path(root) / ".arboria"
    if idea:
        check_idea(state_dir, idea)
    else:
        idea = idea_of_worktree(state_dir, toplevel)
    env = dict(os.environ, ARBORIA_UI_ROOT=str(root))
    env.pop("ARBORIA_UI_IDEA", None)
    if idea:
        env["ARBORIA_UI_IDEA"] = idea
    return run_marimo(command(root, idea, port), env)
