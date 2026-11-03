"""The arb-managed block in the main worktree's CLAUDE.md."""

from __future__ import annotations

from pathlib import Path

import pytest

from arboria import claudemd
from arboria.claudemd import BEGIN, END
from arboria.errors import ArbError

from .conftest import git


def _head(repo: Path) -> str:
    return git(repo, "rev-parse", "HEAD")


def _block_of(text: str) -> str:
    start = text.index(BEGIN)
    end = text.index(END) + len(END) + 1
    return text[start:end]


def _lesson(text: str, source: str | None = "exp1", day: str = "2026-11-01") -> dict:
    return {"text": text, "source_id": source, "created_at": f"{day}T00:00:00Z",
            "decided_at": f"{day}T01:00:00Z"}


# ---------------------------------------------------------------- pure block logic


def test_render_block_content() -> None:
    block = claudemd.render_block([])
    lines = block.splitlines()
    assert lines[0] == BEGIN and lines[-1] == END and block.endswith(END + "\n")
    assert "## 研究工作流（Arboria）" in lines and "## 经验（已确认）" in lines
    assert lines[-2] == "- 暂无"
    for needle in ("`arb status`", "arb note", "arb run new --reason", "arb approve",
                   "arb submit --reason", "[-- CMD]", "--from rNNN --set k=v",
                   "批准只能由人在自己的终端里做", "arb lesson accept", "arb logs RUN",
                   "arb remote ls RUN", "不用 ssh", "arb lesson propose",
                   "wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)", "ARBORIA_IDEA",
                   "$ARBORIA_OUT/config.yaml", "ARBORIA_*", "sbatch",
                   "`notebook.py`", "绝不修改", "arb todo list", "arb todo done tNNN --result",
                   "`arb note` 只由人写", "arb run summary"):
        assert needle in block, needle
    assert "ARBORIA_RUN_ID" not in block  # wandb ids are namespaced by project now
    assert len(lines) <= claudemd.MAX_BLOCK_LINES

    block = claudemd.render_block([_lesson("b", day="2026-11-02"), _lesson("a", None)])
    assert block.splitlines()[-3:-1] == ["- b（来自 exp1，2026-11-02）", "- a（2026-11-01）"]


def test_block_fits_max_lessons() -> None:
    lessons = [_lesson("x" * 200) for _ in range(claudemd.MAX_ACCEPTED_LESSONS)]
    assert len(claudemd.render_block(lessons).splitlines()) <= claudemd.MAX_BLOCK_LINES


def test_line_cap_enforced() -> None:
    lessons = [_lesson(f"l{i}") for i in range(claudemd.MAX_BLOCK_LINES)]
    with pytest.raises(ArbError, match=r"would have \d+ lines \(max 50\).*arb lesson retire"):
        claudemd.render_block(lessons)


def test_apply_block_create_append_replace() -> None:
    block = claudemd.render_block([])
    assert claudemd.apply_block(None, block) == (block, "created")
    assert claudemd.apply_block("", block) == (block, "appended")
    assert claudemd.apply_block("# mine", block) == ("# mine\n\n" + block, "appended")
    assert claudemd.apply_block("# mine\n", block) == ("# mine\n\n" + block, "appended")
    assert claudemd.apply_block("# mine\n\n", block) == ("# mine\n\n" + block, "appended")

    before = "# head\r\n\ttabs  \n\n"
    after = "\n## tail\nno newline at end"
    old = before + BEGIN + "\nstale content\n" + END + "\n" + after
    new, action = claudemd.apply_block(old, block)
    assert action == "replaced" and new == before + block + after


@pytest.mark.parametrize("text,msg", [
    (f"{BEGIN}\nx\n", "without arboria:end"),
    (f"x\n{END}\n", "without arboria:begin"),
    (f"{BEGIN}\n{END}\n{BEGIN}\n{END}\n", "2 arboria:begin and 2 arboria:end"),
    (f"{BEGIN}\n{BEGIN}\n{END}\n", "2 arboria:begin"),
    (f"{END}\n{BEGIN}\n", "end before"),
])
def test_malformed_markers(text: str, msg: str) -> None:
    with pytest.raises(ArbError, match=msg):
        claudemd.apply_block(text, claudemd.render_block([]))


# ---------------------------------------------------------------- sync in a repo


def test_init_creates_and_commits_block(repo: Path, arb) -> None:
    head = _head(repo)
    res = arb(repo, "init")
    assert "created arboria block" in res.stdout
    text = (repo / "CLAUDE.md").read_text()
    assert text == claudemd.render_block([])
    assert git(repo, "log", "-1", "--format=%s") == "arboria: update CLAUDE.md"
    assert git(repo, "rev-parse", "HEAD~1") == head
    assert git(repo, "show", "--name-only", "--format=", "HEAD") == "CLAUDE.md"
    assert git(repo, "log", "-1", "--format=%an <%ae>") == "Test User <test@example.com>"
    assert git(repo, "status", "--porcelain") == "?? arboria.toml"

    res = arb(repo, "init")  # idempotent: no change, no commit
    assert "already initialized" in res.stdout
    assert git(repo, "rev-list", "--count", "HEAD") == "2"


