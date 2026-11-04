"""lab.inspect: the agent reads code for the human; arb extracts the code from git."""

from __future__ import annotations

import json
import sqlite3
import subprocess
from pathlib import Path

import pytest

from arboria import commands, db, inspection, lab, tasks
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git, snapshot_files
from .test_seed_home import _in_cell, home  # noqa: F401  (fixture)
from .test_sessions import FakeInbox, _hook

TRAIN = "".join(
    [
        "import torch\n",
        "\n",
        "def train(model, data, *, steps=100):\n",
        "    opt = torch.optim.AdamW(model.parameters(), lr=3e-4)\n",
        "    for step, batch in zip(range(steps), data):\n",
        "        loss = model(batch).mean()  # <script>alert('x')</script> & co\n",
        "        loss.backward()\n",
        "        opt.step()\n",
        "        opt.zero_grad()\n",
        "\tif step % 10 == 0: print(f\"{step=} {loss=}\")\n",
        "    return model\n",
    ]
    + [f"# filler {i}\n" for i in range(12, 501)]
)


@pytest.fixture(autouse=True)
def unpinned():
    lab.use(None)
    yield
    lab.use(None)


@pytest.fixture
def code(idea: Path, initialized: Path) -> Path:
    """exp1's worktree with an uncommitted train.py; lab pinned to exp1."""
    (idea / "train.py").write_text(TRAIN)
    lab.use("exp1", repo=initialized)
    return idea


def _tasks(main: Path) -> list[dict]:
    with Ledger(main / ".arboria") as ledger:
        return ledger.all("SELECT * FROM tasks ORDER BY id")


def _deliver(main: Path, task_id: str, answer: dict) -> dict:
    with Ledger(main / ".arboria") as ledger:
        return inspection.deliver(ledger, main, task_id, answer)


def _check(main: Path, task_id: str, answer: dict) -> dict:
    with Ledger(main / ".arboria") as ledger:
        task = tasks.get_task(ledger, task_id)
    return inspection.validate_result(main, task, answer)


SPAN = {"id": "loop", "path": "train.py", "start": 3, "end": 11,
        "note": "训练循环：每步前向、反向、更新", "line_notes": {"6": "loss 取平均", "10": "每 10 步打日志"}}
ANSWER = {"title": "训练循环", "summary": "AdamW，固定步数。", "spans": [SPAN]}


# ---------------------------------------------------------------- asking


def test_first_call_is_a_todo_and_rerunning_reuses_it(code: Path, initialized: Path) -> None:
    before_files = snapshot_files(code)
    before = (git(code, "rev-parse", "HEAD"), git(code, "status", "--porcelain"),
              git(code, "branch", "--show-current"))
    x = lab.inspect("DMD 的训练循环")
    assert (x.status, x.task_id, x.result) == ("pending", "t001", None)
    assert "已交给 agent（t001）：代码摘录 · DMD 的训练循环" in str(x)
    (t,) = _tasks(initialized)
    assert t["kind"] == "inspect" and t["status"] == "open"
    p = json.loads(t["payload_json"])
    assert p["question"] == "DMD 的训练循环" and p["view"] == "code" and p["target"] == "idea"
    # the snapshot includes the uncommitted train.py and is anchored by a hidden ref
    assert git(initialized, "show", f"{p['code_ref']}:train.py") + "\n" == TRAIN
    assert git(initialized, "rev-parse", f"{inspection.ref_name('t001')}") == p["code_ref"]
    assert git(initialized, "rev-parse", f"{p['code_ref']}^{{tree}}") == p["tree"]
    assert "arb inspect guide" in t["text"]
    # re-running the cell: same todo, nothing new
    again = lab.inspect("DMD   的训练循环 ")
    assert (again.status, again.task_id) == ("pending", "t001") and len(_tasks(initialized)) == 1
    assert "等 agent（t001）" in str(again)
    # another view is another question
    assert lab.inspect("DMD 的训练循环", view="map").task_id == "t002"
    # the user's branch, index and files were not touched
    assert snapshot_files(code) == before_files
    assert (git(code, "rev-parse", "HEAD"), git(code, "status", "--porcelain"),
            git(code, "branch", "--show-current")) == before


