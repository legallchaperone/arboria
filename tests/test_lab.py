"""`from arboria import lab`: idea resolution, runs, todos and the human's records."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from arboria import lab, tasks
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git
from .test_submit import _bump, _run, cluster, jn  # noqa: F401  (fixture)


@pytest.fixture(autouse=True)
def no_pinned_idea():
    lab.use(None)
    yield
    lab.use(None)


def _nb(main: Path, name: str = "exp1") -> Path:
    return main / ".arboria" / "notebooks" / f"{name}.py"


def _subj(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


# ---------------------------------------------------------------- resolution


def test_resolve_order(idea: Path, initialized: Path, tmp_path: Path, monkeypatch) -> None:
    arb_state = initialized / ".arboria"
    monkeypatch.chdir(tmp_path)  # not a repo: only explicit sources work
    t = lab.resolve(file=_nb(initialized))
    assert (t.idea, t.source, t.ctx.main) == ("exp1", "file", initialized)
    assert lab.resolve(file=idea / "notebook.py").idea == "exp1"  # the worktree symlink

    # the caller's __file__, as in a marimo cell
    scope = {"__name__": "__marimo__", "__file__": str(_nb(initialized)), "lab": lab}
    exec("t = lab.resolve()", scope)
    assert (scope["t"].idea, scope["t"].source) == ("exp1", "file")

    monkeypatch.setattr(lab, "_marimo_file", lambda: str(_nb(initialized)))
    assert lab.resolve().source == "marimo"
    monkeypatch.setattr(lab, "_marimo_file", lambda: None)

    with pytest.raises(Exception, match="not inside a git repository"):
        lab.resolve()
    monkeypatch.chdir(idea / ".")
    assert (lab.resolve().idea, lab.resolve().source) == ("exp1", "cwd")
    sub = idea / "src"
    sub.mkdir()
    assert lab.resolve(cwd=sub).idea == "exp1"  # any folder of the worktree
    with pytest.raises(ArbError, match=r"lab.use\(\"<idea>\"\)"):
        lab.resolve(cwd=initialized)

    lab.use("exp1", repo=initialized)
    assert lab.resolve(cwd=tmp_path).source == "use"
    lab.use("nope", repo=initialized)
    with pytest.raises(ArbError, match="no idea named 'nope'"):
        lab.runs()
    assert arb_state.is_dir()


# ---------------------------------------------------------------- runs


def test_run_creates_card_and_never_approves(cluster, idea: Path, initialized: Path,
                                             monkeypatch) -> None:
    monkeypatch.chdir(idea)
    r = lab.run(reason="baseline", gpus=1, time="1h", set={"lr": 3e-4, "amp": True})
    assert r.id == "r001" and not r.existed and r.status() == "awaiting_approval"
    req = json.loads(_run(initialized)["request_json"])
    assert req["command"] == ["python", "train.py", "lr=0.0003", "amp=true"]
    md = r._repr_markdown_()
    assert md.startswith("**r001** · 待批准 · baseline\n\n在你自己的终端里批准：`arb approve r001`\n\n")
    assert "审核单 r001" in md and "审稿" not in md
    html = r._repr_html_()
    assert "<strong>r001</strong> · " in html and "待批准</span> · baseline</div>" in html
    assert re.search(r"在你自己的终端里批准：<code[^>]*>arb approve r001</code>", html)
    # the card is there, but collapsed
    assert "<details" in html and html.index("<details") < html.index("审核单 r001")
    assert "审核单 r001" in r.card() and r.jobs() == [] and r.wandb_url is None
    assert cluster.calls() == []

    again = lab.run(reason="re-executed cell", gpus=1, time="1h", set={"lr": 3e-4, "amp": True})
    assert again.id == "r001" and again.existed
    head = again._repr_html_().split("<details")[0]
    assert "没有重复提交" not in head and "没有重复提交" in again._repr_html_()
    child = lab.run(reason="lower lr", from_=r, set={"lr": 1e-4})
    assert child.id == "r002" and "**r002** · 待批准 · lower lr · 来自 r001\n" in \
        child._repr_markdown_()
    assert json.loads(_run(initialized, "r002")["request_json"])["command"][-2:] == \
        ["lr=0.0001", "amp=true"]
    rows = lab.runs()
    assert list(rows[0]) == ["run", "状态", "父 run", "原因", "资源", "SU", "创建"]
    assert [x["run"] for x in rows] == ["r001", "r002"]
    assert rows[0] | {"创建": ""} == {"run": "r001", "状态": "待批准", "父 run": "", "原因": "baseline",
                                    "资源": "1 GPU × 1h", "SU": "≤ 1", "创建": ""}
    assert rows[1]["父 run"] == "r001" and re.fullmatch(r"\d\d-\d\d \d\d:\d\d", rows[1]["创建"])
    raw = lab.runs(raw=True)
    assert raw[1]["parent_run"] == "r001" and raw[1]["status"] == "awaiting_approval"
    assert "request_json" in raw[1] and raw[1]["est_cost"] == 1.0
    assert "approve" not in [n for n in dir(lab) if not n.startswith("_")]


def test_run_status_rendering_after_approval(cluster, idea: Path, initialized: Path, arb,
                                             monkeypatch) -> None:
    monkeypatch.chdir(idea)
    cluster.script(jn(cluster, "r001", 1), run_s=3600, billed=1.5)
    r = lab.run(reason="x", gpus=4, time="2h")
    arb(idea, "approve", "r001")  # the human, in their terminal
    cluster.advance(10)
    arb(idea, "poll")
    md = r._repr_markdown_()
    assert md.startswith("**r001** · 运行中 · x\n\n已运行 10s\n\n")
    assert "| s1 | 1001 | 运行中 | 10s | " in md and "approve" not in md
    assert r.jobs()[0]["job_id"] == "1001"
    cluster.advance(3600)
    arb(idea, "poll")
    md = r._repr_markdown_()
    assert md.startswith("**r001** · 成功 · x\n\n用时 1h · 1.50 SU\n\n"), md
    assert "<span style=\"color:#2f855a\">成功</span>" in r._repr_html_()


def test_run_display_failure_reason_and_escaping(cluster, idea: Path, initialized: Path, arb,
                                                 monkeypatch) -> None:
    monkeypatch.chdir(idea)
    toml = initialized / "arboria.toml"
    toml.write_text(toml.read_text().replace('entity = ""', 'entity = "team"')
                    .replace('project = ""', 'project = "lm"'))
    cluster.script(jn(cluster, "r001", 1), run_s=7200,
                   log="step 1\nRuntimeError: CUDA out of memory. <b>x</b>\n")
    r = lab.run(reason="<script>alert(1)</script> big batch", gpus=4, time="2h")
    arb(idea, "approve", "r001")
    cluster.advance(120)
    arb(idea, "watch", "--once")
    html = r._repr_html_()
    assert "<script>" not in html and "&lt;script&gt;alert(1)&lt;/script&gt; big batch" in html
    assert "<b>x</b>" not in html and "失败</span>" in html
    line = html.split("</div>")[1]
    assert "s1 日志出现异常（匹配 CUDA out of memory）" in line and "wandb</a>" in line
    assert "href=\"https://wandb.ai/team/lm/runs/" in line

    # without a reason in the run_final event: the failed jobs' states
    cluster.script(jn(cluster, "r002", 1), run_s=60, state="timeout")
    r2 = lab.run(reason="y", gpus=1, time="1h")
    arb(idea, "approve", "r002")
    cluster.advance(120)
    arb(idea, "poll")
    assert r2._repr_markdown_().split("\n\n")[1].startswith("s1 超时"), r2._repr_markdown_()


class FakeWandbRun:
    def __init__(self, path):
        self.path = path

    def scan_history(self, keys):
        return [{"_step": i, **{k: i * 0.5 for k in keys}} for i in range(3)]


class FakeApi:
    paths: list = []

    def run(self, path):
        FakeApi.paths.append(path)
        return FakeWandbRun(path)


def test_metrics_and_compare(cluster, idea: Path, initialized: Path, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    r = lab.run(reason="x", gpus=1, time="1h")
    with pytest.raises(ArbError, match="no wandb link"):
        r.metrics("val/loss")
    toml = initialized / "arboria.toml"
    toml.write_text(toml.read_text().replace('entity = ""', 'entity = "team"')
                    .replace('project = ""', 'project = "lm"'))
    r2 = lab.run(reason="y", gpus=2, time="1h")
    with pytest.raises(ArbError, match="uv add wandb"):  # not installed in the test env
        r2.metrics(["val/loss"])
    FakeApi.paths = []
    monkeypatch.setattr(lab, "_wandb_api", lambda: FakeApi())
    rows = r2.metrics(["val/loss"])
    assert rows == [{"_step": 0, "val/loss": 0.0}, {"_step": 1, "val/loss": 0.5},
                    {"_step": 2, "val/loss": 1.0}]
    assert FakeApi.paths == [f"team/lm/{cluster.project}-r002"]
    r3 = lab.run(reason="z", gpus=3, time="1h")
    table = lab.compare(["r002", r3], "val/loss")
    assert [row["run"] for row in table] == ["r002"] * 3 + ["r003"] * 3


# ---------------------------------------------------------------- todos


def test_todos_from_notebook_to_claude(idea: Path, initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    t = lab.todo("把 eval 改成每 500 步一次")
    assert t["id"] == "t001" and t["status"] == "open" and t["idea"] == "exp1"
    lab.todo("加一个 cosine schedule")
    assert _subj(initialized)[0] == "arboria backup: todo_new t002"
    out = arb(idea, "todo", "list").stdout
    assert "t001" in out and "把 eval 改成每 500 步一次" in out and "t002" in out
    hook = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(hook)["hookSpecificOutput"]["additionalContext"]
    assert "来自 notebook 的待办：" in ctx and "- t001: 把 eval 改成每 500 步一次" in ctx
    assert ctx.index("来自 notebook 的待办") < ctx.index("Runs (")

    res = arb(idea, "todo", "done", "t001", "--result", "x" * 601, ok=False)
    assert "result is too long: 601 chars (max 600)" in res.stderr
    arb(idea, "todo", "done", "t001", "--result", "改好了，见 train.py:42")
    assert _subj(initialized)[0] == "arboria backup: todo_done t001"
    assert "already done" in arb(idea, "todo", "done", "t001", "--result", "x", ok=False).stderr
    arb(idea, "todo", "drop", "t002", "--reason", "不需要了")
    assert _subj(initialized)[0] == "arboria backup: todo_drop t002"
    assert arb(idea, "todo", "list").stdout.strip() == "no open todos"
    rows = json.loads(arb(idea, "todo", "list", "--all", "--json").stdout)
    assert [(r["id"], r["status"], r["result"]) for r in rows] == [
        ("t001", "done", "改好了，见 train.py:42"), ("t002", "dropped", "不需要了")]
    assert all(r["done_at"] for r in rows)
    assert "no todo 't404'" in arb(idea, "todo", "drop", "t404", ok=False).stderr
    hook = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    assert "来自 notebook 的待办" not in hook
    assert [r["id"] for r in lab.todos(include_closed=True)] == ["t001", "t002"]


def test_todo_and_todos_render_as_calm_lines(idea: Path, initialized: Path) -> None:
    lab.use("exp1", repo=initialized)
    t = lab.todo("把 <eval> 改成每 *500* 步一次")
    assert isinstance(t, lab.Todo) and not t.existed
    # still reads like the row
    assert t.id == t["id"] == "t001" and t.get("status") == "open" and dict(t) == t.data
    assert t.text == "把 <eval> 改成每 *500* 步一次" and t.result is None
    assert repr(t) == str(t) == "已交给 agent（t001）：把 <eval> 改成每 *500* 步一次"
    html = t._repr_html_()
    assert "已交给 agent（t001）：" in html and "&lt;eval&gt;" in html and "<eval>" not in html
    assert "{" not in html  # no JSON tree
    assert t._repr_markdown_() == "已交给 agent（t001）：把 &lt;eval&gt; 改成每 \\*500\\* 步一次"
    again = lab.todo("把 <eval> 改成每 *500* 步一次")
    assert again.existed and again.id == "t001"
    assert str(again) == "已在待办里（t001）：把 <eval> 改成每 *500* 步一次"
    assert "已在待办里（t001）：" in again._repr_html_()
    lab.todo("x" * 300)  # long text: one clipped line, the full text in the tooltip
    long = lab.todos()[1]
    assert str(long).endswith("…") and len(str(long)) < 200 and "x" * 300 in long._repr_html_()

    with Ledger(initialized / ".arboria") as l:
        tasks.done(l, "t001", "改好了 <train.py:42>")
        tasks.drop(l, "t002")
    rows = lab.todos(include_closed=True)
    assert isinstance(rows, lab.TodoList) and len(rows) == 2
    assert [r["id"] for r in rows] == ["t001", "t002"] and rows.data[0]["result"]
    assert str(rows[0]) == "已完成（t001）：把 <eval> 改成每 *500* 步一次 → 改好了 <train.py:42>"
    assert "已完成（t001）：" in rows[0]._repr_html_() and "&lt;train.py:42&gt;" in rows[0]._repr_html_()
    assert str(rows[1]).startswith("已放弃（t002）：")
    lines = str(rows).splitlines()
    assert lines[0] == "t001 · 已完成 · 把 <eval> 改成每 *500* 步一次 → 改好了 <train.py:42> · 刚刚"
    assert lines[1].startswith("t002 · 已放弃 · xxx") and lines[1].endswith(" · 刚刚")
    html = rows._repr_html_()
    assert "等 agent" not in html and "已完成" in html and "刚刚" in html
    assert "<eval>" not in html and "{" not in html
    md = rows._repr_markdown_().splitlines()
    assert md[0].startswith("- t001 · 已完成 · 把 &lt;eval&gt;") and len(md) == 2
    assert str(lab.todos()) == "没有未完成的待办" and not lab.todos()
    assert "没有未完成的待办" in lab.todos()._repr_html_()


def _events(main: Path) -> int:
    with Ledger(main / ".arboria") as l:
        return l.one("SELECT COUNT(*) AS n FROM events")["n"]


def test_rerun_cells_write_nothing_new(idea: Path, initialized: Path) -> None:
    """Cells re-run on every open: an unchanged conf / metric / todo adds no record."""
    lab.use("exp1", repo=initialized)
    lab.conf("prelim")
    lab.metric("val/acc @ 10k")
    first = lab.todo("把 eval 改成每 500 步一次")
    events, commits = _events(initialized), _subj(initialized)
    assert lab.conf("prelim")["confidence"] == "prelim"
    assert lab.metric("  val/acc @ 10k ")["metric"] == "val/acc @ 10k"
    again = lab.todo("把 eval 改成每 500 步一次 ")
    assert again["id"] == first["id"] == "t001" and again["status"] == "open"
    assert (_events(initialized), _subj(initialized)) == (events, commits)
    assert [t["id"] for t in lab.todos()] == ["t001"]
    # a changed value, or a todo whose twin is closed, is written as usual
    assert lab.todo("另一件事")["id"] == "t002"
    with Ledger(initialized / ".arboria") as l:
        tasks.done(l, "t001", "done")
    assert lab.todo("把 eval 改成每 500 步一次")["id"] == "t003"
    lab.conf("confirmed")
    assert _subj(initialized)[0] == "arboria backup: idea_confidence exp1"
    lab.metric("val/loss")
    assert _subj(initialized)[0] == "arboria backup: idea_metric exp1"
    with pytest.raises(ArbError, match="invalid confidence"):
        lab.conf("sure")  # still validated when it would be a no-op


def test_todo_overview_lists_open_ideas(idea: Path, initialized: Path, arb) -> None:
    lab.use("exp1", repo=initialized)
    lab.todo("first")
    wt2 = Path(arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m")
               .stdout.strip())
    lab.use("exp2", repo=initialized)
    lab.todo("second")
    out = arb(initialized, "todo", "list").stdout
    assert "t001" in out and "t002" in out
    assert "t001" not in arb(wt2, "todo", "list").stdout  # only this idea's in a worktree
    assert "- t002 [exp2]: second" in arb(initialized, "status", "--brief").stdout
    arb(initialized, "idea", "close", "exp2", "--drop")
    assert "t002" not in arb(initialized, "todo", "list").stdout
    with pytest.raises(ArbError, match="is dropped"):
        lab.todo("too late")


@pytest.mark.parametrize("call", [
    lambda: lab.todo("x"), lambda: lab.conf("prelim"), lambda: lab.metric("acc"),
    lambda: lab.prune("dead end"),
])
def test_human_records_refused_inside_claude_code(idea: Path, initialized: Path, monkeypatch,
                                                  call) -> None:
    lab.use("exp1", repo=initialized)
    monkeypatch.setenv("CLAUDECODE", "1")
    with pytest.raises(ArbError, match="只由人在 notebook 里做"):
        call()
    monkeypatch.delenv("CLAUDECODE")
    assert idea.exists()


# ---------------------------------------------------------------- the human's records


def test_conf_metric_prune(idea: Path, initialized: Path, arb) -> None:
    lab.use("exp1", repo=initialized)
    assert lab.conf("prelim")["confidence"] == "prelim"
    assert _subj(initialized)[0] == "arboria backup: idea_confidence exp1"
    with pytest.raises(ArbError, match="invalid confidence"):
        lab.conf("sure")
    assert lab.metric("val/acc @ 10k")["metric"] == "val/acc @ 10k"
    assert _subj(initialized)[0] == "arboria backup: idea_metric exp1"
    with pytest.raises(ArbError, match="metric is too long"):
        lab.metric("m" * 201)
    with pytest.raises(ArbError, match="note is too long"):
        lab.prune("r" * 1001)
    assert idea.exists()
    closed = lab.prune("loss 不降，放弃")
    assert closed["status"] == "dropped" and not idea.exists()
    assert _nb(initialized).is_file()  # the notebook outlives the idea
    with Ledger(initialized / ".arboria") as l:
        assert l.all("SELECT text, author FROM notes") == [
            {"text": "prune: loss 不降，放弃", "author": "human"}]
    st = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert (st["metric"], st["confidence"]) == ("val/acc @ 10k", "prelim")


def test_fork_from_run_snapshot(cluster, idea: Path, initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    (idea / "wip.py").write_text("uncommitted\n")
    r = lab.run(reason="x", gpus=1, time="1h")
    snap = _run(initialized)["snapshot_commit"]
    _bump(idea)
    new = lab.fork("exp1-b", from_=r, hypothesis="同上但 lr 更小", metric="val/loss")
    wt = Path(new["worktree_path"])
    assert git(wt, "rev-parse", "HEAD") == snap and (wt / "wip.py").read_text() == "uncommitted\n"
    assert not (wt / "bump0.txt").exists()
    assert git(wt, "symbolic-ref", "--short", "HEAD") == "idea/exp1-b"
    text = _nb(initialized, "exp1-b").read_text()
    assert f"**父 idea：** exp1（从 r001 的快照 `{snap[:10]}` 分出）" in text
    assert (wt / "notebook.py").resolve() == _nb(initialized, "exp1-b").resolve()
    rows = {x["name"]: x for x in json.loads(arb(initialized, "idea", "list", "--json").stdout)}
    assert rows["exp1-b"]["parent_name"] == "exp1"
    # without from_: branches off the idea's branch, like `arb idea new --parent`
    monkeypatch.chdir(idea)  # (the arb helper above changed directory)
    plain = lab.fork("exp1-c", hypothesis="h", metric="m")
    assert git(Path(plain["worktree_path"]), "rev-parse", "HEAD") == git(idea, "rev-parse", "HEAD")
    with pytest.raises(ArbError, match="already exists"):
        lab.fork("exp1-c", hypothesis="h", metric="m")
