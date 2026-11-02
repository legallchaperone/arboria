"""`arb lesson ...`, and lessons in CLAUDE.md, status, the hook and export."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from arboria import claudemd, hook
from arboria.db import Ledger

from .conftest import git

FLASH = "flash-attn 在 aarch64 上要从 conda-forge 装"
EVAL = "eval 每 500 步一次，否则 spike 看不到"


def _lessons(arb, cwd: Path, *flags: str) -> list[dict]:
    return json.loads(arb(cwd, "lesson", "list", "--json", *flags).stdout)


def _block(main: Path) -> list[str]:
    text = (main / "CLAUDE.md").read_text()
    start = text.index(claudemd.BEGIN)
    return text[start:text.index(claudemd.END) + len(claudemd.END)].splitlines()


def _backup_log(main: Path) -> list[str]:
    return git(main / ".arboria", "log", "--format=%s").splitlines()


def _decision(main: Path, lesson_id: str) -> dict:
    with Ledger(main / ".arboria") as ledger:
        return ledger.one("SELECT * FROM decisions WHERE subject_type = 'lesson' AND subject_id = ?",
                          (lesson_id,))


def test_lesson_lifecycle(idea: Path, initialized: Path, arb) -> None:
    head = git(initialized, "rev-parse", "HEAD")
    res = arb(idea, "lesson", "propose", FLASH)
    assert "proposed l001 (decision d001)" in res.stdout and "arb lesson accept l001" in res.stdout
    assert _backup_log(initialized)[0] == "arboria backup: decision d001; lesson_propose l001"
    (l1,) = _lessons(arb, initialized)
    assert (l1["id"], l1["status"], l1["source_type"], l1["source_id"], l1["decided_at"]) == \
        ("l001", "proposed", "idea", "exp1", None)
    dec = _decision(initialized, "l001")
    assert (dec["question"], json.loads(dec["options_json"]), dec["choice"]) == \
        ("接受这条经验吗？", ["accept", "reject"], None)
    assert git(initialized, "rev-parse", "HEAD") == head  # proposing does not touch CLAUDE.md
    assert "- 暂无" in _block(initialized)

    res = arb(idea, "lesson", "accept", "l001")
    assert "accepted l001" in res.stdout and "committed CLAUDE.md" in res.stdout
    today = _lessons(arb, initialized)[0]["decided_at"][:10]
    assert f"- {FLASH}（来自 exp1，{today}）" in _block(initialized)
    assert "- 暂无" not in _block(initialized)
    assert git(initialized, "log", "-1", "--format=%s") == "arboria: update CLAUDE.md"
    assert git(initialized, "show", "--name-only", "--format=", "HEAD") == "CLAUDE.md"
    assert _backup_log(initialized)[0] == "arboria backup: decision d001; lesson_accept l001"
    dec = _decision(initialized, "l001")
    assert (dec["choice"], dec["decided_by"]) == ("accept", "human")
    res = arb(idea, "lesson", "accept", "l001", ok=False)
    assert res.exit_code == 1 and "is accepted, not proposed" in res.stderr

    # second lesson from a run, newest first in the block
    arb(idea, "run", "new", "--reason", "baseline")
    arb(initialized, "lesson", "propose", EVAL, "--run", "r001")
    arb(initialized, "lesson", "accept", "l002")
    lines = _block(initialized)
    assert lines.index(f"- {EVAL}（来自 r001，{today}）") < lines.index(f"- {FLASH}（来自 exp1，{today}）")

    # reject a proposal
    arb(initialized, "lesson", "propose", "no source here")
    assert _lessons(arb, initialized)[0]["source_type"] == "none"
    head = git(initialized, "rev-parse", "HEAD")
    res = arb(initialized, "lesson", "reject", "l003")
    assert "rejected l003" in res.stdout
    assert git(initialized, "rev-parse", "HEAD") == head
    assert _decision(initialized, "l003")["choice"] == "reject"
    assert _backup_log(initialized)[0] == "arboria backup: decision d003; lesson_reject l003"
    assert [l["id"] for l in _lessons(arb, initialized)] == ["l002", "l001"]
    assert [l["id"] for l in _lessons(arb, initialized, "--all")] == ["l003", "l002", "l001"]
    assert _lessons(arb, initialized, "--all")[0]["status"] == "retired"

    # retire an accepted one
    res = arb(initialized, "lesson", "retire", "l001")
    assert "retired l001" in res.stdout
    assert all(FLASH not in line for line in _block(initialized))
    assert _backup_log(initialized)[0] == "arboria backup: lesson_retire l001"
    assert git(initialized, "log", "-1", "--format=%s") == "arboria: update CLAUDE.md"
    for args, msg in [(["retire", "l003"], "is retired, not accepted"),
                      (["reject", "l002"], "use `arb lesson retire`"),
                      (["accept", "l999"], "no lesson 'l999'")]:
        res = arb(initialized, "lesson", *args, ok=False)
        assert res.exit_code == 1 and msg in res.stderr, args

    text = arb(initialized, "lesson", "list").stdout
    assert "l002" in text and EVAL in text and "l001" not in text


def test_propose_validation(idea: Path, initialized: Path, arb) -> None:
    for args, msg in [(["x" * 201], "lesson is too long"),
                      (["a\nb"], "single line"),
                      (["  "], "must not be empty"),
                      (["x", "--idea", "nope"], "no idea named"),
                      (["x", "--run", "r404"], "no run"),
                      (["x", "--idea", "exp1", "--run", "r1"], "only one of")]:
        res = arb(idea, "lesson", "propose", *args, ok=False)
        assert res.exit_code == 1 and msg in res.stderr, args
    arb(initialized, "lesson", "propose", FLASH, "--idea", "exp1")
    assert _lessons(arb, initialized)[0]["source_id"] == "exp1"
    res = arb(initialized, "lesson", "propose", FLASH, ok=False)
    assert "already exists as l001" in res.stderr


def test_accepted_lesson_cap(initialized: Path, arb) -> None:
    with Ledger(initialized / ".arboria") as ledger, ledger.write() as conn:
        for i in range(1, claudemd.MAX_ACCEPTED_LESSONS + 1):
            ts = f"2026-11-01T00:00:{i:02d}Z"
            conn.execute("INSERT INTO lessons (id, text, status, created_at, decided_at)"
                         " VALUES (?, ?, 'accepted', ?, ?)", (f"l{i:03d}", f"lesson {i}", ts, ts))
    arb(initialized, "claudemd", "sync")
    assert len(_block(initialized)) <= claudemd.MAX_BLOCK_LINES
    res = arb(initialized, "lesson", "propose", "one more", ok=False)
    assert res.exit_code == 1 and "already 20 accepted lessons" in res.stderr
    assert "arb lesson retire" in res.stderr
    assert len(_lessons(arb, initialized)) == 20

    arb(initialized, "lesson", "retire", "l001")
    arb(initialized, "lesson", "propose", "one more")
    arb(initialized, "lesson", "propose", "and another")
    arb(initialized, "lesson", "accept", "l021")
    res = arb(initialized, "lesson", "accept", "l022", ok=False)  # cap re-checked on accept
    assert "already 20 accepted lessons" in res.stderr
    assert _lessons(arb, initialized)[0]["status"] == "proposed"


def test_propose_refused_when_block_would_overflow(initialized: Path, arb, monkeypatch) -> None:
    base = len(claudemd.render_block([]).splitlines())  # includes the "- 暂无" line
    monkeypatch.setattr(claudemd, "MAX_BLOCK_LINES", base + 1)
    arb(initialized, "lesson", "propose", "a")
    arb(initialized, "lesson", "accept", "l001")
    arb(initialized, "lesson", "propose", "b")
    arb(initialized, "lesson", "accept", "l002")
    res = arb(initialized, "lesson", "propose", "c", ok=False)
    assert res.exit_code == 1 and "overflow the CLAUDE.md block" in res.stderr
    assert "arb lesson retire" in res.stderr


def test_accept_refused_on_malformed_claudemd(initialized: Path, arb) -> None:
    arb(initialized, "lesson", "propose", FLASH)
    path = initialized / "CLAUDE.md"
    path.write_text(path.read_text() + claudemd.BEGIN + "\n")
    res = arb(initialized, "lesson", "accept", "l001", ok=False)
    assert res.exit_code == 1 and "arboria:begin" in res.stderr
    assert _lessons(arb, initialized)[0]["status"] == "proposed"  # nothing changed
    assert _decision(initialized, "l001")["choice"] is None


def test_hook_and_status_show_lessons(idea: Path, initialized: Path, arb) -> None:
    for i in range(12):
        arb(initialized, "lesson", "propose", f"lesson number {i}")
        arb(initialized, "lesson", "accept", f"l{i + 1:03d}")
    arb(idea, "lesson", "propose", "pending one")
    arb(idea, "lesson", "propose", "pending two")

    out = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert "exp1" in ctx  # still the idea's state
    assert "lesson number 11（" in ctx and "lesson number 2（" in ctx
    assert "lesson number 1（" not in ctx and "lesson number 0（" not in ctx  # capped at 10
    assert ctx.index("lesson number 11") < ctx.index("lesson number 2")  # newest first
    assert "另有 2 条" in ctx
    assert "待你确认的经验：2 条，用 `arb lesson list` 查看" in ctx
    assert "pending one" not in ctx

    brief = arb(initialized, "status", "--brief").stdout
    assert "lesson number 11" in brief and "待你确认的经验：2 条" in brief
    st = json.loads(arb(idea, "status", "--json").stdout)
    assert len(st["lessons"]) == 10 and st["proposed_lessons"] == 2 and st["accepted_lessons"] == 12
    # lesson decisions show up as open decisions in the overview
    ov = json.loads(arb(initialized, "status", "--json").stdout)
    assert [d["subject_id"] for d in ov["open_decisions"]] == ["l013", "l014"]


def test_hook_keeps_lessons_when_state_is_long(idea: Path, initialized: Path, arb) -> None:
    for i in range(10):
        arb(initialized, "lesson", "propose", f"{i:02d} " + "y" * 190)
        arb(initialized, "lesson", "accept", f"l{i + 1:03d}")
    for i in range(12):
        arb(idea, "run", "new", "--reason", f"run {i} " + "x" * 280)
        arb(idea, "run", "summary", f"r{i + 1:03d}", "s" * 1400)
        arb(idea, "note", "n" * 900)
    arb(idea, "lesson", "propose", "pending")
    out = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert len(ctx) <= hook.MAX_CONTEXT_CHARS
    assert "truncated" in ctx
    assert "09 yyyy" in ctx and "00 yyyy" in ctx and "待你确认的经验：1 条" in ctx


def test_hook_without_lessons_has_no_lesson_section(idea: Path, arb) -> None:
    out = arb(idea, "hook", "session-start", input=json.dumps({"cwd": str(idea)})).stdout
    ctx = json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert "经验" not in ctx and "arb lesson propose" in ctx


def test_idea_close_hint(idea: Path, initialized: Path, arb) -> None:
    res = arb(initialized, "idea", "close", "exp1", "--drop")
    assert res.stdout.rstrip("\n").splitlines()[-1] == \
        '如果这次有可复用的教训，用 arb lesson propose "..." 提出。'
    res = arb(initialized, "idea", "new", "exp2", "--hypothesis", "h", "--metric", "m")
    res = arb(initialized, "idea", "close", "exp2", "--merge")
    assert res.stdout.rstrip("\n").endswith('arb lesson propose "..." 提出。')


def test_export_has_lessons_section(idea: Path, initialized: Path, arb) -> None:
    md = arb(initialized, "export").stdout
    assert "## 经验\n\n_暂无_" in md
    arb(idea, "lesson", "propose", FLASH)
    arb(idea, "lesson", "accept", "l001")
    arb(idea, "lesson", "propose", EVAL)
    md = arb(initialized, "export").stdout
    assert md.index("## 经验") < md.index("## exp1")
    assert f"- {FLASH}（来自 exp1，" in md
    assert f"- l002: {EVAL}" in md


def test_install_claude_ask_entry(initialized: Path, arb) -> None:
    arb(initialized, "install-claude")
    data = json.loads((initialized / ".claude" / "settings.json").read_text())
    assert "Bash(arb lesson accept:*)" in data["permissions"]["ask"]


@pytest.mark.parametrize("cmd", ["accept", "reject", "retire"])
def test_lesson_commands_require_init(repo: Path, arb, cmd: str) -> None:
    res = arb(repo, "lesson", cmd, "l001", ok=False)
    assert res.exit_code == 1 and "arb init" in res.stderr
