"""Todos: written by the human (in a notebook, `lab.todo`), done by Claude (`arb todo done`).

The todo text is the human's; the result is one of the free-text fields an LLM may
write (≤600 chars, see db.LIMITS).
"""

from __future__ import annotations

import json
from typing import Any

from .db import TASK_KINDS, TASK_STATUS, Ledger, now, validate_choice, validate_text
from .errors import ArbError


def get_task(ledger: Ledger, task_id: str) -> dict[str, Any]:
    task = ledger.one("SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id"
                      " WHERE t.id = ?", (task_id,))
    if task is None:
        raise ArbError(f"no todo {task_id!r} (see `arb todo list`)")
    return task


def payload(task: dict[str, Any]) -> dict[str, Any]:
    """The task's payload_json as a dict ({} for plain todos)."""
    raw = task.get("payload_json")
    return json.loads(raw) if raw else {}


def add(ledger: Ledger, idea: dict[str, Any], text: str, kind: str = "plain",
        data: dict[str, Any] | None = None) -> dict[str, Any]:
    """A new open todo. `kind` / `data` (payload_json) are set by code, never by an LLM."""
    text = validate_text("todo", text)
    validate_choice("todo kind", kind, TASK_KINDS)
    if idea["status"] != "open":
        raise ArbError(f"idea {idea['name']!r} is {idea['status']}")
    raw = json.dumps(data, ensure_ascii=False, sort_keys=True) if data is not None else None
    with ledger.write() as conn:
        task_id = ledger.next_id("tasks", "t")
        conn.execute("INSERT INTO tasks (id, idea_id, text, status, created_at, kind, payload_json)"
                     " VALUES (?, ?, ?, 'open', ?, ?, ?)",
                     (task_id, idea["id"], text, now(), kind, raw))
        ledger.event("todo_new", task_id, {"idea": idea["name"], "author": "human", "kind": kind})
    return get_task(ledger, task_id)


def open_plain_task(ledger: Ledger, idea_id: int, text: str) -> dict[str, Any] | None:
    """The oldest open plain todo of the (open) idea with exactly this text."""
    return ledger.one("SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id"
                      " WHERE t.idea_id = ? AND i.status = 'open' AND t.status = 'open'"
                      " AND t.kind = 'plain'"
                      " AND t.text = ? ORDER BY t.id LIMIT 1", (idea_id, text.strip()))


def open_command_tasks(ledger: Ledger, name: str, kind: str | None = None
                       ) -> list[dict[str, Any]]:
    """Open command todos (of open ideas) about the command `name`, oldest first."""
    kinds = (kind,) if kind else ("command_new", "command_revise")
    marks = ", ".join("?" for _ in kinds)
    rows = ledger.all(
        "SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id"
        f" WHERE t.status = 'open' AND i.status = 'open' AND t.kind IN ({marks}) ORDER BY t.id",
        kinds)
    return [r for r in rows if payload(r).get("name") == name]


def list_tasks(ledger: Ledger, idea_id: int | None = None,
               include_closed: bool = False) -> list[dict[str, Any]]:
    """Todos oldest first; by default only open ones (of open ideas when idea_id is None)."""
    where, params = [], []
    if idea_id is not None:
        where.append("t.idea_id = ?")
        params.append(idea_id)
    else:
        where.append("i.status = 'open'")
    if not include_closed:
        where.append("t.status = 'open'")
    sql = ("SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id WHERE "
           + " AND ".join(where) + " ORDER BY t.id")
    return ledger.all(sql, tuple(params))


def _close(ledger: Ledger, task_id: str, status: str, result: str | None) -> dict[str, Any]:
    assert status in TASK_STATUS
    task = get_task(ledger, task_id)
    if task["status"] != "open":
        raise ArbError(f"todo {task_id} is already {task['status']}")
    with ledger.write() as conn:
        conn.execute("UPDATE tasks SET status = ?, done_at = ?, result = ? WHERE id = ?",
                     (status, now(), result, task_id))
        kind = "todo_done" if status == "done" else "todo_drop"
        ledger.event(kind, task_id, {"idea": task["idea"],
                                     "result_chars": len(result) if result else 0})
    return get_task(ledger, task_id)


def done(ledger: Ledger, task_id: str, result: str) -> dict[str, Any]:
    return _close(ledger, task_id, "done", validate_text("result", result))


def drop(ledger: Ledger, task_id: str, reason: str | None = None) -> dict[str, Any]:
    return _close(ledger, task_id, "dropped", validate_text("result", reason, required=False))