def test_bad_questions_and_views(code: Path) -> None:
    with pytest.raises(ArbError, match="view"):
        lab.inspect("x", view="diagram")
    with pytest.raises(ArbError, match="question"):
        lab.inspect("   ")
    with pytest.raises(ArbError, match="too long"):
        lab.inspect("问" * 301)
    with pytest.raises(ArbError):
        lab.inspect(42)  # type: ignore[arg-type]


def test_inspect_is_a_builtin_and_a_reserved_name(code: Path) -> None:
    rows = {r["name"]: r for r in lab.commands()}
    assert rows["inspect"]["kind"] == "builtin"
    assert rows["inspect"]["signature"] == ("inspect(question, *, view='code', run=None, "
                                            "refresh=False)")
    with pytest.raises(ArbError, match="already part of lab"):
        commands.validate_name("inspect")


def test_seed_notebook_points_to_planting(home, tmp_path: Path, monkeypatch) -> None:  # noqa: F811
    monkeypatch.chdir(tmp_path)
    with pytest.raises(ArbError) as exc:
        _in_cell(home.notebook("rank-schedule"), 'lab.inspect("训练循环")')
    assert "种子还没有仓库" in str(exc.value) and "arb idea plant rank-schedule" in str(exc.value)
    rows = {r["name"] for r in _in_cell(home.notebook("rank-schedule"),
                                        "rows = lab.commands()")["rows"]}
    assert "inspect" not in rows


def test_the_agent_cannot_ask_but_can_read(code: Path, initialized: Path, monkeypatch) -> None:
    monkeypatch.setenv("CLAUDECODE", "1")
    with pytest.raises(ArbError, match="只由人在 notebook 里做"):
        lab.inspect("训练循环")
    assert _tasks(initialized) == []
    monkeypatch.delenv("CLAUDECODE")
    lab.inspect("训练循环")
    _deliver(initialized, "t001", ANSWER)
    monkeypatch.setenv("CLAUDECODE", "1")
    assert lab.inspect("训练循环").status == "fresh"


# ---------------------------------------------------------------- the code shown is the repo's


def test_extracted_lines_are_exactly_the_blob(code: Path, initialized: Path) -> None:
    (code / "crlf.py").write_bytes(b"a = 1\r\nb = '\xe4\xb8\xad'\r\n\r\nc = 3")  # no final newline
    lab.inspect("训练循环")
    commit = json.loads(_tasks(initialized)[0]["payload_json"])["code_ref"]
    answer = {"title": "t", "spans": [SPAN, {"path": "crlf.py", "start": 1, "end": 4}]}
    stored = _deliver(initialized, "t001", answer)
    for s in stored["spans"]:
        blob = subprocess.run(["git", "cat-file", "blob", f"{commit}:{s['path']}"],
                              cwd=initialized, capture_output=True).stdout.decode()
        assert s["lines"] == blob.split("\n")[s["start"] - 1:s["end"]]
    assert stored["spans"][1]["lines"] == ["a = 1\r", "b = '中'\r", "\r", "c = 3"]
    assert stored["spans"][0]["lines"][3] == TRAIN.splitlines()[5]
    # what is stored is what is shown, and the agent's notes are separate rows
    x = lab.inspect("训练循环")
    assert x.status == "fresh" and x.result == stored
    html = x._repr_html_()
    assert "&lt;script&gt;alert(&#x27;x&#x27;)&lt;/script&gt; &amp; co" in html
    assert "<script>" not in html
    assert html.count('class="arb-inspect-note"') == 2
    assert "train.py:3–11" in html and f"@ {commit[:7]}" in html
    text = str(x)
    assert "10 │ \tif step % 10 == 0" in text and "   ↳ agent：每 10 步打日志" in text


def test_editing_the_worktree_later_does_not_change_the_answer(code: Path,
                                                              initialized: Path) -> None:
    lab.inspect("训练循环")
    (code / "train.py").write_text("nothing here\n")  # before the agent delivers
    stored = _deliver(initialized, "t001", ANSWER)
    assert stored["spans"][0]["lines"][0] == "def train(model, data, *, steps=100):"


# ---------------------------------------------------------------- validation


