"""SessionStart hook and `arb install-claude`."""

from __future__ import annotations

import io
import json
import os
import subprocess
import sys
from pathlib import Path

from arboria import hook, install

from .conftest import git


def _run_hook(arb, cwd: Path, stdin: str):
    res = arb(cwd, "hook", "session-start", input=stdin)
    assert res.exit_code == 0
    return res.stdout


def test_hook_outside_repo_prints_nothing(tmp_path: Path, arb) -> None:
    assert _run_hook(arb, tmp_path, json.dumps({"cwd": str(tmp_path)})) == ""


def test_hook_uninitialized_repo_prints_nothing(repo: Path, arb) -> None:
    assert _run_hook(arb, repo, json.dumps({"cwd": str(repo)})) == ""
    assert not (repo / ".arboria").exists()


def test_hook_in_idea_worktree(idea: Path, tmp_path: Path, arb) -> None:
    arb(idea, "note", "first observation")
    # process cwd is elsewhere; the JSON cwd wins
    out = _run_hook(arb, tmp_path, json.dumps({"session_id": "s", "cwd": str(idea),
                                                "hook_event_name": "SessionStart"}))
    data = json.loads(out)
    assert data["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    ctx = data["hookSpecificOutput"]["additionalContext"]
    assert "exp1" in ctx and "warmup helps" in ctx and "first observation" in ctx


def test_hook_falls_back_to_process_cwd(idea: Path, arb) -> None:
    for stdin in ("", "not json", json.dumps({"cwd": "/does/not/exist"}), "[1, 2]"):
        out = _run_hook(arb, idea, stdin)
        assert "exp1" in json.loads(out)["hookSpecificOutput"]["additionalContext"]


def test_hook_caps_context(idea: Path, arb) -> None:
    for i in range(40):
        arb(idea, "run", "new", "--reason", f"run {i} " + "x" * 280)
    out = _run_hook(arb, idea, json.dumps({"cwd": str(idea)}))
    ctx = json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert len(ctx) <= hook.MAX_CONTEXT_CHARS
    assert hook.cap("y" * 5000).endswith("`arb status` for the full view)")
    assert len(hook.cap("y" * 5000)) == hook.MAX_CONTEXT_CHARS


def test_hook_never_crashes(idea: Path, initialized: Path, monkeypatch) -> None:
    def boom(*a, **k):
        raise RuntimeError("kaboom")

    monkeypatch.setattr(hook, "status_data", boom)
    stdout = io.StringIO()
    rc = hook.session_start(io.StringIO(json.dumps({"cwd": str(idea)})), stdout)
    assert rc == 0 and stdout.getvalue() == ""
    log = (initialized / ".arboria" / "hook.log").read_text()
    assert "kaboom" in log
    assert "hook.log" not in git(initialized / ".arboria", "status", "--porcelain")


def test_hook_corrupt_db_exits_zero(idea: Path, initialized: Path) -> None:
    (initialized / ".arboria" / "ledger.db").write_bytes(b"garbage" * 100)
    for suffix in ("-wal", "-shm"):
        (initialized / ".arboria" / f"ledger.db{suffix}").unlink(missing_ok=True)
    proc = subprocess.run(
        [sys.executable, "-c", "from arboria.cli import app; app()", "hook", "session-start"],
        input=json.dumps({"cwd": str(idea)}), capture_output=True, text=True,
        env={**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parent.parent / "src")},
    )
    assert proc.returncode == 0 and proc.stdout == ""
    assert "Traceback" in (initialized / ".arboria" / "hook.log").read_text()


def test_install_claude_creates_and_merges(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "install-claude")
    path = idea / ".claude" / "settings.json"
    data = json.loads(path.read_text())
    assert data["hooks"]["SessionStart"] == [
        {"hooks": [{"type": "command", "command": "arb hook session-start"}]}
    ]
    assert "ask" not in data["permissions"]  # approvals are denied, not asked
    deny = data["permissions"]["deny"]
    assert deny == install.DENY_RULES
    assert deny[:9] == [
        "Bash(sbatch:*)", "Bash(srun:*)", "Bash(salloc:*)", "Bash(scancel:*)", "Bash(ssh:*)",
        "Bash(scp:*)", "Bash(rsync:*)", "Bash(arb approve:*)", "Bash(arb lesson accept:*)"]
    for tool in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
        for pattern in ("**/.arboria/notebooks/**", "**/.arboria/ledger.db*",
                        "**/.arboria/backup/**", "**/.arboria/*.toml",
                        "**/.arboria/sessions/**", "**/notebook.py"):
            assert f"{tool}({pattern})" in deny
    # reading stays allowed, and the rest of .arboria/ (notably commands/) too
    assert not any(r.startswith("Read(") for r in deny)
    assert not any("**/.arboria/**" in r or "commands" in r for r in deny)
    assert len(deny) == 9 + 4 * 6 + len(install.SEED_RULES)
    # seed-only commands are denied in a project too (`arb idea plant` runs inside one)
    for rule in ("Bash(arb idea plant:*)", "Bash(arb idea set:*)", "Bash(arb idea revive:*)",
                 "Bash(arb seed plant:*)", "Bash(arb seed new:*)"):
        assert rule in deny
    assert "Bash(arb idea new:*)" not in deny and "Bash(arb idea drop:*)" not in deny
    # the global command library is a working directory Claude may write to; its absolute
    # path is this machine's, so it goes into the uncommitted settings.local.json
    gdir = Path(os.environ["ARBORIA_CONFIG_DIR"]) / "commands"
    assert "additionalDirectories" not in data["permissions"]
    local = json.loads((idea / ".claude" / "settings.local.json").read_text())
    assert local == {"permissions": {"additionalDirectories": [str(gdir)]}} and gdir.is_dir()
    assert "/.claude/settings.local.json" in (initialized / ".git" / "info" / "exclude").read_text()
    assert git(idea, "status", "--porcelain", "--untracked-files=all") == "?? .claude/settings.json"
    assert not (initialized / ".claude").exists()

    res = arb(idea, "install-claude")  # idempotent
    assert "already up to date" in res.stdout
    assert json.loads(path.read_text()) == data
    assert json.loads((idea / ".claude" / "settings.local.json").read_text()) == local


