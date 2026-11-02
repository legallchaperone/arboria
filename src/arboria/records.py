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
    worktree = Path(idea["worktree_path"])
    if not worktree.is_dir():
        raise ArbError(f"idea worktree {worktree} is missing")
    with ledger.write() as conn:
        run_id = ledger.next_id("runs", "r")
        snap = snapshot.snapshot(
            worktree, conn, f"arboria snapshot {run_id}",
            allow_large=allow_large, warn=warn, file_limit=file_limit, total_limit=total_limit,
        )
        snapshot.update_ref(worktree, run_ref(run_id), snap.commit, f"arboria: run {run_id}")
        ts = now()
        conn.execute(
            "INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason,"
            " created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (run_id, idea["id"], snap.commit, snap.tree, reason, ts, ts),
        )
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


def add_note(
    ledger: Ledger,
    idea: dict[str, Any],
    text: str,
    run_id: str | None = None,
    confidence: str | None = None,
) -> dict[str, Any]:
    """Attach a note to the idea; `confidence` also updates the idea's confidence."""
    text = validate_text("note", text)
    validate_choice("confidence", confidence, CONFIDENCE)
    if run_id is not None and get_run(ledger, run_id)["idea_id"] != idea["id"]:
        raise ArbError(f"run {run_id} belongs to another idea")
    with ledger.write() as conn:
        cur = conn.execute(
            "INSERT INTO notes (idea_id, run_id, text, confidence, created_at) VALUES (?, ?, ?, ?, ?)",
            (idea["id"], run_id, text, confidence, now()),
        )
        if confidence:
            conn.execute("UPDATE ideas SET confidence = ? WHERE id = ?", (confidence, idea["id"]))
        ledger.event("note", idea["name"], {"note_id": cur.lastrowid, "run": run_id,
                                             "confidence": confidence})
    return ledger.one("SELECT * FROM notes WHERE id = ?", (cur.lastrowid,))


# ---------------------------------------------------------------- decisions
# No CLI for run/idea decisions yet (approval flow is a later step); lessons use them.


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


def set_decision(ledger: Ledger, decision_id: str, choice: str, decided_by: str) -> None:
    """Record the choice of an open decision; call inside `ledger.write()`."""
    dec = ledger.one("SELECT * FROM decisions WHERE id = ?", (decision_id,))
    if dec is None:
        raise ArbError(f"no decision {decision_id!r}")
    if dec["choice"] is not None:
        raise ArbError(f"decision {decision_id} was already decided: {dec['choice']}")
    options = json.loads(dec["options_json"] or "[]")
    if options and choice not in options:
        raise ArbError(f"invalid choice {choice!r}; options: {', '.join(options)}")
    ledger.conn.execute("UPDATE decisions SET choice = ?, decided_by = ? WHERE id = ?",
                        (choice, decided_by, decision_id))
    ledger.event("decision", decision_id, {"action": "decide", "choice": choice, "by": decided_by})


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
