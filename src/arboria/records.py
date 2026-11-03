"""Writes for runs, notes and decisions (all attached to the current idea)."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any, Callable

from . import snapshot
from .db import CONFIDENCE, SUBJECT_TYPES, Ledger, now, validate_choice, validate_text
from .errors import ArbError

# ---------------------------------------------------------------- runs


def run_ref(run_id: str) -> str:
    return f"refs/arboria/runs/{run_id}"


def insert_run_at(
    ledger: Ledger,
    idea: dict[str, Any],
    reason: str,
    commit: str,
    tree: str,
    *,
    parent_run: str | None = None,
) -> str:
    """Insert a draft run at an existing snapshot commit (+ its ref); call inside `write()`."""
    reason = validate_text("change_reason", reason)
    worktree = Path(idea["worktree_path"])
    if not worktree.is_dir():
        raise ArbError(f"idea worktree {worktree} is missing")
    run_id = ledger.next_id("runs", "r")
    snapshot.update_ref(worktree, run_ref(run_id), commit, f"arboria: run {run_id}")
    ts = now()
    ledger.conn.execute(
        "INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, parent_run,"
        " created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (run_id, idea["id"], commit, tree, reason, parent_run, ts, ts),
    )
    return run_id


def take_snapshot(
    ledger: Ledger,
    idea: dict[str, Any],
    *,
    allow_large: bool = False,
    warn: Callable[[str], None] | None = None,
    file_limit: int | None = None,
    total_limit: int | None = None,
) -> snapshot.SnapshotResult:
    """Snapshot the idea's worktree (no ledger write; the commit is anchored by a run ref)."""
    worktree = Path(idea["worktree_path"])
    if not worktree.is_dir():
        raise ArbError(f"idea worktree {worktree} is missing")
    return snapshot.snapshot(
        worktree, ledger.conn, f"arboria snapshot {ledger.next_id('runs', 'r')}",
        allow_large=allow_large, warn=warn, file_limit=file_limit, total_limit=total_limit,
    )


def insert_run(
    ledger: Ledger,
    idea: dict[str, Any],
    reason: str,
    *,
    allow_large: bool = False,
    warn: Callable[[str], None] | None = None,
    file_limit: int | None = None,
    total_limit: int | None = None,
) -> tuple[str, snapshot.SnapshotResult]:
    """Snapshot the idea's worktree and insert a draft run; call inside `ledger.write()`."""
    reason = validate_text("change_reason", reason)
    snap = take_snapshot(ledger, idea, allow_large=allow_large, warn=warn,
                         file_limit=file_limit, total_limit=total_limit)
    run_id = insert_run_at(ledger, idea, reason, snap.commit, snap.tree)
    return run_id, snap


def new_run(
    ledger: Ledger,
    idea: dict[str, Any],
    reason: str,
    *,
    allow_large: bool = False,
    warn: Callable[[str], None] | None = None,
    file_limit: int | None = None,
    total_limit: int | None = None,
) -> dict[str, Any]:
    """Snapshot the idea's worktree and record a draft run with ref refs/arboria/runs/<id>."""
    reason = validate_text("change_reason", reason)
    with ledger.write():
        run_id, snap = insert_run(ledger, idea, reason, allow_large=allow_large, warn=warn,
                                  file_limit=file_limit, total_limit=total_limit)
        ledger.event("run_new", run_id, {"idea": idea["name"], "commit": snap.commit,
                                          "tree": snap.tree, "reused": snap.reused})
    run = get_run(ledger, run_id)
    return run | {"ref": run_ref(run_id), "reused_snapshot": snap.reused, "warnings": snap.warnings}


def get_run(ledger: Ledger, run_id: str) -> dict[str, Any]:
    run = ledger.one("SELECT * FROM runs WHERE id = ?", (run_id,))
    if run is None:
        raise ArbError(f"no run {run_id!r}")
    return run


def set_run_summary(ledger: Ledger, idea: dict[str, Any], run_id: str, summary: str) -> dict[str, Any]:
    summary = validate_text("summary", summary)
    run = get_run(ledger, run_id)
    if run["idea_id"] != idea["id"]:
        raise ArbError(f"run {run_id} belongs to another idea")
    with ledger.write() as conn:
        conn.execute("UPDATE runs SET summary = ?, updated_at = ? WHERE id = ?", (summary, now(), run_id))
        ledger.event("run_summary", run_id, {"chars": len(summary)})
    return get_run(ledger, run_id)


# ---------------------------------------------------------------- notes


def insert_note(ledger: Ledger, idea_id: int, text: str, run_id: str | None = None,
                confidence: str | None = None, author: str = "human") -> int:
    """Insert a note row; call inside `ledger.write()`. Notes are written by the human."""
    cur = ledger.conn.execute(
        "INSERT INTO notes (idea_id, run_id, text, confidence, created_at, author)"
        " VALUES (?, ?, ?, ?, ?, ?)", (idea_id, run_id, text, confidence, now(), author))
    return cur.lastrowid


