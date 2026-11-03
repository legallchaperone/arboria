"""Todos: written by the human (in a notebook, `lab.todo`), done by Claude (`arb todo done`).

The todo text is the human's; the result is one of the free-text fields an LLM may
write (≤600 chars, see db.LIMITS).
"""

from __future__ import annotations

from typing import Any

from .db import TASK_STATUS, Ledger, now, validate_text
from .errors import ArbError


def get_task(ledger: Ledger, task_id: str) -> dict[str, Any]:
    task = ledger.one("SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id"
                      " WHERE t.id = ?", (task_id,))
    if task is None:
        raise ArbError(f"no todo {task_id!r} (see `arb todo list`)")
    return task


def add(ledger: Ledger, idea: dict[str, Any], text: str) -> dict[str, Any]:
    text = validate_text("todo", text)
    if idea["status"] != "open":
        raise ArbError(f"idea {idea['name']!r} is {idea['status']}")
    with ledger.write() as conn:
        task_id = ledger.next_id("tasks", "t")
        conn.execute("INSERT INTO tasks (id, idea_id, text, status, created_at)"
                     " VALUES (?, ?, ?, 'open', ?)", (task_id, idea["id"], text, now()))
        ledger.event("todo_new", task_id, {"idea": idea["name"], "author": "human"})
    return get_task(ledger, task_id)


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
