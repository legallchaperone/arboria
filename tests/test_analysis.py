"""The built-in analyses: lab.plot, lab.diff and lab.spend (analysis.py)."""

from __future__ import annotations

import json
import math
from pathlib import Path

import pytest

from arboria import analysis, lab
from arboria.db import Ledger
from arboria.errors import ArbError

from . import uiutil
from .conftest import git

@pytest.fixture(autouse=True)
def no_pinned_idea():
    lab.use(None)
    yield
    lab.use(None)


# ---------------------------------------------------------------- lab.plot


def _history(run_id: str):
    base = {"r001": 2.0, "r002": 2.4, "r003": 1.8}[run_id]
    return [{"_step": s * 100, "val/loss": base * math.exp(-s / 20) + 0.3} for s in range(60)]


def test_plot_draws_one_line_per_run(idea: Path, initialized: Path, arb, monkeypatch) -> None:
    monkeypatch.chdir(idea)
    for reason in ("a", "b"):
        arb(idea, "run", "new", "--reason", reason)
        (idea / f"{reason}.txt").write_text(reason)
    arb(idea, "run", "new", "--reason", "c")
    calls = []

    def history(self, keys):
        calls.append((self.id, keys))
        if self.id == "r003":
            raise ArbError("run r003 has no wandb link")
        return _history(self.id) + [{"_step": 6000, "val/loss": float("nan")},
                                    {"_step": 6100, "val/loss": None}]

    monkeypatch.setattr(lab.Run, "history", history)
    p = lab.plot(["r001", lab.get("r002"), "r003"], "val/loss")
    assert calls[0] == ("r001", ["val/loss", "_step"])
    assert [s.run for s in p.series] == ["r001", "r002"] and len(p.series[0].points) == 60
    assert p.skipped == [("r003", "run r003 has no wandb link")]
    assert len(p.data) == 120 and p.data[0] == {"run": "r001", "_step": 0.0, "val/loss": 2.3}
    page = p._repr_html_()
    assert page.count("<path ") == 2 and page.count("<svg") == 1
    assert analysis.SERIES_COLORS[0] in page and analysis.SERIES_COLORS[1] in page
    assert page.count(">r001<") == 1 and "r003：run r003 has no wandb link" in page
    text = str(p)
    assert text.splitlines()[0] == "val/loss" and "r001：60 个点，最后 0.4" in text

    log = lab.plot("r001", "val/loss", log=True)
    assert "对数" in log._repr_html_() and log.svg().count("<path ") == 1
    with pytest.raises(ArbError, match="no wandb link"):
        lab.plot(["r003"], "val/loss")
    with pytest.raises(ArbError, match="wandb 指标名"):
        lab.plot("r001", "")


def test_plot_helpers() -> None:
    assert analysis.nice_ticks(0, 1, 5) == [0, 0.2, 0.4, 0.6, 0.8, 1.0]
    assert analysis.nice_ticks(0, 5800, 5) == [0, 2000, 4000]
    assert analysis.fmt_num(12000) == "12k" and analysis.fmt_num(0.000012) == "1.2e-05"
    assert [analysis.fmt_step(v, 10000) for v in (0, 2500, 10000)] == ["0", "2.5k", "10k"]
    assert analysis.fmt_step(2500, 5000) == "2500"
    many = [(f"r{i:03d}", [{"_step": 0, "x": 1}]) for i in range(9)]
    with pytest.raises(ArbError, match="最多 8 个 run"):
        analysis.make_plot("x", many)
    thin = analysis._thin([(float(i), 1.0) for i in range(5000)])
    assert len(thin) <= analysis.MAX_POINTS + 1 and thin[-1] == (4999.0, 1.0)
    flat = analysis.make_plot("x", [("r001", [{"_step": 0, "x": 1.0}, {"_step": 1, "x": 1.0}])])
    assert "<path " in flat.svg()
    empty = analysis.make_plot("x", [("r001", [{"_step": 0, "y": 1.0}])])
    assert "没有可画的点" in empty._repr_html_() and "没有 x 的数据" in str(empty)


# ---------------------------------------------------------------- lab.diff and lab.spend


@pytest.fixture
def project(tmp_path_factory, monkeypatch, idea: Path, initialized: Path, arb) -> dict:
    cluster = uiutil.setup_cluster(tmp_path_factory.mktemp("cluster"), initialized, monkeypatch)
    out = uiutil.build(arb, initialized, idea, cluster)
    lab.use("exp1", repo=initialized)
    return out


