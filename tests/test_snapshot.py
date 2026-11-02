"""The snapshot must capture everything without touching branch, index or working tree."""

from __future__ import annotations

from pathlib import Path

import pytest

from arboria import snapshot
from arboria.db import Ledger
from arboria.paths import locate

from .conftest import git, snapshot_files


def _dirty(wt: Path) -> None:
    """Every kind of change: modified, staged, deleted, untracked, nested untracked, ignored."""
    (wt / "train.py").write_text("print('train v2')\n")  # modified, unstaged
    (wt / "staged.py").write_text("x = 1\n")
    git(wt, "add", "staged.py")  # staged new file
    (wt / "README.md").write_text("# changed and staged\n")
    git(wt, "add", "README.md")
    (wt / "README.md").write_text("# changed again, unstaged\n")  # staged + further unstaged edit
    (wt / "untracked.py").write_text("y = 2\n")
    (wt / "pkg" / "deep").mkdir(parents=True)
    (wt / "pkg" / "deep" / "mod.py").write_text("z = 3\n")
    (wt / "model.ckpt").write_bytes(b"\0" * 1000)  # ignored
    (wt / "outputs").mkdir()
    (wt / "outputs" / "log.txt").write_text("ignored dir\n")


def _state(wt: Path) -> dict:
    index_path = Path(git(wt, "rev-parse", "--path-format=absolute", "--git-path", "index"))
    return {
        "head": git(wt, "rev-parse", "HEAD"),
        "branch": git(wt, "symbolic-ref", "HEAD"),
        "cached": git(wt, "diff", "--cached"),
        "unstaged": git(wt, "diff"),
        "status": git(wt, "status", "--porcelain", "--untracked-files=all"),
        "index_bytes": index_path.read_bytes(),
        "files": snapshot_files(wt),
        "branches": git(wt, "for-each-ref", "refs/heads"),
    }


def _tree_files(wt: Path, commit: str) -> dict[str, str]:
    names = git(wt, "ls-tree", "-r", "--name-only", commit).splitlines()
    return {n: git(wt, "show", f"{commit}:{n}") for n in names}


def test_snapshot_captures_everything_and_touches_nothing(idea: Path, arb) -> None:
    _dirty(idea)
    git(idea, "rm", "-q", "--cached", ".gitignore")  # staged deletion from the index...
    (idea / ".gitignore").write_text("*.ckpt\noutputs/\n")  # ...but file still on disk
    before = _state(idea)

    run_id = arb(idea, "run", "new", "--reason", "try warmup").stdout.strip()
    assert run_id == "r001"

    after = _state(idea)
    assert after == before, "snapshot modified branch/index/working tree"

    ctx = locate(idea)
    with Ledger(ctx.state_dir) as ledger:
        run = ledger.one("SELECT * FROM runs WHERE id = 'r001'")
    commit = run["snapshot_commit"]
    files = _tree_files(idea, commit)
    assert files["train.py"] == "print('train v2')"
    assert files["staged.py"] == "x = 1"
    assert files["README.md"] == "# changed again, unstaged"
    assert files["untracked.py"] == "y = 2"
    assert files["pkg/deep/mod.py"] == "z = 3"
    assert ".gitignore" in files  # present on disk => in snapshot
    assert "model.ckpt" not in files and not any(f.startswith("outputs/") for f in files)

    # parent is the branch HEAD; tree hash recorded; ref created
    assert git(idea, "rev-parse", f"{commit}^") == before["head"]
    assert git(idea, "rev-parse", f"{commit}^{{tree}}") == run["tree_hash"]
    assert git(idea, "rev-parse", "refs/arboria/runs/r001") == commit
    assert git(idea, "log", "-1", "--format=%s", commit) == "arboria snapshot r001"