@pytest.mark.parametrize("change, message", [
    ({"spans": [{**SPAN, "path": "../etc/passwd"}]}, "plain repo-relative path"),
    ({"spans": [{**SPAN, "path": "/etc/passwd"}]}, "plain repo-relative path"),
    ({"spans": [{**SPAN, "path": "nope.py"}]}, "does not exist"),
    ({"spans": [{**SPAN, "path": ".arboria"}]}, "does not exist"),
    ({"spans": [{**SPAN, "start": 0}]}, "outside the file"),
    ({"spans": [{**SPAN, "end": 9999}]}, "outside the file"),
    ({"spans": [{**SPAN, "start": 9, "end": 3}]}, "outside the file"),
    ({"spans": [{**SPAN, "start": "3"}]}, "integer"),
    ({"spans": [{**SPAN, "line_notes": {"2": "x"}}]}, "not a line in"),
    ({"spans": [{**SPAN, "line_notes": {"6": "长" * 201}}]}, "line_note is too long"),
    ({"spans": [{**SPAN, "note": "长" * 601}]}, "span_note is too long"),
    ({"spans": [{"path": "train.py", "start": 1, "end": 401}]}, "more than 400 lines"),
    ({"spans": [{"path": "train.py", "start": i, "end": i} for i in range(1, 8)]}, "max 6"),
    ({"spans": [SPAN, SPAN]}, "unique"),
    ({"spans": []}, "at least one span"),
    ({"spans": [{**SPAN, "code": "x"}]}, "unknown keys"),
    ({"text": "pseudo"}, "unknown keys for view=code"),
    ({"title": ""}, "title must not be empty"),
    ({"title": "长" * 81}, "too long"),
    ({"summary": "长" * 601}, "too long"),
])
def test_bad_answers_are_refused(code: Path, initialized: Path, change: dict,
                                 message: str) -> None:
    lab.inspect("训练循环")
    with pytest.raises(ArbError, match=message):
        _check(initialized, "t001", {**ANSWER, **change})
    with pytest.raises(ArbError, match=message):
        _deliver(initialized, "t001", {**ANSWER, **change})
    assert _tasks(initialized)[0]["status"] == "open"


def test_line_note_budget(code: Path, initialized: Path) -> None:
    lab.inspect("训练循环")
    spans = [{"id": f"s{k}", "path": "train.py", "start": 1 + 50 * k, "end": 50 + 50 * k,
              "line_notes": {str(1 + 50 * k + i): "x" for i in range(9)}} for k in range(5)]
    with pytest.raises(ArbError, match="more than 40 line notes"):
        _check(initialized, "t001", {"title": "t", "spans": spans})


MAP = "flowchart TD\n  data[数据] --> loop[训练循环]\n  loop --> ckpt[(checkpoint)]\n"


@pytest.mark.parametrize("mermaid, message", [
    ("hello --> world", "must start with a diagram type"),
    ("%%{init: {'securityLevel': 'loose'}}%%\nflowchart TD\n a-->b", "init"),
    ("flowchart TD\n a-->b\n click a callback", "click"),
    ("flowchart TD\n a[<img src=x onerror=alert(1)>]-->b", "no HTML"),
    ("flowchart TD\n a[\"<a href='x'>y</a>\"]-->b", "links"),
    ("flowchart TD\n a-->b\x07", "control characters"),
    ("flowchart TD\n" + "a-->b\n" * 2000, "too long"),
])
def test_bad_mermaid_is_refused(code: Path, initialized: Path, mermaid: str,
                                message: str) -> None:
    lab.inspect("整个仓库", view="map")
    with pytest.raises(ArbError, match=message):
        _check(initialized, "t001", {"title": "架构", "mermaid": mermaid})


def test_map_nodes_must_point_at_spans(code: Path, initialized: Path) -> None:
    lab.inspect("整个仓库", view="map")
    ok = {"title": "架构", "mermaid": MAP, "spans": [SPAN], "nodes": {"loop": ["loop"]}}
    assert _check(initialized, "t001", ok)["nodes"] == {"loop": ["loop"]}
    assert _check(initialized, "t001", {**ok, "mermaid": "flowchart TD\n a-->b<br/>c",
                                        "nodes": {}})
    with pytest.raises(ArbError, match="span ids"):
        _check(initialized, "t001", {**ok, "nodes": {"loop": ["nope"]}})
    with pytest.raises(ArbError, match="does not appear"):
        _check(initialized, "t001", {**ok, "nodes": {"opt": ["loop"]}})
    with pytest.raises(ArbError, match="mermaid is required"):
        _check(initialized, "t001", {"title": "架构"})


