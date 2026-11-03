"""arb submit / approve / reject / cancel / poll / watch / logs against FakeSlurm."""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

from arboria import backends, monitor, slurm, submission
from arboria.backends.fake import FakeSlurm
from arboria.clusters import load_profile
from arboria.config import project_config
from arboria.db import Ledger
from arboria.errors import ArbError
from arboria.paths import locate

from .clusterutil import fake_toml
from .conftest import git

SRC = Path(__file__).parent.parent / "src" / "arboria"
CMD = ["--", "python", "train.py", "--lr", "3e-4"]


@pytest.fixture
def cluster(tmp_path_factory, monkeypatch, initialized: Path) -> FakeSlurm:
    base = tmp_path_factory.mktemp("cluster")
    cfg = base / "config"
    cfg.mkdir()
    (cfg / "clusters.toml").write_text(fake_toml(base))
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    toml = initialized / "arboria.toml"
    toml.write_text(toml.read_text().replace('default_machine = ""', 'default_machine = "fake"')
                    .replace("total_su = 0", "total_su = 1000"))
    return FakeSlurm(load_profile("fake"), project_config(locate(initialized)).project_id)


def jn(cluster: FakeSlurm, run: str, k: int) -> str:
    return slurm.job_name(cluster.project, run, k)


def _bump(wt: Path, tag: str = "") -> None:
    """Change the worktree so the next submit is a different run (identical ones are reused)."""
    n = len(list(wt.glob("bump*.txt")))
    (wt / f"bump{n}{tag}.txt").write_text(f"{n}\n")


def _submit(arb, idea: Path, *extra: str, ok: bool = True):
    return arb(idea, "submit", "--reason", "warmup 1k", "--gpus", "4", "--time", "12h",
               *extra, *CMD, ok=ok)


def _ledger(main: Path) -> Ledger:
    return Ledger(main / ".arboria")


def _run(main: Path, run_id: str = "r001") -> dict:
    with _ledger(main) as l:
        return l.one("SELECT * FROM runs WHERE id = ?", (run_id,))


def _jobs(main: Path, run_id: str = "r001") -> list[dict]:
    with _ledger(main) as l:
        return l.all("SELECT * FROM jobs WHERE run_id = ? ORDER BY segment", (run_id,))


def _decisions(main: Path, run_id: str = "r001") -> list[dict]:
    with _ledger(main) as l:
        return l.all("SELECT * FROM decisions WHERE subject_type = 'run' AND subject_id = ?",
                     (run_id,))


def _backups(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


# ---------------------------------------------------------------- submit


def test_submit_is_local_and_prints_card(cluster, idea: Path, initialized: Path, arb) -> None:
    res = _submit(arb, idea)
    out = res.stdout
    for label in ("审核单 r001（idea exp1）", "检查:", "资源:", "估计:", "预算:"):
        assert label in out, label
    assert "审稿" not in out
    assert out.rstrip().endswith("由你批准：arb approve r001")
    assert "✓ 计费卡数 = 申请卡数 = 4" in out and "✓ 账户 acct1" in out
    assert "上限 4 卡 × 12.00 h × 1 = 48.00 SU" in out
    assert "项目已用 0.00 / 1000 SU" in out and "本 idea 已用 0.00 SU" in out
    assert "命令 python train.py --lr 3e-4" in out
    assert cluster.calls() == []  # never touched the backend

    run = _run(initialized)
    assert run["status"] == "awaiting_approval" and run["machine"] == "fake"
    assert run["gpus"] == 4 and run["walltime"] == "12h" and run["est_cost"] == 48.0
    req = json.loads(run["request_json"])
    assert req["command"] == ["python", "train.py", "--lr", "3e-4"]
    assert req["segments"] == [43200] and req["billed_gpus"] == 4 and req["partition"] == "ghx4"
    assert len(run["card_hash"]) == 64 and run["card_hash"][:16] in out
    assert git(idea, "rev-parse", "refs/arboria/runs/r001") == run["snapshot_commit"]
    [dec] = _decisions(initialized)
    assert dec["question"] == "批准这个 run 上集群吗？" and dec["choice"] is None
    assert json.loads(dec["options_json"]) == ["approve", "reject", "revise"]
    assert _backups(initialized)[0] == "arboria backup: decision d001; run_submit r001"

    # identical request again: the same run comes back (see test_identical_submit_*)
    data = json.loads(_submit(arb, idea, "--json").stdout)
    assert data["run"]["id"] == "r001" and data["existed"] and data["card"]["est_su"] == 48.0
    _bump(idea)
    data = json.loads(_submit(arb, idea, "--json").stdout)
    assert data["run"]["id"] == "r002" and not data["existed"]


def test_submit_flags_over_request(cluster, idea: Path, arb) -> None:
    out = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--mem", "200", "--time", "1h",
              *CMD).stdout
    assert "⚠ 计费卡数 2 > 申请卡数 1" in out and "上限 2 卡" in out


