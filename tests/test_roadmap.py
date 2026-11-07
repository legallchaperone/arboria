"""The roadmap: human-only milestones replayed from ledger events, spend from the ledger,
milestone lines on the approval card, and the app's roadmap/inbox view."""

from __future__ import annotations

import json
from datetime import date, timedelta
from pathlib import Path

import pytest

from arboria import arbd, gitutil, roadmap
from arboria.db import Ledger
from arboria.install import DENY_RULES

from .test_submit import _submit, cluster, jn  # noqa: F401


def _day(n: int) -> str:
    return (date.today() + timedelta(days=n)).isoformat()


def test_milestone_lifecycle(arb, idea: Path, initialized: Path) -> None:
    arb(initialized, "milestone", "new", "baseline", "--title", "复现 baseline", "--deadline", _day(20),
        "--su", "100", "--idea", "exp1", "--item", "写好 eval 脚本", "--item", "和论文数字对上")
    out = arb(initialized, "milestone", "list").stdout
    assert "baseline · 复现 baseline" in out and "还有 20 天" in out and "已用 0 / 100 SU" in out
    assert "[ ] 1. 写好 eval 脚本" in out

    arb(initialized, "milestone", "check", "baseline", "1")
    arb(initialized, "milestone", "set", "baseline", "--su", "80", "--add-item", "画图", "--no-deadline")
    p = json.loads(arb(initialized, "milestone", "show", "baseline", "--json").stdout)
    assert p["su_budget"] == 80 and p["deadline"] is None and p["items_done"] == 1
    assert [it["text"] for it in p["items"]] == ["写好 eval 脚本", "和论文数字对上", "画图"]
    arb(initialized, "milestone", "set", "baseline", "--remove-item", "2")
    arb(initialized, "milestone", "done", "baseline")
    assert "no milestones" in arb(initialized, "milestone", "list").stdout
    assert "已完成" in arb(initialized, "milestone", "list", "--all").stdout
    arb(initialized, "milestone", "reopen", "baseline")

    with Ledger(initialized / ".arboria") as ledger:
        events = ledger.all("SELECT kind, subject FROM events WHERE kind = 'milestone'")
        m = roadmap.get(ledger, "baseline")
    assert len(events) == 6 and {e["subject"] for e in events} == {"baseline"}
    assert m.status == "open" and [it["text"] for it in m.items] == ["写好 eval 脚本", "画图"]
    log = gitutil.out(["log", "--format=%s", "-1"], initialized / ".arboria")
    assert "milestone baseline" in log  # every change is backed up


@pytest.mark.parametrize("args, msg", [
    (["--title", "x", "--deadline", "12/01"], "YYYY-MM-DD"),
    (["--title", "x" * 81], "at most 80"),
    (["--title", "x", "--idea", "nope"], "no idea named"),
    (["--title", "x", "--su", "-1"], "≥ 0"),
    (["--title", "x", "--item", "y" * 121], "at most 120"),
])
def test_milestone_validation(arb, idea: Path, initialized: Path, args, msg) -> None:
    res = arb(initialized, "milestone", "new", "m1", *args, ok=False)
    assert res.exit_code != 0 and msg in res.output


def test_milestones_are_the_humans(arb, idea: Path, initialized: Path, monkeypatch) -> None:
    arb(initialized, "milestone", "new", "m1", "--title", "x", "--item", "a")
    monkeypatch.setenv("CLAUDECODE", "1")
    for args in (["new", "m2", "--title", "y"], ["set", "m1", "--su", "5"], ["check", "m1", "1"],
                 ["uncheck", "m1", "1"], ["done", "m1"], ["drop", "m1"]):
        res = arb(initialized, "milestone", *args, ok=False)
        assert res.exit_code != 0 and "只由人" in res.output, args
    assert "m1" in arb(initialized, "milestone", "list").stdout  # reading is fine
    for c in ("new", "set", "check", "uncheck", "done", "reopen", "drop"):
        assert f"Bash(arb milestone {c}:*)" in DENY_RULES
    assert not any("milestone list" in r or "milestone show" in r for r in DENY_RULES)


def test_spend_card_and_app_view(cluster, arb, idea: Path, initialized: Path, monkeypatch) -> None:
    from arboria import human

    arb(initialized, "milestone", "new", "m1", "--title", "第一阶段", "--deadline", _day(3),
        "--su", "10", "--idea", "exp1")
    arb(initialized, "milestone", "new", "later", "--title", "以后", "--deadline", _day(-2))
    cluster.script(jn(cluster, "r001", 1), run_s=600, queue_s=0, billed=4.0)
    _submit(arb, idea)
    card = arb(idea, "card", "r001").stdout
    assert "里程碑 m1：已用 0.00 / 10 SU，剩余 10.00，截止" in card and "还有 3 天" in card
    assert "⚠ 估计上限 48.00 SU 超过里程碑 m1 剩余 10.00 SU" in card
    assert "later" not in card  # only milestones that list the run's idea

    d = arbd.Daemon(initialized)
    ms = d.handle({"id": 1, "method": "roadmap"})["result"]["milestones"]
    assert [m["name"] for m in ms] == ["later", "m1"]  # by deadline
    m1 = ms[1]
    assert m1["awaiting_su"] == 48 and set(m1["flags"]) == {"soon", "tight"}
    inbox = {i["id"]: i for i in d.handle({"id": 2, "method": "inbox"})["result"]["items"]
             if i["kind"] == "milestone"}
    assert inbox["later"]["tone"] == "bad" and "已过截止 2 天" in inbox["later"]["meta"]
    assert inbox["m1"]["tone"] == "warn"

    monkeypatch.setattr(human, "require_terminal", lambda *a, **k: None)
    monkeypatch.setattr(human, "read_confirmation", lambda prompt: "r001")
    arb(idea, "approve", "r001")
    cluster.advance(700)
    arb(idea, "poll")
    with Ledger(initialized / ".arboria") as ledger:
        p = roadmap.progress(ledger, roadmap.get(ledger, "m1"))
    assert p["spent_su"] == 4.0 and p["left_su"] == 6.0 and p["awaiting_su"] == 0
    assert p["idea_progress"] == [{"name": "exp1", "status": "open", "su": 4.0, "runs": 1}]
