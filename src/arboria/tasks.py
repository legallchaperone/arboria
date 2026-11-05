"""Todos: written by the human (in a notebook, `lab.todo`), done by Claude (`arb todo done`;
an inspect todo is delivered with `arb inspect done`, see inspection.py).

The todo text is the human's; the result is one of the free-text fields an LLM may
write (≤600 chars, see db.LIMITS). A todo belongs to an idea (a project ledger) or to a
seed (the home ledger); rows carry both names, `idea` and `seed`, one of them None.
"""

from __future__ import annotations

import json
from typing import Any

from .db import TASK_KINDS, TASK_STATUS, Ledger, now, validate_choice, validate_text
from .errors import ArbError


SELECT = ("SELECT t.*, i.name AS idea, s.name AS seed FROM tasks t"
          " LEFT JOIN ideas i ON i.id = t.idea_id LEFT JOIN seeds s ON s.id = t.seed_id")
# The todo's idea is open, or its seed is not dropped.
LIVE = "(i.status = 'open' OR s.status IN ('thinking', 'planted'))"
# A schema v8 ledger (arb-rs, read-only here): every todo has an `idea_id`, a seed is an
# ideas row without a branch. The same columns as SELECT, `seed_id` included.
SELECT_V8 = ("SELECT t.*, CASE WHEN i.branch IS NOT NULL THEN i.name END AS idea,"
             " CASE WHEN i.branch IS NULL THEN i.name END AS seed,"
             " CASE WHEN i.branch IS NULL THEN t.idea_id END AS seed_id"
             " FROM tasks t LEFT JOIN ideas i ON i.id = t.idea_id")
LIVE_V8 = "(i.stage IN ('open', 'seed'))"


def sql(ledger: Ledger) -> tuple[str, str]:
    """(SELECT, LIVE) for this ledger's schema."""
    return (SELECT_V8, LIVE_V8) if ledger.read_only else (SELECT, LIVE)


def is_seed(owner: dict[str, Any]) -> bool:
    """A seeds row (it has a gist column) rather than an ideas row."""
    return "gist" in owner


def owner_name(task: dict[str, Any]) -> str:
    return task.get("idea") or task.get("seed") or "?"


def get_task(ledger: Ledger, task_id: str) -> dict[str, Any]:
    task = ledger.one(sql(ledger)[0] + " WHERE t.id = ?", (task_id,))
    if task is None:
        raise ArbError(f"no todo {task_id!r} (see `arb todo list`)")
    return task


def payload(task: dict[str, Any]) -> dict[str, Any]:
    """The task's payload_json as a dict ({} for plain todos)."""
    raw = task.get("payload_json")
    return json.loads(raw) if raw else {}


def add(ledger: Ledger, owner: dict[str, Any], text: str, kind: str = "plain",
        data: dict[str, Any] | None = None) -> dict[str, Any]:
    """A new open todo of an idea or a seed (`owner` is its row). `kind` / `data`
    (payload_json) are set by code, never by an LLM."""
    text = validate_text("todo", text)
    validate_choice("todo kind", kind, TASK_KINDS)
    seed = is_seed(owner)
    if seed and owner["status"] == "dropped":
        raise ArbError(f"seed {owner['name']!r} is dropped")
    if not seed and owner["status"] != "open":
        raise ArbError(f"idea {owner['name']!r} is {owner['status']}")
    raw = json.dumps(data, ensure_ascii=False, sort_keys=True) if data is not None else None
    with ledger.write() as conn:
        task_id = ledger.next_id("tasks", "t")
        conn.execute("INSERT INTO tasks (id, idea_id, seed_id, text, status, created_at, kind,"
                     " payload_json) VALUES (?, ?, ?, ?, 'open', ?, ?, ?)",
                     (task_id, None if seed else owner["id"], owner["id"] if seed else None,
                      text, now(), kind, raw))
        ledger.event("todo_new", task_id, {"seed" if seed else "idea": owner["name"],
                                           "author": "human", "kind": kind})
    return get_task(ledger, task_id)


def open_plain_task(ledger: Ledger, owner_id: int, text: str, *,
                    seed: bool = False) -> dict[str, Any] | None:
    """The oldest open plain todo of the (live) idea or seed with exactly this text."""
    select, live = sql(ledger)
    col = "t.seed_id" if seed and not ledger.read_only else "t.idea_id"
    return ledger.one(select + f" WHERE {col} = ? AND {live} AND t.status = 'open'"
                      " AND t.kind = 'plain' AND t.text = ? ORDER BY t.id LIMIT 1",
                      (owner_id, text.strip()))


def open_command_tasks(ledger: Ledger, name: str, kind: str | None = None
                       ) -> list[dict[str, Any]]:
    """Open command todos (of open ideas / live seeds) about the command `name`, oldest first."""
    kinds = (kind,) if kind else ("command_new", "command_revise")
    marks = ", ".join("?" for _ in kinds)
    select, live = sql(ledger)
    rows = ledger.all(
        select + f" WHERE t.status = 'open' AND {live} AND t.kind IN ({marks}) ORDER BY t.id",
        kinds)
    return [r for r in rows if payload(r).get("name") == name]


def list_tasks(ledger: Ledger, idea_id: int | None = None,
               include_closed: bool = False, *, seed_id: int | None = None
               ) -> list[dict[str, Any]]:
    """Todos oldest first; by default only open ones (of open ideas and live seeds when
    neither idea_id nor seed_id is given)."""
    select, live = sql(ledger)
    where, params = [], []
    if idea_id is not None:
        where.append("t.idea_id = ?")
        params.append(idea_id)
    elif seed_id is not None:
        where.append("t.idea_id = ?" if ledger.read_only else "t.seed_id = ?")
        params.append(seed_id)
    else:
        where.append(live)
    if not include_closed:
        where.append("t.status = 'open'")
    query = select + " WHERE " + " AND ".join(where) + " ORDER BY t.id"
    return ledger.all(query, tuple(params))


def _close(ledger: Ledger, task_id: str, status: str, result: str | None) -> dict[str, Any]:
    assert status in TASK_STATUS
    task = get_task(ledger, task_id)
    if task["status"] != "open":
        raise ArbError(f"todo {task_id} is already {task['status']}")
    with ledger.write() as conn:
        conn.execute("UPDATE tasks SET status = ?, done_at = ?, result = ? WHERE id = ?",
                     (status, now(), result, task_id))
        kind = "todo_done" if status == "done" else "todo_drop"
        owner = {"seed": task["seed"]} if task["seed"] else {"idea": task["idea"]}
        ledger.event(kind, task_id, owner | {"result_chars": len(result) if result else 0})
    return get_task(ledger, task_id)


INSPECT_DONE = ("{id} 是读代码待办：用 `arb inspect done {id} --file …` 交付答案（代码由 arb 从 "
                "git 取），约定见 `arb inspect guide`；做不了就 `arb todo drop {id}`")


def done(ledger: Ledger, task_id: str, result: str) -> dict[str, Any]:
    if get_task(ledger, task_id).get("kind") == "inspect":
        raise ArbError(INSPECT_DONE.format(id=task_id))
    return _close(ledger, task_id, "done", validate_text("result", result))


def drop(ledger: Ledger, task_id: str, reason: str | None = None) -> dict[str, Any]:
    return _close(ledger, task_id, "dropped", validate_text("result", reason, required=False))
