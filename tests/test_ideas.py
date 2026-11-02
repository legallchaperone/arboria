"""`arb idea new/list/close`."""

from __future__ import annotations

import json
from pathlib import Path

from .conftest import git


def test_idea_new_creates_branch_and_worktree(initialized: Path, arb) -> None:
    res = arb(initialized, "idea", "new", "lr-warmup", "--hypothesis", "warmup helps",
              "--metric", "val/loss")
    wt = Path(res.stdout.strip())
    assert wt == initialized.parent / "proj-lr-warmup"
    assert wt.is_dir() and (wt / "train.py").exists()
    assert git(wt, "symbolic-ref", "--short", "HEAD") == "idea/lr-warmup"
    assert git(wt, "rev-parse", "HEAD") == git(initialized, "rev-parse", "HEAD")
    assert git(initialized, "symbolic-ref", "--short", "HEAD") == "main"  # main untouched

    rows = json.loads(arb(initialized, "idea", "list", "--json").stdout)
    assert len(rows) == 1
    r = rows[0]
    assert (r["name"], r["status"], r["confidence"], r["branch"]) == \
        ("lr-warmup", "open", "guess", "idea/lr-warmup")
    assert r["worktree_path"] == str(wt)
    assert r["created_at"].endswith("Z")
    # backup committed
    assert "idea_new lr-warmup" in git(initialized / ".arboria", "log", "-1", "--format=%s")


def test_idea_new_from_parent_branch(idea: Path, initialized: Path, arb) -> None:
    (idea / "feature.py").write_text("f\n")
    git(idea, "add", "feature.py")
    git(idea, "commit", "-q", "-m", "feature")
    res = arb(idea, "idea", "new", "child", "--hypothesis", "h", "--metric", "m", "--parent", "exp1")
    child = Path(res.stdout.strip())
    assert child == initialized.parent / "proj-child"
    assert (child / "feature.py").exists()
    assert git(child, "rev-parse", "HEAD") == git(idea, "rev-parse", "HEAD")
    rows = {r["name"]: r for r in json.loads(arb(initialized, "idea", "list", "--json").stdout)}
    assert rows["child"]["parent_name"] == "exp1"


def test_idea_new_validation(idea: Path, initialized: Path, arb) -> None:
    cases = [
        (["Bad_Name"], "invalid idea name"),
        (["exp1"], "already exists"),
        (["ok", "--parent", "nope"], "no idea named"),
    ]
    for extra, msg in cases:
        res = arb(initialized, "idea", "new", *extra[:1], "--hypothesis", "h", "--metric", "m",
                  *extra[1:], ok=False)
        assert res.exit_code == 1 and msg in res.stderr, (extra, res.stderr)
    res = arb(initialized, "idea", "new", "long", "--hypothesis", "x" * 301, "--metric", "m", ok=False)
    assert "hypothesis is too long" in res.stderr
    assert not (initialized.parent / "proj-long").exists()
    (initialized.parent / "proj-taken").mkdir()
    res = arb(initialized, "idea", "new", "taken", "--hypothesis", "h", "--metric", "m", ok=False)
    assert "already exists" in res.stderr
    assert git(initialized, "branch", "--list", "idea/taken") == ""


def test_idea_list_text(idea: Path, initialized: Path, arb) -> None:
    out = arb(initialized, "idea", "list").stdout
    assert "exp1" in out and "open" in out and "warmup helps" in out


def test_close_drop_archives_final_snapshot(idea: Path, initialized: Path, arb) -> None:
    (idea / "wip.py").write_text("unsaved work\n")
    (idea / "train.py").write_text("modified\n")
    main_head = git(initialized, "rev-parse", "HEAD")
    res = arb(idea, "idea", "close", "--drop", "--confidence", "rejected")  # name from cwd
    assert "dropped exp1" in res.stdout

    final = git(initialized, "rev-parse", "refs/arboria/ideas/exp1/final")
    assert git(initialized, "rev-parse", "arboria/archive/exp1") == final
    assert git(initialized, "show", f"{final}:wip.py") == "unsaved work"
    assert git(initialized, "show", f"{final}:train.py") == "modified"
    assert not idea.exists()
    assert git(initialized, "branch", "--list", "idea/exp1") == ""
    assert str(idea) not in git(initialized, "worktree", "list")
    assert git(initialized, "rev-parse", "HEAD") == main_head

    row = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert row["status"] == "dropped" and row["confidence"] == "rejected" and row["closed_at"]
    res = arb(initialized, "idea", "close", "exp1", "--drop", ok=False)
    assert "already dropped" in res.stderr


