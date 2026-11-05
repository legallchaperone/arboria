"""Seeds (种子): repo-less ideas in the global home, and planting one into a repo."""

from __future__ import annotations

import ast
import json
import os
import sqlite3
import time
from pathlib import Path

import pytest

from arboria import db, notebooks, seeds
from arboria.db import Ledger
from arboria.paths import Home, seed_of_notebook

from .conftest import git

THINKING = '''

@app.cell(hide_code=True)
def _(mo):
    mo.md(r"""
    ## 为什么会有用

    低秩更新的**秩**也许应该随层变化。
    """)
    return


@app.cell
def _():
    import numpy as np

    x = np.linspace(0, 1, 5)
    return


@app.cell
def _(mo):
    mo.md(f"动态的 {1 + 1}")
    return


@app.cell
def _(mo):
    mo.md("""
    - 先在小模型上看奇异值谱
    """)
    return


if __name__ == "__main__":'''


def _home(isolated_home: Path) -> Home:
    return Home(isolated_home)


def _subjects(state: Path) -> list[str]:
    return git(state, "log", "--format=%s").splitlines()


def _events(state: Path, kind: str) -> list[dict]:
    with Ledger(state) as ledger:
        rows = ledger.all("SELECT subject, payload_json FROM events WHERE kind = ? ORDER BY id",
                          (kind,))
    return [{"subject": r["subject"], **json.loads(r["payload_json"])} for r in rows]


# ---------------------------------------------------------------- home + CRUD


def test_seed_new_initializes_the_home_from_anywhere(arb, tmp_path: Path, repo: Path,
                                                     isolated_home: Path) -> None:
    h = _home(isolated_home)
    assert not isolated_home.exists()
    res = arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
              "--metric", "同参数量下的 val/loss")
    nb = h.notebook("rank-schedule")
    assert res.stdout.strip() == str(nb)  # stdout: just the notebook path
    assert f"种子园建在 {isolated_home}" in res.stderr and f"marimo edit {nb}" in res.stderr
    assert h.initialized and nb.is_file() and not nb.is_symlink()
    assert (h.state_dir / ".git").is_dir() and not (isolated_home / ".git").exists()
    text = nb.read_text()
    ast.parse(text)
    for needle in ("# rank-schedule", "LoRA 的秩随层变化", "**指标：** 同参数量下的 val/loss",
                   "import marimo as mo", "@app.cell(hide_code=True)"):
        assert needle in text, needle
    assert "lab" not in text and "**假设：**" not in text
    # the second command (from inside a repo, then from inside the home) says nothing about it
    res = arb(repo, "seed", "new", "sparse-attn")
    assert "种子园建在" not in res.stderr and h.notebook("sparse-attn").is_file()
    arb(h.seeds_dir, "seed", "new", "curriculum", "--hypothesis", "先易后难收敛更快")
    assert not (repo / ".arboria").exists()  # a repo cwd is not touched
    names = [r["name"] for r in json.loads(arb(tmp_path, "seed", "list", "--json").stdout)]
    assert sorted(names) == ["curriculum", "rank-schedule", "sparse-attn"]
    # every seed event is a backup commit, which versions the notebooks too
    assert _subjects(h.state_dir)[0] == "arboria backup: seed_new curriculum"
    files = git(h.state_dir, "ls-files").splitlines()
    assert {"backup/ledger.sql", "seeds/curriculum.py", "seeds/rank-schedule.py"} <= set(files)


