"""One marimo notebook per idea, linked into the worktree and kept out of snapshots."""

from __future__ import annotations

import ast
import json
from pathlib import Path

from arboria import notebooks
from arboria.db import Ledger

from .conftest import git


def _nb(main: Path, name: str = "exp1") -> Path:
    return main / ".arboria" / "notebooks" / f"{name}.py"


def test_idea_new_creates_notebook_and_link(initialized: Path, arb) -> None:
    res = arb(initialized, "idea", "new", "exp1", "--hypothesis", "warmup helps",
              "--metric", "val/loss")
    wt = Path(res.stdout.strip())  # stdout stays just the worktree path
    nb = _nb(initialized)
    assert f"notebook: {nb}" in res.stderr
    text = nb.read_text()
    ast.parse(text)  # a plain Python (marimo) file
    assert text.startswith("import marimo\n") and "app = marimo.App(" in text
    assert "from arboria import lab" in text and "@app.cell" in text
    for needle in ("# exp1", "**假设：** warmup helps", "**指标：** val/loss",
                   f"**创建：** {json.loads(arb(initialized, 'idea', 'list', '--json').stdout)[0]['created_at'][:10]}"):
        assert needle in text, needle
    assert "父 idea" not in text
    link = wt / "notebook.py"
    assert link.is_symlink() and link.resolve() == nb.resolve()
    exclude = (initialized / ".git" / "info" / "exclude").read_text()
    assert exclude.count("\n/notebook.py\n") == 1
    assert git(wt, "status", "--porcelain", "--untracked-files=all") == ""
    # the notebook is in the backup commit of idea_new
    assert git(initialized / ".arboria", "show", "--name-only", "--format=", "HEAD").splitlines() \
        == ["backup/ledger.sql", "notebooks/exp1.py"]


def test_child_notebook_links_parent_and_quotes_safely(idea: Path, initialized: Path, arb) -> None:
    hyp = 'a """quoted""" claim ending in \\'
    arb(initialized, "idea", "new", "child", "--hypothesis", hyp, "--metric", "m",
        "--parent", "exp1")
    text = _nb(initialized, "child").read_text()
    tree = ast.parse(text)
    strings = [n.value for n in ast.walk(tree) if isinstance(n, ast.Constant)
               and isinstance(n.value, str)]
    assert any(f"**假设：** {hyp}" in s and "**父 idea：** exp1" in s for s in strings)
    assert initialized.parent.joinpath("proj-child", "notebook.py").is_symlink()
    assert (initialized / ".git" / "info" / "exclude").read_text().count("/notebook.py") == 1


def test_snapshot_never_includes_notebook(idea: Path, initialized: Path, arb) -> None:
    assert (idea / "notebook.py").is_symlink()
    (idea / "new.py").write_text("x\n")
    arb(idea, "run", "new", "--reason", "with notebook present")
    names = git(idea, "ls-tree", "-r", "--name-only", "refs/arboria/runs/r001").splitlines()
    assert "new.py" in names and "notebook.py" not in names
    # editing the notebook does not change the snapshot either
    _nb(initialized).write_text(_nb(initialized).read_text() + "\n# edited\n")
    arb(idea, "run", "new", "--reason", "notebook edited only")
    assert git(idea, "rev-parse", "refs/arboria/runs/r002") == \
        git(idea, "rev-parse", "refs/arboria/runs/r001")


def test_existing_tracked_notebook_py_is_not_clobbered(initialized: Path, arb) -> None:
    (initialized / "notebook.py").write_text("print('mine')\n")
    git(initialized, "add", "notebook.py")
    git(initialized, "commit", "-qm", "own notebook.py")
    res = arb(initialized, "idea", "new", "exp1", "--hypothesis", "h", "--metric", "m")
    wt = Path(res.stdout.strip())
    assert not (wt / "notebook.py").is_symlink()
    assert (wt / "notebook.py").read_text() == "print('mine')\n"
    assert "not linking the notebook" in res.stderr and _nb(initialized).is_file()


def test_close_keeps_notebook(idea: Path, initialized: Path, arb) -> None:
    nb = _nb(initialized)
    nb.write_text(nb.read_text() + "\n# my analysis\n")
    arb(initialized, "idea", "close", "exp1", "--drop")
    assert not idea.exists()
    assert nb.read_text().endswith("# my analysis\n")
    # the edit went into the idea_close backup commit
    assert git(initialized / ".arboria", "show", "HEAD:notebooks/exp1.py").endswith("# my analysis")

    wt2 = Path(arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m")
               .stdout.strip())
    git(wt2, "commit", "-q", "--allow-empty", "-m", "work")
    arb(initialized, "idea", "close", "exp2", "--merge")
    assert not wt2.exists() and _nb(initialized, "exp2").is_file()
    assert "notebook.py" not in git(initialized, "ls-files")