def test_deleted_tracked_file_is_absent(idea: Path, arb) -> None:
    (idea / "train.py").unlink()
    arb(idea, "run", "new", "--reason", "remove train")
    commit = git(idea, "rev-parse", "refs/arboria/runs/r001")
    assert "train.py" not in _tree_files(idea, commit)
    assert not (idea / "train.py").exists()
    assert "train.py" in git(idea, "ls-files")  # still tracked in the real index


def test_identical_state_reuses_commit(idea: Path, arb) -> None:
    (idea / "new.py").write_text("a = 1\n")
    arb(idea, "run", "new", "--reason", "first")
    arb(idea, "run", "new", "--reason", "same state again")
    c1 = git(idea, "rev-parse", "refs/arboria/runs/r001")
    c2 = git(idea, "rev-parse", "refs/arboria/runs/r002")
    assert c1 == c2

    (idea / "new.py").write_text("a = 2\n")
    arb(idea, "run", "new", "--reason", "changed")
    c3 = git(idea, "rev-parse", "refs/arboria/runs/r003")
    assert c3 != c1

    (idea / "new.py").write_text("a = 1\n")  # back to the first state
    res = arb(idea, "run", "new", "--reason", "back", "--json")
    assert '"reused_snapshot": true' in res.stdout
    assert git(idea, "rev-parse", "refs/arboria/runs/r004") == c1


def test_snapshot_committed_state_equals_head_tree(idea: Path) -> None:
    ctx = locate(idea)
    with Ledger(ctx.state_dir) as ledger:
        res = snapshot.snapshot(idea, ledger.conn, "m")
    assert res.tree == git(idea, "rev-parse", "HEAD^{tree}")
    assert not res.reused


def test_large_untracked_files_refused_unless_allowed(idea: Path, arb) -> None:
    ctx = locate(idea)
    (idea / "big.bin").write_bytes(b"x" * 3000)
    (idea / "small.bin").write_bytes(b"x" * 10)
    with Ledger(ctx.state_dir) as ledger:
        with pytest.raises(snapshot.LargeUntrackedError, match="big.bin"):
            snapshot.snapshot(idea, ledger.conn, "m", file_limit=1000, total_limit=10_000)
        with pytest.raises(snapshot.LargeUntrackedError, match="total"):
            snapshot.snapshot(idea, ledger.conn, "m", file_limit=10_000, total_limit=2000)
        warned: list[str] = []
        res = snapshot.snapshot(idea, ledger.conn, "m", file_limit=1000, total_limit=10_000,
                                allow_large=True, warn=warned.append)
    assert warned and "big.bin" in warned[0]
    assert "big.bin" in git(idea, "ls-tree", "-r", "--name-only", res.commit)
    # ignored large files never count
    (idea / "big.bin").unlink()
    (idea / "huge.ckpt").write_bytes(b"x" * 5000)
    with Ledger(ctx.state_dir) as ledger:
        snapshot.snapshot(idea, ledger.conn, "m", file_limit=1000, total_limit=2000)


def test_large_check_via_cli(idea: Path, arb, monkeypatch) -> None:
    monkeypatch.setattr(snapshot, "DEFAULT_FILE_LIMIT", 100)
    (idea / "data.bin").write_bytes(b"x" * 500)
    res = arb(idea, "run", "new", "--reason", "big", ok=False)
    assert res.exit_code == 1
    assert "--allow-large" in res.stderr
    assert git(idea, "for-each-ref", "refs/arboria/runs") == ""
    res = arb(idea, "run", "new", "--reason", "big", "--allow-large")
    assert res.stdout.strip() == "r001"
    assert "warning: large untracked file data.bin" in res.stderr


def test_snapshot_from_subdirectory(idea: Path, arb) -> None:
    sub = idea / "pkg"
    sub.mkdir()
    (sub / "a.py").write_text("a\n")
    (idea / "top.py").write_text("t\n")
    arb(sub, "run", "new", "--reason", "from subdir")
    files = _tree_files(idea, "refs/arboria/runs/r001")
    assert "pkg/a.py" in files and "top.py" in files
