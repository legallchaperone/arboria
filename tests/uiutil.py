"""A fixture project for the dashboard: several ideas (a fork from a run, a child that was
dropped, a merged one), runs in every state, a two-segment job, lessons, todos and
notifications. Built through `arb` and FakeSlurm, then its timestamps are spread over
ten days so the time axis means something."""

from __future__ import annotations

import sqlite3
import subprocess
from datetime import datetime, timedelta, timezone
from pathlib import Path

from arboria import ideas, records, slurm, tasks
from arboria.backends.fake import FakeSlurm
from arboria.clusters import load_profile
from arboria.config import project_config
from arboria.db import Ledger
from arboria.paths import locate

from .clusterutil import fake_toml

XSS = "<script>alert(1)</script> 试试转义"
CMD = ["--", "python", "train.py"]


def _git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def setup_cluster(base: Path, main: Path, monkeypatch) -> FakeSlurm:
    cfg = base / "config"
    cfg.mkdir(parents=True)
    (cfg / "clusters.toml").write_text(fake_toml(base))
    monkeypatch.setenv("ARBORIA_CONFIG_DIR", str(cfg))
    toml = main / "arboria.toml"
    toml.write_text(toml.read_text().replace('default_machine = ""', 'default_machine = "fake"')
                    .replace("total_su = 0", "total_su = 400"))
    return FakeSlurm(load_profile("fake"), project_config(locate(main)).project_id)


def build(arb, main: Path, exp1: Path, cluster: FakeSlurm) -> dict:
    jn = lambda run, k: slurm.job_name(cluster.project, run, k)  # noqa: E731

    def bump(wt: Path) -> None:
        n = len(list(wt.glob("bump*.txt")))
        (wt / f"bump{n}.txt").write_text(f"{n}\n")

    # exp1: r001 two segments, succeeded
    cluster.script(jn("r001", 1), run_s=48 * 3600, billed=190.0,
                   out_files={"metrics.json": '{"val_loss": 2.0}', "config.yaml": "lr: 0.0003\n"})
    cluster.script(jn("r001", 2), run_s=12 * 3600, billed=47.5)
    arb(exp1, "submit", "--reason", "baseline 60h", "--gpus", "4", "--time", "60h",
        "--set", "lr=3e-4", *CMD)
    arb(exp1, "approve", "r001")
    for _ in range(3):
        cluster.advance(30 * 3600)
        arb(exp1, "poll")
    arb(exp1, "run", "summary", "r001", "val/loss 2.0，比上一版好\nlr 3e-4 稳定")
    # r002: failed
    cluster.script(jn("r002", 1), state="failed", run_s=600, billed=0.7)
    arb(exp1, "submit", "--from", "r001", "--set", "lr=1e-3", "--reason", "lr 大一点")
    arb(exp1, "approve", "r002")
    cluster.advance(700)
    arb(exp1, "poll")
    # r003: awaiting approval (from r001, longer)
    arb(exp1, "submit", "--from", "r001", "--set", "lr=1e-4", "--time", "24h",
        "--reason", "lr 小一点")
    # r004: running
    cluster.script(jn("r004", 1), run_s=40 * 3600, queue_s=60)
    bump(exp1)
    arb(exp1, "submit", "--reason", "加 warmup", "--gpus", "2", "--time", "40h", *CMD)
    arb(exp1, "approve", "r004")
    cluster.advance(3600)
    arb(exp1, "poll")
    # r005: a draft
    arb(exp1, "run", "new", "--reason", "只是快照")
    arb(exp1, "note", "baseline 看起来不错", "--run", "r001", "--confidence", "prelim")

    # exp2: forked from r001's snapshot
    ctx = locate(main)
    with Ledger(ctx.state_dir) as ledger:
        run = records.get_run(ledger, "r001")
        exp2 = Path(ideas.new_idea(ctx, ledger, "exp2", "更长的 warmup", "val/loss",
                                   parent="exp1", from_run=run)["worktree_path"])
    bump(exp2)
    cluster.script(jn("r006", 1), run_s=10 * 3600)
    arb(exp2, "submit", "--reason", "warmup 5k", "--gpus", "4", "--time", "12h", *CMD)
    arb(exp2, "approve", "r006")
    cluster.advance(1800)
    arb(exp2, "poll")
    arb(exp2, "cancel", "r006")
    bump(exp2)
    arb(exp2, "submit", "--reason", XSS, "--gpus", "1", "--time", "1h", *CMD)
    arb(exp2, "reject", "r007", "--reason", "先别跑")

    # exp3: child of exp1, dropped; exp4: merged
    arb(main, "idea", "new", "exp3", "--hypothesis", "去掉 dropout", "--metric", "val/loss",
        "--parent", "exp1")
    arb(main, "idea", "close", "exp3", "--drop")
    res = arb(main, "idea", "new", "exp4", "--hypothesis", "更好的数据清洗",
              "--metric", "val/acc")
    exp4 = Path(res.stdout.strip())
    (exp4 / "clean.py").write_text("print('clean')\n")
    _git(exp4, "add", "clean.py")
    _git(exp4, "commit", "-q", "-m", "clean")
    arb(main, "idea", "close", "exp4", "--merge", "--confidence", "confirmed")

    # lessons, todos
    arb(exp1, "lesson", "propose", "eval 前先固定随机种子")
    arb(exp1, "lesson", "propose", "数据路径只从 ARBORIA_DATA 读")
    arb(main, "lesson", "accept", "l002")
    with Ledger(ctx.state_dir) as ledger:
        tasks.add(ledger, ideas.get_idea(ledger, "exp1"), "把 eval 改成每 500 步一次")
        tasks.add(ledger, ideas.get_idea(ledger, "exp2"), "命令 svd：画奇异值",
                  kind="command_new", data={"name": "svd", "scope": "project"})
    spread_times(main)
    return {"exp1": exp1, "exp2": exp2, "exp4": exp4}


