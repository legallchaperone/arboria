"""Notebook commands: library, lab.<name>(...), command todos, revise, watching."""

from __future__ import annotations

import json
import sqlite3
import threading
import time
from pathlib import Path

import pytest

from arboria import commands, db, lab, tasks
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git
from .test_submit import cluster  # noqa: F401  (fixture)

SVD = '''\
def svd(ctx, run, note=None, *, k=20):
    """对 run 的权重做 SVD，画前 k 个奇异值。"""
    r = ctx.run(run)
    (ctx.cache_dir / "seen.txt").write_text(r.id)
    return {"run": r.id, "k": k, "idea": ctx.idea.name, "out": str(r.local_out),
            "config": r.config, "mo": ctx.mo is not None, "note": note}
'''


@pytest.fixture(autouse=True)
def config_dir(tmp_path_factory, monkeypatch) -> Path:
    cfg = tmp_path_factory.mktemp("arbcfg")
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    lab.use(None)
    yield cfg
    lab.use(None)


def _write(folder: Path, name: str, text: str) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f"{name}.py"
    path.write_text(text)
    return path


def _project(main: Path) -> Path:
    return main / ".arboria" / "commands"


def _tasks(main: Path) -> list[dict]:
    with Ledger(main / ".arboria") as l:
        return l.all("SELECT * FROM tasks ORDER BY id")


def _has_marimo() -> bool:
    import importlib.util

    return importlib.util.find_spec("marimo") is not None


