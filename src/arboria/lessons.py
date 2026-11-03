"""Lessons: durable project pitfalls. Claude proposes them; only the human accepts them.

Accepted lessons are rendered into the managed CLAUDE.md block (see claudemd) and
injected by the SessionStart hook.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable

from . import claudemd, human, records
from .db import Ledger, now, validate_text
from .errors import ArbError
from .ideas import current_idea, get_idea
from .paths import RepoContext

QUESTION = "接受这条经验吗？"
OPTIONS = ["accept", "reject"]
CAP_HINT = "retire or merge some first (`arb lesson retire ID`)"


@dataclass
class LessonResult:
    lesson: dict[str, Any]
    sync: claudemd.SyncResult | None = None
    warning: str | None = None


def get_lesson(ledger: Ledger, lesson_id: str) -> dict[str, Any]:
    lesson = ledger.one("SELECT * FROM lessons WHERE id = ?", (lesson_id,))
    if lesson is None:
        raise ArbError(f"no lesson {lesson_id!r} (see `arb lesson list --all`)")
    return lesson


def list_lessons(ledger: Ledger, include_retired: bool = False) -> list[dict[str, Any]]:
    """Lessons newest first; by default only proposed + accepted."""
    where = "" if include_retired else " WHERE status IN ('proposed', 'accepted')"
    return ledger.all(f"SELECT * FROM lessons{where} ORDER BY created_at DESC, id DESC")


def proposed_count(ledger: Ledger) -> int:
    return ledger.one("SELECT COUNT(*) AS n FROM lessons WHERE status = 'proposed'")["n"]


def _open_decision_id(ledger: Ledger, lesson_id: str) -> str | None:
    row = ledger.one(
        "SELECT id FROM decisions WHERE subject_type = 'lesson' AND subject_id = ?"
        " AND choice IS NULL ORDER BY id DESC LIMIT 1",
        (lesson_id,),
    )
    return row["id"] if row else None


def _check_room(ctx: RepoContext, ledger: Ledger, candidate: dict[str, Any]) -> None:
    """Refuse if accepting `candidate` would exceed the lesson count or block line cap."""
    accepted = claudemd.accepted_lessons(ledger)
    if len(accepted) >= claudemd.MAX_ACCEPTED_LESSONS:
        raise ArbError(
            f"there are already {len(accepted)} accepted lessons "
            f"(max {claudemd.MAX_ACCEPTED_LESSONS}); {CAP_HINT}"
        )
    try:
        claudemd.render_block([candidate, *accepted])
    except ArbError as exc:
        raise ArbError(f"accepting this lesson would overflow the CLAUDE.md block "
                       f"(max {claudemd.MAX_BLOCK_LINES} lines); {CAP_HINT}") from exc


def _sync(ctx: RepoContext, ledger: Ledger, result: LessonResult) -> LessonResult:
    try:
        result.sync = claudemd.sync(ctx, ledger)
    except ArbError as exc:  # ledger change is committed; tell the user how to finish
        result.warning = f"CLAUDE.md not updated: {exc}"
    return result


def propose(
    ctx: RepoContext,
    ledger: Ledger,
    text: str,
    *,
    idea: str | None = None,
    run: str | None = None,
) -> dict[str, Any]:
    """Record a proposed lesson plus an open Decision for the human."""
    text = validate_text("lesson", text)
    if idea and run:
        raise ArbError("use only one of --idea / --run")
    if run:
        records.get_run(ledger, run)
        source_type, source_id = "run", run
    elif idea:
        source_type, source_id = "idea", get_idea(ledger, idea)["name"]
    else:
        cur = current_idea(ctx, ledger)
        source_type, source_id = ("idea", cur["name"]) if cur else ("none", None)
    dup = ledger.one("SELECT id, status FROM lessons WHERE text = ? AND status != 'retired'", (text,))
    if dup:
        raise ArbError(f"the same lesson already exists as {dup['id']} ({dup['status']})")
    ts = now()
    _check_room(ctx, ledger, {"text": text, "source_id": source_id, "created_at": ts,
                              "decided_at": ts})
    with ledger.write() as conn:
        lesson_id = ledger.next_id("lessons", "l")
        conn.execute(
            "INSERT INTO lessons (id, text, source_type, source_id, status, created_at)"
            " VALUES (?, ?, ?, ?, 'proposed', ?)",
            (lesson_id, text, source_type, source_id, ts),
        )
        dec_id = records.insert_decision(ledger, "lesson", lesson_id, QUESTION, OPTIONS)
        ledger.event("lesson_propose", lesson_id, {"source": f"{source_type}:{source_id or ''}",
                                                    "decision": dec_id})
    return get_lesson(ledger, lesson_id) | {"decision_id": dec_id}


def accept(ctx: RepoContext, ledger: Ledger, lesson_id: str,
           show: Callable[[str], None] | None = None) -> LessonResult:
    """Human accepts a proposed lesson; decides its Decision and syncs CLAUDE.md.

    Only in the human's own terminal: refused inside Claude Code or without a TTY; the
    human sees the lesson (`show`) and types its id."""
    human.require_terminal(f"arb lesson accept {lesson_id}")
    lesson = get_lesson(ledger, lesson_id)
    if lesson["status"] != "proposed":
        raise ArbError(f"lesson {lesson_id} is {lesson['status']}, not proposed")
    ts = now()
    _check_room(ctx, ledger, lesson | {"decided_at": ts})
    claudemd.check(ctx, [lesson | {"decided_at": ts}, *claudemd.accepted_lessons(ledger)])
    if show is not None:
        show(f"{lesson_id}: {lesson['text']}")
    human.confirm_typed(lesson_id)
    prov = human.provenance()
    with ledger.write() as conn:
        conn.execute("UPDATE lessons SET status = 'accepted', decided_at = ? WHERE id = ?",
                     (ts, lesson_id))
        dec_id = _open_decision_id(ledger, lesson_id)
        if dec_id:
            records.set_decision(ledger, dec_id, "accept", "human", prov)
        ledger.event("lesson_accept", lesson_id, {"decision": dec_id, "author": "human"})
    return _sync(ctx, ledger, LessonResult(get_lesson(ledger, lesson_id)))


def reject(ctx: RepoContext, ledger: Ledger, lesson_id: str) -> LessonResult:
    """Reject a proposed lesson (it becomes retired)."""
    lesson = get_lesson(ledger, lesson_id)
    if lesson["status"] != "proposed":
        hint = "; use `arb lesson retire` for an accepted one" if lesson["status"] == "accepted" else ""
        raise ArbError(f"lesson {lesson_id} is {lesson['status']}, not proposed{hint}")
    with ledger.write() as conn:
        conn.execute("UPDATE lessons SET status = 'retired', decided_at = ? WHERE id = ?",
                     (now(), lesson_id))
        dec_id = _open_decision_id(ledger, lesson_id)
        if dec_id:
            records.set_decision(ledger, dec_id, "reject", "human")
        ledger.event("lesson_reject", lesson_id, {"decision": dec_id})
    return LessonResult(get_lesson(ledger, lesson_id))


def retire(ctx: RepoContext, ledger: Ledger, lesson_id: str) -> LessonResult:
    """Retire an accepted lesson and drop it from CLAUDE.md."""
    lesson = get_lesson(ledger, lesson_id)
    if lesson["status"] != "accepted":
        hint = "; use `arb lesson reject` for a proposed one" if lesson["status"] == "proposed" else ""
        raise ArbError(f"lesson {lesson_id} is {lesson['status']}, not accepted{hint}")
    claudemd.check(ctx, [l for l in claudemd.accepted_lessons(ledger) if l["id"] != lesson_id])
    with ledger.write() as conn:
        conn.execute("UPDATE lessons SET status = 'retired', decided_at = ? WHERE id = ?",
                     (now(), lesson_id))
        ledger.event("lesson_retire", lesson_id, {})
    return _sync(ctx, ledger, LessonResult(get_lesson(ledger, lesson_id)))