def test_seed_validation(arb, tmp_path: Path) -> None:
    for args, msg in ((["Bad_Name"], "invalid seed name"),
                      (["ok", "x" * 301], "gist is too long"),
                      (["ok", "--hypothesis", "x" * 301], "hypothesis is too long"),
                      (["ok", "--metric", "x" * 201], "metric is too long")):
        assert msg in arb(tmp_path, "seed", "new", *args, ok=False).stderr
    arb(tmp_path, "seed", "new", "ok", "x" * 300)
    assert "already exists" in arb(tmp_path, "seed", "new", "ok", ok=False).stderr
    assert "no seed named 'nope'" in arb(tmp_path, "seed", "show", "nope", ok=False).stderr
    assert "nothing to change" in arb(tmp_path, "seed", "set", "ok", ok=False).stderr
    assert "metric is too long" in arb(tmp_path, "seed", "set", "ok", "--metric", "m" * 201,
                                       ok=False).stderr


def test_existing_notebook_is_kept(arb, tmp_path: Path, isolated_home: Path) -> None:
    h = _home(isolated_home)
    seeds.ensure_home(h=h)
    h.notebook("mine").write_text("# my own file\n")
    res = arb(tmp_path, "seed", "new", "mine", "想法")
    assert "already existed; left as is" in res.stderr
    assert h.notebook("mine").read_text() == "# my own file\n"


def test_list_show_set_drop(arb, tmp_path: Path, isolated_home: Path, monkeypatch) -> None:
    h = _home(isolated_home)
    arb(tmp_path, "seed", "new", "aaa", "第一个")
    arb(tmp_path, "seed", "new", "bbb", "第二个")
    arb(tmp_path, "seed", "new", "ccc", "第三个")
    # last edited: the notebook file counts too
    future = time.time() + 3600
    os.utime(h.notebook("aaa"), (future, future))
    rows = json.loads(arb(tmp_path, "seed", "list", "--json").stdout)
    assert [r["name"] for r in rows][0] == "aaa"
    assert rows[0]["last_edited"] > rows[0]["updated_at"]
    assert rows[0]["status"] == "thinking" and rows[0]["planted_to"] == []
    out = arb(tmp_path, "seed", "list").stdout.splitlines()
    assert out[0].startswith("aaa") and "思考中" in out[0] and out[0].endswith("第一个")

    arb(tmp_path, "seed", "set", "bbb", "--hypothesis", "秩随深度递减更好", "--metric", "val/loss")
    shown = arb(tmp_path, "seed", "show", "bbb").stdout
    assert "假设：秩随深度递减更好" in shown and "指标：val/loss" in shown
    assert f"notebook：{h.notebook('bbb')}" in shown
    arb(tmp_path, "seed", "set", "bbb", "--gist", "")  # "" clears
    assert json.loads(arb(tmp_path, "seed", "show", "bbb", "--json").stdout)["gist"] is None
    n_commits = len(_subjects(h.state_dir))
    arb(tmp_path, "seed", "set", "bbb", "--metric", "val/loss")  # unchanged: no-op
    assert len(_subjects(h.state_dir)) == n_commits

    # drop: human only (refused inside Claude Code, like arb note)
    monkeypatch.setenv("CLAUDECODE", "1")
    res = arb(tmp_path, "seed", "drop", "ccc", ok=False)
    assert "只由人" in res.stderr
    monkeypatch.delenv("CLAUDECODE")
    arb(tmp_path, "seed", "drop", "ccc")
    assert h.notebook("ccc").is_file()
    assert "ccc" not in arb(tmp_path, "seed", "list").stdout
    rows = json.loads(arb(tmp_path, "seed", "list", "--all", "--json").stdout)
    assert {r["name"]: r["status"] for r in rows}["ccc"] == "dropped"
    assert "already dropped" in arb(tmp_path, "seed", "drop", "ccc", ok=False).stderr
    assert _subjects(h.state_dir)[0] == "arboria backup: seed_drop ccc"

    # revive: human only too; only a dropped seed, back to thinking
    monkeypatch.setenv("CLAUDECODE", "1")
    assert "只由人" in arb(tmp_path, "seed", "revive", "ccc", ok=False).stderr
    monkeypatch.delenv("CLAUDECODE")
    res = arb(tmp_path, "seed", "revive", "aaa", ok=False)
    assert "is not dropped (it is thinking · 思考中)" in res.stderr
    assert "no seed named 'nope'" in arb(tmp_path, "seed", "revive", "nope", ok=False).stderr
    assert "revived ccc" in arb(tmp_path, "seed", "revive", "ccc").stdout
    assert json.loads(arb(tmp_path, "seed", "show", "ccc", "--json").stdout)["status"] == "thinking"
    assert "ccc" in arb(tmp_path, "seed", "list").stdout
    assert _subjects(h.state_dir)[0] == "arboria backup: seed_revive ccc"
    assert [e["subject"] for e in _events(h.state_dir, "seed_revive")] == ["ccc"]
    assert "is not dropped" in arb(tmp_path, "seed", "revive", "ccc", ok=False).stderr