def _subj(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


# ---------------------------------------------------------------- schema


def test_migration_v4_to_v5_adds_task_kind(tmp_path: Path) -> None:
    state = tmp_path / "state"
    state.mkdir()
    conn = db.connect(state / "ledger.db")
    for script in db.MIGRATIONS[:4]:
        for stmt in db._split_sql(script):
            conn.execute(stmt)
    conn.execute("PRAGMA user_version = 4")
    conn.execute("INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
                 " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')")
    conn.execute("INSERT INTO tasks (id, idea_id, text, created_at) VALUES ('t001', 1, 'x', 't')")
    conn.close()
    with Ledger(state) as ledger:
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION == 5
        row = ledger.one("SELECT * FROM tasks")
        assert (row["id"], row["text"], row["kind"], row["payload_json"]) == \
            ("t001", "x", "plain", None)
        with pytest.raises(sqlite3.IntegrityError), ledger.write() as c:
            c.execute("INSERT INTO tasks (id, idea_id, text, created_at, kind)"
                      " VALUES ('t002', 1, 'x', 't', 'other')")


# ---------------------------------------------------------------- the library


def test_discover_scopes_override_and_problems(initialized: Path, config_dir: Path, arb) -> None:
    g = config_dir / "commands"
    _write(g, "svd", '"""module doc"""\ndef svd(ctx):\n    """全局版本。"""\n')
    _write(g, "loss_plot", 'def loss_plot(ctx, *runs):\n    """画 loss。\n\n    细节。"""\n')
    _write(g, "Bad-Name", "x = 1\n")  # not a command name: ignored
    _write(_project(initialized), "svd", 'def svd(ctx, run):\n    """项目版本。"""\n')
    _write(_project(initialized), "nodoc", "def nodoc(ctx):\n    return 1\n")
    _write(_project(initialized), "nofunc", '"""只有模块说明。"""\nx = 1\n')
    _write(_project(initialized), "run", 'def run(ctx):\n    """撞名。"""\n')
    _write(_project(initialized), "broken", "def broken(ctx:\n")
    rows = commands.discover(initialized / ".arboria")
    by = {(r["name"], r["scope"]): r for r in rows}
    assert [(r["name"], r["scope"]) for r in rows] == [
        ("broken", "project"), ("loss_plot", "global"), ("nodoc", "project"),
        ("nofunc", "project"), ("run", "project"), ("svd", "project"), ("svd", "global")]
    assert by[("svd", "project")]["active"] and not by[("svd", "global")]["active"]
    assert by[("svd", "project")]["description"] == "项目版本。"
    assert by[("loss_plot", "global")]["description"] == "画 loss。"  # first line
    assert by[("loss_plot", "global")]["problem"] is None
    assert by[("nodoc", "project")]["problem"] == "no docstring"
    assert "no top-level `def nofunc" in by[("nofunc", "project")]["problem"]
    assert by[("nofunc", "project")]["description"] == "只有模块说明。"
    assert "shadowed by lab.run" in by[("run", "project")]["problem"]
    assert "cannot parse" in by[("broken", "project")]["problem"]
    sha = commands._sha(_project(initialized) / "svd.py")
    assert by[("svd", "project")]["version"] == sha[:8]

    found = commands.find("svd", initialized / ".arboria")
    assert (found.scope, found.version, found.label) == ("project", sha[:8], "项目")
    assert commands.find("loss_plot", initialized / ".arboria").scope == "global"
    assert commands.find("loss_plot", None).scope == "global"
    assert commands.find("svd", None).scope == "global"

    data = json.loads(arb(initialized, "commands", "list", "--json").stdout)
    assert [(r["name"], r["scope"], r["active"]) for r in data][-2:] == [
        ("svd", "project", True), ("svd", "global", False)]
    out = arb(initialized, "commands", "list").stdout
    assert "svd" in out and "项目" in out and "(shadowed by the project's)" in out
    assert "⚠ shadowed by lab.run" in out


def test_commands_list_outside_repo_and_empty(tmp_path: Path, config_dir: Path, arb) -> None:
    assert "no commands yet" in arb(tmp_path, "commands", "list").stdout
    _write(config_dir / "commands", "hello", 'def hello(ctx):\n    """hi"""\n')
    assert "hello" in arb(tmp_path, "commands", "list").stdout


def test_guide_has_the_conventions(arb, tmp_path: Path, config_dir: Path) -> None:
    out = arb(tmp_path, "commands", "guide").stdout
    for needle in ("ctx.run(run_or_id)", ".local_out", ".config", ".metrics(keys)", "ctx.idea",
                   "ctx.mo", "ctx.cache_dir", "ctx.value", "docstring", "不联网",
                   "只在 `ctx.cache_dir` 下写文件", "只读", "arb todo done tNNN --result",
                   "Claude 面板里问人", ".arboria/commands", str(config_dir / "commands"),
                   "ctx.mo.ui", "notebook 是人的"):
        assert needle in out, needle


def test_names_are_validated() -> None:
    for bad in ("Svd", "1svd", "_svd", "s" * 42, "a-b", ""):
        with pytest.raises(ArbError, match="invalid command name"):
            commands.validate_name(bad)
    with pytest.raises(ArbError, match="already part of lab"):
        commands.validate_name("runs")
    assert commands.validate_name("s" * 41) == "s" * 41
    with pytest.raises(AttributeError):
        lab._private_thing  # noqa: B018
    with pytest.raises(AttributeError):
        lab.CamelCase  # noqa: B018
    assert repr(lab.svd) == "lab.svd"


def test_safe_repr() -> None:
    class Loud:
        def __repr__(self):
            raise RuntimeError("never called")

    assert "..." in commands.safe_repr("x" * 500) and len(commands.safe_repr("x" * 500)) < 220
    assert commands.safe_repr(3) == "3" and commands.safe_repr(None) == "None"
    assert commands.safe_repr([1, 2]) == "[1, 2]"
    assert commands.safe_repr(Loud()) == "<Loud>"
    assert commands.safe_repr([Loud()]) == "<list of 1>"
    assert commands.safe_repr(Loud(), lambda v: "named") == "named"


# ---------------------------------------------------------------- missing command -> todo


def test_missing_command_becomes_a_todo(idea: Path, initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    arb(idea, "run", "new", "--reason", "baseline")
    r = lab.get("r001")
    before = _subj(initialized)[0]
    p = lab.svd(r, "对 attention 权重做 SVD，画前 20 个奇异值", k=20)
    assert isinstance(p, lab.Pending) and not p
    assert p._repr_markdown_() == (
        "命令 svd 还不存在，已交给 Claude（t001）。写好后重新运行这个 cell。"
        "在 Claude 面板里说'处理待办'即可。")
    assert repr(p) == p.text()
    (task,) = _tasks(initialized)
    assert (task["kind"], task["status"]) == ("command_new", "open")
    data = json.loads(task["payload_json"])
    assert data == {
        "name": "svd", "scope": "project", "args": ["Run('r001')",
                                                    "'对 attention 权重做 SVD，画前 20 个奇异值'"],
        "kwargs": {"k": "20"}, "description": "对 attention 权重做 SVD，画前 20 个奇异值",
        "idea": "exp1",
        "call": "lab.svd(Run('r001'), '对 attention 权重做 SVD，画前 20 个奇异值', k=20)",
        "path": str(initialized / ".arboria" / "commands" / "svd.py")}
    assert task["text"].startswith("写 notebook 命令 svd（项目）：对 attention 权重做 SVD")
    assert "arb commands guide" in task["text"]
    assert _subj(initialized)[0] == "arboria backup: todo_new t001" != before

    # re-running the cell reuses the open todo
    again = lab.svd(r, "对 attention 权重做 SVD，画前 20 个奇异值", k=20)
    assert again.task_id == "t001" and not again.created and len(_tasks(initialized)) == 1
    assert lab.svd("r001", "别的说法").task_id == "t001"

    # the hook brief shows it distinctly, and todo list marks the kind
    hook = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(hook)["hookSpecificOutput"]["additionalContext"]
    assert "命令待办：svd（t001）" in ctx and "arb commands guide" in ctx
    assert "来自 notebook 的待办" not in ctx
    assert "[command_new]" in arb(idea, "todo", "list").stdout
    rows = json.loads(arb(idea, "todo", "list", "--json").stdout)
    assert json.loads(rows[0]["payload_json"])["name"] == "svd"

    # global scope on request
    lab.plot_loss("画 loss", scope="global")
    t2 = _tasks(initialized)[1]
    assert json.loads(t2["payload_json"])["scope"] == "global"
    assert json.loads(t2["payload_json"])["path"].startswith(str(commands.global_dir()))
    assert "（全局）" in t2["text"]
    with pytest.raises(ArbError, match="invalid command scope"):
        lab.plot_loss(scope="team")

    # once Claude is done, a new call opens a new todo only if the file still is missing
    arb(idea, "todo", "done", "t001", "--result", "写好了")
    assert lab.svd(r, "x").task_id == "t003"


def test_overview_hook_lists_command_todos(idea: Path, initialized: Path, arb) -> None:
    lab.use("exp1", repo=initialized)
    lab.svd("做 SVD")
    lab.todo("plain one")
    out = arb(initialized, "status", "--brief").stdout
    assert "命令待办：svd（t001，exp1）" in out and "- t002 [exp1]: plain one" in out


def test_placeholder_never_raises(idea: Path, initialized: Path, monkeypatch) -> None:
    lab.use("exp1", repo=initialized)
    monkeypatch.setenv("CLAUDECODE", "1")  # todos are the human's
    p = lab.svd("x")
    assert p.task_id is None and "没能交给 Claude" in p.text() and "只由人在 notebook 里做" in p.text()
    assert _tasks(initialized) == []
    monkeypatch.delenv("CLAUDECODE")
    lab.use("nope", repo=initialized)
    assert "no idea named 'nope'" in lab.svd.revise("x").text()


# ---------------------------------------------------------------- running a command


def test_existing_command_runs_with_ctx(idea: Path, initialized: Path, arb, config_dir: Path,
                                        monkeypatch) -> None:
    monkeypatch.chdir(idea)
    arb(idea, "run", "new", "--reason", "baseline")
    with Ledger(initialized / ".arboria") as l, l.write() as c:
        c.execute("UPDATE runs SET config_json = ? WHERE id = 'r001'", ('{"lr": 0.001}',))
    path = _write(_project(initialized), "svd", SVD)
    backups = _subj(initialized)
    out = lab.svd(lab.get("r001"), "做 SVD", k=5)
    assert out == {"run": "r001", "k": 5, "idea": "exp1",
                   "out": str(initialized / ".arboria" / "runs" / "r001" / "out"),
                   "config": {"lr": 0.001}, "mo": _has_marimo(), "note": "做 SVD"}
    cache = initialized / ".arboria" / "cache" / "commands" / "svd"
    assert (cache / "seen.txt").read_text() == "r001"
    assert _subj(initialized) == backups  # calls are logged without a backup commit
    with Ledger(initialized / ".arboria") as l:
        ev = l.one("SELECT * FROM events WHERE kind = 'command_call'")
    payload = json.loads(ev["payload_json"])
    assert ev["subject"] == "svd" and payload["ok"] and payload["idea"] == "exp1"
    assert payload["version"] == commands._sha(path)[:8] and payload["scope"] == "project"
    assert _tasks(initialized) == []

    # edited file: reloaded; the global one is shadowed by the project's
    _write(config_dir / "commands", "svd", 'def svd(ctx, *a, **k):\n    """g"""\n    return "global"\n')
    path.write_text(SVD.replace('"k": k', '"k": k * 2'))
    assert lab.svd("r001", k=5)["k"] == 10
    path.unlink()
    assert lab.svd("r001") == "global"
    info = lab.svd.info()
    assert info["scope"] == "global" and info["description"] == "g"

    # an error in the command propagates (it is the human's kernel) and is logged
    _write(_project(initialized), "boom", 'def boom(ctx):\n    """x"""\n    raise ValueError("bad")\n')
    with pytest.raises(ValueError, match="bad"):
        lab.boom()
    with Ledger(initialized / ".arboria") as l:
        last = l.one("SELECT * FROM events WHERE kind = 'command_call' ORDER BY id DESC LIMIT 1")
    assert last["subject"] == "boom" and json.loads(last["payload_json"])["ok"] is False
    _write(_project(initialized), "nofunc", "x = 1\n")
    with pytest.raises(ArbError, match="defines no function nofunc"):
        lab.nofunc()


def test_ctx_value_and_source(idea: Path, initialized: Path) -> None:
    lab.use("exp1", repo=initialized)
    assert lab.svd.source() == "命令 svd 还不存在"
    assert lab.svd.info() is None
    _write(_project(initialized), "svd", SVD.replace("SVD", "S```VD"))
    src = lab.svd.source()
    assert str(src).startswith("def svd(ctx")
    md = src._repr_markdown_()
    assert md.startswith(f"`{_project(initialized) / 'svd.py'}`（v") and "````python\n" in md

    class FakeUI:
        value = 7

    FakeUI.__module__ = "marimo._plugins.ui._impl.input"
    assert lab.Ctx.value(FakeUI()) == 7 and lab.Ctx.value(3) == 3
    rows = lab.commands()
    assert [(r["name"], r["scope"]) for r in rows] == [("svd", "project")]
    # outside marimo: one readable line per command, not a truncated table
    assert repr(rows) == f"svd  项目 · v{rows[0]['version']} · {rows[0]['description']}"
    assert repr(lab.CommandList()).startswith("还没有命令")


def test_revise_creates_command_revise_todo(idea: Path, initialized: Path, config_dir: Path) -> None:
    lab.use("exp1", repo=initialized)
    g = _write(config_dir / "commands", "svd", 'def svd(ctx):\n    """g"""\n')
    p = lab.svd.revise("改成 log 坐标")
    assert p.kind == "command_revise" and p.task_id == "t001"
    assert p.text().startswith("已请 Claude 修改命令 svd（t001）。改好后用到它的 cell 会自动重跑。")
    (task,) = _tasks(initialized)
    data = json.loads(task["payload_json"])
    assert (task["kind"], data["scope"], data["path"], data["description"]) == \
        ("command_revise", "global", str(g), "改成 log 坐标")
    assert task["text"].startswith("修改 notebook 命令 svd（全局）：改成 log 坐标")
    assert lab.svd.revise("改成 log 坐标").task_id == "t001"  # same ask: reused
    assert lab.svd.revise("再加一条参考线").task_id == "t002"
    assert lab.svd.revise("  ").error and len(_tasks(initialized)) == 2
    # revising a command that does not exist yet asks for a new one
    assert lab.nothing.revise("画个图").kind == "command_new"
    assert _tasks(initialized)[-1]["kind"] == "command_new"


def test_command_todo_text_is_bounded(idea: Path, initialized: Path) -> None:
    lab.use("exp1", repo=initialized)
    lab.svd("长" * 3000)
    (task,) = _tasks(initialized)
    assert len(task["text"]) <= db.LIMITS["todo"]
    assert len(json.loads(task["payload_json"])["description"]) <= db.LIMITS["todo"]
    assert task["text"].endswith("约定见 `arb commands guide`")


# ---------------------------------------------------------------- re-running the cell


def test_watch_loop_reruns_once_the_file_settles(initialized: Path) -> None:
    state = initialized / ".arboria"
    calls: list[float] = []
    stop = threading.Event()
    t = threading.Thread(target=lambda: calls.append(lab._watch_loop(
        "svd", state, None, lambda: calls.append(time.monotonic()), stop.is_set, 0.05)))
    t.start()
    time.sleep(0.2)
    assert calls == []
    _write(_project(initialized), "svd", SVD)
    t.join(5)
    assert len(calls) == 2 and calls[-1] is True  # one rerun, then the loop returned

    seen = commands.fingerprint("svd", state)
    calls.clear()
    t = threading.Thread(target=lambda: calls.append(lab._watch_loop(
        "svd", state, seen, lambda: calls.append("rerun"), stop.is_set, 0.05)))
    t.start()
    time.sleep(0.3)
    assert calls == []  # unchanged
    stop.set()
    t.join(5)
    assert calls == [False]


def test_cell_rerunner_outside_marimo_is_none(initialized: Path) -> None:
    assert lab._cell_rerunner() is None
    lab.use("exp1", repo=initialized)


def test_marimo_private_api_still_there() -> None:
    """`_cell_rerunner` relies on these marimo internals (checked against 0.25.2)."""
    pytest.importorskip("marimo")
    import dataclasses

    from marimo._runtime.context.kernel_context import KernelRuntimeContext
    from marimo._runtime.context.types import ExecutionContext
    from marimo._runtime.dataflow.graph import DirectedGraph
    from marimo._runtime.runtime import Kernel

    assert "_kernel" in {f.name for f in dataclasses.fields(KernelRuntimeContext)}
    assert "cell_id" in {f.name for f in dataclasses.fields(ExecutionContext)}
    assert callable(Kernel._execute_stale_cells_callback) and callable(Kernel.lazy)
    assert callable(DirectedGraph.set_stale)


def test_tasks_add_rejects_unknown_kind(idea: Path, initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as ledger:
        idea_row = ledger.one("SELECT * FROM ideas")
        with pytest.raises(ArbError, match="invalid todo kind"):
            tasks.add(ledger, idea_row, "x", "weird")


def test_approvals_refused_while_a_command_runs(cluster, idea: Path, initialized: Path,
                                                monkeypatch) -> None:
    """Command code is Claude's and runs in the human's kernel: it cannot approve."""
    from .test_submit import _run

    monkeypatch.chdir(idea)
    lab.run(reason="x", gpus=1, time="1h")
    _write(_project(initialized), "sneaky", '''\
def sneaky(ctx, run):
    """tries to approve"""
    from arboria import submission
    from arboria.db import Ledger
    from arboria.paths import locate
    c = locate()
    with Ledger(c.state_dir) as ledger:
        submission.approve(c, ledger, run)
''')
    with pytest.raises(ArbError, match=r"不能在 notebook 命令（lab.sneaky）里运行"):
        lab.sneaky("r001")
    assert _run(initialized)["status"] == "awaiting_approval"
    assert cluster.calls() == []
    from arboria import human

    assert human.command_running() is None  # cleared after the call


def test_project_commands_are_versioned_in_the_backup_repo(idea: Path, initialized: Path,
                                                            arb) -> None:
    lab.use("exp1", repo=initialized)
    lab.svd("做 SVD")
    _write(_project(initialized), "svd", SVD)
    cache = initialized / ".arboria" / "cache" / "commands" / "svd"
    cache.mkdir(parents=True)
    (cache / "big.npy").write_text("x")
    (_project(initialized) / "notes.txt").write_text("not a command")
    arb(idea, "todo", "done", "t001", "--result", "写好了")
    state = initialized / ".arboria"
    files = git(state, "ls-files").splitlines()
    assert "commands/svd.py" in files
    assert not any(f.startswith(("cache/", "sessions/")) or f == "commands/notes.txt"
                   for f in files)
    assert "def svd(ctx" in git(state, "show", "HEAD:commands/svd.py")