def test_pseudo_and_simple_need_text(code: Path, initialized: Path) -> None:
    lab.inspect("训练循环", view="pseudo")
    lab.inspect("训练循环", view="simple")
    with pytest.raises(ArbError, match="inspect_text is required"):
        _check(initialized, "t001", {"title": "t"})
    with pytest.raises(ArbError, match="unknown keys for view=pseudo: lang"):
        _check(initialized, "t001", {"title": "t", "text": "x", "lang": "python"})
    with pytest.raises(ArbError, match="inspect_text is too long"):
        _check(initialized, "t001", {"title": "t", "text": "x" * 8001})
    out = _check(initialized, "t002", {"title": "t", "text": "for b in data: step(b)",
                                       "lang": "python", "spans": [SPAN]})
    assert out["lang"] == "python" and out["spans"][0]["lines"]
    with pytest.raises(ArbError, match="lang"):
        _check(initialized, "t002", {"title": "t", "text": "x", "lang": "<b>"})


def test_other_todos_cannot_be_delivered_as_answers(code: Path, initialized: Path) -> None:
    lab.todo("plain")
    with pytest.raises(ArbError, match="not a 读代码 todo"):
        _deliver(initialized, "t001", ANSWER)


# ---------------------------------------------------------------- the CLI


def test_cli_source_check_done(code: Path, initialized: Path, arb) -> None:
    lab.inspect("训练循环")
    files = arb(code, "inspect", "source", "t001").stdout.split()
    assert "train.py" in files and "README.md" in files and "notebook.py" not in files
    src = arb(code, "inspect", "source", "t001", "train.py").stdout.splitlines()
    assert src[0].startswith("# train.py @ ") and src[6] == "  6          loss = model(batch).mean()" \
        "  # <script>alert('x')</script> & co"
    assert arb(code, "inspect", "source", "t001", "nope.py", ok=False).exit_code == 1
    res = arb(code, "inspect", "check", "t001", "--file", "-", input=json.dumps(ANSWER))
    assert "[loop] train.py:3–11" in res.stdout and "nothing saved" in res.stdout
    assert _tasks(initialized)[0]["status"] == "open"
    bad = arb(code, "inspect", "check", "t001", "--file", "-", input="{not json", ok=False)
    assert bad.exit_code == 1 and "not valid JSON" in bad.stderr
    answer = code.parent / "answer.json"
    answer.write_text(json.dumps(ANSWER, ensure_ascii=False))
    out = arb(code, "inspect", "done", "t001", "--file", str(answer)).stdout
    assert out.startswith("done t001: 训练循环 (1 spans, 9 lines @ ")
    (t,) = _tasks(initialized)
    assert t["status"] == "done" and t["result"] == "训练循环：AdamW，固定步数。"
    again = arb(code, "inspect", "done", "t001", "--file", str(answer), ok=False)
    assert "already done" in again.stderr
    # an answer is a backup event
    log = git(initialized / ".arboria", "log", "--format=%s", "-1")
    assert "inspect_done t001" in log
    assert "inspect" in arb(code, "inspect", "guide").stdout


def test_todo_done_refuses_inspect_todos_and_drop_is_shown(code: Path, initialized: Path,
                                                           arb) -> None:
    lab.inspect("训练循环")
    res = arb(code, "todo", "done", "t001", "--result", "看过了", ok=False)
    assert res.exit_code == 1 and "arb inspect done t001" in res.stderr
    arb(code, "todo", "drop", "t001", "--reason", "问题太大，请拆开问")
    x = lab.inspect("训练循环")
    assert (x.status, x.task_id) == ("dropped", "t001") and len(_tasks(initialized)) == 1
    assert "agent 没有做（t001：问题太大，请拆开问）" in str(x)
    y = x.refresh()
    assert (y.status, y.task_id) == ("pending", "t002")


