"""`lab` on a schema v8 ledger (written by the Rust arb, opened read-only here): every write
goes to `arb lab` (crates/arboria-core/src/kernel.rs) with the same checks and is read back.
Needs the Rust binary: $ARBORIA_ARB, else target/debug/arb (`cargo build`); skipped
without it."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from arboria import lab
from arboria.db import Ledger
from arboria.errors import ArbError

from .conftest import git
from .test_submit import cluster  # noqa: F401  (fixture)
from .test_v8_read_only import to_v8

ROOT = Path(__file__).resolve().parent.parent


def _rust_arb() -> str | None:
    exe = os.environ.get("ARBORIA_ARB") or str(ROOT / "target" / "debug" / "arb")
    return exe if Path(exe).is_file() else None


@pytest.fixture
def rust(monkeypatch: pytest.MonkeyPatch) -> str:
    exe = _rust_arb()
    if exe is None:
        pytest.skip("no Rust arb (cargo build, or set ARBORIA_ARB)")
    monkeypatch.setenv("ARBORIA_ARB", exe)
    # the notebooks the Rust arb writes come from this package
    monkeypatch.setenv("ARBORIA_PYTHON", sys.executable)
    monkeypatch.setenv("PYTHONPATH", str(ROOT / "src"))
    return exe


@pytest.fixture(autouse=True)
def no_pinned_idea():
    lab.use(None)
    yield
    lab.use(None)


def _one(main: Path, sql: str, *args) -> dict:
    with Ledger(main / ".arboria") as ledger:
        return ledger.one(sql, args)


def test_lab_writes_through_the_rust_arb(rust, cluster, idea: Path, initialized: Path,
                                         monkeypatch) -> None:
    to_v8(initialized / ".arboria")
    monkeypatch.chdir(idea)
    backups = len(git(initialized / ".arboria", "log", "--format=%s").splitlines())

    r = lab.run(reason="baseline", gpus=1, time="1h", set={"lr": 3e-4})
    assert (r.id, r.existed, r.status()) == ("r001", False, "awaiting_approval")
    req = json.loads(_one(initialized, "SELECT request_json FROM runs WHERE id = 'r001'")
                     ["request_json"])
    assert req["command"][-1] == "lr=0.0003"
    assert "审核单 r001" in r.card() and cluster.calls() == []  # never approved
    again = lab.run(reason="re-run cell", gpus=1, time="1h", set={"lr": 3e-4})
    assert (again.id, again.existed) == ("r001", True)

    t = lab.todo("看一下 loss 尖峰")
    assert (t.id, t.existed, t["idea"]) == ("t001", False, "exp1")
    assert lab.todo("看一下 loss 尖峰").existed
    assert "看一下 loss 尖峰" in str(lab.todos())

    assert lab.conf("prelim")["confidence"] == "prelim"
    assert lab.metric("val/ppl")["metric"] == "val/ppl"

    pending = lab.loss_curve(ask="画 loss 曲线")
    assert not pending and pending.task_id == "t002" and pending.created
    assert _one(initialized, "SELECT kind FROM tasks WHERE id = 't002'")["kind"] == "command_new"

    st = lab.inspect("训练循环在哪")
    assert st.status == "pending" and st.task_id == "t003" and st._state.created
    assert lab.inspect("训练循环在哪").task_id == "t003"  # the cell re-ran: same todo

    kid = lab.fork("exp2", r, hypothesis="lower lr", metric="val/loss")
    assert kid["name"] == "exp2" and Path(kid["notebook"]).is_file()
    # the Rust arb writes the app's markdown notebook (ADR 0005)
    assert kid["notebook"].endswith("/exp2.md")
    assert (Path(kid["worktree_path"]) / "notebook.md").is_symlink()

    # the ledger is still v8, and the Rust arb committed the backups
    with Ledger(initialized / ".arboria") as ledger:
        assert ledger.version == 8
    assert len(git(initialized / ".arboria", "log", "--format=%s").splitlines()) > backups

    closed = lab.prune("没有用")
    assert closed["stage"] == "dropped" and closed["close"]["mode"] == "drop"
    assert _one(initialized, "SELECT text FROM notes ORDER BY id DESC LIMIT 1")["text"] == \
        "prune: 没有用"


def test_human_only_holds_in_the_rust_arb(rust, idea: Path, initialized: Path,
                                          monkeypatch) -> None:
    to_v8(initialized / ".arboria")
    monkeypatch.chdir(idea)
    monkeypatch.setenv("CLAUDECODE", "1")
    with pytest.raises(ArbError, match="写待办 只由人在 notebook 里做"):
        lab.todo("x")
    # calling `arb lab` directly gains nothing
    req = {"op": "todo", "repo": str(initialized), "idea": "exp1", "text": "x"}
    out = subprocess.run([rust, "lab"], input=json.dumps(req), capture_output=True,
                         text=True, env=os.environ | {"CLAUDECODE": "1"})
    assert out.returncode == 1
    assert "写待办 只由人在 notebook 里做" in json.loads(out.stdout)["error"]
    assert _one(initialized, "SELECT COUNT(*) AS n FROM tasks")["n"] == 0


def test_seed_notebook_on_v8(rust, arb, tmp_path: Path, isolated_home: Path) -> None:
    arb(tmp_path, "seed", "new", "xss", "一句话")
    to_v8(isolated_home / ".arboria")
    scope = {"__name__": "__marimo__", "__file__": str(isolated_home / "seeds" / "xss.py"),
             "lab": lab}
    exec('t = lab.todo("找三篇相关论文")\nm = lab.metric("召回率")', scope)
    assert scope["t"]["seed"] == "xss" and scope["t"]["idea"] is None
    assert scope["m"]["metric"] == "召回率" and scope["m"]["status"] == "thinking"
    exec("ts = lab.todos()", scope)
    assert "找三篇相关论文" in str(scope["ts"])
    with pytest.raises(ArbError, match="种子没有置信度"):
        exec('lab.conf("prelim")', scope)


def test_no_rust_arb_says_so(idea: Path, initialized: Path, monkeypatch,
                             tmp_path: Path) -> None:
    to_v8(initialized / ".arboria")
    monkeypatch.chdir(idea)
    monkeypatch.setenv("ARBORIA_ARB", str(tmp_path / "missing"))
    with pytest.raises(ArbError, match=r"schema v8.*运行 .*missing 失败"):
        lab.todo("x")
    fake = tmp_path / "arb"
    fake.write_text("#!/bin/sh\necho 'Usage: arb [OPTIONS]' >&2\nexit 2\n")
    fake.chmod(0o755)
    monkeypatch.setenv("ARBORIA_ARB", str(fake))
    with pytest.raises(ArbError, match="不认 `arb lab`"):
        lab.todo("x")
