"""Read-side views: status (full / brief) and the Markdown export."""

from __future__ import annotations

from typing import Any

from .claudemd import accepted_lessons, lesson_line
from .inspection import VIEW_LABELS
from .db import Ledger, now
from .tasks import list_tasks, owner_name, payload
from .ideas import current_idea
from .paths import RepoContext

BRIEF_FOOTER = (
    "Ledger rules: experiment state lives in the arboria ledger, written only by `arb`. "
    "Work comes from the todos: finish one with `arb todo done tNNN --result \"...\"`; "
    "for a command todo read `arb commands guide` first, for an inspect todo "
    "`arb inspect guide` (deliver with `arb inspect done`, not `arb todo done`); "
    "`notebook.py` is the human's notebook: read it, never edit it; `arb note` is human-only. "
    "Use `arb status`, `arb run summary RUN \"...\"`, "
    "`arb submit --reason \"...\" --gpus N --time T [-- CMD]`; never edit .arboria/ by hand. "
    "Approval happens only in the human's own terminal: you cannot run `arb approve` or "
    "`arb lesson accept`. Read cluster logs with `arb logs RUN` / `arb remote ls RUN`, not ssh. "
    "Propose reusable pitfalls with `arb lesson propose \"...\"`."
)
TODOS_HEADER = "来自 notebook 的待办："
COMMAND_TODOS_HEADER = "命令待办："
INSPECT_TODOS_HEADER = "读代码待办："
# Accepted lessons shown in status / the hook (CLAUDE.md in an idea worktree may be stale).
STATUS_LESSONS = 10


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


def _jobs(ledger: Ledger, run_id: str) -> list[dict[str, Any]]:
    return ledger.all("SELECT segment, job_name, job_id, state, elapsed_s, billed_su FROM jobs"
                      " WHERE run_id = ? ORDER BY segment", (run_id,))


def _runs(ledger: Ledger, idea_id: int) -> list[dict[str, Any]]:
    runs = ledger.all(
        "SELECT id, status, change_reason, snapshot_commit, tree_hash, summary, wandb_url,"
        " machine, \"partition\", gpus, walltime, est_cost, actual_cost,"
        " created_at, updated_at FROM runs WHERE idea_id = ? ORDER BY id",
        (idea_id,),
    )
    for r in runs:
        r["jobs"] = _jobs(ledger, r["id"])
    return runs


def pending_approvals(ledger: Ledger) -> int:
    return ledger.one("SELECT COUNT(*) AS n FROM runs WHERE status = 'awaiting_approval'")["n"]


# Finished runs of open ideas that ended after the idea's last human note (notes without an
# author predate v4 and were the human's). The dashboard lists them (ui/data.py).
FINISHED_UNSEEN_SQL = (
    "SELECT COUNT(*) AS n FROM runs r JOIN ideas i ON i.id = r.idea_id"
    " WHERE i.status = 'open' AND r.status IN ('succeeded', 'failed', 'cancelled')"
    " AND COALESCE((SELECT MAX(e.ts) FROM events e WHERE e.kind = 'run_final'"
    "               AND e.subject = r.id), r.updated_at)"
    "   > COALESCE((SELECT MAX(n.created_at) FROM notes n WHERE n.idea_id = r.idea_id"
    "               AND (n.author IS NULL OR n.author = 'human')), '')"
)
BOARD_LINE = "看板：arb ui"


def finished_unseen(ledger: Ledger) -> int:
    return ledger.one(FINISHED_UNSEEN_SQL)["n"]


def jobs_line(run: dict[str, Any]) -> str | None:
    """`s1 1001 completed, s2 1002 running; 3.20 SU` (None for runs never submitted)."""
    jobs = run.get("jobs") or []
    if not jobs and run.get("est_cost") is None:
        return None
    parts = [f"s{j['segment']} {j['job_id'] or '-'} {j['state']}" for j in jobs]
    cost = run.get("actual_cost")
    money = f"{cost:.2f} SU" if cost is not None else "0 SU"
    if run.get("est_cost") is not None:
        money += f"（估计上限 {run['est_cost']:.2f}）"
    where = f"{run['machine']}/{run['partition']} {run['gpus']} GPU {run['walltime']}" \
        if run.get("machine") else ""
    return "; ".join(x for x in (where, ", ".join(parts), money) if x)


def _notes(ledger: Ledger, idea_id: int, limit: int | None = None) -> list[dict[str, Any]]:
    sql = "SELECT * FROM notes WHERE idea_id = ? ORDER BY id DESC"
    if limit:
        sql += f" LIMIT {int(limit)}"
    return ledger.all(sql, (idea_id,))


