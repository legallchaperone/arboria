"""Recording Claude Code sessions in the hook and posting command todos to a fake inbox."""

from __future__ import annotations

import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

import pytest

from arboria import lab, sessions


class FakeInbox:
    """A Unix-socket server standing in for a Claude Code session's inbox."""

    def __init__(self, accept: bool = True):
        self.dir = Path(tempfile.mkdtemp(prefix="arbs"))  # short: AF_UNIX paths are limited
        self.path = str(self.dir / "inbox.sock")
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.path)
        self.server.listen(4)
        self.lines: list[str] = []
        self.accept = accept
        self._thread = threading.Thread(target=self._serve, daemon=True)
        if accept:
            self._thread.start()

    def _serve(self) -> None:
        while True:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            with conn:
                data = b""
                while chunk := conn.recv(4096):
                    data += chunk
                self.lines += data.decode("utf-8").splitlines()

    def messages(self, n: int = 1, timeout: float = 3.0) -> list[dict]:
        import time

        end = time.monotonic() + timeout
        while len(self.lines) < n and time.monotonic() < end:
            time.sleep(0.02)
        return [json.loads(line) for line in self.lines]

    def close(self) -> None:
        self.server.close()
        shutil.rmtree(self.dir, ignore_errors=True)


@pytest.fixture
def inbox():
    box = FakeInbox()
    yield box
    box.close()


@pytest.fixture(autouse=True)
def unpinned():
    lab.use(None)
    yield
    lab.use(None)


def _hook(arb, cwd: Path, monkeypatch, socket_path: str | None) -> str:
    if socket_path:
        monkeypatch.setenv(sessions.SOCKET_ENV, socket_path)
        monkeypatch.setenv("CLAUDE_CODE_MESSAGING_TOKEN", "secret-token")
    out = arb(cwd, "hook", "session-start", input=json.dumps({"cwd": str(cwd)})).stdout
    monkeypatch.delenv(sessions.SOCKET_ENV, raising=False)
    monkeypatch.delenv("CLAUDE_CODE_MESSAGING_TOKEN", raising=False)
    return out