def test_submit_segments_and_interactive_charge(cluster, idea: Path, initialized: Path, arb) -> None:
    out = arb(idea, "submit", "--reason", "x", "--gpus", "4", "--time", "100h", *CMD).stdout
    assert "分段 48:00:00 + 48:00:00 + 04:00:00" in out and "分 3 段" in out
    out = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h",
              "--partition", "ghx4-interactive", *CMD).stdout
    assert "⚠ 分区 ghx4-interactive 按 2 倍计费" in out and "= 2.00 SU" in out


@pytest.mark.parametrize("extra,msg", [
    (["--time", "300h"], "max_segments"),
    (["--partition", "nope"], "no partition"),
    (["--machine", "nope"], "no machine 'nope'"),
    (["--gpus", "8"], "GPUs per node"),
    (["--time", "soon"], "invalid walltime"),
])
def test_submit_refusals_write_nothing(cluster, idea: Path, initialized: Path, arb,
                                       extra, msg) -> None:
    args = {"--gpus": "4", "--time": "12h"}
    rest = []
    it = iter(extra)
    for k in it:
        v = next(it)
        if k in args:
            args[k] = v
        else:
            rest += [k, v]
    res = arb(idea, "submit", "--reason", "x", *[x for kv in args.items() for x in kv], *rest,
              *CMD, ok=False)
    assert res.exit_code == 1 and msg in res.stderr, res.stderr
    assert _run(initialized) is None and git(idea, "for-each-ref", "refs/arboria") == ""


def test_submit_needs_machine_account_and_command(cluster, idea: Path, initialized: Path, arb,
                                                  monkeypatch, tmp_path) -> None:
    toml = initialized / "arboria.toml"
    toml.write_text(toml.read_text().replace('default_machine = "fake"', 'default_machine = ""')
                    .replace('command = "python train.py"', 'command = ""'))
    res = _submit(arb, idea, ok=False)
    assert "no machine" in res.stderr
    # no `-- CMD` and no [run] command
    res = arb(idea, "submit", "--reason", "x", "--gpus", "1", "--time", "1h", "--machine", "fake",
              ok=False)
    assert res.exit_code == 1 and submission.NO_COMMAND in res.stderr
    res = arb(idea, "submit", "--reason", "x", "--machine", "fake", *CMD, ok=False)
    assert res.exit_code == 1 and "--gpus and --time are required" in res.stderr
    cfg = tmp_path / "cfg2"
    cfg.mkdir()
    (cfg / "clusters.toml").write_text(fake_toml(tmp_path / "c2", account=""))
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    res = _submit(arb, idea, "--machine", "fake", ok=False)
    assert "has no account" in res.stderr
    res = arb(initialized, "submit", "--reason", "x", "--gpus", "1", "--time", "1h", *CMD, ok=False)
    assert "not an idea worktree" in res.stderr
    assert _run(initialized) is None


# ---------------------------------------------------------------- approve