def _idea_public(idea: dict[str, Any]) -> dict[str, Any]:
    keys = ("name", "hypothesis", "metric", "confidence", "status", "branch", "worktree_path",
            "created_at", "closed_at")
    return {k: idea[k] for k in keys}


def _lesson_data(ledger: Ledger) -> dict[str, Any]:
    total = ledger.one("SELECT COUNT(*) AS n FROM lessons WHERE status = 'accepted'")["n"]
    proposed = ledger.one("SELECT COUNT(*) AS n FROM lessons WHERE status = 'proposed'")["n"]
    return {"lessons": accepted_lessons(ledger, limit=STATUS_LESSONS),
            "accepted_lessons": total, "proposed_lessons": proposed}


def status_data(ctx: RepoContext, ledger: Ledger) -> dict[str, Any]:
    return (_status_data(ctx, ledger) | _lesson_data(ledger)
            | {"pending_approvals": pending_approvals(ledger),
               "finished_unseen": finished_unseen(ledger)})


def _status_data(ctx: RepoContext, ledger: Ledger) -> dict[str, Any]:
    idea = current_idea(ctx, ledger)
    if idea is not None:
        parent = ledger.one("SELECT name FROM ideas WHERE id = ?", (idea["parent_id"],)) \
            if idea["parent_id"] else None
        return {
            "scope": "idea",
            "idea": _idea_public(idea) | {"parent": parent["name"] if parent else None},
            "runs": _runs(ledger, idea["id"]),
            "notes": _notes(ledger, idea["id"], limit=5),
            "todos": list_tasks(ledger, idea["id"]),
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
        "todos": list_tasks(ledger),
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
    head, tail = status_parts(data, brief=brief)
    return "\n".join(part for part in (head, tail) if part)


def todo_lines(todos: list[dict[str, Any]], brief: bool, with_idea: bool) -> list[str]:
    """Open todos the human left in a notebook (the work Claude should pick up)."""
    plain = [t for t in todos if t.get("kind", "plain") == "plain"]
    cmds = [t for t in todos if t.get("kind", "plain") in ("command_new", "command_revise")]
    reads = [t for t in todos if t.get("kind", "plain") == "inspect"]
    lines: list[str] = []
    if cmds:
        items = []
        for t in cmds:
            name = payload(t).get("name", "?")
            extra = "，修改" if t["kind"] == "command_revise" else ""
            where = f"，{owner_name(t)}" if with_idea else ""
            items.append(f"{name}（{t['id']}{extra}{where}）")
        lines.append(COMMAND_TODOS_HEADER + "、".join(items)
                     + "——按 `arb commands guide` 的约定写，详情见 `arb todo list --json`")
    if reads:
        items = []
        for t in reads:
            p = payload(t)
            view = VIEW_LABELS.get(p.get("view"), "?")
            where = f"，{owner_name(t)}" if with_idea else ""
            items.append(f"{_clip(p.get('question'), 60)}（{t['id']}，{view}{where}）")
        lines.append(INSPECT_TODOS_HEADER + "、".join(items)
                     + "——按 `arb inspect guide` 的约定读代码，用 `arb inspect done` 交付")
    if plain:
        lines.append(TODOS_HEADER + "（做完用 `arb todo done tNNN --result \"...\"`）")
        for t in plain:
            where = f" [{owner_name(t)}]" if with_idea else ""
            lines.append(f"- {t['id']}{where}: {_clip(t['text'], 300 if brief else 10_000)}")
    return lines


def lesson_lines(data: dict[str, Any]) -> list[str]:
    """Accepted lessons (newest first) and the count awaiting the human."""
    lines: list[str] = []
    lessons = data.get("lessons") or []
    if lessons:
        lines.append(f"经验（已确认，最新 {len(lessons)} 条）:")
        lines.extend(lesson_line(l) for l in lessons)
        more = data.get("accepted_lessons", len(lessons)) - len(lessons)
        if more > 0:
            lines.append(f"  … 另有 {more} 条，见 `arb lesson list`")
    pending = data.get("proposed_lessons") or 0
    if pending:
        lines.append(f"待你确认的经验：{pending} 条，用 `arb lesson list` 查看")
    return lines


def status_parts(data: dict[str, Any], brief: bool = False) -> tuple[str, str]:
    """(state, lessons + footer): the hook truncates the first part, never the second."""
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
        lines.extend(todo_lines(data.get("todos") or [], brief, with_idea=False))
        runs = data["runs"]
        shown = runs[-8:] if brief else runs
        lines.append(f"Runs ({len(runs)})" + (":" if runs else ": none"))
        if brief and len(runs) > len(shown):
            lines.append(f"  … {len(runs) - len(shown)} older runs omitted")
        for r in shown:
            line = f"- {r['id']} [{r['status']}] {_clip(r['change_reason'], clip)} @{r['snapshot_commit'][:10]}"
            lines.append(line)
            jl = jobs_line(r)
            if jl:
                lines.append(f"    jobs: {jl}")
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
        lines.extend(todo_lines(data.get("todos") or [], brief, with_idea=True))
    decisions = data["open_decisions"]
    lines.append("Open decisions" + (":" if decisions else ": none"))
    lines.extend(_decision_lines(decisions))
    tail = lesson_lines(data)
    if data.get("pending_approvals"):
        tail.insert(0, f"待你批准的 run：{data['pending_approvals']} 个"
                       "（`arb card RUN` 看审核单，由你运行 `arb approve RUN`）")
    if data.get("pending_approvals") or data.get("proposed_lessons") or data.get("finished_unseen"):
        tail.append(BOARD_LINE)  # something waits for the human
    if brief:
        tail.append(BRIEF_FOOTER)
    return "\n".join(lines), "\n".join(tail)


def export_markdown(ctx: RepoContext, ledger: Ledger) -> str:
    """All ideas, newest first, as Markdown meant to be pasted into a chat."""
    ideas = ledger.all(
        "SELECT i.*, p.name AS parent_name FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id"
        " ORDER BY i.created_at DESC, i.id DESC"
    )
    out = [f"# Arboria research log: {ctx.main.name}", "", f"_Exported {now()} — {len(ideas)} idea(s), newest first._", ""]
    out += ["## 经验", ""]
    out += [lesson_line(l) for l in accepted_lessons(ledger)] or ["_暂无_"]
    pending = ledger.all("SELECT * FROM lessons WHERE status = 'proposed' ORDER BY id DESC")
    if pending:
        out += ["", "待确认：", *(f"- {l['id']}: {l['text']}" for l in pending)]
    out.append("")
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
            jl = jobs_line(r | {"jobs": _jobs(ledger, r["id"])})
            if jl:
                out.append(f"  - jobs: {jl}")
            if r["summary"]:
                for line in r["summary"].splitlines():
                    out.append(f"  > {line}")
        out.append("")
    return "\n".join(out).rstrip() + "\n"


# ---------------------------------------------------------------- the seed home

HOME_FOOTER = (
    "Seed home rules: this folder is the human's seed garden (ideas without a repo yet). "
    "`seeds/<name>.py` are the human's marimo notebooks: read them for context, never edit "
    "them; never edit .arboria/. Here (outside any repo) `arb idea` works on the seeds: read "
    "them with `arb idea list` / `arb idea show NAME`; seeds are created, changed, dropped "
    "and planted only by the human (`arb idea new/set/drop/revive/plant`). "
    "Work comes from the todos: `arb todo list`, finish with `arb todo done tNNN --result \"...\"`; "
    "a command todo here is a global command (read `arb commands guide` first)."
)
HOME_SHOWN = 5


def home_status(h: Any, ledger: Ledger, brief: bool = False) -> str:
    """The home at a glance: counts, the most recently edited seeds, open todos."""
    from .notebooks import display_path
    from .seeds import counts, list_seeds

    n = counts(ledger)
    seeds = list_seeds(h, ledger)
    lines = [f"[arboria] 种子园 {display_path(h.root)} — {n['thinking']} 个在思考中，"
             f"{n['planted']} 个已种下，{n['dropped']} 个已放弃"]
    shown = seeds[:HOME_SHOWN] if brief else seeds
    if shown:
        lines.append("最近编辑的种子：" if brief else "种子（最近编辑的在前）：")
    for s in shown:
        planted = "".join(f" → {p['idea']}" for p in s["planted_to"])
        lines.append(f"- {s['name']}（{s['last_edited'][:10]}{'，已种下' + planted if planted else ''}）"
                     + (f"：{_clip(s['gist'], 160 if brief else 10_000)}" if s["gist"] else ""))
        if not brief and s["hypothesis"]:
            lines.append(f"    假设：{s['hypothesis']}")
    if brief and len(seeds) > len(shown):
        lines.append(f"  … 另有 {len(seeds) - len(shown)} 个，见 `arb idea list`")
    if not seeds:
        lines.append("还没有种子（人在仓库外用 `arb idea new NAME \"一句话想法\"` 记下第一颗）")
    lines.extend(todo_lines(list_tasks(ledger), brief, with_idea=True))
    if brief:
        lines.append(HOME_FOOTER)
    return "\n".join(lines)
