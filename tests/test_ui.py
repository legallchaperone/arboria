"""`arb ui`: the read-only dashboard (data layer, views, marimo app, launcher)."""

from __future__ import annotations

import ast
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from arboria import db, report
from arboria.ui import APP, GARDEN, data, launch, views

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


def _has_cmd(html: str, text: str) -> bool:
    """A select-all command chip (primary or secondary) showing exactly `text`."""
    return re.search(r'<code class="cmd( alt)?"[^>]*>' + re.escape(text) + "</code>",
                     html) is not None


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
    for text in ("arb approve r003", "arb reject r003", "arb lesson accept l001",
                 "arb lesson reject l001"):
        assert _has_cmd(html, text), text
    for text in ("审核单 r003（idea exp1）", "新结束的 run", "写命令 svd",
                 "把 eval 改成每 500 步一次", "2 天前", "r006 已取消（arb cancel）"):
        assert text in html, text
    # the approval card stays collapsed
    assert '<details class="more"><summary>审核单</summary><pre>审核单 r003' in html
    assert html.index('<span class="id">r006</span>') < html.index('<span class="id">r002</span>')
    assert "<button" not in html and "<form" not in html and "<input" not in html
    header = views.header(b)
    assert re.search(r">更新于 \d\d:\d\d:\d\d<", header) and views.UPDATED_TIP in header
    assert '<div class="k">需要你处理</div><div class="v">4</div>' in header
    assert str(board_project) not in header  # no absolute path in the header


def test_long_lists_show_the_newest_few(board_project: Path) -> None:
    b = data.load(board_project)
    t = b.loaded_at
    b.notifications = [(t, f"消息 {k}") for k in range(7)]
    html = views.inbox(b)
    shown, _, rest = html.partition('<details class="more"><summary>更早 4 条</summary>')
    assert "消息 2" in shown and "消息 3" not in shown and "消息 6" in rest
    first = b.human_notes(b.idea("exp1")["id"])[0]
    b.notes += [dict(first, id=900 + k, text=f"笔记 {k}") for k in range(4)]
    detail = views.idea_detail(b, "exp1")
    assert "更早 2 条" in detail and "笔记 0" in detail and "笔记 3" in detail


def test_empty_groups_collapse(initialized: Path) -> None:
    b = data.load(initialized)
    html = views.inbox(b)
    for title in ("待批准的 run", "待确认的经验", "等 Claude 的待办", "最近通知"):
        assert f'<div class="group empty">{title} · 无</div>' in html
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
    assert labels == ["exp1", "exp2", "exp3", "exp4"]
    # confidence (and merged / dropped) as a short text label at the end of each row
    ends = ["".join(s.text or "" for s in t.iter(f"{ns}tspan")) for t in root.iter(f"{ns}text")
            if any(True for _ in t.iter(f"{ns}tspan"))]
    assert ends == ["初步", "猜测", "猜测 · 已放弃", "已确认 · 已合并"]
    titles = [t.text for t in root.iter(f"{ns}title")]
    assert "exp1 → exp2（从 r001 分出）" in titles and "exp1 → exp3（分支）" in titles
    ticks = [t for t in titles if re.match(r"r\d{3} · ", t)]
    assert len(ticks) == 7 and any(t.startswith("r004 · 运行中") for t in ticks)
    assert any(t.startswith("exp1 · 置信度 初步") for t in titles)
    assert any(t.startswith("exp4 · 置信度 已确认") for t in titles)
    dimmed = [g for g in root.iter(f"{ns}g") if g.get("opacity") == ".42"]
    assert len(dimmed) == 1 and "exp3" in ET.tostring(dimmed[0], encoding="unicode")
    # readable in light and dark: every colour is a theme variable
    assert not re.search(r"#[0-9a-f]{3,8}\b|black|white|rgb", svg, re.I)
    # run ticks are neutral; only the exceptions are coloured
    rects = [r.get("style", "") for r in root.iter(f"{ns}rect") if r.find(f"{ns}title") is not None]
    assert len(rects) == 7
    assert sum("--arb-bad" in r for r in rects) == 1 and sum("--arb-warn" in r for r in rects) == 1
    assert sum("--arb-tick" in r for r in rects) == 5
    assert "--arb-ok" not in svg and "--arb-run" not in svg
    # dropped rows can be collapsed
    hidden = views.tree(b, show_dropped=False)
    assert "exp3" not in views.tree_svg(b, show_dropped=False)
    assert "已隐藏 1 个已放弃的 idea：exp3" in hidden


