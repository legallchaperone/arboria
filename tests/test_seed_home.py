"""The seed home with Claude Code and the notebook API: lab in a seed notebook, todos,
the SessionStart hook, `arb status` and `arb install-claude --home`."""

from __future__ import annotations

import io
import json
import os
from pathlib import Path

import pytest

from arboria import hook, install, lab, sessions
from arboria.db import Ledger
from arboria.errors import ArbError
from arboria.paths import Home


@pytest.fixture(autouse=True)
def no_pinned_idea():
    lab.use(None)
    yield
    lab.use(None)


@pytest.fixture
def home(arb, tmp_path: Path, isolated_home: Path) -> Home:
    arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--metric", "同参数量下的 val/loss")
    arb(tmp_path, "seed", "new", "sparse-attn", "稀疏注意力的长度外推")
    return Home(isolated_home)


def _in_cell(nb: Path, code: str) -> dict:
    """Run `code` as a marimo cell of the notebook `nb` would (its __file__ is the notebook)."""
    scope = {"__name__": "__marimo__", "__file__": str(nb), "lab": lab}
    exec(code, scope)
    return scope


# ---------------------------------------------------------------- lab in a seed notebook


def test_resolve_recognises_seed_notebooks(home: Home, tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    t = lab.resolve(file=home.notebook("rank-schedule"))
    assert isinstance(t, lab.SeedTarget) and (t.seed, t.source) == ("rank-schedule", "file")
    assert t.home.root == home.root and t.state_dir == home.state_dir
    scope = _in_cell(home.notebook("sparse-attn"), "t = lab.resolve()")
    assert scope["t"].seed == "sparse-attn"
    monkeypatch.setattr(lab, "_marimo_file", lambda: str(home.notebook("sparse-attn")))
    assert lab.resolve().source == "marimo"


def test_runs_and_ideas_refuse_with_a_pointer_to_plant(home: Home, arb, initialized: Path,
                                                      tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    nb = home.notebook("rank-schedule")
    for code in ('lab.run(reason="x")', 'lab.get("r001")', "lab.runs()",
                 'lab.compare(["r001"], "loss")', 'lab.fork("x", hypothesis="h", metric="m")',
                 'lab.prune("no")'):
        with pytest.raises(ArbError) as exc:
            _in_cell(nb, code)
        assert str(exc.value) == ("这是一个种子（rank-schedule），还没种进仓库："
                                  "先在目标仓库里运行 arb idea plant rank-schedule"), code
    with pytest.raises(ArbError, match="种子没有置信度"):
        _in_cell(nb, 'lab.conf("prelim")')
    # once planted, the error says where the idea is
    arb(initialized, "seed", "plant", "rank-schedule", "--hypothesis", "浅层要更高的秩")
    with pytest.raises(ArbError, match="它已种进 proj · rank-schedule"):
        _in_cell(nb, 'lab.run(reason="x")')
    assert "approve" not in [n for n in dir(lab) if not n.startswith("_")]


def test_todo_and_metric_in_a_seed_notebook(home: Home, arb, tmp_path: Path,
                                            monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    nb = home.notebook("rank-schedule")
    t = _in_cell(nb, 't = lab.todo("找三篇讲 LoRA 秩分配的论文，各写一句结论")')["t"]
    assert (t["id"], t["seed"], t["idea"], t["status"]) == ("t001", "rank-schedule", None, "open")
    again = _in_cell(nb, 't = lab.todo("找三篇讲 LoRA 秩分配的论文，各写一句结论")')["t"]
    assert again["id"] == "t001"  # re-running the cell does not pile up todos
    rows = _in_cell(nb, "rows = lab.todos()")["rows"]
    assert [r["id"] for r in rows] == ["t001"]
    s = _in_cell(nb, 's = lab.metric("val/loss @ 1B tokens")')["s"]
    assert s["metric"] == "val/loss @ 1B tokens"
    shown = json.loads(arb(tmp_path, "seed", "show", "rank-schedule", "--json").stdout)
    assert shown["metric"] == "val/loss @ 1B tokens"
    # the human's writes only: refused inside Claude Code
    monkeypatch.setenv("CLAUDECODE", "1")
    with pytest.raises(ArbError, match="只由人在 notebook 里做"):
        _in_cell(nb, 'lab.todo("x")')
    with pytest.raises(ArbError, match="只由人在 notebook 里做"):
        _in_cell(nb, 'lab.metric("x")')
    monkeypatch.delenv("CLAUDECODE")
    # Claude, in a session opened in the home, sees and finishes the todo there
    out = arb(home.root, "todo", "list").stdout
    assert "t001" in out and "rank-schedule" in out
    arb(home.seeds_dir, "todo", "done", "t001", "--result", "三篇：AdaLoRA、…")
    assert "no open todos" in arb(home.root, "todo", "list").stdout
    assert "→ 三篇：AdaLoRA" in arb(home.root, "todo", "list", "--all").stdout
    with Ledger(home.state_dir) as ledger:
        ev = ledger.one("SELECT payload_json FROM events WHERE kind = 'todo_done'")
    assert json.loads(ev["payload_json"])["seed"] == "rank-schedule"


GLOBAL_CMD = '''\
def wordcount(ctx, note=None):
    """Count the words of the seed's gist."""
    assert ctx.idea is None
    try:
        ctx.run("r001")
    except Exception as exc:
        err = str(exc)
    return {"seed": ctx.seed.name, "words": len(ctx.seed.gist), "err": err,
            "cache": ctx.cache_dir.is_dir(), "nb": ctx.seed.notebook.name}
'''


def test_global_commands_in_a_seed_notebook(home: Home, tmp_path: Path, monkeypatch) -> None:
    monkeypatch.chdir(tmp_path)
    gdir = Path(os.environ["ARBORIA_CONFIG_DIR"]) / "commands"
    gdir.mkdir(parents=True)
    (gdir / "wordcount.py").write_text(GLOBAL_CMD)
    nb = home.notebook("rank-schedule")
    v = _in_cell(nb, "v = lab.wordcount()")["v"]
    assert v["seed"] == "rank-schedule" and v["words"] == len("LoRA 的秩随层变化")
    assert v["err"].startswith("这是一个种子") and v["cache"] and v["nb"] == "rank-schedule.py"
    assert (home.state_dir / "cache" / "commands" / "wordcount").is_dir()
    # a missing command becomes a global command todo in the home ledger
    p = _in_cell(nb, 'p = lab.svd("画奇异值")')["p"]
    assert not p and p.task_id == "t001" and p.error is None
    with Ledger(home.state_dir) as ledger:
        task = ledger.one("SELECT * FROM tasks")
    data = json.loads(task["payload_json"])
    assert task["kind"] == "command_new" and data["scope"] == "global"
    assert data["seed"] == "rank-schedule" and data["path"] == str(gdir / "svd.py")
    assert _in_cell(nb, 'p = lab.svd("画奇异值")')["p"].task_id == "t001"  # reused
    with pytest.raises(ArbError, match="只能用全局命令"):
        _in_cell(nb, 'lab.svd("x", scope="project")')
    names = [r["name"] for r in _in_cell(nb, "c = lab.commands()")["c"]]
    assert names == ["todo", "todos", "metric", "wordcount"]  # no run built-ins in a seed


def test_command_todo_is_pushed_to_the_home_session(home: Home, tmp_path: Path,
                                                    monkeypatch) -> None:
    sent: list[tuple] = []
    monkeypatch.setattr(sessions, "notify", lambda *a, **k: sent.append(a) or True)
    p = _in_cell(home.notebook("sparse-attn"), 'p = lab.attn_map("画注意力图")')["p"]
    assert p.pushed
    state, worktree, text = sent[0]
    assert (state, worktree) == (home.state_dir, home.root)
    assert "种子 sparse-attn" in text and "t001" in text


# ---------------------------------------------------------------- Claude Code in the home


def test_seed_writes_refused_inside_claude_code(home: Home, arb, tmp_path: Path,
                                                monkeypatch) -> None:
    monkeypatch.setenv("CLAUDECODE", "1")
    for args in (["new", "x"], ["set", "sparse-attn", "--gist", "y"],
                 ["drop", "sparse-attn"], ["revive", "sparse-attn"], ["plant", "sparse-attn"]):
        res = arb(home.root, "seed", *args, ok=False)
        assert res.exit_code == 1 and "只由人" in res.stderr, args
    # reading is fine
    assert "rank-schedule" in arb(home.root, "seed", "list").stdout
    assert "LoRA" in arb(home.root, "seed", "show", "rank-schedule").stdout


def _hook(arb, cwd: Path) -> str:
    res = arb(cwd, "hook", "session-start", input=json.dumps({"cwd": str(cwd)}))
    assert res.exit_code == 0
    return res.stdout


def test_hook_in_the_home(home: Home, arb, tmp_path: Path, monkeypatch) -> None:
    _in_cell(home.notebook("rank-schedule"), 'lab.todo("整理三篇相关工作")')
    for k in range(5):
        arb(tmp_path, "seed", "new", f"extra-{k}", f"第 {k} 个")
    arb(tmp_path, "seed", "drop", "extra-0")
    for cwd in (home.root, home.seeds_dir):
        ctx = json.loads(_hook(arb, cwd))["hookSpecificOutput"]["additionalContext"]
        assert ctx.startswith("[arboria] 种子园 ") and "6 个在思考中" in ctx
        assert "1 个已放弃" in ctx and "最近编辑的种子：" in ctx
        assert ctx.count("\n- ") >= 5 and "… 另有 1 个" in ctx and "extra-0" not in ctx
        assert "t001 [rank-schedule]: 整理三篇相关工作" in ctx
        assert "never edit" in ctx and "arb todo done" in ctx
    # the session's inbox is recorded for the home (where seed command todos are pushed)
    import socket
    import tempfile

    # macOS caps AF_UNIX paths at 104 bytes; pytest's tmp_path can be longer
    sock = Path(tempfile.mkdtemp(prefix="arb", dir="/tmp")) / "inbox.sock"

    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(str(sock))
    try:
        monkeypatch.setenv(sessions.SOCKET_ENV, str(sock))
        monkeypatch.setattr(sessions, "claude_pid", lambda limit=6: None)
        _hook(arb, home.root)
        assert sessions.live_session(home.state_dir, home.root)["socket"] == str(sock)
    finally:
        srv.close()


def test_hook_in_a_home_not_yet_created_prints_nothing(arb, isolated_home: Path) -> None:
    isolated_home.mkdir(parents=True)
    assert _hook(arb, isolated_home) == ""
    assert not (isolated_home / ".arboria").exists()


def test_hook_in_the_home_never_crashes(home: Home, monkeypatch) -> None:
    def boom(*a, **k):
        raise RuntimeError("kaboom")

    monkeypatch.setattr(hook, "home_status", boom)
    out = io.StringIO()
    assert hook.session_start(io.StringIO(json.dumps({"cwd": str(home.root)})), out) == 0
    assert out.getvalue() == ""
    assert "kaboom" in (home.state_dir / "hook.log").read_text()


def test_status_in_the_home(home: Home, arb) -> None:
    out = arb(home.root, "status").stdout
    assert "2 个在思考中" in out and "种子（最近编辑的在前）：" in out
    data = json.loads(arb(home.root, "status", "--json").stdout)
    assert data["scope"] == "home" and {s["name"] for s in data["seeds"]} == {
        "rank-schedule", "sparse-attn"}


def test_install_claude_home(arb, tmp_path: Path, isolated_home: Path, repo: Path) -> None:
    res = arb(tmp_path, "install-claude", "--home")
    assert "种子园建在" in res.stderr
    settings = json.loads((isolated_home / ".claude" / "settings.json").read_text())
    assert settings["hooks"]["SessionStart"] == [
        {"hooks": [{"type": "command", "command": "arb hook session-start"}]}]
    deny = settings["permissions"]["deny"]
    assert deny == install.HOME_DENY_RULES
    for tool in ("Edit",):
        assert f"{tool}(**/seeds/**)" in deny and f"{tool}(**/.arboria/**)" in deny
    # every seed write, under both spellings: in the home `arb idea new/drop` act on seeds
    for rule in ("Bash(arb approve:*)", "Bash(sbatch:*)", "Bash(arb seed plant:*)",
                 "Bash(arb seed new:*)", "Bash(arb seed set:*)", "Bash(arb seed drop:*)",
                 "Bash(arb seed revive:*)", "Bash(arb idea new:*)", "Bash(arb idea set:*)",
                 "Bash(arb idea drop:*)", "Bash(arb idea revive:*)", "Bash(arb idea plant:*)"):
        assert rule in deny
    assert not any(r.startswith(("Bash(arb idea list", "Bash(arb idea show")) for r in deny)
    assert not any(r.startswith("Read(") for r in deny)  # reading seeds stays allowed
    gdir = Path(os.environ["ARBORIA_CONFIG_DIR"]) / "commands"
    assert settings["permissions"]["additionalDirectories"] == [str(gdir)]
    assert not (isolated_home / ".claude" / "settings.local.json").exists()
    md = (isolated_home / "CLAUDE.md").read_text()
    assert md.startswith("<!-- arboria:begin") and md.rstrip().endswith("<!-- arboria:end -->")
    assert "## 种子园（Arboria）" in md and "绝不修改" in md and "arb idea plant NAME" in md
    assert "`arb idea new/set/drop/revive`" in md and "arb idea list" in md
    assert "经验（已确认）" not in md
    # idempotent; the user's own text around the block is kept
    (isolated_home / "CLAUDE.md").write_text("# 我的笔记\n\n" + md)
    res = arb(isolated_home, "install-claude", "--home")
    assert "already up to date" in res.stdout and "arboria block" not in res.stdout
    assert (isolated_home / "CLAUDE.md").read_text().startswith("# 我的笔记\n\n<!-- arboria")
    assert "only one of" in arb(repo, "install-claude", "--home", "--main", ok=False).stderr
    assert not (repo / ".claude").exists()