# ---------------------------------------------------------------- staleness and refresh


def test_stale_answers_are_marked_and_refresh_asks_again(code: Path, initialized: Path) -> None:
    lab.inspect("训练循环")
    first = _deliver(initialized, "t001", ANSWER)
    x = lab.inspect("训练循环")
    assert x.status == "fresh" and x.refresh().status == "fresh"  # nothing changed: no-op
    assert len(_tasks(initialized)) == 1
    (code / "train.py").write_text(TRAIN.replace("lr=3e-4", "lr=1e-3"))
    stale = lab.inspect("训练循环")
    assert stale.status == "stale" and stale.result == first  # never regenerated by itself
    assert f"代码已变（答案基于 {first['code_ref'][:7]}" in str(stale)
    assert "代码已变" in stale._repr_html_() and len(_tasks(initialized)) == 1
    pending = stale.refresh()
    assert (pending.status, pending.task_id) == ("pending", "t002")
    assert pending.result == first and "下面是上一版" in str(pending)
    assert lab.inspect("训练循环", refresh=True).task_id == "t002"  # deduplicated
    new_ref = json.loads(_tasks(initialized)[1]["payload_json"])["code_ref"]
    assert new_ref != first["code_ref"]
    second = _deliver(initialized, "t002", ANSWER)
    assert "lr=1e-3" in second["spans"][0]["lines"][1]
    now = lab.inspect("训练循环")
    assert now.status == "fresh" and now.result == second
    # back to the old code: the older answer would match, but the newest answer wins
    (code / "train.py").write_text(TRAIN)
    assert lab.inspect("训练循环").status == "stale"


def test_a_run_snapshot_never_goes_stale(code: Path, initialized: Path, arb) -> None:
    rid = arb(code, "run", "new", "--reason", "baseline").stdout.strip()
    x = lab.inspect("训练循环", run=rid)
    with Ledger(initialized / ".arboria") as ledger:
        run = ledger.one("SELECT * FROM runs WHERE id = ?", (rid,))
    p = json.loads(_tasks(initialized)[0]["payload_json"])
    assert (p["target"], p["code_ref"], p["tree"]) == (rid, run["snapshot_commit"],
                                                      run["tree_hash"])
    assert not git(initialized, "for-each-ref", "refs/arboria/inspect/")  # the run's ref holds it
    (code / "train.py").write_text("changed\n")
    _deliver(initialized, x.task_id, ANSWER)
    y = lab.inspect("训练循环", run=rid)
    assert y.status == "fresh" and "def train" in y.result["spans"][0]["lines"][0]
    assert "run r001" in y._repr_html_()
    # the same question about the idea's current code is a different question
    assert lab.inspect("训练循环").status == "pending"
    with pytest.raises(ArbError, match="no run"):
        lab.inspect("训练循环", run="r099")


# ---------------------------------------------------------------- rendering


def test_everything_the_agent_writes_is_escaped(code: Path, initialized: Path) -> None:
    evil = "<script>alert(1)</script>"
    for view in ("code", "pseudo", "map"):
        lab.inspect("q", view=view)
    span = {**SPAN, "note": evil, "line_notes": {"4": evil}}
    _deliver(initialized, "t001", {"title": evil, "summary": evil, "spans": [span]})
    _deliver(initialized, "t002", {"title": "p", "text": f"x = 1  # {evil}", "spans": [span]})
    _deliver(initialized, "t003", {"title": "m", "summary": evil, "mermaid": MAP,
                                   "spans": [span], "nodes": {"loop": ["loop"]}})
    for view in ("code", "pseudo", "map"):
        html = lab.inspect("q", view=view)._repr_html_()
        assert "<script>" not in html and "&lt;script&gt;" in html, view
    pseudo = lab.inspect("q", view="pseudo")._repr_html_()
    assert "agent 写的伪代码" in pseudo and "<details" in pseudo  # source spans to expand
    m = lab.inspect("q", view="map")
    assert "flowchart TD" in m._repr_html_() and "图中 loop" in m._repr_html_()