def test_idea_detail(board_project: Path) -> None:
    b = data.load(board_project)
    html = views.idea_detail(b, "exp1")
    nb = board_project / ".arboria" / "notebooks" / "exp1.py"
    assert f'href="vscode://file{nb}"' in html
    assert _has_cmd(html, f"marimo edit {nb}")
    assert "lr: 3e-4 → 1e-4" in html and "时长: 2d12h → 1d" in html and "lr=3e-4" in html
    assert "baseline 看起来不错" in html and "置信度 → 初步" in html
    assert "<details><summary><b>r001</b>" in html  # summaries are collapsed
    assert "r005" in html and "草稿" in html
    # runs: run · 状态 · 改动 · 资源 · SU · wandb; the rest opens per row
    assert re.search(r"<dt>估计上限</dt><dd><span class=\"num\">240\.00 SU</span></dd></div>"
                     r"<div><dt>实际</dt><dd><span class=\"num\">237\.50 SU</span>", html)
    assert '<span class="r">237.50</span>' in html and "config.yaml" in html
    # branch / worktree / created behind 更多
    more = html.split('<details class="more"><summary>更多</summary>', 1)[1]
    assert "idea/exp1" in more.split("</details>", 1)[0]
    exp2 = views.idea_detail(b, "exp2")
    assert "exp1（从 r001 的快照分出）" in exp2 and "写命令 svd" in exp2
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
    assert re.search(r"已用 [\d,.]+<small> / 400 SU</small>", html) and "剩余" in html
    assert '<details class="more sub"><summary>估计 vs 实际' in html  # collapsed
    assert "活跃作业 <span class=\"count\">1</span>" in html
    assert '<span class="id">r004</span>' in html
    assert re.search(r"<b>r001</b>.*?<td class=\"num\">99%</td>", html, re.S)
    for part in re.findall(r"<svg.*?</svg>", html, re.S):
        _svg(part)
    assert "预算 400 SU" in views.su_line(b)  # used ≥ 25% of the budget: budget line drawn


def test_one_stylesheet_for_the_app_and_standalone_html(board_project: Path) -> None:
    css = (UI / "dashboard.css").read_text()
    app = (UI / "dashboard.py").read_text()
    assert 'css_file="dashboard.css"' in app and 'width="full"' in app
    # dark tokens follow marimo's theme class and, without an explicit light theme, the system
    for sel in ('body.dark', 'body[data-theme="dark"]', "@media (prefers-color-scheme: dark)",
                'body:not(.light):not(.light-theme):not([data-theme="light"])'):
        assert sel in css, sel
    assert "@import" not in css and "url(" not in css  # offline: no web fonts or assets
    page = views.page(data.load(board_project), "exp1")
    assert page.startswith("<style>" + css) and 'class="arb-grid"' in page
    for part in ("待你处理", "idea 树", "idea 详情 · exp1", "算力与预算"):
        assert part in page, part


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


DASHBOARD_MODULES = ("data.py", "views.py", "dashboard.py", "garden.py", "__init__.py")
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
    app = (UI / "dashboard.py").read_text()
    assert set(re.findall(r"mo\.ui\.(\w+)", app)) == {"refresh", "dropdown", "switch"}
    garden = (UI / "garden.py").read_text()
    assert set(re.findall(r"mo\.ui\.(\w+)", garden)) == {"refresh"}
    # transitively, too: importing the dashboard pulls in no write module
    code = ("import sys; import arboria.ui.data, arboria.ui.views; "
            "print(sorted(m for m in sys.modules if m.startswith('arboria')))")
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True,
                         env={**os.environ, "PYTHONPATH": str(SRC)})
    loaded = set(json.loads(res.stdout.replace("'", '"')))
    for bad in ("db", "records", "submission", "monitor", "backends", "tasks", "lessons",
                "ideas", "claudemd", "human", "notebooks", "commands", "state", "snapshot"):
        assert f"arboria.{bad}" not in loaded, bad


