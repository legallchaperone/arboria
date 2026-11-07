"""The research tree (`arb tree`) and the SessionStart index built from it (ADR 0004)."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from arboria import hook, tree
from arboria.config import project_config
from arboria.errors import ArbError
from arboria.db import Ledger
from arboria.paths import locate


def _metrics(repo: Path, run: str, values: list[float], key: str = "val/loss") -> None:
    out = repo / ".arboria" / "runs" / run / "out"
    out.mkdir(parents=True, exist_ok=True)
    rows = [{"step": i * 10, key: v, "lr": 0.1} for i, v in enumerate(values)]
    (out / "metrics.jsonl").write_text("\n".join(json.dumps(r) for r in rows) + "\nnot json\n")


def _hook_ctx(arb, cwd: Path) -> str:
    out = arb(cwd, "hook", "session-start", input=json.dumps({"cwd": str(cwd)})).stdout
    return json.loads(out)["hookSpecificOutput"]["additionalContext"]


def _grow(arb, idea: Path, initialized: Path) -> Path:
    """exp1 with three runs (r002 best), a child exp2 with one run, a dropped exp3."""
    for reason in ("baseline", "warmup 500 steps", "warmup 2000 steps"):
        arb(idea, "run", "new", "--reason", reason)
    _metrics(initialized, "r001", [3.0, 2.5])
    _metrics(initialized, "r002", [3.0, 2.2])
    _metrics(initialized, "r003", [3.0, 2.4])
    arb(idea, "note", "--run", "r002", "warmup 有用，但 2000 太长")
    arb(idea, "run", "summary", "r003", "比 r002 差 0.2。可能是 warmup 太长。")
    res = arb(initialized, "idea", "new", "exp2", "--hypothesis", "cosine decay on top",
              "--metric", "val/loss ↓", "--parent", "exp1")
    exp2 = Path(res.stdout.strip())
    arb(exp2, "run", "new", "--reason", "cosine")
    _metrics(initialized, "r004", [2.9, 2.1])
    arb(initialized, "idea", "new", "exp3", "--hypothesis", "bigger batch", "--metric", "val/loss")
    arb(initialized, "idea", "close", "exp3", "--drop")
    return exp2


def test_tree_lines(arb, idea: Path, initialized: Path) -> None:
    exp2 = _grow(arb, idea, initialized)
    out = arb(exp2, "tree").stdout
    assert "3 个 idea（2 个进行中）· 4 个 run" in out
    assert "◆ exp1 [进行中] · 最好 val/loss 2.2（r002） · 3 个 run" in out
    assert "r002 ← r001 [draft] 改：warmup 500 steps → val/loss 2.2（−0.3） · 人：warmup 有用" in out
    assert "r003 ← r002 [draft] 改：warmup 2000 steps → val/loss 2.4（+0.2） · AI 摘要：比 r002 差 0.2。" in out
    assert "可能是" not in out  # only the summary's first sentence
    assert "  ◆ exp2 [进行中] ← exp1 · 最好 val/loss 2.1（r004） · 1 个 run" in out and "← 你在这里" in out
    assert "◆ exp3 [已放弃]" in out
    assert out.index("exp1") < out.index("exp2") < out.index("exp3")  # by id
    assert arb(exp2, "tree").stdout == out  # stable

    sub = arb(initialized, "tree", "--idea", "exp2").stdout
    assert "exp2" in sub and "◆ exp1" not in sub and "你在这里" not in sub
    shallow = arb(initialized, "tree", "--depth", "1").stdout
    assert "◆ exp2" not in shallow and "1 个子 idea" in shallow

    data = json.loads(arb(exp2, "tree", "--json").stdout)
    assert data["here"] == "exp2" and data["conclusion_sources"][:2] == ["verdict", "ai_oneline"]
    e1 = data["ideas"][0]
    assert e1["metric_key"] == "val/loss" and e1["goal"] == "min" and e1["best"] == "r002"
    assert [r["conclusion_source"] for r in e1["runs"]] == [None, "note", "summary"]
    assert e1["children"][0]["name"] == "exp2"


def test_metric_key_and_goal() -> None:
    keys = {"loss", "val_loss", "val_acc", "lr"}
    assert tree.metric_key("val_loss, lower is better", keys) == "val_loss"
    assert tree.metric_key("top-1 val_acc ↑", keys) == "val_acc"
    assert tree.metric_key("perplexity", keys) is None
    assert tree.infer_goal("val_acc", "val_acc") == "max"
    assert tree.infer_goal("val_acc 越低越好?", "val_acc") == "min"
    assert tree.infer_goal("loss", "val_loss") == "min"
    assert tree.infer_goal("reward ↑", None) == "max"


def test_project_metric_overrides(arb, idea: Path, initialized: Path) -> None:
    arb(idea, "run", "new", "--reason", "a")
    arb(idea, "run", "new", "--reason", "b")
    _metrics(initialized, "r001", [0.5], key="acc")
    _metrics(initialized, "r002", [0.7], key="acc")
    cfg = initialized / "arboria.toml"
    cfg.write_text(cfg.read_text().replace("[project]\n", '[project]\nmetric = "acc"\ngoal = "max"\n', 1))
    out = arb(idea, "tree").stdout
    assert "结果 = metrics.jsonl 的 acc（越高越好）" in out and "最好 acc 0.7（r002）" in out
    cfg.write_text(cfg.read_text().replace('goal = "max"', 'goal = "up"'))
    with pytest.raises(ArbError, match="goal must be"):
        project_config(locate(idea))
    assert "◆ exp1" in arb(idea, "tree").stdout  # a broken config never hides the tree


def test_hook_gives_the_index(arb, idea: Path, initialized: Path) -> None:
    exp2 = _grow(arb, idea, initialized)
    ctx = _hook_ctx(arb, exp2)
    assert "你在这里：exp1 → exp2" in ctx and "← 你在这里" in ctx
    assert "上一层：◆ exp1" in ctx and "r004 [draft] 改：cosine → val/loss 2.1" in ctx
    assert "最好的结果：\n- exp2/r004 val/loss 2.1" in ctx and "- exp1/r002 val/loss 2.2" in ctx
    assert "`arb tree`" in ctx and "arb lesson propose" in ctx  # footer kept

    main = _hook_ctx(arb, initialized)
    assert "你在主工作区" in main and "- ◆ exp1" in main and "- ◆ exp2" in main
    assert "◆ exp3" not in main.split("最好的结果")[0]  # only open ideas listed


def test_index_and_tree_stay_small_at_100_runs(arb, idea: Path, initialized: Path) -> None:
    ctx = locate(initialized)
    with Ledger(ctx.state_dir) as ledger, ledger.write() as conn:
        exp1 = ledger.one("SELECT * FROM ideas WHERE name = 'exp1'")
        for n in range(1, 101):
            rid = f"r{n:03d}"
            conn.execute(
                "INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, status,"
                " summary, created_at, updated_at) VALUES (?, ?, 'c', 't', ?, 'succeeded', ?, 'x', 'x')",
                (rid, exp1["id"], f"变体 {n}：" + "调参 " * 30, "结果还行。" + "细节" * 200))
            _metrics(initialized, rid, [3.0, 2.0 + (n % 17) / 10])
    with Ledger(ctx.state_dir) as ledger:
        t = tree.build(locate(idea), ledger)
    idx = tree.index(t, "exp1")
    assert len(idx) <= tree.INDEX_CHARS and "更早的 95 个 run" in idx
    full = tree.render(t, here="exp1")
    assert len(full) <= tree.TREE_CHARS
    assert len(_hook_ctx(arb, idea)) <= hook.MAX_CONTEXT_CHARS


def test_hook_falls_back_to_status_when_the_tree_breaks(arb, idea: Path, initialized: Path,
                                                         monkeypatch) -> None:
    def boom(*a, **k):
        raise RuntimeError("tree broke")

    monkeypatch.setattr(tree, "build", boom)
    from io import StringIO

    out = StringIO()
    assert hook.session_start(StringIO(json.dumps({"cwd": str(idea)})), out) == 0
    ctx = json.loads(out.getvalue())["hookSpecificOutput"]["additionalContext"]
    assert "[arboria] idea `exp1`" in ctx and "Hypothesis: warmup helps" in ctx
    assert "tree broke" in (initialized / ".arboria" / "hook.log").read_text()
