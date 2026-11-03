"""`arb ui`: the read-only dashboard (data layer, views, marimo app, launcher)."""

from __future__ import annotations

import ast
import json
import os
import re
import sqlite3
import subprocess
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from arboria import db, report
from arboria.ui import data, views

from . import uiutil

SRC = Path(__file__).parent.parent / "src"
UI = SRC / "arboria" / "ui"


@pytest.fixture
def board_project(tmp_path_factory, monkeypatch, idea: Path, initialized: Path, arb) -> Path:
    cluster = uiutil.setup_cluster(tmp_path_factory.mktemp("cluster"), initialized, monkeypatch)
    uiutil.build(arb, initialized, idea, cluster)
    return initialized


def _svg(text: str) -> ET.Element:
    return ET.fromstring(text)  # well-formed XML, or this raises


# ---------------------------------------------------------------- data layer


def test_constants_match_the_ledger() -> None:
    assert data.SCHEMA_VERSION == db.SCHEMA_VERSION
    assert data.RUN_FINAL == db.RUN_FINAL and data.RUN_ACTIVE == db.RUN_ACTIVE
    assert data.JOB_TERMINAL == db.JOB_TERMINAL
    assert set(views.RUN_STATUS) == set(db.RUN_STATUS)
    assert set(views.CONFIDENCE) == set(db.CONFIDENCE)
    assert set(views.IDEA_STATUS) == set(db.IDEA_STATUS)
    assert set(views.JOB_STATE) == set(db.JOB_STATES)


def test_board_reads_the_fixture_project(board_project: Path) -> None:
    b = data.load(board_project)
    assert {i["name"]: i["status"] for i in b.ideas} == {
        "exp1": "open", "exp2": "open", "exp3": "dropped", "exp4": "merged"}
    exp2 = b.idea("exp2")
    assert exp2["parent_name"] == "exp1" and exp2["from_run"] == "r001"
    assert b.idea("exp3")["parent_name"] == "exp1" and b.idea("exp3")["from_run"] is None
    assert {r["id"]: r["status"] for r in b.runs} == {
        "r001": "succeeded", "r002": "failed", "r003": "awaiting_approval", "r004": "running",
        "r005": "draft", "r006": "cancelled", "r007": "rejected"}
    assert [j["segment"] for j in b.run("r001")["jobs"]] == [1, 2]
    # what needs the human
    assert [r["id"] for r in b.awaiting()] == ["r003"]
    card = b.cards["r003"]
    assert card["matches"] is True and card["text"].startswith("审核单 r003（idea exp1）")
    assert "复用 r001 的快照" in card["text"]
    # r001 ended before the note on exp1; r002 after it; r006 after exp2's reject note
    assert [r["id"] for r in b.finished_unseen()] == ["r006", "r002"]
    assert [l["id"] for l in b.lessons] == ["l001"]
    assert [(t["id"], t["kind"]) for t in b.todos] == [("t001", "plain"), ("t002", "command_new")]
    assert b.needs_you() == 4
    assert b.notifications[0][1].startswith("r006 已取消")
    assert any("r001 成功" in m for _, m in b.notifications)
    # money: the ledger's own sums
    con = sqlite3.connect(board_project / ".arboria" / "ledger.db")
    total = con.execute("SELECT SUM(billed_su) FROM jobs").fetchone()[0]
    con.close()
    assert b.su_used() == pytest.approx(round(total, 2))
    assert b.su_used(b.idea("exp1")["id"]) > 237 and b.total_su == 400
    assert [(j["run"]["id"], j["state"]) for j in b.active_jobs()] == [("r004", "running")]
    cum = [v for _, v, _ in b.su_timeline()]
    assert cum == sorted(cum) and cum[-1] == pytest.approx(total, abs=1e-3)
    assert data.out_files(b, "r001")[0] == 2
    # status / the hook count the same runs as the dashboard
    from arboria.db import Ledger

    with Ledger(board_project / ".arboria") as ledger:
        assert report.finished_unseen(ledger) == 2


def test_read_only_connection_refuses_writes(board_project: Path) -> None:
    with data.ReadOnlyLedger(board_project / ".arboria") as ledger:
        with pytest.raises(sqlite3.OperationalError):
            ledger.conn.execute("DELETE FROM runs")
    assert data.load(board_project).run("r001") is not None


