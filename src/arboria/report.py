"""Read-side views: status (full / brief) and the Markdown export."""

from __future__ import annotations

from typing import Any

from .db import Ledger, now
from .ideas import current_idea
from .paths import RepoContext

BRIEF_FOOTER = (
    "Ledger rules: experiment state lives in the arboria ledger, written only by `arb`. "
    "Use `arb status`, `arb note \"...\"`, `arb run new --reason \"...\"`; never edit .arboria/ by hand."
)


def _open_decisions(ledger: Ledger, idea_id: int | None = None) -> list[dict[str, Any]]:
    if idea_id is None:
        return ledger.all("SELECT * FROM decisions WHERE choice IS NULL ORDER BY id")
    return ledger.all(
        "SELECT * FROM decisions WHERE choice IS NULL AND ("
        " (subject_type = 'idea' AND subject_id IN (?, (SELECT name FROM ideas WHERE id = ?)))"
        " OR (subject_type = 'run' AND subject_id IN (SELECT id FROM runs WHERE idea_id = ?))"
        ") ORDER BY id",
        (str(idea_id), idea_id, idea_id),
    )


def _runs(ledger: Ledger, idea_id: int) -> list[dict[str, Any]]:
    return ledger.all(
        "SELECT id, status, change_reason, snapshot_commit, tree_hash, summary, wandb_url,"
        " created_at, updated_at FROM runs WHERE idea_id = ? ORDER BY id",
        (idea_id,),
    )


def _notes(ledger: Ledger, idea_id: int, limit: int | None = None) -> list[dict[str, Any]]:
    sql = "SELECT * FROM notes WHERE idea_id = ? ORDER BY id DESC"
    if limit:
        sql += f" LIMIT {int(limit)}"
    return ledger.all(sql, (idea_id,))


def _idea_public(idea: dict[str, Any]) -> dict[str, Any]:
    keys = ("name", "hypothesis", "metric", "confidence", "status", "branch", "worktree_path",
            "created_at", "closed_at")
    return {k: idea[k] for k in keys}


def status_data(ctx: RepoContext, ledger: Ledger) -> dict[str, Any]:
    idea = current_idea(ctx, ledger)
    if idea is not None:
        parent = ledger.one("SELECT name FROM ideas WHERE id = ?", (idea["parent_id"],)) \
            if idea["parent_id"] else None
        return {
            "scope": "idea",
            "idea": _idea_public(idea) | {"parent": parent["name"] if parent else None},
            "runs": _runs(ledger, idea["id"]),
            "notes": _notes(ledger, idea["id"], limit=5),
            "open_decisions": _open_decisions(ledger, idea["id"]),
        }
    ideas = ledger.all(
        "SELECT i.*, (SELECT COUNT(*) FROM runs r WHERE r.idea_id = i.id) AS run_count,"
        " (SELECT text FROM notes n WHERE n.idea_id = i.id ORDER BY n.id DESC LIMIT 1) AS last_note"
        " FROM ideas i WHERE i.status = 'open' ORDER BY i.id DESC"
    )
    closed = ledger.one("SELECT COUNT(*) AS n FROM ideas WHERE status != 'open'")["n"]
    return {
        "scope": "overview",
        "main_worktree": str(ctx.main),
        "ideas": [_idea_public(i) | {"run_count": i["run_count"], "last_note": i["last_note"]}
                  for i in ideas],
        "closed_ideas": closed,
        "open_decisions": _open_decisions(ledger),
    }


def _clip(text: str | None, n: int) -> str:
    text = " ".join((text or "").split())
    return text if len(text) <= n else text[: n - 1] + "…"


def _decision_lines(decisions: list[dict[str, Any]]) -> list[str]:
    return [f"- {d['id']} ({d['subject_type']} {d['subject_id']}): {_clip(d['question'], 160)}"
            for d in decisions]