def test_sync_preserves_outside_content_byte_for_byte(initialized: Path, arb) -> None:
    path = initialized / "CLAUDE.md"
    before = "# 我的项目\r\n\nmy own rules, trailing spaces   \n\n".encode()
    after = "\n## more\n\xff\xfe raw bytes\nno final newline".encode("utf-8") + b"\xff"
    path.write_bytes(before + f"{BEGIN}\nold stuff\n{END}\n".encode() + after)
    git(initialized, "add", "CLAUDE.md")
    git(initialized, "commit", "-qm", "edit CLAUDE.md")
    arb(initialized, "claudemd", "sync")
    data = path.read_bytes()
    assert data.startswith(before) and data.endswith(after)
    assert data[len(before):len(data) - len(after)] == claudemd.render_block([]).encode()
    assert git(initialized, "log", "-1", "--format=%s") == "arboria: update CLAUDE.md"


def test_sync_appends_to_existing_file(repo: Path, arb) -> None:
    (repo / "CLAUDE.md").write_text("# Project notes\nuse black\n")
    git(repo, "add", "CLAUDE.md")
    git(repo, "commit", "-qm", "notes")
    arb(repo, "init")
    text = (repo / "CLAUDE.md").read_text()
    assert text == "# Project notes\nuse black\n\n" + claudemd.render_block([])


def test_sync_idempotent_no_commit(initialized: Path, arb) -> None:
    head = _head(initialized)
    res = arb(initialized, "claudemd", "sync")
    assert "already up to date" in res.stdout
    assert _head(initialized) == head


def test_sync_restores_edited_block(initialized: Path, arb) -> None:
    path = initialized / "CLAUDE.md"
    path.write_text(path.read_text().replace("- 暂无", "- hand edit"))
    res = arb(initialized, "claudemd", "sync", "--no-commit")
    assert "replaced arboria block" in res.stdout
    assert "- 暂无" in path.read_text()
    assert git(initialized, "status", "--porcelain") == "?? arboria.toml"  # equal to HEAD again


def test_malformed_markers_refused_and_nothing_changed(initialized: Path, arb) -> None:
    path = initialized / "CLAUDE.md"
    bad = f"# mine\n{BEGIN}\nno end marker\n"
    path.write_text(bad)
    head = _head(initialized)
    res = arb(initialized, "claudemd", "sync", ok=False)
    assert res.exit_code == 1 and "without arboria:end" in res.stderr
    assert path.read_text() == bad and _head(initialized) == head


def test_autocommit_commits_only_claudemd(initialized: Path, arb) -> None:
    (initialized / "train.py").write_text("dirty, unstaged\n")
    (initialized / "staged.py").write_text("staged\n")
    git(initialized, "add", "staged.py")
    (initialized / "README.md").write_text("staged edit\n")
    git(initialized, "add", "README.md")
    (initialized / "README.md").write_text("staged edit\nplus unstaged\n")
    status_before = git(initialized, "status", "--porcelain")
    staged_before = git(initialized, "diff", "--cached")

    path = initialized / "CLAUDE.md"
    path.write_text("# user header\n\n" + path.read_text())  # user's own (uncommitted) edit
    path.write_text(path.read_text().replace("- 暂无", "- stale"))
    arb(initialized, "claudemd", "sync")

    assert git(initialized, "show", "--name-only", "--format=", "HEAD") == "CLAUDE.md"
    assert git(initialized, "status", "--porcelain") == status_before.replace(" M CLAUDE.md\n", "")
    assert git(initialized, "diff", "--cached") == staged_before
    assert (initialized / "train.py").read_text() == "dirty, unstaged\n"
    assert git(initialized, "show", "HEAD:CLAUDE.md") == path.read_text().rstrip("\n")


def test_autocommit_adds_untracked_claudemd(repo: Path, arb) -> None:
    (repo / "CLAUDE.md").write_text("# untracked notes\n")
    (repo / "other.txt").write_text("untracked\n")
    arb(repo, "init")
    assert git(repo, "show", "--name-only", "--format=", "HEAD") == "CLAUDE.md"
    assert git(repo, "status", "--porcelain") == "?? arboria.toml\n?? other.txt"


