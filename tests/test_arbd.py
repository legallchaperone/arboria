"""arbd: the Mac app's core process (ADR 0004, P1)."""

from __future__ import annotations

import ast
import importlib.util
import json
import socket
import threading
import urllib.request
from pathlib import Path

import pytest

from arboria import arbd
from arboria.errors import ArbError

SRC = Path(arbd.__file__)


class FakeProc:
    def __init__(self) -> None:
        self.returncode: int | None = None
        self.terminated = False

    def poll(self) -> int | None:
        return self.returncode

    def terminate(self) -> None:
        self.terminated = True
        self.returncode = -15

    def wait(self, timeout: float | None = None) -> int:
        return self.returncode or 0

    def kill(self) -> None:
        self.returncode = -9


@pytest.fixture
def started(monkeypatch):
    calls: list[dict] = []

    def start(cmd, cwd, env):
        p = FakeProc()
        calls.append({"cmd": cmd, "cwd": cwd, "env": env, "proc": p})
        return p

    return calls, start


def _daemon(root: Path, start) -> arbd.Daemon:
    return arbd.Daemon(root, start=start, ready=lambda port, proc: None)


def test_project_lists_ideas_with_their_worktrees(idea: Path, initialized: Path, started) -> None:
    d = _daemon(idea, started[1])  # any worktree of the project will do
    p = d.project()
    assert p["root"] == str(initialized) and p["name"] == "proj"
    [i] = p["ideas"]
    assert i["name"] == "exp1" and i["status"] == "open"
    assert i["worktree"] == str(idea) and i["has_notebook"] is True
    assert i["hypothesis"] == "warmup helps" and i["run_count"] == 0


def test_notebook_open_starts_marimo_in_the_worktree(idea: Path, initialized: Path, started,
                                                     monkeypatch) -> None:
    calls, start = started
    monkeypatch.setenv("CLAUDECODE", "1")
    monkeypatch.setenv("CLAUDE_CODE_CHILD_SESSION", "x")
    d = _daemon(initialized, start)
    r = d.notebook_open("exp1")
    [c] = calls
    assert c["cwd"] == idea
    cmd = c["cmd"]
    assert cmd[1:4] == ["-m", "marimo", "edit"] and cmd[4] == str(idea / "notebook.py")
    assert "--headless" in cmd and cmd[cmd.index("--host") + 1] == "127.0.0.1"
    assert "--token-password" not in cmd  # the token is passed in a 0600 file, not argv
    tf = Path(cmd[cmd.index("--token-password-file") + 1])
    assert tf.stat().st_mode & 0o777 == 0o600
    port = int(cmd[cmd.index("--port") + 1])
    assert r["url"] == f"http://127.0.0.1:{port}/?access_token={tf.read_text()}"
    assert "CLAUDECODE" not in c["env"] and "CLAUDE_CODE_CHILD_SESSION" not in c["env"]
    # opening again reuses the running server
    assert d.notebook_open("exp1")["url"] == r["url"] and len(calls) == 1
    assert d.project()["ideas"][0]["notebook_running"] is True
    d.notebook_close("exp1")
    assert c["proc"].terminated and not tf.exists()


def test_least_recently_opened_notebook_is_stopped(initialized: Path, arb, started,
                                                   monkeypatch) -> None:
    calls, start = started
    monkeypatch.setattr(arbd, "MAX_NOTEBOOKS", 2)
    for n in ("a1", "a2", "a3"):
        arb(initialized, "idea", "new", n, "--hypothesis", "h", "--metric", "m")
    d = _daemon(initialized, start)
    d.notebook_open("a1"); d.notebook_open("a2"); d.notebook_open("a1"); d.notebook_open("a3")
    assert calls[1]["proc"].terminated  # a2 was the least recent
    assert not calls[0]["proc"].terminated and not calls[2]["proc"].terminated
    d.shutdown()
    assert all(c["proc"].terminated for c in calls)


def test_a_dead_server_is_restarted(idea: Path, initialized: Path, started) -> None:
    calls, start = started
    d = _daemon(initialized, start)
    d.notebook_open("exp1")
    calls[0]["proc"].returncode = 1
    d.notebook_open("exp1")
    assert len(calls) == 2


