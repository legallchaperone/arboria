"""The UserPromptSubmit hook: one line about what changed since the session last looked."""

from __future__ import annotations

import json
import stat
from io import StringIO
from pathlib import Path

from arboria import hook, install, updates
from arboria.db import Ledger

from .test_submit import _submit, cluster, jn  # noqa: F401
from .test_tree import _metrics


def _start(cwd: Path, sid: str = "sess-1") -> None:
    out = StringIO()
    assert hook.session_start(StringIO(json.dumps({"cwd": str(cwd), "session_id": sid})), out) == 0


def _prompt(cwd: Path, sid: str | None = "sess-1") -> str | None:
    payload = {"cwd": str(cwd), "prompt": "hi"} | ({"session_id": sid} if sid else {})
    out = StringIO()
    assert hook.prompt_submit(StringIO(json.dumps(payload)), out) == 0
    if not out.getvalue():
        return None
    data = json.loads(out.getvalue())["hookSpecificOutput"]
    assert data["hookEventName"] == "UserPromptSubmit"
    return data["additionalContext"]


def test_one_line_only_when_something_changed(cluster, arb, idea: Path, initialized: Path,
                                              monkeypatch) -> None:
    from arboria import human

    _start(idea)
    assert _prompt(idea) is None  # nothing happened yet
    cluster.script(jn(cluster, "r001", 1), run_s=600, queue_s=0, billed=4.0)
    _submit(arb, idea)
    assert _prompt(idea) is None  # a draft card is not news
    monkeypatch.setattr(human, "require_terminal", lambda *a, **k: None)
    monkeypatch.setattr(human, "read_confirmation", lambda prompt: "r001")
    arb(idea, "approve", "r001")
    assert _prompt(idea) == "[arboria] 自上一条消息：你批准了 r001。"
    cluster.advance(700)
    _metrics(initialized, "r001", [3.0, 2.29])
    arb(idea, "poll")
    arb(idea, "todo", "list")  # reading changes nothing
    line = _prompt(idea)
    assert line is not None and line.startswith("[arboria] 自上一条消息：run 结束：r001 succeeded")
    assert "val/loss 2.29" in line and "\n" not in line
    assert _prompt(idea) is None  # said once


def test_sessions_are_separate_and_silent_without_a_record(arb, idea: Path, initialized: Path) -> None:
    _start(idea, "a")
    from arboria import lab

    lab.use("exp1", repo=initialized)
    lab.todo("看一下 eval")
    assert "新待办 t001" in (_prompt(idea, "a") or "")
    # a session the SessionStart hook never saw starts from now
    assert _prompt(idea, "b") is None
    lab.todo("再看一下")
    assert _prompt(idea, "b") == "[arboria] 自上一条消息：新待办 t002（`arb todo list`）。"
    assert _prompt(idea, None) is None  # no session id: nothing to remember it by
    path = updates.seen_path(initialized / ".arboria", "a")
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert "seen" not in (initialized / ".arboria" / ".gitignore").read_text()  # not backed up


def test_hook_never_fails(idea: Path, initialized: Path, tmp_path: Path, monkeypatch) -> None:
    assert _prompt(tmp_path) is None  # not a repo
    out = StringIO()
    assert hook.prompt_submit(StringIO("not json"), out) == 0 and out.getvalue() == ""

    def boom(*a, **k):
        raise RuntimeError("ledger broke")

    _start(idea)
    monkeypatch.setattr(updates, "since", boom)
    assert _prompt(idea) is None
    assert "ledger broke" in (initialized / ".arboria" / "hook.log").read_text()


def test_long_lists_are_cut(idea: Path, initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as ledger:
        events = [{"kind": "todo_new", "subject": f"t{n:03d}", "payload_json": "{}"} for n in range(200)]
        line = updates.describe(initialized / ".arboria", ledger, events)
    assert line is not None and len(line) <= updates.MAX_LINE and line.endswith("…（`arb status`）")


def test_install_adds_the_prompt_hook() -> None:
    settings: dict = {}
    changes = install.merge_settings(settings)
    assert settings["hooks"]["UserPromptSubmit"] == [
        {"hooks": [{"type": "command", "command": "arb hook prompt-submit"}]}]
    assert "hooks.UserPromptSubmit += arb hook prompt-submit" in changes
    assert not any("UserPromptSubmit" in c for c in install.merge_settings(settings))  # idempotent