def test_seed_of_notebook(arb, tmp_path: Path, isolated_home: Path, initialized: Path) -> None:
    h = _home(isolated_home)
    arb(tmp_path, "seed", "new", "aaa")
    found = seed_of_notebook(h.notebook("aaa"))
    assert found is not None and found[0].root == h.root and found[1] == "aaa"
    assert seed_of_notebook(h.notebook("nope")) is None
    # a project's own seeds/ folder is not a home (its .arboria/ has no seeds/ mirror)
    (initialized / "seeds").mkdir()
    (initialized / "seeds" / "x.py").write_text("")
    assert seed_of_notebook(initialized / "seeds" / "x.py") is None


def test_backup_mirrors_seed_notebooks(arb, tmp_path: Path, isolated_home: Path) -> None:
    h = _home(isolated_home)
    arb(tmp_path, "seed", "new", "aaa")
    nb = h.notebook("aaa")
    nb.write_text(nb.read_text() + "\n# 想到一点\n")
    (h.seeds_dir / "__marimo__").mkdir()
    (h.seeds_dir / "__marimo__" / "session.json").write_text("{}")
    arb(tmp_path, "seed", "set", "aaa", "--gist", "新的一句话")
    assert git(h.state_dir, "show", "HEAD:seeds/aaa.py").endswith("# 想到一点")
    files = git(h.state_dir, "ls-files").splitlines()
    assert files == [".gitignore", "backup/ledger.sql", "seeds/aaa.py"]