def test_map_is_drawn_with_marimo_mermaid(code: Path, initialized: Path) -> None:
    pytest.importorskip("marimo")
    lab.inspect("整个仓库", view="map")
    _deliver(initialized, "t001", {"title": "架构", "mermaid": MAP, "spans": [SPAN],
                                   "nodes": {"loop": ["loop"]}})
    out = lab.inspect("整个仓库", view="map")._display_()
    text = out.text
    assert "marimo-mermaid" in text and "图是 agent 画的" in text
    assert "flowchart TD" in text  # the diagram source goes to the mermaid element


# ---------------------------------------------------------------- push, dashboard, status


def test_new_inspect_todo_is_pushed_to_the_session(code: Path, initialized: Path, arb,
                                                   monkeypatch) -> None:
    box = FakeInbox()
    try:
        _hook(arb, code, monkeypatch, box.path)
        x = lab.inspect("DMD 的训练循环 <b>")
        assert x.pushed and not lab.inspect("DMD 的训练循环 <b>").pushed  # once per todo
        y = lab.inspect("整个仓库", view="map")
        assert y.pushed
        first, second = (m["message"]["content"] for m in box.messages(2))
    finally:
        box.close()
    assert first.startswith("[arboria] notebook 里有新的读代码待办 t001：代码摘录（idea exp1）。")
    assert "arb inspect guide" in first and "arb inspect done t001 --file -" in first
    assert "DMD" not in first and "<b>" not in first  # never the human's free text
    assert "t002：架构图" in second
    status = arb(code, "status", "--brief").stdout
    assert "读代码待办：DMD 的训练循环 <b>（t001，代码摘录）、整个仓库（t002，架构图）" in status
    assert "arb inspect guide" in status


def test_dashboard_labels_inspect_todos() -> None:
    from arboria.ui.views import todo_kind

    assert todo_kind({"kind": "inspect", "payload_json": '{"view": "map"}'}) == "读代码 · 架构图"


# ---------------------------------------------------------------- schema


def test_migration_v6_to_v7_keeps_todos(tmp_path: Path) -> None:
    state = tmp_path / "state"
    state.mkdir()
    conn = db.connect(state / "ledger.db")
    for script in db.MIGRATIONS[:6]:
        for stmt in db._split_sql(script):
            conn.execute(stmt)
    conn.execute("PRAGMA user_version = 6")
    conn.execute("INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
                 " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')")
    conn.execute("INSERT INTO seeds (name, created_at, updated_at) VALUES ('s', 't', 't')")
    conn.execute("INSERT INTO tasks (id, idea_id, text, created_at, kind, payload_json)"
                 " VALUES ('t001', 1, 'x', 't', 'command_new', '{\"name\": \"svd\"}')")
    conn.execute("INSERT INTO tasks (id, seed_id, text, status, created_at, done_at, result)"
                 " VALUES ('t002', 1, 'y', 'done', 't', 't', 'ok')")
    with pytest.raises(sqlite3.IntegrityError):
        conn.execute("INSERT INTO tasks (id, idea_id, text, created_at, kind)"
                     " VALUES ('t003', 1, 'z', 't', 'inspect')")
    conn.close()
    with Ledger(state) as ledger:
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION == 7
        rows = ledger.all("SELECT id, idea_id, seed_id, kind, payload_json, result FROM tasks")
        assert [tuple(r.values()) for r in rows] == [
            ("t001", 1, None, "command_new", '{"name": "svd"}', None),
            ("t002", None, 1, "plain", None, "ok")]
        with ledger.write() as c:
            c.execute("INSERT INTO tasks (id, idea_id, text, created_at, kind)"
                      " VALUES ('t003', 1, 'z', 't', 'inspect')")
            c.execute("INSERT INTO inspections (task_id, idea_id, question, view, target, code_ref,"
                      " tree, result_json, created_at) VALUES ('t003', 1, 'q', 'map', 'idea',"
                      " 'c', 't', '{}', 't')")
            with pytest.raises(sqlite3.IntegrityError):
                c.execute("INSERT INTO inspections (task_id, idea_id, question, view, target,"
                          " code_ref, tree, result_json, created_at) VALUES ('t001', 1, 'q',"
                          " 'film', 'idea', 'c', 't', '{}', 't')")
        assert ledger.conn.execute("PRAGMA foreign_key_check").fetchall() == []