def format_status(data: dict[str, Any], brief: bool = False) -> str:
    """Human/LLM-readable status. `brief` is the compact form injected by the hook."""
    clip = 160 if brief else 10_000
    lines: list[str] = []
    if data["scope"] == "idea":
        idea = data["idea"]
        lines.append(f"[arboria] idea `{idea['name']}` — status {idea['status']}, "
                     f"confidence {idea['confidence']}, branch {idea['branch']}"
                     + (f", parent {idea['parent']}" if idea.get("parent") else ""))
        if not brief:
            lines.append(f"worktree: {idea['worktree_path']}")
        lines.append(f"Hypothesis: {_clip(idea['hypothesis'], clip)}")
        lines.append(f"Metric: {_clip(idea['metric'], clip)}")
        runs = data["runs"]
        shown = runs[-8:] if brief else runs
        lines.append(f"Runs ({len(runs)})" + (":" if runs else ": none"))
        if brief and len(runs) > len(shown):
            lines.append(f"  … {len(runs) - len(shown)} older runs omitted")
        for r in shown:
            line = f"- {r['id']} [{r['status']}] {_clip(r['change_reason'], clip)} @{r['snapshot_commit'][:10]}"
            lines.append(line)
            if r["summary"]:
                lines.append(f"    summary: {_clip(r['summary'], 240 if brief else 10_000)}")
        notes = data["notes"]
        lines.append("Recent notes" + (":" if notes else ": none"))
        for n in notes:
            extra = "".join([f" (run {n['run_id']})" if n["run_id"] else "",
                             f" [confidence→{n['confidence']}]" if n["confidence"] else ""])
            lines.append(f"- {n['created_at'][:10]}{extra}: {_clip(n['text'], 240 if brief else 10_000)}")
    else:
        ideas = data["ideas"]
        lines.append(f"[arboria] main worktree {data['main_worktree']} — {len(ideas)} open idea(s), "
                     f"{data['closed_ideas']} closed")
        for i in ideas:
            lines.append(f"- {i['name']} [{i['confidence']}] runs: {i['run_count']} — "
                         f"{_clip(i['hypothesis'], clip)}")
            if not brief:
                lines.append(f"    worktree: {i['worktree_path']}")
            if i["last_note"]:
                lines.append(f"    last note: {_clip(i['last_note'], 160 if brief else 10_000)}")
        if not ideas:
            lines.append("No open ideas. Start one with `arb idea new NAME --hypothesis ... --metric ...`.")
    decisions = data["open_decisions"]
    lines.append("Open decisions" + (":" if decisions else ": none"))
    lines.extend(_decision_lines(decisions))
    if brief:
        lines.append(BRIEF_FOOTER)
    return "\n".join(lines)


def export_markdown(ctx: RepoContext, ledger: Ledger) -> str:
    """All ideas, newest first, as Markdown meant to be pasted into a chat."""
    ideas = ledger.all(
        "SELECT i.*, p.name AS parent_name FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id"
        " ORDER BY i.created_at DESC, i.id DESC"
    )
    out = [f"# Arboria research log: {ctx.main.name}", "", f"_Exported {now()} — {len(ideas)} idea(s), newest first._", ""]
    for idea in ideas:
        out.append(f"## {idea['name']} — {idea['status']}, confidence: {idea['confidence']}")
        out.append("")
        out.append(f"- **Hypothesis:** {idea['hypothesis']}")
        out.append(f"- **Metric:** {idea['metric']}")
        if idea["parent_name"]:
            out.append(f"- **Parent idea:** {idea['parent_name']}")
        dates = f"created {idea['created_at']}"
        if idea["closed_at"]:
            dates += f", closed {idea['closed_at']}"
        out.append(f"- **Branch:** `{idea['branch']}` ({dates})")
        out.append("")
        notes = ledger.all("SELECT * FROM notes WHERE idea_id = ? ORDER BY id DESC", (idea["id"],))
        out.append("### Notes")
        if not notes:
            out.append("_none_")
        for n in notes:
            tags = []
            if n["run_id"]:
                tags.append(f"run {n['run_id']}")
            if n["confidence"]:
                tags.append(f"confidence → {n['confidence']}")
            tag = f" ({', '.join(tags)})" if tags else ""
            out.append(f"- {n['created_at']}{tag}: {n['text']}")
        out.append("")
        runs = ledger.all("SELECT * FROM runs WHERE idea_id = ? ORDER BY id DESC", (idea["id"],))
        out.append("### Runs")
        if not runs:
            out.append("_none_")
        for r in runs:
            out.append(f"- **{r['id']}** [{r['status']}] {r['created_at']}, "
                       f"snapshot `{r['snapshot_commit'][:10]}` — {r['change_reason']}")
            if r["summary"]:
                for line in r["summary"].splitlines():
                    out.append(f"  > {line}")
        out.append("")
    return "\n".join(out).rstrip() + "\n"
