"""Project ids: repos sharing one machine profile never touch each other's jobs or files."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from arboria.backends.fake import FakeSlurm
from arboria.clusters import load_profile
from arboria.config import project_config
from arboria.db import Ledger
from arboria.paths import locate

from .clusterutil import fake_toml
from .conftest import git

CMD = ["--", "python", "train.py"]


def _make_repo(root: Path) -> Path:
    root.mkdir(parents=True)
    git(root, "init", "-q", "-b", "main")
    (root / "train.py").write_text(f"print('{root.parent.name}')\n")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "initial")
    return root.resolve()


def _setup(arb, main: Path) -> tuple[Path, str]:
    """Init, point at the shared fake machine, create idea exp1; returns (worktree, project)."""
    arb(main, "init")
    toml = main / "arboria.toml"
    toml.write_text(toml.read_text().replace('default_machine = ""', 'default_machine = "fake"'))
    wt = Path(arb(main, "idea", "new", "exp1", "--hypothesis", "h", "--metric", "m").stdout.strip())
    return wt, project_config(locate(main)).project_id


@pytest.fixture
def two(tmp_path: Path, monkeypatch, arb):
    """Two repos, both folders named `proj`, sharing one machine profile."""
    cfg = tmp_path / "config"
    cfg.mkdir()
    (cfg / "clusters.toml").write_text(fake_toml(tmp_path / "cluster"))
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    a_main = _make_repo(tmp_path / "a" / "proj")
    b_main = _make_repo(tmp_path / "b" / "proj")
    a_wt, a_pid = _setup(arb, a_main)
    b_wt, b_pid = _setup(arb, b_main)
    return {"a": (a_main, a_wt, a_pid), "b": (b_main, b_wt, b_pid),
            "fake": FakeSlurm(load_profile("fake"), a_pid)}


def _submit(arb, wt: Path, *extra: str, ok: bool = True):
    return arb(wt, "submit", "--reason", "x", "--gpus", "1", "--time", "2h", *extra, *CMD, ok=ok)


def _jobs(main: Path, run_id: str = "r001") -> list[dict]:
    with Ledger(main / ".arboria") as l:
        return l.all("SELECT * FROM jobs WHERE run_id = ? ORDER BY segment", (run_id,))


def _run(main: Path, run_id: str = "r001") -> dict:
    with Ledger(main / ".arboria") as l:
        return l.one("SELECT * FROM runs WHERE id = ?", (run_id,))


def test_same_folder_name_different_projects(two) -> None:
    (_, _, a), (_, _, b) = two["a"], two["b"]
    assert a != b and a.startswith("proj-") and b.startswith("proj-")
    assert re.fullmatch(r"proj-[0-9a-f]{4}", a) and re.fullmatch(r"proj-[0-9a-f]{4}", b)


def test_two_repos_at_r001_get_different_names_and_dirs(two, arb) -> None:
    (a_main, a_wt, a), (b_main, b_wt, b) = two["a"], two["b"]
    fake = two["fake"]
    fake.script(f"arb-{a}-r001-s1", run_s=7200, out_files={"who.txt": "a"})
    fake.script(f"arb-{b}-r001-s1", run_s=7200, out_files={"who.txt": "b"})
    _submit(arb, a_wt)
    _submit(arb, b_wt)
    assert json.loads(_run(a_main)["request_json"])["project"] == a
    assert json.loads(_run(b_main)["request_json"])["project"] == b
    arb(a_wt, "approve", "r001")
    arb(b_wt, "approve", "r001")
    jobs = fake.jobs()
    assert [(j["id"], j["name"]) for j in jobs.values()] == [
        ("1001", f"arb-{a}-r001-s1"), ("1002", f"arb-{b}-r001-s1")]
    runs = Path(fake.profile.paths["runs"])
    assert (runs / a / "r001" / "code" / "train.py").read_text() == "print('a')\n"
    assert (runs / b / "r001" / "code" / "train.py").read_text() == "print('b')\n"
    for pid, jid in ((a, "1001"), (b, "1002")):
        script = jobs[jid]["script"]
        ckpt, out = fake.profile.paths["ckpt"], fake.profile.paths["out"]
        assert f'export ARBORIA_CKPT_DIR="{ckpt}/{pid}/r001"' in script
        assert f'export ARBORIA_OUT="{out}/{pid}/r001"' in script
        assert f"{runs}/{pid}/r001/logs/s1.%j.out" in script
    fake.advance(7300)
    arb(a_wt, "poll")
    arb(b_wt, "poll")
    for main, who in ((a_main, "a"), (b_main, "b")):
        assert (main / ".arboria" / "runs" / "r001" / "out" / "who.txt").read_text() == who
    # each repo's logs / remote ls only reach its own dirs
    assert arb(a_wt, "remote", "ls", "r001").stdout.splitlines()[0] == f"{runs}/{a}/r001:"
    assert arb(b_wt, "remote", "ls", "r001").stdout.splitlines()[0] == f"{runs}/{b}/r001:"


def test_find_job_in_b_never_adopts_a_job(two, arb) -> None:
    (a_main, a_wt, a), (b_main, b_wt, b) = two["a"], two["b"]
    fake = two["fake"]
    fake.script(f"arb-{a}-r001-s1", run_s=7200)
    _submit(arb, a_wt)
    arb(a_wt, "approve", "r001")
    _submit(arb, b_wt)
    res = arb(b_wt, "approve", "r001")
    assert "s1: job 1002" in res.stdout and "adopted" not in res.stdout
    assert [c[1] for c in fake.calls("find_job")] == [f"arb-{a}-r001-s1", f"arb-{b}-r001-s1"]
    assert [j["job_id"] for j in _jobs(b_main)] == ["1002"]
    # a lost sbatch reply in B is adopted from B's own job only
    _submit(arb, b_wt)
    fake.script(f"arb-{b}-r002-s1", lose_reply=True)
    arb(b_wt, "approve", "r002", ok=False)
    res = arb(b_wt, "approve", "r002")
    assert "s1: job 1003 (already on the cluster, adopted)" in res.stdout
    assert fake.jobs()["1003"]["name"] == f"arb-{b}-r002-s1"
    # B's backend refuses A's job names outright
    b_backend = FakeSlurm(load_profile("fake"), b)
    with pytest.raises(Exception, match=f"belongs to project {a}"):
        b_backend.find_job(f"arb-{a}-r001-s1")
    with pytest.raises(Exception, match="refused"):
        b_backend.tail(f"{fake.profile.paths['runs']}/{a}/r001/logs/s1.1001.out", 5)


def test_cancel_in_b_never_touches_a_jobs(two, arb) -> None:
    (a_main, a_wt, a), (b_main, b_wt, b) = two["a"], two["b"]
    fake = two["fake"]
    fake.script(f"arb-{a}-r001-s1", run_s=7200)
    fake.script(f"arb-{b}-r001-s1", submit_error=True)
    _submit(arb, a_wt)
    arb(a_wt, "approve", "r001")
    _submit(arb, b_wt)
    arb(b_wt, "approve", "r001", ok=False)  # B's sbatch failed: row left `submitting`
    assert [(j["job_id"], j["state"]) for j in _jobs(b_main)] == [(None, "submitting")]
    fake.advance(60)
    res = arb(b_wt, "cancel", "r001")
    assert "no live jobs" in res.stdout
    assert fake.calls("cancel") == []
    assert fake.jobs()["1001"]["state"] == "running"
    assert _jobs(b_main)[0]["job_id"] is None  # A's job was not adopted

    _submit(arb, b_wt)
    arb(b_wt, "approve", "r002")
    arb(b_wt, "cancel", "r002")
    assert fake.calls("cancel") == [["cancel", ["1002"]]]
    assert fake.jobs()["1001"]["state"] == "running"
    assert _run(a_main)["status"] == "queued"


def _strip_project(main: Path, run_id: str = "r001") -> None:
    with Ledger(main / ".arboria") as l, l.write() as c:
        req = json.loads(l.one("SELECT request_json FROM runs WHERE id = ?", (run_id,))
                         ["request_json"])
        del req["project"]
        c.execute("UPDATE runs SET request_json = ? WHERE id = ?", (json.dumps(req), run_id))


def test_old_run_without_project_is_refused_on_approve(two, arb) -> None:
    a_main, a_wt, a = two["a"]
    fake = two["fake"]
    _submit(arb, a_wt)
    _strip_project(a_main)
    res = arb(a_wt, "approve", "r001", ok=False)
    assert res.exit_code == 1 and "加入项目 id 之前提交的" in res.stderr
    assert "重新 arb submit" in res.stderr
    assert _run(a_main)["status"] == "awaiting_approval"
    with Ledger(a_main / ".arboria") as l:
        assert l.one("SELECT choice FROM decisions")["choice"] is None
    assert "重新 arb submit" in arb(a_wt, "card", "r001", ok=False).stderr
    assert fake.calls() == []


def test_old_run_with_jobs_can_still_be_polled_and_cancelled(two, arb) -> None:
    a_main, a_wt, a = two["a"]
    fake = two["fake"]
    fake.script(f"arb-{a}-r001-s1", run_s=7200)
    _submit(arb, a_wt)
    arb(a_wt, "approve", "r001")
    # r002: approved, but its sbatch failed, so it would need `arb approve` to resume
    fake.script(f"arb-{a}-r002-s1", submit_error=True)
    _submit(arb, a_wt)
    arb(a_wt, "approve", "r002", ok=False)
    assert _run(a_main, "r002")["status"] == "approved"
    _strip_project(a_main)
    _strip_project(a_main, "r002")
    assert "重新 arb submit" in arb(a_wt, "approve", "r002", ok=False).stderr
    assert len(fake.calls("submit")) == 2  # r001 + r002's failed try; no resume
    fake.advance(60)
    assert "r001: running" in arb(a_wt, "poll", "r001").stdout
    arb(a_wt, "watch", "--once")  # no observation of a legacy run's log
    assert fake.calls("tail") == []
    assert "重新 arb submit" in arb(a_wt, "logs", "r001", ok=False).stderr
    assert "cancelled r001 (scancel 1001)" in arb(a_wt, "cancel", "r001").stdout
    assert fake.jobs()["1001"]["state"] == "cancelled"
    # legacy job names are never looked up (they could be another repo's)
    with Ledger(a_main / ".arboria") as l, l.write() as c:
        c.execute("UPDATE jobs SET job_name = 'arboria-r002-s1' WHERE run_id = 'r002'")
    n = len(fake.calls("find_job"))
    assert "no live jobs" in arb(a_wt, "cancel", "r002").stdout
    assert len(fake.calls("find_job")) == n


@pytest.mark.parametrize("args", [
    ["submit", "--reason", "x", "--gpus", "1", "--time", "1h", *CMD],
    ["approve", "r002"], ["poll"], ["watch", "--once"], ["logs", "r001"],
    ["remote", "ls", "r001"], ["cancel", "r001"],
])
def test_commands_need_project_id(two, arb, args) -> None:
    a_main, a_wt, a = two["a"]
    _submit(arb, a_wt)
    arb(a_wt, "approve", "r001")
    _submit(arb, a_wt)
    toml = a_main / "arboria.toml"
    original = toml.read_text()
    toml.write_text(re.sub(r"\[project\]\n#[^\n]*\nid = \"[^\"]*\"\n\n", "", original))
    assert "[project]" not in toml.read_text()
    res = arb(a_wt, *args, ok=False)
    assert res.exit_code == 1 and "no [project] id" in res.stderr and "arb init" in res.stderr
    arb(a_main, "init")
    assert project_config(locate(a_main)).project_id == a  # same id derived again