def test_rendering_calls_no_write_path(board_project: Path, monkeypatch) -> None:
    """Spy: every write helper, the backends and process/network entry points raise while
    the board is loaded and rendered, and while the marimo app runs in-process."""
    pytest.importorskip("marimo")
    import socket

    from arboria import backends, commands, db, ideas, lessons, monitor, records, \
        sessions, submission, tasks

    calls: list[str] = []

    def spy(name):
        def boom(*a, **k):
            calls.append(name)
            raise AssertionError(f"dashboard called {name}")
        return boom

    targets = [(db.Ledger, "write"), (db.Ledger, "__init__"), (db.Ledger, "backup"),
               (db, "commit_backup"), (db, "migrate"), (db, "connect"),
               (backends, "get_backend"), (subprocess, "run"), (subprocess, "Popen"),
               (subprocess, "call"), (subprocess, "check_output"), (socket.socket, "connect"),
               (socket, "create_connection"), (sessions, "notify"), (sessions, "record"),
               (monitor, "notify"), (monitor, "poll_run"), (monitor, "cancel")]
    for mod in (records, tasks, lessons, submission):
        for name in dir(mod):
            fn = getattr(mod, name)
            if callable(fn) and getattr(fn, "__module__", "") == mod.__name__ and \
                    not isinstance(fn, type):
                targets.append((mod, name))
    for name in ("new_idea", "close_idea", "ensure_notebook"):
        targets.append((ideas, name))
    targets.append((commands, "request"))
    for obj, name in targets:
        monkeypatch.setattr(obj, name, spy(f"{getattr(obj, '__name__', obj)}.{name}"))

    connects: list[tuple] = []
    real_connect = sqlite3.connect

    def ro_connect(*a, **k):
        connects.append((a, k))
        return real_connect(*a, **k)

    monkeypatch.setattr(sqlite3, "connect", ro_connect)
    ledger = board_project / ".arboria" / "ledger.db"
    before = ledger.read_bytes()
    files_before = sorted(p.name for p in (board_project / ".arboria").iterdir())

    b = data.load(board_project)
    for part in (views.header(b), views.inbox(b), views.tree(b, False, "exp2"),
                 views.idea_detail(b, "exp2"), views.idea_detail(b, "exp3"), views.budget(b)):
        assert part
    monkeypatch.setenv("ARBORIA_UI_ROOT", str(board_project))
    monkeypatch.setenv("ARBORIA_UI_IDEA", "exp2")
    from arboria.ui import dashboard

    outputs, defs = dashboard.app.run()
    assert defs["board"].idea("exp2") and defs["picker"].value == "exp2"
    assert calls == []
    assert connects and all(k.get("uri") and str(a[0]).endswith("ledger.db?mode=ro")
                            for a, k in connects)
    assert ledger.read_bytes() == before
    # only SQLite's own -wal/-shm may appear next to the ledger
    after = sorted(p.name for p in (board_project / ".arboria").iterdir())
    assert set(after) - set(files_before) <= {"ledger.db-wal", "ledger.db-shm"}


# ---------------------------------------------------------------- the real marimo


def _decoded(html: str) -> str:
    return re.sub(r"\\u([0-9a-fA-F]{4})", lambda m: chr(int(m.group(1), 16)), html)


def test_dashboard_passes_marimo_check() -> None:
    pytest.importorskip("marimo")
    res = subprocess.run([sys.executable, "-m", "marimo", "check", "--strict", str(APP)],
                         capture_output=True, text=True, timeout=120)
    assert res.returncode == 0 and "warning" not in (res.stdout + res.stderr).lower(), \
        res.stdout + res.stderr