def test_close_drop_reuses_run_snapshot(idea: Path, initialized: Path, arb) -> None:
    (idea / "x.py").write_text("x\n")
    arb(idea, "run", "new", "--reason", "r")
    run_commit = git(idea, "rev-parse", "refs/arboria/runs/r001")
    arb(initialized, "idea", "close", "exp1", "--drop")
    assert git(initialized, "rev-parse", "refs/arboria/ideas/exp1/final") == run_commit
    assert git(initialized, "cat-file", "-t", "refs/arboria/runs/r001") == "commit"


def test_close_merge(idea: Path, initialized: Path, arb) -> None:
    (idea / "feature.py").write_text("feature\n")
    res = arb(idea, "idea", "close", "--merge", ok=False)
    assert res.exit_code == 1 and "uncommitted changes" in res.stderr and "commit" in res.stderr
    assert idea.exists()

    git(idea, "add", "feature.py")
    git(idea, "commit", "-q", "-m", "add feature")
    idea_head = git(idea, "rev-parse", "HEAD")
    arb(idea, "idea", "close", "--merge", "--confidence", "confirmed")

    assert (initialized / "feature.py").read_text() == "feature\n"
    parents = git(initialized, "log", "-1", "--format=%P").split()
    assert len(parents) == 2 and parents[1] == idea_head  # --no-ff merge commit
    assert git(initialized, "symbolic-ref", "--short", "HEAD") == "main"
    assert not idea.exists()
    assert git(initialized, "branch", "--list", "idea/exp1") == ""
    row = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert (row["status"], row["confidence"]) == ("merged", "confirmed")


def test_close_merge_conflict_aborts(idea: Path, initialized: Path, arb) -> None:
    (idea / "train.py").write_text("idea version\n")
    git(idea, "commit", "-qam", "idea change")
    (initialized / "train.py").write_text("main version\n")
    git(initialized, "commit", "-qam", "main change")
    main_head = git(initialized, "rev-parse", "HEAD")

    res = arb(initialized, "idea", "close", "exp1", "--merge", ok=False)
    assert res.exit_code == 1 and "conflict" in res.stderr and "aborted" in res.stderr
    assert git(initialized, "rev-parse", "HEAD") == main_head
    assert git(initialized, "status", "--porcelain") == "?? arboria.toml"
    assert (initialized / "train.py").read_text() == "main version\n"
    assert idea.exists()
    row = json.loads(arb(initialized, "idea", "list", "--json").stdout)[0]
    assert row["status"] == "open"


def test_close_requires_mode_and_name(idea: Path, initialized: Path, arb) -> None:
    res = arb(idea, "idea", "close", ok=False)
    assert "exactly one of --merge or --drop" in res.stderr
    res = arb(idea, "idea", "close", "--merge", "--drop", ok=False)
    assert "exactly one" in res.stderr
    res = arb(initialized, "idea", "close", "--drop", ok=False)
    assert "not inside an idea worktree" in res.stderr
    res = arb(idea, "idea", "close", "--drop", "--confidence", "sure", ok=False)
    assert "invalid confidence" in res.stderr


def test_close_drop_when_worktree_deleted_by_hand(idea: Path, initialized: Path, arb) -> None:
    import shutil

    tip = git(idea, "rev-parse", "HEAD")
    shutil.rmtree(idea)
    arb(initialized, "idea", "close", "exp1", "--drop")
    assert git(initialized, "rev-parse", "arboria/archive/exp1") == tip