def test_diff_shows_only_what_differs(project: dict, initialized: Path) -> None:
    head = git(initialized, "rev-parse", "HEAD")
    status = git(initialized, "status", "--porcelain")
    d = lab.diff("r001", "r002")  # same snapshot, lr changed, only r001's config fetched
    assert d.rows == [("--set", "lr", "3e-4", "1e-3"), ("config", "", "1 项", "还没拉回")]
    assert str(d).splitlines() == ["r001 → r002 · 2 处不同",
                                   "            r001  r002",
                                   "--set   lr  3e-4  1e-3",
                                   "config      1 项  还没拉回"]

    d = lab.diff(lab.get("r001"), "r004")  # new snapshot, other resources, no config yet
    groups = [r[0] for r in d.rows]
    assert ("--set", "lr", "3e-4", "—") in d.rows
    assert ("资源", "GPU", "4", "2") in d.rows and ("资源", "时长", "2d12h", "1d16h") in d.rows
    assert groups.count("代码") == 1 and ("config", "", "1 项", "还没拉回") in d.rows
    assert "bump0.txt | 1 +" in d.stat and "1 file changed" in d.stat
    assert not d.same_tree and bool(d)
    page = d._repr_html_()
    assert "bump0.txt" in page and page.count(">资源<") == 1

    same = lab.diff("r001", "r001")
    assert not same and str(same) == "r001 和 r001 没有不同（命令、资源、代码、config）"
    # read-only: the user's branch, index and worktree are untouched
    assert git(initialized, "rev-parse", "HEAD") == head
    assert git(initialized, "status", "--porcelain") == status


def test_diff_compares_configs_key_by_key(project: dict, initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as l, l.write() as c:
        c.execute("UPDATE runs SET config_json = ? WHERE id = 'r001'",
                  (json.dumps({"lr": 3e-4, "optim": {"name": "adam", "betas": [0.9, 0.99]},
                               "seed": 1}),))
        c.execute("UPDATE runs SET config_json = ? WHERE id = 'r002'",
                  (json.dumps({"lr": 1e-3, "optim": {"name": "adam", "betas": [0.9, 0.95]},
                               "warmup": 500}),))
    rows = [r for r in lab.diff("r001", "r002").rows if r[0] == "config"]
    assert rows == [("config", "lr", "0.0003", "0.001"),
                    ("config", "optim.betas", "[0.9, 0.99]", "[0.9, 0.95]"),
                    ("config", "seed", "1", "—"), ("config", "warmup", "—", "500")]


def test_spend_per_idea_and_per_run(project: dict, initialized: Path) -> None:
    s = lab.spend()
    by = {r["idea"]: r for r in s.rows}
    assert set(by) == {"exp1", "exp2"} and s.others == 2  # exp3, exp4 never ran
    from arboria.cards import su_used

    with Ledger(initialized / ".arboria") as l:
        assert s.total_su == su_used(l)
        assert by["exp1"]["su"] == su_used(l, l.one("SELECT id FROM ideas WHERE name = 'exp1'")["id"])
    assert by["exp1"]["runs"] == 3 and by["exp1"]["su"] > 190 + 47.5 + 0.7
    # r001: 4 GPUs × 60h; r002: 4 × 600s; r004 running 2 × (elapsed so far)
    assert by["exp1"]["gpu_h"] > 4 * 60
    assert s.budget == 400 and s.total_su == round(sum(r["su"] for r in s.rows), 2)
    text = str(s)
    assert text.splitlines()[0] == f"已用 {s.total_su:.2f} / 400 SU，剩 {400 - s.total_su:.2f}"
    assert "待批准的 run 最多还要" in text and "另有 2 个 idea 还没花过算力" in text
    assert text.splitlines()[1].split() == ["idea", "run", "GPU·h", "SU"]

    one = lab.spend(".")
    assert one.idea == "exp1" and [r["run"] for r in one.rows] == ["r001", "r002", "r004"]
    assert [r["状态"] for r in one.rows] == ["成功", "失败", "运行中"]
    assert one.rows[0]["gpu_h"] == pytest.approx(240.0)
    assert str(one).splitlines()[0] == f"exp1 · {by['exp1']['su']:.2f} SU"
    assert lab.spend("exp2").rows[0]["run"] == "r006"
    assert "<table" in one._repr_html_()
    with pytest.raises(ArbError, match="no idea named"):
        lab.spend("nope")


def test_spend_without_runs_or_budget(idea: Path, initialized: Path) -> None:
    lab.use("exp1", repo=initialized)
    s = lab.spend()
    assert s.rows == [] and str(s).splitlines() == [
        "已用 0.00 SU（arboria.toml 没设 [budget] total_su）", "另有 1 个 idea 还没花过算力"]