def test_old_schema_is_reported_not_migrated(initialized: Path) -> None:
    path = initialized / ".arboria" / "ledger.db"
    con = sqlite3.connect(path)
    con.execute("PRAGMA user_version = 4")
    con.close()
    with pytest.raises(Exception, match="schema v4"):
        data.load(initialized)
    con = sqlite3.connect(path)
    assert con.execute("PRAGMA user_version").fetchone()[0] == 4  # untouched
    con.close()


# ---------------------------------------------------------------- views


def test_inbox_lists_commands_not_buttons(board_project: Path) -> None:
    b = data.load(board_project)
    html = views.inbox(b)
    for text in ('<code class="cmd">arb approve r003</code>',
                 '<code class="cmd">arb reject r003</code>',
                 '<code class="cmd">arb lesson accept l001</code>',
                 '<code class="cmd">arb lesson reject l001</code>',
                 "审核单 r003（idea exp1）", "你上次写笔记之后结束的 run", "命令 svd（新建）",
                 "把 eval 改成每 500 步一次", "2 天前", "r006 已取消（arb cancel）"):
        assert text in html, text
    assert html.index("<b>r006</b>") < html.index("<b>r002</b>")  # newest first
    assert "<button" not in html and "<form" not in html and "<input" not in html
    header = views.header(b)
    assert re.search(r"数据更新于 \d\d:\d\d:\d\d（来自账本；集群状态由 arb watch 更新）", header)
    assert ">4</div><div class=\"k\">需要你处理" in header


def test_empty_groups_collapse(initialized: Path) -> None:
    b = data.load(initialized)
    html = views.inbox(b)
    for title in ("待批准的 run", "待确认的经验", "未完成的待办（等 Claude 处理）"):
        assert f'<div class="group empty">{title} · 0 · ' in html
    assert "还没有 idea" in views.tree(b)
    assert "没有选中的 idea" in views.idea_detail(b, views.default_idea(b))
    assert "未设项目预算" in views.budget(b) and "还没有花费" in views.budget(b)


def test_idea_tree_svg(board_project: Path) -> None:
    b = data.load(board_project)
    svg = views.tree_svg(b, show_dropped=True, selected="exp1")
    root = _svg(svg)
    ns = "{http://www.w3.org/2000/svg}"
    labels = [t.text for t in root.iter(f"{ns}text") if t.text and t.text.startswith("exp")]
    # parents before children: exp1, its children by creation time, then exp4
    assert labels == ["exp1", "exp2", "exp3 ✕", "exp4 ◆"]
    titles = [t.text for t in root.iter(f"{ns}title")]
    assert "exp1 → exp2（从 r001 分出）" in titles and "exp1 → exp3（分支）" in titles
    ticks = [t for t in titles if re.match(r"r\d{3} · ", t)]
    assert len(ticks) == 7 and any(t.startswith("r004 · 运行中") for t in ticks)
    assert any(t.startswith("exp1 · 置信度 初步") for t in titles)
    assert any(t.startswith("exp4 · 置信度 已确认") for t in titles)
    dimmed = [g for g in root.iter(f"{ns}g") if g.get("opacity") == ".42"]
    assert len(dimmed) == 1 and "exp3" in ET.tostring(dimmed[0], encoding="unicode")
    # readable in light and dark: no hard-coded black/white ink
    assert not re.search(r"#000\b|#fff\b|black|white", svg, re.I)
    assert "currentColor" in svg and "var(--arb-ok)" in svg
    # dropped rows can be collapsed
    hidden = views.tree(b, show_dropped=False)
    assert "exp3" not in views.tree_svg(b, show_dropped=False)
    assert "已隐藏 1 个已放弃的 idea：exp3" in hidden


def test_idea_detail(board_project: Path) -> None:
    b = data.load(board_project)
    html = views.idea_detail(b, "exp1")
    nb = board_project / ".arboria" / "notebooks" / "exp1.py"
    assert f'href="vscode://file{nb}"' in html
    assert f'<code class="cmd">marimo edit {nb}</code>' in html
    assert "lr: 3e-4 → 1e-4" in html and "时长: 2d12h → 1d" in html and "lr=3e-4" in html
    assert "baseline 看起来不错" in html and "置信度 → 初步" in html
    assert "<details><summary><b>r001</b>" in html  # summaries are collapsed
    assert "r005" in html and "草稿" in html
    assert re.search(r'<td class="num">240\.00</td><td class="num">237\.50</td>', html)
    exp2 = views.idea_detail(b, "exp2")
    assert "exp1（从 r001 的快照分出）" in exp2 and "命令 svd（新建）" in exp2
    # free text is escaped
    assert "<script>" not in exp2 and "&lt;script&gt;alert(1)&lt;/script&gt;" in exp2
    assert "（已删除）" in views.idea_detail(b, "exp3")


