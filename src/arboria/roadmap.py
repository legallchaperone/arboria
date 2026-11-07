"""The roadmap: the human's milestones, each with a deadline, an SU budget, ideas and a
checklist the human ticks off.

There is no table: a milestone is the replay of its `milestone` events in the ledger (one
event per change, `payload.op` new/set/check/uncheck/done/reopen/drop), so writes take the
ledger lock, every change triggers a backup, and the schema is unchanged. Only the human
writes milestones: every command that changes one refuses inside Claude Code (here, in
code; install-claude also denies them by prefix). Reading is anyone's (`arb milestone list`).

Spend is counted from the ledger, never estimated by an LLM: the billed SU of the jobs of
the runs of the milestone's ideas; "awaiting" is the estimate of their runs waiting for
approval. The approval card shows the milestones a run counts against (`card_lines`).
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from datetime import date
from typing import Any, Protocol

from . import human
from .db import NAME_RE, Ledger
from .errors import ArbError

EVENT = "milestone"
TITLE_MAX = 80
ITEM_MAX = 120
MAX_ITEMS = 30
MAX_IDEAS = 20
SOON_DAYS = 7
OPS = ("new", "set", "check", "uncheck", "done", "reopen", "drop")


class Reader(Protocol):
    def one(self, sql: str, params: tuple = ()) -> dict[str, Any] | None: ...
    def all(self, sql: str, params: tuple = ()) -> list[dict[str, Any]]: ...


@dataclass
class Milestone:
    name: str
    title: str
    deadline: str | None = None  # YYYY-MM-DD
    su_budget: float | None = None
    ideas: list[str] = field(default_factory=list)
    items: list[dict[str, Any]] = field(default_factory=list)  # {"text", "done"}
    status: str = "open"  # open / done / dropped
    created_at: str = ""
    updated_at: str = ""

    def as_dict(self) -> dict[str, Any]:
        return asdict(self)


# ---------------------------------------------------------------- validation


def _name(name: str) -> str:
    if not NAME_RE.match(name or ""):
        raise ArbError(f"milestone name {name!r}: lowercase letters, digits and '-', ≤63 chars")
    return name


def _title(title: str) -> str:
    title = " ".join((title or "").split())
    if not title:
        raise ArbError("a milestone needs a title")
    if len(title) > TITLE_MAX:
        raise ArbError(f"title is {len(title)} chars; at most {TITLE_MAX}")
    return title


def _deadline(value: str | None) -> str | None:
    if value is None:
        return None
    try:
        return date.fromisoformat(value).isoformat()
    except ValueError:
        raise ArbError(f"deadline {value!r}: use YYYY-MM-DD") from None


def _budget(value: float | None) -> float | None:
    if value is None:
        return None
    if not value >= 0:
        raise ArbError("SU budget must be ≥ 0")
    return round(float(value), 2)


def _item(text: str) -> str:
    text = " ".join((text or "").split())
    if not text:
        raise ArbError("a checklist item needs text")
    if len(text) > ITEM_MAX:
        raise ArbError(f"checklist item is {len(text)} chars; at most {ITEM_MAX}")
    return text


def _ideas(ledger: Reader, names: list[str]) -> list[str]:
    out: list[str] = []
    for n in names:
        if ledger.one("SELECT id FROM ideas WHERE name = ?", (n,)) is None:
            raise ArbError(f"no idea named {n!r} in this project")
        if n not in out:
            out.append(n)
    return out


# ---------------------------------------------------------------- replay


def _apply(ms: dict[str, Milestone], ts: str, p: dict[str, Any]) -> None:
    op, name = p.get("op"), p.get("name")
    if op == "new":
        ms[name] = Milestone(name=name, title=p["title"], deadline=p.get("deadline"),
                             su_budget=p.get("su_budget"), ideas=list(p.get("ideas") or []),
                             items=[{"text": t, "done": False} for t in p.get("items") or []],
                             created_at=ts, updated_at=ts)
        return
    m = ms.get(name)
    if m is None:
        return
    m.updated_at = ts
    if op == "set":
        for key in ("title", "deadline", "su_budget"):
            if key in p:
                setattr(m, key, p[key])
        m.ideas = [i for i in m.ideas if i not in (p.get("remove_ideas") or [])]
        m.ideas += [i for i in p.get("add_ideas") or [] if i not in m.ideas]
        m.items += [{"text": t, "done": False} for t in p.get("add_items") or []]
        drop = set(p.get("remove_items") or [])
        m.items = [it for k, it in enumerate(m.items, 1) if k not in drop]
    elif op in ("check", "uncheck"):
        k = p.get("item")
        if isinstance(k, int) and 1 <= k <= len(m.items):
            m.items[k - 1]["done"] = op == "check"
    elif op == "done":
        m.status = "done"
    elif op == "reopen":
        m.status = "open"
    elif op == "drop":
        m.status = "dropped"


def load(ledger: Reader) -> dict[str, Milestone]:
    """All milestones, in the order they were made."""
    import json

    ms: dict[str, Milestone] = {}
    for row in ledger.all("SELECT ts, payload_json FROM events WHERE kind = ? ORDER BY id", (EVENT,)):
        try:
            _apply(ms, row["ts"], json.loads(row["payload_json"]))
        except (ValueError, KeyError, TypeError):
            continue
    return ms


def get(ledger: Reader, name: str) -> Milestone:
    m = load(ledger).get(name)
    if m is None:
        raise ArbError(f"no milestone named {name!r} (arb milestone list)")
    return m


# ---------------------------------------------------------------- writes (human only)


HUMAN_ONLY = ("arb milestone {command} 只由人在自己的终端里做：里程碑（截止日期、算力预算、清单）是人定的。"
              "Claude 可以读（arb milestone list / show），不写")


def _require_human(command: str) -> None:
    if human.in_claude_code():
        raise ArbError(HUMAN_ONLY.format(command=command))


def _write(ledger: Ledger, payload: dict[str, Any]) -> None:
    with ledger.write():
        ledger.event(EVENT, payload["name"], payload)


def new(ledger: Ledger, name: str, title: str, *, deadline: str | None = None,
        su_budget: float | None = None, ideas: list[str] = (), items: list[str] = ()) -> Milestone:
    _require_human("new")
    name = _name(name)
    if name in load(ledger):
        raise ArbError(f"milestone {name!r} already exists")
    items = [_item(t) for t in items]
    ideas = _ideas(ledger, list(ideas))
    if len(items) > MAX_ITEMS or len(ideas) > MAX_IDEAS:
        raise ArbError(f"at most {MAX_ITEMS} checklist items and {MAX_IDEAS} ideas")
    _write(ledger, {"op": "new", "name": name, "title": _title(title), "deadline": _deadline(deadline),
                    "su_budget": _budget(su_budget), "ideas": ideas, "items": items})
    return get(ledger, name)


def update(ledger: Ledger, name: str, *, title: str | None = None, deadline: str | None = None,
           clear_deadline: bool = False, su_budget: float | None = None, clear_budget: bool = False,
           add_ideas: list[str] = (), remove_ideas: list[str] = (), add_items: list[str] = (),
           remove_items: list[int] = ()) -> Milestone:
    _require_human("set")
    m = get(ledger, name)
    p: dict[str, Any] = {"op": "set", "name": name}
    if title is not None:
        p["title"] = _title(title)
    if clear_deadline:
        p["deadline"] = None
    elif deadline is not None:
        p["deadline"] = _deadline(deadline)
    if clear_budget:
        p["su_budget"] = None
    elif su_budget is not None:
        p["su_budget"] = _budget(su_budget)
    if add_ideas:
        p["add_ideas"] = _ideas(ledger, list(add_ideas))
    if remove_ideas:
        missing = [i for i in remove_ideas if i not in m.ideas]
        if missing:
            raise ArbError(f"{', '.join(missing)} not in milestone {name}")
        p["remove_ideas"] = list(remove_ideas)
    if add_items:
        p["add_items"] = [_item(t) for t in add_items]
    if remove_items:
        bad = [k for k in remove_items if not 1 <= k <= len(m.items)]
        if bad:
            raise ArbError(f"no checklist item {bad[0]} (1–{len(m.items)})")
        p["remove_items"] = sorted(set(remove_items))
    if len(p) == 2:
        raise ArbError("nothing to change")
    ideas = [i for i in m.ideas if i not in p.get("remove_ideas", [])] + \
        [i for i in p.get("add_ideas", []) if i not in m.ideas]
    n_items = len(m.items) + len(p.get("add_items", [])) - len(p.get("remove_items", []))
    if n_items > MAX_ITEMS or len(ideas) > MAX_IDEAS:
        raise ArbError(f"at most {MAX_ITEMS} checklist items and {MAX_IDEAS} ideas")
    _write(ledger, p)
    return get(ledger, name)


def check(ledger: Ledger, name: str, item: int, done: bool = True) -> Milestone:
    _require_human("check" if done else "uncheck")
    m = get(ledger, name)
    if not 1 <= item <= len(m.items):
        raise ArbError(f"no checklist item {item} in {name} (1–{len(m.items)})" if m.items
                       else f"milestone {name} has no checklist")
    _write(ledger, {"op": "check" if done else "uncheck", "name": name, "item": item})
    return get(ledger, name)


def set_status(ledger: Ledger, name: str, op: str) -> Milestone:
    assert op in ("done", "reopen", "drop")
    _require_human(op)
    m = get(ledger, name)
    want = {"done": "done", "reopen": "open", "drop": "dropped"}[op]
    if m.status == want:
        raise ArbError(f"milestone {name} is already {want}")
    _write(ledger, {"op": op, "name": name})
    return get(ledger, name)


# ---------------------------------------------------------------- progress (reads only)


def progress(ledger: Reader, m: Milestone, today: date | None = None) -> dict[str, Any]:
    """Spend, ideas, checklist and time left for one milestone, from the ledger."""
    today = today or date.today()
    spent = awaiting = 0.0
    ideas = []
    for name in m.ideas:
        idea = ledger.one("SELECT id, status FROM ideas WHERE name = ?", (name,))
        if idea is None:
            ideas.append({"name": name, "status": "missing", "su": 0.0, "runs": 0})
            continue
        su = float(ledger.one("SELECT COALESCE(SUM(j.billed_su), 0) AS s FROM jobs j JOIN runs r"
                              " ON r.id = j.run_id WHERE r.idea_id = ?", (idea["id"],))["s"])
        wait = float(ledger.one("SELECT COALESCE(SUM(est_cost), 0) AS s FROM runs WHERE idea_id = ?"
                                " AND status = 'awaiting_approval'", (idea["id"],))["s"])
        runs = ledger.one("SELECT COUNT(*) AS n FROM runs WHERE idea_id = ?", (idea["id"],))["n"]
        spent += su
        awaiting += wait
        ideas.append({"name": name, "status": idea["status"], "su": round(su, 2), "runs": runs})
    days_left = (date.fromisoformat(m.deadline) - today).days if m.deadline else None
    left = None if m.su_budget is None else round(m.su_budget - spent, 2)
    flags = []
    if m.status == "open":
        if days_left is not None and days_left < 0:
            flags.append("late")
        elif days_left is not None and days_left <= SOON_DAYS:
            flags.append("soon")
        if left is not None and left < 0:
            flags.append("over")
        elif left is not None and awaiting > left:
            flags.append("tight")
    return {**m.as_dict(), "spent_su": round(spent, 2), "awaiting_su": round(awaiting, 2),
            "left_su": left, "days_left": days_left, "flags": flags, "idea_progress": ideas,
            "items_done": sum(1 for it in m.items if it["done"])}


def roadmap(ledger: Reader, today: date | None = None, *, all_: bool = False) -> list[dict[str, Any]]:
    """Open milestones (all, with `all_`), by deadline (none last)."""
    ms = [progress(ledger, m, today) for m in load(ledger).values()
          if all_ or m.status == "open"]
    return sorted(ms, key=lambda p: (p["status"] != "open", p["deadline"] or "9999", p["created_at"]))


def card_lines(ledger: Reader, idea_name: str, est_su: float,
               today: date | None = None) -> tuple[list[str], list[str]]:
    """(budget lines, warnings) for the approval card of a run of `idea_name`."""
    lines, warns = [], []
    for m in load(ledger).values():
        if m.status != "open" or idea_name not in m.ideas:
            continue
        p = progress(ledger, m, today)
        when = ""
        if p["days_left"] is not None:
            when = (f"，截止 {m.deadline}（已过 {-p['days_left']} 天）" if p["days_left"] < 0
                    else f"，截止 {m.deadline}（还有 {p['days_left']} 天）")
        if m.su_budget is None:
            lines.append(f"里程碑 {m.name}：已用 {p['spent_su']:.2f} SU（未设预算）{when}")
            continue
        lines.append(f"里程碑 {m.name}：已用 {p['spent_su']:.2f} / {m.su_budget:g} SU，"
                     f"剩余 {p['left_su']:.2f}{when}")
        if est_su > p["left_su"]:
            warns.append(f"估计上限 {est_su:.2f} SU 超过里程碑 {m.name} 剩余 {p['left_su']:.2f} SU")
    return lines, warns
