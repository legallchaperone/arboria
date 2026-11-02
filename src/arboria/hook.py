"""Claude Code SessionStart hook: inject a brief ledger status into new sessions."""

from __future__ import annotations

import json
import os
import traceback
from pathlib import Path
from typing import IO

from .db import Ledger, now
from .paths import locate
from .report import format_status, status_data

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


def build_output(cwd: Path) -> str | None:
    """The hook's stdout JSON, or None when cwd is not in an initialized arboria repo."""
    try:
        ctx = locate(cwd)
    except Exception:  # not a git repo, git missing, ...
        return None
    if not ctx.initialized:
        return None
    with Ledger(ctx.state_dir) as ledger:
        text = format_status(status_data(ctx, ledger), brief=True)
    return json.dumps(
        {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": cap(text)}},
        ensure_ascii=False,
    )


def _log_error(cwd: Path | None) -> None:
    try:
        ctx = locate(cwd)
        state = ctx.state_dir
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