def test_idea_selection_defaults(board_project: Path) -> None:
    b = data.load(board_project)
    names = list(views.idea_options(b).values())
    assert names[:2] == ["exp1", "exp2"] and set(names[2:]) == {"exp3", "exp4"}
    assert views.default_idea(b) == "exp1"  # most recently active open idea
    assert views.default_idea(b, "exp3") == "exp3"
    assert views.default_idea(b, "nope") == "exp1"


def test_budget_section(board_project: Path) -> None:
    b = data.load(board_project)
    html = views.budget(b)
    assert "已用（共 400 SU" in html and "剩余" in html
    assert "活跃作业 <span class=\"count\">1</span>" in html and "<b>r004</b>" in html
    assert re.search(r"<b>r001</b>.*?<td class=\"num\">99%</td>", html, re.S)
    for part in re.findall(r"<svg.*?</svg>", html, re.S):
        _svg(part)
    assert "预算 400 SU" in views.su_line(b)  # used ≥ 25% of the budget: budget line drawn


def test_ticks() -> None:
    t0 = datetime(2026, 11, 1, tzinfo=timezone.utc)
    ticks = views.time_ticks(t0, t0 + timedelta(days=10))
    assert 3 <= len(ticks) <= 7 and all(t0 <= t for t, _ in ticks)
    hours = views.time_ticks(t0, t0 + timedelta(hours=5))
    assert len(hours) >= 3
    assert views.nice_ticks(243) == [0, 100, 200] and views.nice_ticks(0) == [0]
    assert views.age("2026-11-01T00:00:00Z", t0 + timedelta(hours=5)) == "5 小时前"
    assert views.fmt_su(1234.5) == "1,234.50 SU"


# ---------------------------------------------------------------- read-only guarantees


DASHBOARD_MODULES = ("data.py", "views.py", "__init__.py")
ALLOWED_ARBORIA = {"arboria.cards", "arboria.config", "arboria.errors", "arboria.paths",
                   "arboria.slurm", "arboria.ui", "arboria.ui.data", "arboria.ui.views"}


def _imports(path: Path) -> set[str]:
    out = set()
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.Import):
            out |= {a.name for a in node.names}
        elif isinstance(node, ast.ImportFrom):
            if node.level:  # relative to arboria.ui
                base = "arboria.ui" if node.level == 1 else "arboria"
                mod = f"{base}.{node.module}" if node.module else base
            else:
                mod = node.module or ""
            if node.module is None or mod in ("arboria", "arboria.ui"):
                out |= {f"{mod}.{a.name}" for a in node.names}  # `from .. import cards`
            else:
                out.add(mod)
    return out


def test_dashboard_imports_no_write_path() -> None:
    """Source scan: the dashboard's modules import only read-side arboria modules and
    nothing that runs processes or talks to the network; and no control that acts."""
    for name in DASHBOARD_MODULES:
        mods = _imports(UI / name)
        ours = {m for m in mods if m.startswith("arboria")}
        assert ours <= ALLOWED_ARBORIA, (name, ours - ALLOWED_ARBORIA)
        assert not mods & {"subprocess", "socket", "urllib.request", "http.client", "requests",
                           "wandb", "fcntl", "shutil"}, name
        text = (UI / name).read_text().replace("ReadOnlyLedger(", "")
        for bad in ("Ledger(", ".write(", "write_text", "write_bytes", "unlink", "mkdir",
                    "commit_backup", "INSERT", "UPDATE ", "DELETE"):
            assert bad not in text, (name, bad)
    # transitively, too: importing the dashboard pulls in no write module
    code = ("import sys; import arboria.ui.data, arboria.ui.views; "
            "print(sorted(m for m in sys.modules if m.startswith('arboria')))")
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                         env={**os.environ, "PYTHONPATH": str(SRC)})
    loaded = set(json.loads(res.stdout.replace("'", '"')))
    for bad in ("db", "records", "submission", "monitor", "backends", "tasks", "lessons",
                "ideas", "claudemd", "human", "notebooks", "commands", "state", "snapshot"):
        assert f"arboria.{bad}" not in loaded, bad