def test_sync_skips_commit_during_merge(initialized: Path, arb) -> None:
    git(initialized, "checkout", "-qb", "side")
    (initialized / "train.py").write_text("side\n")
    git(initialized, "commit", "-qam", "side")
    git(initialized, "checkout", "-q", "main")
    (initialized / "train.py").write_text("main\n")
    git(initialized, "commit", "-qam", "main")
    git(initialized, "merge", "side", check=False)
    assert (initialized / ".git" / "MERGE_HEAD").exists()
    head = _head(initialized)

    path = initialized / "CLAUDE.md"
    path.write_text(path.read_text().replace("- 暂无", "- stale"))
    res = arb(initialized, "claudemd", "sync")
    assert "middle of a merge" in res.stdout and "commit CLAUDE.md yourself" in res.stdout
    assert "- 暂无" in path.read_text()  # still written
    assert _head(initialized) == head
    assert "UU train.py" in git(initialized, "status", "--porcelain")
    assert git(initialized, "diff", "--cached", "--name-only", "--", "CLAUDE.md") == ""


def test_sync_skips_commit_when_claudemd_conflicted(initialized: Path, arb, monkeypatch) -> None:
    # Conflicted index entry for CLAUDE.md without an operation in progress (e.g. after
    # `git merge --quit` / stash pop conflicts).
    git(initialized, "checkout", "-qb", "side")
    (initialized / "CLAUDE.md").write_text("side\n")
    git(initialized, "commit", "-qam", "side")
    git(initialized, "checkout", "-q", "main")
    (initialized / "CLAUDE.md").write_text("main\n")
    git(initialized, "commit", "-qam", "main")
    git(initialized, "merge", "side", check=False)
    git(initialized, "merge", "--quit")
    assert git(initialized, "ls-files", "-u", "--", "CLAUDE.md")
    head = _head(initialized)
    (initialized / "CLAUDE.md").write_text("resolved by hand\n")
    res = arb(initialized, "claudemd", "sync")
    assert "CLAUDE.md has merge conflicts" in res.stdout
    assert _head(initialized) == head


def _commit_stale_block(repo: Path) -> Path:
    path = repo / "CLAUDE.md"
    path.write_text(path.read_text().replace("- 暂无", "- stale"))
    git(repo, "commit", "-qm", "stale block", "--", "CLAUDE.md")
    return path


def test_sync_no_commit_flag_and_config_off(initialized: Path, arb) -> None:
    path = _commit_stale_block(initialized)
    head = _head(initialized)
    arb(initialized, "claudemd", "sync", "--no-commit")
    assert "- 暂无" in path.read_text()
    assert _head(initialized) == head
    assert git(initialized, "status", "--porcelain", "CLAUDE.md") == "M CLAUDE.md"
    git(initialized, "checkout", "--", "CLAUDE.md")

    cfg = initialized / "arboria.toml"
    cfg.write_text(cfg.read_text().replace("autocommit = true", "autocommit = false"))
    res = arb(initialized, "claudemd", "sync")
    assert "autocommit is off" in res.stdout and "- 暂无" in path.read_text()
    assert _head(initialized) == head
    assert git(initialized, "status", "--porcelain", "CLAUDE.md") == "M CLAUDE.md"

    cfg.write_text("[claudemd]\nautocommit = 'yes'\n")
    git(initialized, "checkout", "--", "CLAUDE.md")
    res = arb(initialized, "claudemd", "sync", ok=False)
    assert "must be true or false" in res.stderr


def test_sync_without_git_identity_skips_commit(initialized: Path, arb, tmp_path: Path,
                                                monkeypatch) -> None:
    empty = tmp_path / "empty.gitconfig"
    empty.write_text("[commit]\n\tgpgsign = false\n")
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(empty))
    for var in ("EMAIL", "GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME",
                "GIT_COMMITTER_EMAIL"):
        monkeypatch.delenv(var, raising=False)
    head = _head(initialized)
    path = initialized / "CLAUDE.md"
    path.write_text(path.read_text().replace("- 暂无", "- stale"))
    res = arb(initialized, "claudemd", "sync")
    assert "no git identity" in res.stdout
    assert _head(initialized) == head


def test_sync_from_idea_worktree_writes_main(idea: Path, initialized: Path, arb) -> None:
    path = _commit_stale_block(initialized)
    idea_copy = (idea / "CLAUDE.md").read_bytes()
    arb(idea, "claudemd", "sync")
    assert "- 暂无" in path.read_text()
    assert git(initialized, "log", "-1", "--format=%s") == "arboria: update CLAUDE.md"
    assert (idea / "CLAUDE.md").read_bytes() == idea_copy  # idea worktree untouched
    assert git(idea, "status", "--porcelain") == ""


def test_init_with_malformed_markers_warns(repo: Path, arb) -> None:
    (repo / "CLAUDE.md").write_text(f"{END}\n")
    res = arb(repo, "init")
    assert "CLAUDE.md not updated" in res.stderr and (repo / ".arboria" / "ledger.db").exists()
    assert (repo / "CLAUDE.md").read_text() == f"{END}\n"


def test_install_claude_syncs(initialized: Path, arb) -> None:
    (initialized / "CLAUDE.md").write_text("# mine\n")
    res = arb(initialized, "install-claude")
    assert "appended arboria block" in res.stdout
    assert (initialized / "CLAUDE.md").read_text().startswith("# mine\n\n" + BEGIN)