def test_marimo_export_renders_the_dashboard(board_project: Path, tmp_path: Path) -> None:
    pytest.importorskip("marimo")
    app = tmp_path / "app" / "dashboard.py"  # a copy: export writes __marimo__/ beside it
    app.parent.mkdir()
    shutil.copy(APP, app)
    shutil.copy(APP.with_name("dashboard.css"), app.with_name("dashboard.css"))
    out = tmp_path / "out.html"
    env = {**os.environ, "PYTHONPATH": str(SRC) + os.pathsep + os.environ.get("PYTHONPATH", "")}
    env.pop("ARBORIA_UI_ROOT", None)
    res = subprocess.run([sys.executable, "-m", "marimo", "export", "html", str(app), "-o",
                          str(out), "--no-include-code", "--", "--root", str(board_project),
                          "--idea", "exp2"],
                         cwd=tmp_path, env=env, capture_output=True, text=True, timeout=180)
    assert res.returncode == 0, res.stdout + res.stderr
    html = _decoded(out.read_text())
    for text in ("Arboria 看板", "更新于", views.UPDATED_TIP, "--arb-accent",
                 "待你处理", "待批准的 run", "arb approve r003", "arb lesson accept l001",
                 "idea 树", "exp1 → exp2（从 r001 分出）", "idea 详情 · exp2", "算力与预算",
                 "活跃作业", "估计 vs 实际", "&lt;script&gt;alert(1)"):
        assert text in html, text
    assert "Traceback" not in html and "看板读不到账本" not in html
    preview = os.environ.get("ARBORIA_UI_PREVIEW")
    if preview:  # a rendered sample for humans (not part of the assertions)
        shutil.copy(out, preview)


# ---------------------------------------------------------------- arb ui


def test_arb_ui_launches_marimo_on_localhost(idea: Path, initialized: Path, arb,
                                            monkeypatch) -> None:
    seen: list[tuple[list[str], dict]] = []
    monkeypatch.setattr(launch, "run_marimo", lambda cmd, env: seen.append((cmd, env)) or 0)
    monkeypatch.setattr(launch, "marimo_available", lambda: True)
    arb(idea, "ui")  # from the idea worktree: that idea is selected
    cmd, env = seen[-1]
    assert cmd[:5] == [sys.executable, "-m", "marimo", "run", str(APP)]
    assert cmd[cmd.index("--host") + 1] == "127.0.0.1" and "--token" in cmd
    assert cmd[cmd.index("--") + 1:] == ["--root", str(initialized), "--idea", "exp1"]
    assert "--port" not in cmd
    assert env["ARBORIA_UI_ROOT"] == str(initialized) and env["ARBORIA_UI_IDEA"] == "exp1"
    arb(initialized, "ui", "--port", "8765")  # from main: no idea preselected
    cmd, env = seen[-1]
    assert cmd[cmd.index("--port") + 1] == "8765" and "--idea" not in cmd
    assert "ARBORIA_UI_IDEA" not in env
    arb(initialized, "ui", "--idea", "exp1")
    assert seen[-1][0][-2:] == ["--idea", "exp1"]
    assert "no idea named 'nope'" in arb(initialized, "ui", "--idea", "nope", ok=False).stderr
    assert "--port must be" in arb(initialized, "ui", "--port", "0", ok=False).stderr
    assert len(seen) == 3