def add_note(
    ledger: Ledger,
    idea: dict[str, Any],
    text: str,
    run_id: str | None = None,
    confidence: str | None = None,
) -> dict[str, Any]:
    """Attach a (human) note to the idea; `confidence` also updates the idea's confidence."""
    text = validate_text("note", text)
    validate_choice("confidence", confidence, CONFIDENCE)
    if run_id is not None and get_run(ledger, run_id)["idea_id"] != idea["id"]:
        raise ArbError(f"run {run_id} belongs to another idea")
    with ledger.write() as conn:
        note_id = insert_note(ledger, idea["id"], text, run_id, confidence)
        if confidence:
            conn.execute("UPDATE ideas SET confidence = ? WHERE id = ?", (confidence, idea["id"]))
        ledger.event("note", idea["name"], {"note_id": note_id, "run": run_id,
                                             "confidence": confidence, "author": "human"})
    return ledger.one("SELECT * FROM notes WHERE id = ?", (note_id,))


def set_confidence(ledger: Ledger, idea: dict[str, Any], confidence: str) -> dict[str, Any]:
    if confidence not in CONFIDENCE:
        raise ArbError(f"invalid confidence {confidence!r}; choose one of: {', '.join(CONFIDENCE)}")
    with ledger.write() as conn:
        conn.execute("UPDATE ideas SET confidence = ? WHERE id = ?", (confidence, idea["id"]))
        ledger.event("idea_confidence", idea["name"], {"from": idea["confidence"],
                                                        "to": confidence, "author": "human"})
    return ledger.one("SELECT * FROM ideas WHERE id = ?", (idea["id"],))


def set_metric(ledger: Ledger, idea: dict[str, Any], metric: str) -> dict[str, Any]:
    metric = validate_text("metric", metric)
    with ledger.write() as conn:
        conn.execute("UPDATE ideas SET metric = ? WHERE id = ?", (metric, idea["id"]))
        ledger.event("idea_metric", idea["name"], {"from": idea["metric"], "author": "human"})
    return ledger.one("SELECT * FROM ideas WHERE id = ?", (idea["id"],))


# ---------------------------------------------------------------- decisions
# Lessons and run approvals (see submission) create and decide these.


def insert_decision(
    ledger: Ledger, subject_type: str, subject_id: str, question: str, options: list[str]
) -> str:
    """Insert an open decision; call inside `ledger.write()`. Returns its id."""
    validate_choice("subject_type", subject_type, SUBJECT_TYPES)
    dec_id = ledger.next_id("decisions", "d")
    ledger.conn.execute(
        "INSERT INTO decisions (id, subject_type, subject_id, question, options_json, created_at)"
        " VALUES (?, ?, ?, ?, ?, ?)",
        (dec_id, subject_type, subject_id, question, json.dumps(options, ensure_ascii=False), now()),
    )
    ledger.event("decision", dec_id, {"action": "open", "subject": f"{subject_type}:{subject_id}"})
    return dec_id


def set_decision(ledger: Ledger, decision_id: str, choice: str, decided_by: str,
                 payload: dict[str, Any] | None = None) -> None:
    """Record the choice of an open decision; call inside `ledger.write()`.

    `payload` (e.g. the approving process chain) is stored on the decision."""
    dec = ledger.one("SELECT * FROM decisions WHERE id = ?", (decision_id,))
    if dec is None:
        raise ArbError(f"no decision {decision_id!r}")
    if dec["choice"] is not None:
        raise ArbError(f"decision {decision_id} was already decided: {dec['choice']}")
    options = json.loads(dec["options_json"] or "[]")
    if options and choice not in options:
        raise ArbError(f"invalid choice {choice!r}; options: {', '.join(options)}")
    payload_json = None if payload is None else json.dumps(payload, ensure_ascii=False,
                                                           sort_keys=True)
    ledger.conn.execute("UPDATE decisions SET choice = ?, decided_by = ?, payload_json = ?"
                        " WHERE id = ?", (choice, decided_by, payload_json, decision_id))
    ledger.event("decision", decision_id, {"action": "decide", "choice": choice, "by": decided_by}
                 | ({"payload": payload} if payload else {}))


def open_decision(
    ledger: Ledger, subject_type: str, subject_id: str, question: str, options: list[str]
) -> dict[str, Any]:
    validate_choice("subject_type", subject_type, SUBJECT_TYPES)
    with ledger.write():
        dec_id = insert_decision(ledger, subject_type, subject_id, question, options)
    return ledger.one("SELECT * FROM decisions WHERE id = ?", (dec_id,))


def decide(ledger: Ledger, decision_id: str, choice: str, decided_by: str) -> dict[str, Any]:
    with ledger.write():
        set_decision(ledger, decision_id, choice, decided_by)
    return ledger.one("SELECT * FROM decisions WHERE id = ?", (decision_id,))