def test_approve_submits_and_queues(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    res = arb(idea, "approve", "r001")
    assert "approved r001 (decision d001)" in res.stdout and "s1: job 1001" in res.stdout
    run = _run(initialized)
    assert run["status"] == "queued"
    [dec] = _decisions(initialized)
    assert (dec["choice"], dec["decided_by"]) == ("approve", "human")
    [job] = _jobs(initialized)
    assert (job["segment"], job["job_name"], job["job_id"], job["state"]) == \
        (1, jn(cluster, "r001", 1), "1001", "pending")
    # the decision was recorded (and backed up) before the submission
    log = _backups(initialized)
    assert log[1].startswith("arboria backup: decision d001; run_approve r001")
    assert log[0] == "arboria backup: run_queued r001"
    calls = [c[0] for c in cluster.calls()]
    assert calls == ["ship", "find_job", "submit"]
    # code shipped from the snapshot, script written next to it
    runs = Path(cluster.profile.paths["runs"]) / cluster.project
    assert (runs / "r001" / "code" / "train.py").read_text() == "print('train')\n"
    script = (runs / "r001" / "sbatch" / "s1.sh").read_text()
    assert f"#SBATCH --job-name=arb-{cluster.project}-r001-s1" in script
    assert "--account=acct1" in script
    assert script.rstrip().endswith("python train.py --lr 3e-4")
    # approving again is refused: already queued
    res = arb(idea, "approve", "r001", ok=False)
    assert "is queued" in res.stderr


def test_segmented_run_chains_dependencies(cluster, idea: Path, initialized: Path, arb) -> None:
    arb(idea, "submit", "--reason", "x", "--gpus", "2", "--time", "100h", *CMD)
    arb(idea, "approve", "r001")
    jobs = cluster.jobs()
    assert [(j["name"], j["dependency"]) for j in jobs.values()] == [
        (jn(cluster, "r001", 1), None), (jn(cluster, "r001", 2), "1001"), (jn(cluster, "r001", 3), "1002")]
    assert "--time=48:00:00" in jobs["1001"]["script"] and "--time=04:00:00" in jobs["1003"]["script"]
    assert [j["job_id"] for j in _jobs(initialized)] == ["1001", "1002", "1003"]


def test_card_change_refuses_approval(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    cfg = Path(cluster.profile.source)
    cfg.write_text(cfg.read_text().replace("env_activate = []", 'env_activate = ["pixi shell"]'))
    res = arb(idea, "approve", "r001", ok=False)
    assert res.exit_code == 1 and "审核单已变化，请重新 arb submit" in res.stderr
    assert "审核单已变化" in arb(idea, "card", "r001").stdout
    assert _run(initialized)["status"] == "awaiting_approval"
    assert _decisions(initialized)[0]["choice"] is None
    assert cluster.calls() == []

    # tampering with the stored request is caught the same way
    cfg.write_text(cfg.read_text().replace('env_activate = ["pixi shell"]', "env_activate = []"))
    req = json.loads(_run(initialized)["request_json"])
    req["ask"]["gpus"] = 1
    with _ledger(initialized) as l, l.write() as c:
        c.execute("UPDATE runs SET request_json = ? WHERE id = 'r001'", (json.dumps(req),))
    assert "审核单已变化" in arb(idea, "approve", "r001", ok=False).stderr
    assert cluster.calls() == []


def test_approve_only_awaiting_runs(cluster, idea: Path, initialized: Path, arb) -> None:
    arb(idea, "run", "new", "--reason", "draft only")
    res = arb(idea, "approve", "r001", ok=False)
    assert "is draft" in res.stderr
    assert "no run 'r404'" in arb(idea, "approve", "r404", ok=False).stderr
    assert cluster.calls() == []


def test_reject(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    arb(idea, "reject", "r001", "--reason", "lr too high for 4 GPUs")
    run = _run(initialized)
    assert run["status"] == "rejected"
    [dec] = _decisions(initialized)
    assert (dec["choice"], dec["decided_by"]) == ("reject", "human")
    with _ledger(initialized) as l:
        assert l.one("SELECT text, run_id FROM notes") == {"text": "lr too high for 4 GPUs",
                                                          "run_id": "r001"}
    assert _backups(initialized)[0] == "arboria backup: decision d001; run_reject r001"
    assert "is rejected" in arb(idea, "approve", "r001", ok=False).stderr
    assert "is rejected" in arb(idea, "reject", "r001", ok=False).stderr
    assert cluster.calls() == []


def test_resume_after_failed_segment_submission(cluster, idea: Path, initialized: Path, arb) -> None:
    arb(idea, "submit", "--reason", "x", "--gpus", "4", "--time", "60h", *CMD)
    cluster.script(jn(cluster, "r001", 2), submit_error=True)
    res = arb(idea, "approve", "r001", ok=False)
    assert "已批准，但提交没有完成" in res.stderr and "arb approve r001" in res.stderr
    assert _run(initialized)["status"] == "approved"
    assert [(j["job_id"], j["state"]) for j in _jobs(initialized)] == [
        ("1001", "pending"), (None, "submitting")]

    res = arb(idea, "approve", "r001")
    assert "resuming approved r001" in res.stdout and "s2: job 1002" in res.stdout
    assert "s1:" not in res.stdout
    assert len(_decisions(initialized)) == 1  # no second Decision
    assert _run(initialized)["status"] == "queued"
    assert [c[1] for c in cluster.calls("submit")] == [
        jn(cluster, "r001", 1), jn(cluster, "r001", 2), jn(cluster, "r001", 2)]
    assert len(cluster.calls("ship")) == 1
    assert cluster.jobs()["1002"]["dependency"] == "1001"


def test_lost_sbatch_reply_is_adopted_not_resubmitted(cluster, idea: Path, initialized: Path,
                                                      arb) -> None:
    _submit(arb, idea)
    cluster.script(jn(cluster, "r001", 1), lose_reply=True)
    assert "connection lost" in arb(idea, "approve", "r001", ok=False).stderr
    assert [(j["job_id"], j["state"]) for j in _jobs(initialized)] == [(None, "submitting")]
    res = arb(idea, "approve", "r001")
    assert "s1: job 1001 (already on the cluster, adopted)" in res.stdout
    assert len(cluster.jobs()) == 1 and len(cluster.calls("submit")) == 1


def test_need_login_during_approve_is_resumable(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    cluster.set_need_login()
    res = arb(idea, "approve", "r001", ok=False)
    assert "请先在终端运行 `ssh fake` 完成 Duo 认证" in res.stderr
    assert _run(initialized)["status"] == "approved"
    assert [c[0] for c in cluster.calls()] == ["ship"]  # stopped at the first remote call
    cluster.set_need_login(False)
    arb(idea, "approve", "r001")
    assert _run(initialized)["status"] == "queued" and len(cluster.jobs()) == 1


# ---------------------------------------------------------------- the approval gate


def test_submit_is_called_only_from_the_guard() -> None:
    """By construction: one call site of Backend.submit, inside _guarded_submit."""
    hits = []
    for path in SRC.rglob("*.py"):
        if "backends" in path.parts:
            continue
        for i, line in enumerate(path.read_text().splitlines(), 1):
            if re.search(r"(?<!submission)\.submit\(", line):
                hits.append((path.name, line.strip()))
    assert hits == [("submission.py", "return backend.submit(script, name, dependency)")]
    src = (SRC / "submission.py").read_text()
    guard = src[src.index("def _guarded_submit"):src.index("def _set_job")]
    assert "approval_decision(ledger, run_id) is None" in guard
    assert "auto" not in src.lower().replace("autocommit", "")


class Spy(FakeSlurm):
    submitted: list = []

    def submit(self, script_text, job_name, dependency_job_id):
        Spy.submitted.append(job_name)
        return super().submit(script_text, job_name, dependency_job_id)


def test_no_other_path_reaches_backend_submit(cluster, idea: Path, initialized: Path, arb,
                                              monkeypatch) -> None:
    Spy.submitted = []
    monkeypatch.setattr(backends, "get_backend", lambda profile, project: Spy(profile, project))
    _submit(arb, idea)
    _bump(idea)
    _submit(arb, idea)
    _submit(arb, idea)  # identical: returns r002
    arb(idea, "submit", "--from", "r001", "--set", "lr=1", "--reason", "x")
    arb(idea, "card", "r001")
    arb(idea, "reject", "r002")
    arb(idea, "reject", "r003")
    arb(idea, "poll")
    arb(idea, "poll", "r001")
    arb(idea, "watch", "--once")
    arb(idea, "status")
    arb(initialized, "export")
    arb(idea, "cancel", "r001", ok=False)
    assert Spy.submitted == []
    arb(idea, "approve", "r001")
    assert Spy.submitted == [jn(cluster, "r001", 1)]


def test_guard_refuses_without_human_decision(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    ctx = locate(idea)
    with _ledger(initialized) as l:
        run = l.one("SELECT * FROM runs WHERE id = 'r001'")
        _, plan = submission.replan(run)
        with pytest.raises(ArbError, match="no human approval Decision"):
            submission._guarded_submit(l, cluster, "r001", "S", jn(cluster, "r001", 1), None)
        with pytest.raises(ArbError, match="no human approval Decision"):
            submission.submit_segments(ctx, l, run, plan, cluster)
        # flipping the status by hand is not enough
        with l.write() as c:
            c.execute("UPDATE runs SET status = 'approved' WHERE id = 'r001'")
        with pytest.raises(ArbError, match="no human approval Decision"):
            submission._guarded_submit(l, cluster, "r001", "S", jn(cluster, "r001", 1), None)
        # nor is a decision made by someone other than the human
        with l.write() as c:
            c.execute("UPDATE decisions SET choice = 'approve', decided_by = 'claude'")
        with pytest.raises(ArbError, match="no human approval Decision"):
            submission._guarded_submit(l, cluster, "r001", "S", jn(cluster, "r001", 1), None)
    assert "no human approval Decision" in arb(idea, "approve", "r001", ok=False).stderr
    assert cluster.calls("submit") == []


# ---------------------------------------------------------------- poll / cancel


def test_poll_to_success_fetches_out(cluster, idea: Path, initialized: Path, arb) -> None:
    cluster.script(jn(cluster, "r001", 1), run_s=3600, queue_s=60, billed=4.25,
                   log="epoch 1 loss 2.1\n", out_files={"metrics.json": '{"val_loss": 2.0}'})
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    n_backups = len(_backups(initialized))
    assert "r001: queued" in arb(idea, "poll").stdout
    cluster.advance(120)
    assert "r001: running (was queued)" in arb(idea, "poll").stdout
    cluster.advance(600)
    arb(idea, "poll")
    [job] = _jobs(initialized)
    assert (job["state"], job["elapsed_s"]) == ("running", 660)
    assert job["billed_su"] == pytest.approx(660 / 3600 * 4, abs=1e-3)  # local estimate while running
    assert len(_backups(initialized)) == n_backups  # heartbeats never back up
    cluster.advance(3600)
    out = arb(idea, "poll").stdout
    assert "r001: succeeded (was running)" in out and "成功（4.25 SU）" in out
    run = _run(initialized)
    assert run["status"] == "succeeded" and run["actual_cost"] == 4.25
    assert _backups(initialized)[0] == "arboria backup: run_final r001"
    local = initialized / ".arboria" / "runs" / "r001" / "out" / "metrics.json"
    assert json.loads(local.read_text()) == {"val_loss": 2.0}
    assert "成功" in (initialized / ".arboria" / "notifications.log").read_text()
    assert "no active runs" in arb(idea, "poll").stdout
    # status / export / hook show it
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert st["runs"][0]["jobs"][0]["job_id"] == "1001" and st["runs"][0]["actual_cost"] == 4.25
    text = arb(idea, "status").stdout
    assert "jobs: fake/ghx4 4 GPU 12h; s1 1001 completed; 4.25 SU（估计上限 48.00）" in text
    assert "jobs: fake/ghx4 4 GPU 12h; s1 1001 completed" in arb(initialized, "export").stdout


def test_poll_failure_cancels_remaining_segments(cluster, idea: Path, initialized: Path, arb) -> None:
    cluster.script(jn(cluster, "r001", 1), state="failed", run_s=30)
    arb(idea, "submit", "--reason", "x", "--gpus", "4", "--time", "60h", *CMD)
    arb(idea, "approve", "r001")
    cluster.advance(60)
    out = arb(idea, "poll").stdout
    assert "r001: failed" in out
    assert [j["state"] for j in _jobs(initialized)] == ["failed", "cancelled"]
    assert cluster.jobs()["1002"]["state"] == "cancelled"
    assert cluster.calls("cancel") == [["cancel", ["1002"]]]


def test_last_segment_timeout_is_failure_but_earlier_is_fine(cluster, idea: Path,
                                                            initialized: Path, arb) -> None:
    cluster.script(jn(cluster, "r001", 1), state="timeout", run_s=100)
    cluster.script(jn(cluster, "r001", 2), state="completed", run_s=100)
    arb(idea, "submit", "--reason", "x", "--gpus", "4", "--time", "60h", *CMD)
    arb(idea, "approve", "r001")
    cluster.advance(150)
    assert "r001: running" in arb(idea, "poll").stdout
    cluster.advance(100)
    assert "r001: succeeded" in arb(idea, "poll").stdout
    assert monitor.derive_status(["timeout", "timeout"], 2, "running") == "failed"
    assert monitor.derive_status(["completed", "pending"], 2, "running") == "running"
    assert monitor.derive_status(["pending"], 2, "approved") == "approved"


def test_cancel(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    assert "arb reject r001" in arb(idea, "cancel", "r001", ok=False).stderr
    arb(idea, "approve", "r001")
    cluster.advance(10)
    res = arb(idea, "cancel", "r001")
    assert "cancelled r001 (scancel 1001)" in res.stdout
    assert _run(initialized)["status"] == "cancelled"
    assert [j["state"] for j in _jobs(initialized)] == ["cancelled"]
    assert cluster.jobs()["1001"]["state"] == "cancelled"
    assert _backups(initialized)[0] == "arboria backup: run_cancel r001; run_final r001"
    assert "nothing to cancel" in arb(idea, "cancel", "r001", ok=False).stderr


# ---------------------------------------------------------------- watch


@pytest.mark.parametrize("log,pattern", [
    ("step 10 loss nan\n", r"(?i)\bloss\b[^\n]{0,40}\bnan\b"),
    ("step 120 loss nan\n", r"(?i)\bloss\b[^\n]{0,40}\bnan\b"),
    ("epoch 1 | loss: NaN | lr 3e-4\n", r"(?i)\bloss\b[^\n]{0,40}\bnan\b"),
    ("warning: nan detected in loss\n", r"(?i)\bnan\b[^\n]{0,40}\bloss\b"),
    ("RuntimeError: CUDA out of memory. Tried\n", "CUDA out of memory"),
    ("torch.OutOfMemoryError: allocation failed\n", r"torch\.OutOfMemoryError"),
    ("step 10 loss 2.3 nanoseconds\n", None),
    ("0 NaN batches skipped\n", None),
    ("nan_policy=skip, banana loss-free warmup\n", None),
])
def test_watch_observation_window(cluster, idea: Path, initialized: Path, arb, log,
                                  pattern) -> None:
    cluster.script(jn(cluster, "r001", 1), run_s=7200, log="step 1 loss 2.5\n" + log)
    arb(idea, "submit", "--reason", "x", "--gpus", "4", "--time", "60h", *CMD)
    arb(idea, "approve", "r001")
    cluster.advance(120)
    res = arb(idea, "watch", "--once")
    run = _run(initialized)
    notes = initialized / ".arboria" / "notifications.log"
    if pattern is not None:
        assert run["status"] == "failed"
        line = log.strip()
        assert f"r001 失败：s1 日志出现异常（匹配 {pattern}）：{line}，已取消剩余作业" in res.stdout
        assert f"（匹配 {pattern}）：{line}" in notes.read_text()
        assert sorted(cluster.calls("cancel")[0][1]) == ["1001", "1002"]
        assert [j["state"] for j in _jobs(initialized)] == ["cancelled", "cancelled"]
        with _ledger(initialized) as l:
            ev = l.one("SELECT payload_json FROM events WHERE kind = 'run_final'")
        payload = json.loads(ev["payload_json"])
        assert (payload["pattern"], payload["line"], payload["segment"]) == (pattern, line, 1)
        assert payload["by"] == "arb watch" and payload["job_id"] == "1001"
    else:
        assert run["status"] == "running" and cluster.calls("cancel") == []
        assert len(cluster.calls("tail")) == 1


def test_watch_hit_line_is_truncated(cluster, idea: Path, initialized: Path, arb) -> None:
    long = "step 7 loss nan " + "x" * 400
    cluster.script(jn(cluster, "r001", 1), run_s=7200, log=long + "\n")
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    cluster.advance(60)
    out = arb(idea, "watch", "--once").stdout
    assert long[:200] + "，已取消剩余作业" in out and long[:201] not in out


def _set_watch(main: Path, body: str) -> None:
    toml = main / "arboria.toml"
    text = toml.read_text()
    toml.write_text(text[:text.index("[watch]")] + "[watch]\n" + body)


def test_watch_custom_patterns_and_window(cluster, idea: Path, initialized: Path, arb) -> None:
    _set_watch(initialized, "observation_minutes = 20\nfail_patterns = ['^Traceback ']\n")
    cluster.script(jn(cluster, "r001", 1), run_s=7200, log="step 1 loss nan\n")
    cluster.script(jn(cluster, "r002", 1), run_s=7200,
                   log="step 1 loss 2.0\nTraceback (most recent call last):\n")
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    _bump(idea)
    _submit(arb, idea)
    arb(idea, "approve", "r002")
    cluster.advance(900)  # past the default 10 minutes, inside the configured 20
    out = arb(idea, "watch", "--once").stdout
    assert _run(initialized, "r001")["status"] == "running"  # defaults replaced
    assert _run(initialized, "r002")["status"] == "failed"
    assert "r002 失败：s1 日志出现异常（匹配 ^Traceback ）：Traceback (most recent call last):" in out
    assert len(cluster.calls("tail")) == 2


def test_watch_invalid_pattern_errors_clearly(cluster, idea: Path, initialized: Path, arb) -> None:
    _set_watch(initialized, "fail_patterns = ['loss', '(?i)loss[ nan']\n")
    res = arb(idea, "watch", "--once", ok=False)
    assert res.exit_code == 1
    assert "[watch] fail_patterns[1] '(?i)loss[ nan' is not a valid regular expression" in res.stderr
    assert "arboria.toml" in res.stderr


def test_watch_ignores_logs_after_window(cluster, idea: Path, initialized: Path, arb) -> None:
    cluster.script(jn(cluster, "r001", 1), run_s=7200, log="loss nan\n")
    _submit(arb, idea)
    arb(idea, "approve", "r001")
    cluster.advance(700)
    arb(idea, "watch", "--once")
    assert _run(initialized)["status"] == "running" and cluster.calls("tail") == []
    arb(idea, "watch", "--once", "--window", "15")
    assert _run(initialized)["status"] == "failed"


def test_watch_pauses_on_need_login(cluster, idea: Path, initialized: Path, monkeypatch) -> None:
    from arboria import human
    from arboria.cli import app  # noqa: F401  (import check only)

    monkeypatch.setattr(human, "read_confirmation", lambda prompt: "r001")
    ctx = locate(idea)
    with _ledger(initialized) as l:
        run, card = submission.submit(ctx, l, l.one("SELECT * FROM ideas"), "x",
                                      ["python", "t.py"], gpus=4, time="1h")
        submission.approve(ctx, l, run["id"])
        cluster.set_need_login()
        echoed: list[str] = []
        sleeps: list[float] = []
        code = monitor.watch(ctx, l, interval=60, echo=echoed.append, sleep=sleeps.append,
                             max_iterations=5)
    assert code == 1 and sleeps == [60] * 4
    assert len([e for e in echoed if "Duo" in e]) == 1
    assert len(cluster.calls("poll")) == 1  # one attempt, then paused: no retry storm
    log = (initialized / ".arboria" / "notifications.log").read_text()
    assert log.count("请先在终端运行 `ssh fake` 完成 Duo 认证") == 1


# ---------------------------------------------------------------- logs / remote ls / status


def test_logs_and_remote_ls(cluster, idea: Path, initialized: Path, arb) -> None:
    cluster.script(jn(cluster, "r001", 1), run_s=7200, log="".join(f"line {i}\n" for i in range(300)))
    _submit(arb, idea)
    assert "no submitted jobs" in arb(idea, "logs", "r001", ok=False).stderr
    arb(idea, "approve", "r001")
    out = arb(idea, "logs", "r001", "-n", "3").stdout
    assert out == "line 297\nline 298\nline 299\n"
    assert len(arb(idea, "logs", "r001", "--segment", "1").stdout.splitlines()) == 100
    assert "no submitted segment 2" in arb(idea, "logs", "r001", "--segment", "2", ok=False).stderr
    out = arb(idea, "remote", "ls", "r001").stdout
    assert out.splitlines()[1:] == ["  code", "  logs", "  sbatch"]
    assert "  train.py" in arb(idea, "remote", "ls", "r001", "code").stdout
    assert "invalid subpath" in arb(idea, "remote", "ls", "r001", "../..", ok=False).stderr
    assert "no such directory" in arb(idea, "remote", "ls", "r001", "--out", ok=False).stderr


def test_pending_approvals_in_status_and_hook(cluster, idea: Path, initialized: Path, arb) -> None:
    _submit(arb, idea)
    _bump(idea)
    _submit(arb, idea)
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert st["pending_approvals"] == 2
    assert [d["question"] for d in st["open_decisions"]] == ["批准这个 run 上集群吗？"] * 2
    out = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert "待你批准的 run：2 个" in ctx and "you cannot run `arb approve`" in ctx
    assert "看板：arb ui" in ctx
    assert "待你批准的 run：2 个" in arb(initialized, "status").stdout
    arb(idea, "reject", "r002")
    assert "待你批准的 run：1 个" in arb(idea, "status", "--brief").stdout
