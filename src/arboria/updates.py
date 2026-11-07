"""What changed in the ledger since this Claude Code session last looked (ADR 0004).

The UserPromptSubmit hook adds at most one line ("r031 结束：val/loss 2.29；新待办 t004")
when something happened since the session's previous prompt, and nothing otherwise. We
never push into the session; the session picks this up when the human next writes.

Per-session state is the last ledger event id seen, in `.arboria/seen/<hash of the
session id>.json` (0600, not in the ledger and not backed up). SessionStart records the
id the session started at, so the first prompt reports only what happened after it.
Read only, apart from that file. Every failure is silent (see hook.py).
"""

from __future__ import annotations

import hashlib
import json
import os
import time
from pathlib import Path
from typing import Any

from . import analysis
from .db import Ledger
from .tree import metric_key

SEEN_DIR = "seen"
KEEP_DAYS = 14  # seen files of sessions older than this are removed
MAX_LINE = 400
# The events a working session wants to hear about, in the order they are listed.
KINDS = ("run_final", "run_approve", "run_reject", "todo_new", "lesson_accept", "milestone")


def seen_path(state_dir: Path, session_id: str) -> Path:
    digest = hashlib.sha256(session_id.encode("utf-8")).hexdigest()[:24]
    return state_dir / SEEN_DIR / f"{digest}.json"


def last_event(ledger: Ledger) -> int:
    row = ledger.one("SELECT MAX(id) AS n FROM events")
    return int(row["n"] or 0) if row else 0


def read_seen(state_dir: Path, session_id: str) -> int | None:
    try:
        data = json.loads(seen_path(state_dir, session_id).read_text(encoding="utf-8"))
        n = data.get("event")
        return n if isinstance(n, int) and not isinstance(n, bool) and n >= 0 else None
    except (OSError, ValueError, AttributeError):
        return None


def write_seen(state_dir: Path, session_id: str, event_id: int) -> None:
    path = seen_path(state_dir, session_id)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as fh:
        json.dump({"event": event_id}, fh)
    os.replace(tmp, path)
    _clean(path.parent)


def _clean(folder: Path) -> None:
    cutoff = time.time() - KEEP_DAYS * 86400
    for p in folder.glob("*.json"):
        try:
            if p.stat().st_mtime < cutoff:
                p.unlink()
        except OSError:
            pass


def run_result(state_dir: Path, ledger: Ledger, run_id: str, metric: str = "") -> str | None:
    """`val/loss 2.29` for a run whose metrics.jsonl was fetched, else None."""
    row = ledger.one("SELECT i.metric FROM runs r JOIN ideas i ON i.id = r.idea_id WHERE r.id = ?",
                     (run_id,))
    series = analysis.read_metrics(state_dir, run_id)
    key = metric or metric_key((row or {}).get("metric") or "", set(series))
    pts = series.get(key or "")
    if not pts:
        return None
    return f"{key} {analysis.fmt_num(pts[-1][1])}"


def _payload(e: dict[str, Any]) -> dict[str, Any]:
    try:
        p = json.loads(e["payload_json"] or "{}")
        return p if isinstance(p, dict) else {}
    except ValueError:
        return {}


def describe(state_dir: Path, ledger: Ledger, events: list[dict[str, Any]],
             metric: str = "") -> str | None:
    """One line for the events (oldest first), or None when none matter."""
    finals: list[str] = []
    approved: list[str] = []
    rejected: list[str] = []
    todos: list[str] = []
    lessons: list[str] = []
    milestones: list[str] = []
    for e in events:
        kind, subject = e["kind"], e["subject"] or "?"
        if kind == "run_final":
            status = _payload(e).get("status") or "结束"
            result = run_result(state_dir, ledger, subject, metric)
            finals.append(f"{subject} {status}" + (f"（{result}）" if result else ""))
        elif kind == "run_approve":
            approved.append(subject)
        elif kind == "run_reject":
            rejected.append(subject)
        elif kind == "todo_new":
            todos.append(subject)
        elif kind == "lesson_accept":
            lessons.append(subject)
        elif kind == "milestone" and subject not in milestones:
            milestones.append(subject)
    parts = []
    if finals:
        parts.append("run 结束：" + "、".join(finals))
    if approved:
        parts.append("你批准了 " + "、".join(approved))
    if rejected:
        parts.append("你退回了 " + "、".join(rejected))
    if todos:
        parts.append("新待办 " + "、".join(todos) + "（`arb todo list`）")
    if lessons:
        parts.append("新确认的经验 " + "、".join(lessons) + "（`arb lesson list`）")
    if milestones:
        parts.append("路线图改了：" + "、".join(milestones) + "（`arb milestone list`）")
    if not parts:
        return None
    line = "[arboria] 自上一条消息：" + "；".join(parts) + "。"
    if len(line) > MAX_LINE:
        line = line[: MAX_LINE - len("…（`arb status`）")] + "…（`arb status`）"
    return line


def since(state_dir: Path, ledger: Ledger, session_id: str, metric: str = "") -> str | None:
    """The line for this session's next prompt, and remember what it has now seen. A
    session we have no record of (the hook was installed mid-session) starts from now."""
    top = last_event(ledger)
    seen = read_seen(state_dir, session_id)
    if seen is None or seen > top:
        write_seen(state_dir, session_id, top)
        return None
    if seen == top:
        return None
    marks = ", ".join("?" * len(KINDS))
    events = ledger.all(f"SELECT kind, subject, payload_json FROM events WHERE id > ? AND id <= ?"
                        f" AND kind IN ({marks}) ORDER BY id", (seen, top, *KINDS))
    line = describe(state_dir, ledger, events, metric)
    write_seen(state_dir, session_id, top)
    return line
