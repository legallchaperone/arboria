"""Claude Code SessionStart hook: inject a brief ledger status into new sessions."""

from __future__ import annotations

import json
import os
import traceback
from pathlib import Path
from typing import IO

from . import sessions
from .db import Ledger, now
from .paths import Home, RepoContext, home, locate
from .report import home_status, status_data, status_parts

MAX_CONTEXT_CHARS = 4000


def _hook_cwd(stdin_text: str) -> Path:
    try:
        payload = json.loads(stdin_text) if stdin_text.strip() else {}
        cwd = payload.get("cwd") if isinstance(payload, dict) else None
        if isinstance(cwd, str) and cwd and Path(cwd).is_dir():
            return Path(cwd)
    except ValueError:
        pass
    return Path(os.getcwd())


def cap(text: str, limit: int = MAX_CONTEXT_CHARS) -> str:
    if len(text) <= limit:
        return text
    marker = "\n… (truncated; run `arb status` for the full view)"
    return text[: limit - len(marker)] + marker


def _json(text: str) -> str:
    return json.dumps(
        {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": text}},
        ensure_ascii=False,
    )


def build_home_output(h: Home) -> str | None:
    """In the seed home: the seeds at a glance and the open todos."""
    if not h.initialized:
        return None
    try:
        sessions.record(h.state_dir, h.root)
    except Exception:
        _log_error(None, h.state_dir)
    with Ledger(h.state_dir) as ledger:
        text = home_status(h, ledger, brief=True)
    return _json(cap(text))


def build_output(cwd: Path) -> str | None:
    """The hook's stdout JSON, or None when cwd is neither in an initialized arboria repo
    nor in the seed home."""
    h = home()
    if h.contains(cwd):
        return build_home_output(h)
    try:
        ctx = locate(cwd)
    except Exception:  # not a git repo, git missing, ...
        return None
    if not ctx.initialized:
        return None
    remember_session(ctx)
    with Ledger(ctx.state_dir) as ledger:
        head, tail = status_parts(status_data(ctx, ledger), brief=True)
    # Lessons + rules (tail) are kept whole; the state part is truncated to fit.
    budget = max(MAX_CONTEXT_CHARS - len(tail) - 1, 500)
    text = cap(cap(head, budget) + "\n" + tail)
    return _json(text)


def remember_session(ctx: RepoContext) -> None:
    """Record this session's inbox socket for its worktree (see sessions.py); silent."""
    try:
        if ctx.toplevel is not None:
            sessions.record(ctx.state_dir, ctx.toplevel)
    except Exception:
        _log_error(ctx.cwd)


def _log_error(cwd: Path | None, state: Path | None = None) -> None:
    try:
        if state is None:
            h = home()
            state = h.state_dir if cwd is not None and h.contains(cwd) else locate(cwd).state_dir
        if state.is_dir():
            with open(state / "hook.log", "a", encoding="utf-8") as fh:
                fh.write(f"--- {now()} session-start hook error (cwd={cwd})\n")
                fh.write(traceback.format_exc())
    except Exception:
        pass


def session_start(stdin: IO[str], stdout: IO[str]) -> int:
    """Run the hook. Always returns 0; never raises."""
    cwd: Path | None = None
    try:
        text = "" if stdin is None or stdin.isatty() else stdin.read()
        cwd = _hook_cwd(text)
        output = build_output(cwd)
        if output:
            stdout.write(output + "\n")
            stdout.flush()
    except Exception:
        _log_error(cwd)
    return 0