def _ts(days_ago: float, now: datetime) -> str:
    return (now - timedelta(days=days_ago)).strftime("%Y-%m-%dT%H:%M:%SZ")


def spread_times(main: Path) -> None:
    """Rewrite timestamps (test-only, straight SQL) so the story spans ten days:
    the note on exp1 is after r001 ended but before r002 failed; exp2's only human note
    (the reject reason) is before r006 was cancelled."""
    now = datetime.now(timezone.utc)
    t = lambda d: _ts(d, now)  # noqa: E731
    con = sqlite3.connect(main / ".arboria" / "ledger.db")
    with con:
        for name, created, closed in (("exp1", 10, None), ("exp2", 6, None), ("exp3", 4, 2),
                                      ("exp4", 8, 3)):
            con.execute("UPDATE ideas SET created_at = ?, closed_at = ? WHERE name = ?",
                        (t(created), t(closed) if closed is not None else None, name))
        runs = {"r001": (9.5, 7), "r002": (5, 4.5), "r003": (1, 1), "r004": (0.5, 0.2),
                "r005": (0.3, 0.3), "r006": (5.5, 2), "r007": (4, 3.5)}
        for rid, (created, updated) in runs.items():
            con.execute("UPDATE runs SET created_at = ?, updated_at = ? WHERE id = ?",
                        (t(created), t(updated), rid))
            con.execute("UPDATE jobs SET submitted_at = ?, updated_at = ? WHERE run_id = ?",
                        (t(created), t(updated), rid))
            con.execute("UPDATE events SET ts = ? WHERE kind = 'run_final' AND subject = ?",
                        (t(updated), rid))
        con.execute("UPDATE jobs SET updated_at = ? WHERE run_id = 'r001' AND segment = 1",
                    (t(8),))
        con.execute("UPDATE notes SET created_at = ? WHERE text = 'baseline 看起来不错'", (t(6),))
        con.execute("UPDATE notes SET created_at = ? WHERE text = '先别跑'", (t(3.5),))
        con.execute("UPDATE tasks SET created_at = ?", (t(2),))
    con.close()