def test_arb_ui_without_marimo(initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.setattr(launch, "marimo_available", lambda: False)
    monkeypatch.setattr(launch, "run_marimo", lambda cmd, env: pytest.fail("launched"))
    res = arb(initialized, "ui", ok=False)
    assert res.exit_code == 1 and "arb ui 需要 marimo" in res.stderr and "[ui]" in res.stderr


def test_arb_ui_needs_an_initialized_repo(repo: Path, arb) -> None:
    assert "arb init" in arb(repo, "ui", ok=False).stderr


def test_dashboard_app_on_an_empty_project(initialized: Path, monkeypatch) -> None:
    pytest.importorskip("marimo")
    monkeypatch.setenv("ARBORIA_UI_ROOT", str(initialized))
    monkeypatch.delenv("ARBORIA_UI_IDEA", raising=False)
    from arboria.ui import dashboard

    outputs, defs = dashboard.app.run()
    assert defs["picker"].value is None and defs["board"].ideas == []
    html = "".join(getattr(o, "text", "") for o in outputs if o is not None)
    assert "还没有 idea" in html and "未设项目预算" in html


# ---------------------------------------------------------------- 种子园 (arb ui --home)


@pytest.fixture
def garden_home(arb, tmp_path: Path, isolated_home: Path, initialized: Path) -> Path:
    arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--hypothesis", "浅层需要更高的秩", "--metric", "val/loss")
    arb(tmp_path, "seed", "new", "xss", uiutil.XSS)
    arb(tmp_path, "seed", "new", "muon-small", "Muon 在小模型上")
    arb(tmp_path, "seed", "drop", "muon-small")
    arb(initialized, "seed", "plant", "rank-schedule")
    from arboria import lab

    scope = {"__name__": "__marimo__", "__file__": str(isolated_home / "seeds" / "xss.py"),
             "lab": lab}
    exec('lab.todo("找三篇相关论文")', scope)
    return isolated_home


def test_garden_data_and_views(garden_home: Path) -> None:
    g = data.load_home(garden_home)
    assert {s["name"] for s in g.seeds} == {"xss", "rank-schedule", "muon-small"}
    edited = [s["last_edited"] for s in g.seeds]
    assert edited == sorted(edited, reverse=True)  # most recently edited first
    assert (g.count("thinking"), g.count("planted"), g.count("dropped")) == (1, 1, 1)
    rs = next(s for s in g.seeds if s["name"] == "rank-schedule")
    assert [p["idea"] for p in rs["planted_to"]] == ["rank-schedule"]
    assert [(t["id"], t["seed"]) for t in g.todos] == [("t001", "xss")]
    head = views.garden_header(g)
    for label, n in (("思考中", 1), ("已种下", 1), ("已放弃", 1)):
        assert f'<div class="k">{label}</div><div class="v">{n}</div>' in head
    body = views.garden_body(g)
    nb = garden_home / "seeds" / "rank-schedule.py"
    assert f'href="vscode://file{nb}"' in body and _has_cmd(body, views.edit_command(nb))
    assert "浅层需要更高的秩" in body and "proj · rank-schedule" in body and "已种下" in body
    assert "找三篇相关论文" in body and "等 Claude 的待办" in body
    # dropped seeds are behind one disclosure; free text is escaped
    assert '<details class="more"><summary>已放弃 1 个</summary>' in body
    assert body.index("muon-small") > body.index("已放弃 1 个")
    assert "<script>" not in body and "&lt;script&gt;alert(1)&lt;/script&gt;" in body
    assert "<button" not in body and "<form" not in body and "<input" not in body
    page = views.garden_page(g)
    assert page.startswith("<style>") and "Arboria 种子园" in page


def test_edit_command_quotes_odd_paths() -> None:
    assert views.edit_command(Path.home() / "arboria" / "seeds" / "a-b.py") == \
        "marimo edit ~/arboria/seeds/a-b.py"
    assert views.edit_command(Path("/tmp/my seeds/a.py")) == "marimo edit '/tmp/my seeds/a.py'"


def test_empty_garden(arb, tmp_path: Path, isolated_home: Path) -> None:
    from arboria import seeds

    seeds.ensure_home()
    g = data.load_home(isolated_home)
    assert g.seeds == [] and "还没有种子" in views.garden_body(g)


def test_project_header_counts_thinking_seeds(board_project: Path, arb, tmp_path: Path) -> None:
    assert data.load(board_project).home_thinking is None  # no home yet: no line
    assert "种子园" not in views.header(data.load(board_project))
    arb(tmp_path, "seed", "new", "aaa")
    arb(tmp_path, "seed", "new", "bbb")
    b = data.load(board_project)
    assert b.home_thinking == 2
    assert "种子园：2 个在思考中" in views.header(b) and "<code>arb ui --home</code>" in views.header(b)


def test_garden_reads_only(garden_home: Path, monkeypatch) -> None:
    """Spy: no write helper, process or socket while the garden loads and renders, and
    while its marimo app runs in-process; the ledger bytes stay the same."""
    pytest.importorskip("marimo")
    import socket

    from arboria import db, seeds, sessions, tasks

    calls: list[str] = []

    def spy(name):
        def boom(*a, **k):
            calls.append(name)
            raise AssertionError(f"garden called {name}")
        return boom

    targets = [(db.Ledger, "write"), (db.Ledger, "__init__"), (db, "commit_backup"),
               (db, "migrate"), (db, "connect"), (subprocess, "run"), (subprocess, "Popen"),
               (socket.socket, "connect"), (sessions, "notify"), (sessions, "record"),
               (seeds, "init_home"), (seeds, "new_seed"), (seeds, "set_fields"),
               (seeds, "drop"), (seeds, "plant"), (tasks, "add")]
    for obj, name in targets:
        monkeypatch.setattr(obj, name, spy(f"{getattr(obj, '__name__', obj)}.{name}"))
    ledger = garden_home / ".arboria" / "ledger.db"
    before = ledger.read_bytes()
    g = data.load_home(garden_home)
    assert views.garden_header(g) and views.garden_body(g)
    monkeypatch.setenv("ARBORIA_UI_HOME", str(garden_home))
    from arboria.ui import garden

    outputs, defs = garden.app.run()
    assert defs["garden"].count("thinking") == 1
    html = "".join(getattr(o, "text", "") for o in outputs if o is not None)
    assert "Arboria 种子园" in html and "rank-schedule" in html
    assert calls == [] and ledger.read_bytes() == before


def test_garden_passes_marimo_check() -> None:
    pytest.importorskip("marimo")
    res = subprocess.run([sys.executable, "-m", "marimo", "check", "--strict",
                          str(UI / "garden.py")], capture_output=True, text=True, timeout=120)
    assert res.returncode == 0 and "warning" not in (res.stdout + res.stderr).lower(), \
        res.stdout + res.stderr


def test_arb_ui_home(arb, tmp_path: Path, isolated_home: Path, idea: Path, initialized: Path,
                     monkeypatch) -> None:
    seen: list[tuple[list[str], dict]] = []
    monkeypatch.setattr(launch, "run_marimo", lambda cmd, env: seen.append((cmd, env)) or 0)
    monkeypatch.setattr(launch, "marimo_available", lambda: True)
    res = arb(tmp_path, "ui")  # outside any repo: the seed home, created on first use
    assert "种子园建在" in res.stderr and (isolated_home / ".arboria" / "ledger.db").is_file()
    cmd, env = seen[-1]
    assert cmd[:5] == [sys.executable, "-m", "marimo", "run", str(GARDEN)]
    assert cmd[cmd.index("--host") + 1] == "127.0.0.1" and "--token" in cmd
    assert cmd[cmd.index("--") + 1:] == ["--home", str(isolated_home)]
    assert env["ARBORIA_UI_HOME"] == str(isolated_home)
    arb(isolated_home, "ui", "--port", "8800")  # inside the home
    assert seen[-1][0][4] == str(GARDEN) and "8800" in seen[-1][0]
    arb(idea, "ui", "--home")  # from a repo, explicitly
    assert seen[-1][0][4] == str(GARDEN)
    arb(idea, "ui")  # a repo without --home: still the project's dashboard
    assert seen[-1][0][4] == str(APP)
    assert "--idea is for a project" in arb(tmp_path, "ui", "--idea", "x", ok=False).stderr