def test_migration_v5_to_v6_keeps_todos(tmp_path: Path) -> None:
    state = tmp_path / "state"
    state.mkdir()
    conn = db.connect(state / "ledger.db")
    for script in db.MIGRATIONS[:5]:
        for stmt in db._split_sql(script):
            conn.execute(stmt)
    conn.execute("PRAGMA user_version = 5")
    conn.execute("INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
                 " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')")
    conn.execute("INSERT INTO tasks (id, idea_id, text, created_at, kind, payload_json)"
                 " VALUES ('t001', 1, 'x', 't', 'command_new', '{\"name\": \"svd\"}')")
    conn.execute("INSERT INTO tasks (id, idea_id, text, status, created_at, done_at, result)"
                 " VALUES ('t002', 1, 'y', 'done', 't', 't2', 'ok')")
    conn.close()
    with Ledger(state) as ledger:
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION == 7
        rows = ledger.all("SELECT id, idea_id, seed_id, status, kind, payload_json, result"
                          " FROM tasks ORDER BY id")
        assert [tuple(r.values()) for r in rows] == [
            ("t001", 1, None, "open", "command_new", '{"name": "svd"}', None),
            ("t002", 1, None, "done", "plain", None, "ok")]
        indexes = {r[0] for r in ledger.conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'tasks'")}
        assert {"tasks_idea", "tasks_seed"} <= indexes
        with ledger.write() as c:
            c.execute("INSERT INTO seeds (name, created_at, updated_at) VALUES ('s', 't', 't')")
            c.execute("INSERT INTO tasks (id, seed_id, text, created_at) VALUES ('t003', 1, 'z', 't')")
        for bad in ("INSERT INTO tasks (id, text, created_at) VALUES ('t004', 'no owner', 't')",
                    "INSERT INTO tasks (id, idea_id, seed_id, text, created_at)"
                    " VALUES ('t005', 1, 1, 'both', 't')",
                    "INSERT INTO seeds (name, status, created_at, updated_at)"
                    " VALUES ('s2', 'growing', 't', 't')"):
            with pytest.raises(sqlite3.IntegrityError), ledger.write() as c:
                c.execute(bad)


# ---------------------------------------------------------------- plant


@pytest.fixture
def thinking_seed(arb, tmp_path: Path, isolated_home: Path) -> Home:
    h = _home(isolated_home)
    arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--hypothesis", "浅层需要更高的秩", "--metric", "同参数量下的 val/loss")
    nb = h.notebook("rank-schedule")
    nb.write_text(nb.read_text().replace('\n\nif __name__ == "__main__":', THINKING))
    return h


def test_plant_creates_an_idea_from_the_seed(arb, thinking_seed: Home, initialized: Path) -> None:
    h = thinking_seed
    res = arb(initialized, "seed", "plant", "rank-schedule")
    wt = Path(res.stdout.strip())
    assert wt == initialized.parent / "proj-rank-schedule" and (wt / "notebook.py").is_symlink()
    assert "planted rank-schedule as idea rank-schedule" in res.stderr
    idea = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert (idea["name"], idea["hypothesis"], idea["metric"]) == (
        "rank-schedule", "浅层需要更高的秩", "同参数量下的 val/loss")
    # the idea's notebook: header links back, the seed's markdown is copied, code is not
    text = (initialized / ".arboria" / "notebooks" / "rank-schedule.py").read_text()
    tree = ast.parse(text)
    assert f"- **来自种子：** rank-schedule（`{h.notebook('rank-schedule')}`）" in text \
        or "- **来自种子：** rank-schedule（`~/" in text
    strings = [n.value for n in ast.walk(tree) if isinstance(n, ast.Constant)
               and isinstance(n.value, str)]
    thoughts = next(s for s in strings if "为什么会有用" in s)
    assert thoughts.strip().startswith(f"/// details | {notebooks.SEED_THOUGHTS}")
    assert "低秩更新的**秩**也许应该随层变化。" in thoughts and "先在小模型上看奇异值谱" in thoughts
    assert "LoRA 的秩随层变化" not in thoughts  # the seed's own header is not copied
    assert "np.linspace" not in text and "动态的" not in text
    assert "lab.runs()" in text
    # the seed is planted, with its target recorded
    s = json.loads(arb(initialized, "seed", "show", "rank-schedule", "--json").stdout)
    assert s["status"] == "planted"
    from arboria.config import project_config
    from arboria.paths import locate

    pid = project_config(locate(initialized)).project_id
    assert [(p["repo"], p["project_id"], p["idea"]) for p in s["planted_to"]] == [
        (str(initialized), pid, "rank-schedule")]
    # events (and backup commits) in both ledgers
    assert _events(h.state_dir, "seed_plant") == [{
        "subject": "rank-schedule", "repo": str(initialized), "project": pid,
        "idea": "rank-schedule", "markdown_cells": 2}]
    new = _events(initialized / ".arboria", "idea_new")[0]
    assert new["seed"] == "rank-schedule" and new["seed_home"] == str(h.root)
    assert _subjects(h.state_dir)[0] == "arboria backup: seed_plant rank-schedule"
    assert _subjects(initialized / ".arboria")[0] == "arboria backup: idea_new rank-schedule"
    assert "→ proj · rank-schedule" in arb(initialized, "seed", "list").stdout


def test_plant_options_and_again(arb, thinking_seed: Home, initialized: Path,
                                 tmp_path: Path) -> None:
    arb(initialized, "seed", "plant", "rank-schedule", "--as", "rank-v1",
        "--metric", "val/acc")
    idea = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert (idea["name"], idea["metric"]) == ("rank-v1", "val/acc")
    res = arb(initialized, "seed", "plant", "rank-schedule", ok=False)
    assert "already planted" in res.stderr and "--again" in res.stderr
    # --again: into another repo (from anywhere, with --repo)
    other = tmp_path / "other"
    other.mkdir()
    git(other, "init", "-q", "-b", "main")
    (other / "a.py").write_text("x\n")
    git(other, "add", "-A")
    git(other, "commit", "-qm", "init")
    arb(other, "init")
    arb(thinking_seed.root, "seed", "plant", "rank-schedule", "--repo", str(other), "--again")
    s = json.loads(arb(tmp_path, "seed", "show", "rank-schedule", "--json").stdout)
    assert [(Path(p["repo"]).name, p["idea"]) for p in s["planted_to"]] == [
        ("proj", "rank-v1"), ("other", "rank-schedule")]


def test_plant_refusals(arb, thinking_seed: Home, repo: Path, tmp_path: Path,
                        monkeypatch) -> None:
    h = thinking_seed
    res = arb(repo, "seed", "plant", "rank-schedule", ok=False)
    assert "not set up for arboria" in res.stderr and "arb init" in res.stderr
    assert "not inside a git repository" in arb(tmp_path, "seed", "plant", "rank-schedule",
                                                 ok=False).stderr
    assert "which repo?" in arb(h.root, "seed", "plant", "rank-schedule", ok=False).stderr
    arb(repo, "init")
    arb(tmp_path, "seed", "new", "vague", "还没想清楚")
    res = arb(repo, "seed", "plant", "vague", ok=False)
    assert "种下需要假设和指标" in res.stderr and "假设和指标" in res.stderr
    res = arb(repo, "seed", "plant", "vague", "--hypothesis", "h", ok=False)
    assert "还没有指标" in res.stderr
    monkeypatch.setenv("CLAUDE_CODE_ENTRYPOINT", "cli")
    res = arb(repo, "seed", "plant", "rank-schedule", ok=False)
    assert "只由人" in res.stderr and "建分支和 worktree" in res.stderr
    monkeypatch.delenv("CLAUDE_CODE_ENTRYPOINT")
    arb(tmp_path, "seed", "drop", "vague")
    assert "is dropped" in arb(repo, "seed", "plant", "vague", "--hypothesis", "h",
                               "--metric", "m", ok=False).stderr
    # nothing was planted by the refusals
    assert json.loads(arb(repo, "idea", "list", "--json").stdout) == []
    assert json.loads(arb(tmp_path, "seed", "show", "rank-schedule", "--json").stdout)[
        "status"] == "thinking"


def test_seed_markdown_extraction() -> None:
    src = notebooks.render_seed({"name": "x", "gist": "g", "created_at": "2026-11-04"})
    assert notebooks.seed_markdown(src, "x") == []
    src = src.replace('\n\nif __name__ == "__main__":', THINKING)
    md = notebooks.seed_markdown(src, "x")
    assert md[0].startswith("## 为什么会有用\n\n低秩") and md[1] == "- 先在小模型上看奇异值谱"
    assert notebooks.seed_markdown("def broken(:\n", "x") == []


def test_a_repo_under_the_home_folder_is_that_repo(isolated_home: Path) -> None:
    """`~/arboria/lm` with `~/arboria` the home: inside lm, `arb idea …` is about lm's
    ideas (the more specific place wins); the home's own folders are still the home."""
    h = seeds.home()
    lm = h.root / "code" / "lm"
    (lm / "src").mkdir(parents=True)
    (lm / ".git").mkdir()
    assert h.contains(h.root)
    assert h.contains(h.root / "seeds")
    assert h.contains(h.root / "code")
    assert not h.contains(lm)
    assert not h.contains(lm / "src")
    assert seeds.home_of(lm / "src") is None