def test_closed_idea_has_no_notebook_to_open(idea: Path, initialized: Path, arb, started) -> None:
    arb(initialized, "idea", "drop", "exp1")
    d = _daemon(initialized, started[1])
    assert d.project()["ideas"][0]["worktree"] is None
    reply = d.handle({"id": 1, "method": "notebook.open", "params": {"idea": "exp1"}})
    assert "no worktree" in reply["error"]["message"]
    assert "no idea named" in d.handle(
        {"id": 2, "method": "notebook.open", "params": {"idea": "nope"}})["error"]["message"]


def test_failed_start_cleans_up(idea: Path, initialized: Path, started) -> None:
    calls, start = started

    def never_ready(port, proc):
        raise ArbError("marimo did not answer")

    d = arbd.Daemon(initialized, start=start, ready=never_ready)
    reply = d.handle({"id": 1, "method": "notebook.open", "params": {"idea": "exp1"}})
    assert "did not answer" in reply["error"]["message"]
    assert calls[0]["proc"].terminated


def test_dispatch_errors(initialized: Path, started) -> None:
    d = _daemon(initialized, started[1])
    assert d.handle({"id": 7, "method": "hello"})["result"]["protocol"] == arbd.PROTOCOL
    assert "unknown method" in d.handle({"id": 1, "method": "approve"})["error"]["message"]
    assert "missing param" in d.handle({"id": 1, "method": "notebook.open"})["error"]["message"]


def test_needs_an_initialized_project(repo: Path) -> None:
    with pytest.raises(ArbError):
        arbd.Daemon(repo)


def test_socket_round_trip(initialized: Path, started, tmp_path_factory) -> None:
    sock = Path("/tmp") / f"arbd-test-{id(started)}.sock"
    d = _daemon(initialized, started[1])
    server = arbd._Server(str(sock), arbd._handler(d))
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    try:
        with socket.socket(socket.AF_UNIX) as c:
            c.connect(str(sock))
            f = c.makefile("rwb")
            f.write(b'{"id": 1, "method": "project"}\n{"id": 2, "method": "nope"}\nnot json\n')
            f.flush()
            replies = [json.loads(f.readline()) for _ in range(3)]
        assert replies[0]["result"]["ideas"] == []
        assert replies[1]["id"] == 2 and "unknown method" in replies[1]["error"]["message"]
        assert "bad request" in replies[2]["error"]["message"]
    finally:
        server.shutdown(); server.server_close(); sock.unlink(missing_ok=True)


def test_serve_refuses_a_socket_path_too_long_for_macos(initialized: Path, tmp_path) -> None:
    with pytest.raises(ArbError, match="too long"):
        arbd.serve(tmp_path / ("x" * 120), initialized, stop_on_stdin_eof=False)


# ------------------------------------------------------------ guards (no approval path)


def _imported_modules(path: Path) -> set[str]:
    mods: set[str] = set()
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.ImportFrom):
            mods.add((node.module or "").split(".")[0] if node.level == 0 else (node.module or ""))
            mods.update(a.name for a in node.names)
        elif isinstance(node, ast.Import):
            mods.update(a.name for a in node.names)
    return mods


def test_arbd_has_no_approval_path() -> None:
    mods = _imported_modules(SRC)
    for forbidden in ("submission", "human", "lessons", "monitor", "backends", "db"):
        assert forbidden not in mods, forbidden
    text = SRC.read_text()
    for word in ("approve", "accept", "Decision", "sbatch", "_guarded_submit"):
        # only the docstring may mention approval (to say there is none)
        code = "\n".join(l for l in text.splitlines()[14:] if not l.lstrip().startswith("#"))
        assert word not in code, word


# ------------------------------------------------------------ real marimo


@pytest.mark.skipif(importlib.util.find_spec("marimo") is None, reason="needs marimo")
def test_real_marimo_serves_the_notebook(idea: Path, initialized: Path) -> None:
    d = arbd.Daemon(initialized)
    try:
        url = d.notebook_open("exp1")["url"]
        jar = urllib.request.build_opener(urllib.request.HTTPCookieProcessor())
        with jar.open(url, timeout=10) as r:  # token -> session cookie -> the notebook
            assert r.status == 200 and "/auth/login" not in r.url
        port = int(url.split(":")[2].split("/")[0])
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=10) as r:
            assert "/auth/login" in r.url  # no token: the login page
    finally:
        d.shutdown()