def test_install_claude_moves_the_commands_dir_out_of_settings_json(initialized: Path,
                                                                    arb) -> None:
    """Older versions wrote the machine-specific dir into the committed settings.json."""
    gdir = str(Path(os.environ["ARBORIA_CONFIG_DIR"]) / "commands")
    claude = initialized / ".claude"
    claude.mkdir()
    (claude / "settings.json").write_text(json.dumps({"permissions": {
        "additionalDirectories": ["/data/shared", gdir, "/home/alice/.config/arboria/commands"]}}))
    (claude / "settings.local.json").write_text(json.dumps({"model": "opus"}))
    out = arb(initialized, "install-claude", "--main").stdout
    assert f"permissions.additionalDirectories -= {gdir}" in out
    data = json.loads((claude / "settings.json").read_text())
    assert data["permissions"]["additionalDirectories"] == ["/data/shared"]  # the user's stays
    local = json.loads((claude / "settings.local.json").read_text())
    assert local == {"model": "opus", "permissions": {"additionalDirectories": [gdir]}}
    # only our own dir, and the key goes away once nothing is left
    (claude / "settings.json").write_text(json.dumps({"permissions": {
        "additionalDirectories": [gdir]}}))
    arb(initialized, "install-claude", "--main")
    assert "additionalDirectories" not in json.loads((claude / "settings.json").read_text())[
        "permissions"]
    assert "already up to date" in arb(initialized, "install-claude", "--main").stdout


def test_install_claude_preserves_existing(initialized: Path, arb) -> None:
    path = initialized / ".claude" / "settings.json"
    path.parent.mkdir()
    existing = {
        "model": "opus",
        "hooks": {"SessionStart": [{"matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}]}],
                  "Stop": [{"hooks": [{"type": "command", "command": "x"}]}]},
        "permissions": {"allow": ["Bash(ls:*)"], "deny": ["Bash(sbatch:*)", "Bash(ssh:* sbatch*)"],
                        "ask": ["Bash(rm:*)", "Bash(arb lesson accept:*)"]},
    }
    path.write_text(json.dumps(existing))
    arb(initialized, "install-claude", "--main")
    data = json.loads(path.read_text())
    assert data["model"] == "opus" and data["hooks"]["Stop"] == existing["hooks"]["Stop"]
    assert len(data["hooks"]["SessionStart"]) == 2
    assert data["hooks"]["SessionStart"][0] == existing["hooks"]["SessionStart"][0]
    assert data["permissions"]["allow"] == ["Bash(ls:*)"]
    # existing rules (incl. the older ssh rule) kept in place, ours appended once
    assert data["permissions"]["deny"] == ["Bash(sbatch:*)", "Bash(ssh:* sbatch*)",
                                           *[r for r in install.DENY_RULES
                                             if r != "Bash(sbatch:*)"]]
    # an approval rule an older arb put under "ask" moves to deny; the user's stay
    assert data["permissions"]["ask"] == ["Bash(rm:*)"]
    arb(initialized, "install-claude", "--main")
    assert json.loads(path.read_text()) == data


def test_install_claude_main_flag_from_worktree(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "install-claude", "--main")
    assert (initialized / ".claude" / "settings.json").exists()
    assert not (idea / ".claude").exists()


def test_install_claude_user_level(idea: Path, arb, tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    arb(idea, "install-claude", "--user")
    data = json.loads((tmp_path / ".claude" / "settings.json").read_text())
    # ~/.claude/settings.json is personal already: the commands dir stays there
    gdir = Path(os.environ["ARBORIA_CONFIG_DIR"]) / "commands"
    assert data["permissions"]["additionalDirectories"] == [str(gdir)]
    assert not (tmp_path / ".claude" / "settings.local.json").exists()


def test_install_rejects_invalid_json(initialized: Path, arb) -> None:
    path = initialized / ".claude" / "settings.json"
    path.parent.mkdir()
    path.write_text("{not json")
    res = arb(initialized, "install-claude", ok=False)
    assert res.exit_code == 1 and "not valid JSON" in res.stderr
    assert path.read_text() == "{not json"


def test_merge_settings_type_errors() -> None:
    import pytest

    from arboria.errors import ArbError

    with pytest.raises(ArbError, match="must be a list"):
        install.merge_settings({"hooks": {"SessionStart": {}}})