def test_hook_records_session_privately(idea: Path, initialized: Path, arb, monkeypatch,
                                        inbox: FakeInbox) -> None:
    state = initialized / ".arboria"
    out = _hook(arb, idea, monkeypatch, inbox.path)
    assert json.loads(out)["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    path = sessions.session_file(state, idea)
    assert path.parent == state / "sessions"
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    entry = json.loads(path.read_text())
    assert entry["socket"] == inbox.path and entry["worktree"] == str(idea.resolve())
    assert entry["started_at"] and "pid" in entry
    assert "secret-token" not in path.read_text()  # the token is never stored
    assert sessions.live_session(state, idea)["socket"] == inbox.path
    assert sessions.live_session(state, initialized) is None  # another worktree
    # the session file is the human's/arb's: never in the backup repo
    assert "sessions" not in subprocess.run(
        ["git", "ls-files"], cwd=state, capture_output=True, text=True).stdout


def test_hook_without_inbox_records_nothing(idea: Path, initialized: Path, arb,
                                            monkeypatch) -> None:
    _hook(arb, idea, monkeypatch, None)
    assert not (initialized / ".arboria" / "sessions").exists()
    monkeypatch.setenv(sessions.SOCKET_ENV, "relative/path.sock")
    _hook(arb, idea, monkeypatch, None)
    assert not (initialized / ".arboria" / "sessions").exists()


def test_hook_stays_silent_when_recording_fails(idea: Path, initialized: Path, arb,
                                                monkeypatch, inbox: FakeInbox) -> None:
    def boom(*a, **k):
        raise OSError("disk full")

    monkeypatch.setattr(sessions, "record", boom)
    out = _hook(arb, idea, monkeypatch, inbox.path)
    assert "exp1" in json.loads(out)["hookSpecificOutput"]["additionalContext"]
    assert "disk full" in (initialized / ".arboria" / "hook.log").read_text()


def test_command_todo_is_pushed_to_the_live_session(idea: Path, initialized: Path, arb,
                                                    monkeypatch, inbox: FakeInbox) -> None:
    _hook(arb, idea, monkeypatch, inbox.path)
    lab.use("exp1", repo=initialized)
    p = lab.svd("对 attention 权重做 SVD")
    assert p.pushed and p.task_id == "t001"
    assert p.text() == "命令 svd 还不存在，已交给 agent（t001）。写好后重新运行这个 cell。"
    (msg,) = inbox.messages()
    assert msg["type"] == "user" and msg["message"]["role"] == "user"
    text = msg["message"]["content"]
    assert "t001" in text and "svd" in text and "运行 arb commands guide 查看约定" in text
    assert "attention" not in text  # only validated fields, never the human's free text
    assert "auth" not in json.dumps(msg)
    # re-running the cell reuses the todo and does not post again
    assert not lab.svd("对 attention 权重做 SVD").pushed
    lab.svd.revise  # noqa: B018
    assert len(inbox.messages(2, timeout=0.3)) == 1


def test_push_can_be_turned_off(idea: Path, initialized: Path, arb, monkeypatch,
                                inbox: FakeInbox) -> None:
    toml = initialized / "arboria.toml"
    assert "[claude]\n" in toml.read_text() and "push = true" in toml.read_text()
    toml.write_text(toml.read_text().replace("push = true", "push = false"))
    _hook(arb, idea, monkeypatch, inbox.path)
    lab.use("exp1", repo=initialized)
    p = lab.svd("x")
    assert not p.pushed and "在 agent 会话里说'处理待办'即可" in p.text()
    assert inbox.messages(1, timeout=0.3) == []


def test_env_socket_alone_is_never_used(idea: Path, initialized: Path, monkeypatch,
                                        inbox: FakeInbox) -> None:
    """Only a hook-recorded session is posted to, never a socket in the process env."""
    monkeypatch.setenv(sessions.SOCKET_ENV, inbox.path)
    lab.use("exp1", repo=initialized)
    p = lab.svd("x")
    assert p.task_id == "t001" and not p.pushed
    assert inbox.messages(1, timeout=0.3) == []


def _dead_pid() -> int:
    proc = subprocess.Popen([sys.executable, "-c", "pass"])
    proc.wait()
    return proc.pid


def test_stale_and_bogus_sessions_are_skipped(idea: Path, initialized: Path, arb,
                                              monkeypatch, inbox: FakeInbox) -> None:
    state = initialized / ".arboria"
    lab.use("exp1", repo=initialized)
    # a dead pid: cleaned up, nothing sent
    sessions.record(state, idea, {sessions.SOCKET_ENV: inbox.path}, pid=_dead_pid())
    assert sessions.live_session(state, idea) is None
    assert not sessions.session_file(state, idea).exists()
    # a socket path that is not a socket / a symlink to one / gone
    plain = inbox.dir / "plain"
    plain.write_text("")
    link = inbox.dir / "link.sock"
    link.symlink_to(inbox.path)
    for bad in (str(plain), str(link), str(inbox.dir / "missing.sock")):
        sessions.record(state, idea, {sessions.SOCKET_ENV: bad}, pid=os.getpid())
        assert not sessions.notify(state, idea, "hi")
    assert not lab.svd("x").pushed
    assert inbox.messages(1, timeout=0.3) == []
    # a live entry for the same worktree works; the other cleaned on record()
    sessions.record(state, idea, {sessions.SOCKET_ENV: inbox.path}, pid=os.getpid())
    assert sessions.notify(state, idea, "hello")
    assert inbox.messages()[0]["message"]["content"] == "hello"
    # garbage in the file
    sessions.session_file(state, idea).write_text("{not json")
    assert not sessions.notify(state, idea, "x")


def test_send_times_out_and_never_raises() -> None:
    box = FakeInbox(accept=False)  # listening but never reading
    try:
        assert sessions.send(box.path, "hi", timeout=0.5)  # small writes fit the buffer
        assert not sessions.send(box.path + ".nope", "hi", timeout=0.5)
        box.server.close()
        assert not sessions.send(box.path, "hi", timeout=0.5)  # refused
    finally:
        box.close()


def test_claude_pid_finds_an_ancestor() -> None:
    pid = sessions.claude_pid()
    assert pid is None or sessions.pid_alive(pid)
    assert sessions.pid_alive(os.getpid(), sessions.start_time(os.getpid()))
    assert not sessions.pid_alive(_dead_pid())