def test_backup_repo_versions_notebooks_only(idea: Path, initialized: Path, arb) -> None:
    state = initialized / ".arboria"
    (state / "notebooks" / "__marimo__").mkdir()
    (state / "notebooks" / "__marimo__" / "session.json").write_text("{}")
    (state / "notebooks" / "notes.txt").write_text("x")
    _nb(initialized).write_text(_nb(initialized).read_text() + "\n# v2\n")
    arb(idea, "run", "new", "--reason", "backup please")
    files = git(state, "ls-files").splitlines()
    assert files == [".gitignore", "backup/ledger.sql", "notebooks/exp1.py"]
    assert git(state, "show", "HEAD:notebooks/exp1.py").endswith("# v2")


def test_old_backup_gitignore_is_upgraded(initialized: Path, arb) -> None:
    state = initialized / ".arboria"
    old = "# Only the SQL dump is versioned here.\n/*\n!/.gitignore\n!/backup/\n/backup/*\n" \
          "!/backup/ledger.sql\n"
    (state / ".gitignore").write_text(old)
    git(state, "commit", "-qam", "old gitignore")
    arb(initialized, "idea", "new", "exp1", "--hypothesis", "h", "--metric", "m")
    assert (state / ".gitignore").read_text() == old + notebooks_gitignore()
    assert "notebooks/exp1.py" in git(state, "ls-files")


def notebooks_gitignore() -> str:
    from arboria.db import NOTEBOOKS_GITIGNORE

    return NOTEBOOKS_GITIGNORE


def test_idea_of_notebook(idea: Path, initialized: Path, tmp_path: Path) -> None:
    state = (initialized / ".arboria").resolve()
    assert notebooks.idea_of_notebook(_nb(initialized)) == (state, "exp1")
    assert notebooks.idea_of_notebook(idea / "notebook.py") == (state, "exp1")  # via the link
    assert notebooks.idea_of_notebook(idea / "train.py") is None
    assert notebooks.idea_of_notebook(tmp_path / "notebooks" / "x.py") is None


def test_idea_created_before_failure_leaves_no_notebook(initialized: Path, arb,
                                                        monkeypatch) -> None:
    def boom(*a, **k):
        raise RuntimeError("disk full")

    monkeypatch.setattr(notebooks, "create", boom)
    try:
        arb(initialized, "idea", "new", "exp1", "--hypothesis", "h", "--metric", "m", ok=False)
    except RuntimeError:
        pass
    with Ledger(initialized / ".arboria") as l:
        assert l.one("SELECT COUNT(*) AS n FROM ideas")["n"] == 0
    assert not (initialized.parent / "proj-exp1").exists()


def test_idea_notebook_creates_a_missing_notebook(idea: Path, initialized: Path, arb) -> None:
    nb = _nb(initialized)
    out = arb(idea, "idea", "notebook").stdout
    assert "already has a notebook" in out and "left as is" in out
    nb.write_text("# mine\n")
    (idea / "notebook.py").unlink()
    arb(idea, "idea", "notebook")  # repairs the link, keeps the notebook
    assert (idea / "notebook.py").resolve() == nb.resolve() and nb.read_text() == "# mine\n"

    nb.unlink()
    (idea / "notebook.py").unlink()
    out = arb(initialized, "idea", "notebook", "exp1").stdout
    assert out.startswith(f"created {nb}") and "**假设：** warmup helps" in nb.read_text()
    assert (idea / "notebook.py").resolve() == nb.resolve()
    assert git(initialized / ".arboria", "log", "-1", "--format=%s") == \
        "arboria backup: notebook_new exp1"
    assert "notebooks/exp1.py" in git(initialized / ".arboria", "ls-files")

    # a closed idea gets the file only (its worktree is gone)
    arb(initialized, "idea", "close", "exp1", "--drop")
    nb.unlink()
    assert "created" in arb(initialized, "idea", "notebook", "exp1").stdout and nb.is_file()
    assert "not inside an idea worktree" in arb(initialized, "idea", "notebook", ok=False).stderr
    assert "no idea named" in arb(initialized, "idea", "notebook", "nope", ok=False).stderr


def test_template_follows_marimo_format(idea: Path, initialized: Path) -> None:
    text = _nb(initialized).read_text()
    assert '__generated_with = "' in text
    assert '    mo.md(r"""\n    # exp1\n' in text
    assert "    from arboria import lab\n\n    return lab, mo\n" in text
    assert "lab.commands()" in text
