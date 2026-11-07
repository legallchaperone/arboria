"""arbd: the core process behind the Mac app (ADR 0004).

The app (Swift) starts `arb arbd --socket PATH --project ROOT` and talks newline-delimited
JSON over that Unix socket: one request `{"id", "method", "params"}` per line, one reply
`{"id", "result"}` or `{"id", "error": {"message"}}` per line. arbd exits when its stdin
closes (the app went away) and then stops every marimo server it started.

Phase 1 (P1) is read-only on the ledger (`ui.data.ReadOnlyLedger`, re-opened per request
because `arb` in a terminal writes concurrently) plus starting/stopping `marimo edit` for
an idea's notebook. There is no approval method here, nor anywhere in the app's API:
approving stays in the human's terminal (`human.require_terminal`) and, later, Touch ID
signatures verified by arbd; `tests/test_arbd.py` scans this file for that.
"""

from __future__ import annotations

import json
import os
import secrets
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from .config import project_config
from .errors import ArbError
from .paths import STATE_DIR_NAME, locate
from .ui import data as board_data
from .ui.data import SCHEMA_VERSION, ReadOnlyLedger

PROTOCOL = 1
HOST = "127.0.0.1"
MAX_NOTEBOOKS = 3  # marimo servers kept alive; the least recently opened is stopped
READY_TIMEOUT = 60.0
MAX_LINE = 1 << 20


def child_env(base: dict[str, str] | None = None) -> dict[str, str]:
    """The environment for processes arbd starts: never leak a Claude Code session
    (CLAUDECODE / CLAUDE_CODE_*) into the human's kernel."""
    env = dict(os.environ if base is None else base)
    for k in list(env):
        if k == "CLAUDECODE" or k.startswith("CLAUDE_CODE_"):
            del env[k]
    return env


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind((HOST, 0))
        return s.getsockname()[1]


@dataclass
class Notebook:
    idea: str
    port: int
    token: str
    proc: subprocess.Popen
    token_file: Path

    @property
    def url(self) -> str:
        return f"http://{HOST}:{self.port}/?access_token={self.token}"

    def stop(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.token_file.unlink(missing_ok=True)


def wait_ready(port: int, proc: subprocess.Popen, timeout: float = READY_TIMEOUT) -> None:
    """Poll marimo's unauthenticated health endpoint until it answers."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise ArbError(f"marimo exited with status {proc.returncode} before it was ready")
        try:
            with urllib.request.urlopen(f"http://{HOST}:{port}/health", timeout=1) as r:
                if r.status == 200:
                    return
        except (urllib.error.URLError, OSError):
            pass
        time.sleep(0.2)
    raise ArbError(f"marimo did not answer on port {port} within {timeout:.0f}s")


def marimo_command(notebook: Path, port: int, token_file: Path) -> list[str]:
    return [sys.executable, "-m", "marimo", "edit", str(notebook), "--headless",
            "--host", HOST, "--port", str(port), "--token-password-file", str(token_file),
            "--skip-update-check", "--no-sandbox"]


Starter = Callable[[list[str], Path, dict[str, str]], subprocess.Popen]


def _popen(cmd: list[str], cwd: Path, env: dict[str, str]) -> subprocess.Popen:
    return subprocess.Popen(cmd, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                            start_new_session=True)


class Daemon:
    """The state behind the socket: one project, a few marimo servers."""

    def __init__(self, project: Path | str, *, start: Starter = _popen,
                 ready: Callable[[int, subprocess.Popen], None] = wait_ready):
        self.ctx = locate(project).require_initialized()
        self.root = self.ctx.main
        self._start, self._ready = start, ready
        self._notebooks: OrderedDict[str, Notebook] = OrderedDict()
        self._lock = threading.Lock()

    # -- reads (ledger re-opened per call)

    def _ledger(self) -> ReadOnlyLedger:
        ledger = ReadOnlyLedger(self.root / STATE_DIR_NAME)
        if ledger.version() != SCHEMA_VERSION:
            v = ledger.version()
            ledger.close()
            raise ArbError(f"ledger schema v{v}, arbd reads v{SCHEMA_VERSION}: run any arb "
                           "command (e.g. `arb status`) once to upgrade it, or upgrade arb")
        return ledger

    def _ideas(self) -> list[dict[str, Any]]:
        with self._ledger() as ledger:
            rows = ledger.all(
                "SELECT i.id, i.name, i.status, i.hypothesis, i.metric, i.worktree_path,"
                " i.created_at, p.name AS parent,"
                " (SELECT COUNT(*) FROM runs r WHERE r.idea_id = i.id) AS run_count"
                " FROM ideas i LEFT JOIN ideas p ON p.id = i.parent_id ORDER BY i.id")
        for r in rows:
            wt = Path(r.pop("worktree_path") or "")
            r["worktree"] = str(wt) if r["status"] == "open" and wt.is_dir() else None
            r["has_notebook"] = bool(r["worktree"]) and (wt / "notebook.py").exists()
            r["notebook_running"] = r["name"] in self._notebooks
        return rows

    def project(self) -> dict[str, Any]:
        try:
            project_id = project_config(self.ctx).project_id
        except ArbError:
            project_id = None
        return {"root": str(self.root), "name": self.root.name, "project_id": project_id,
                "ideas": self._ideas()}

    def inbox(self) -> dict[str, Any]:
        """What needs the human (the home page): awaiting cards, finished runs not yet
        written about, proposed lessons; then open todos (they wait for the agent). Every
        action is a command for the human's terminal, never something arbd does."""
        board = board_data.load(self.root)
        idea_name = {i["id"]: i["name"] for i in board.ideas}
        items: list[dict[str, Any]] = []
        for r in board.awaiting():
            card = board.cards.get(r["id"]) or {}
            su = card.get("est_su")
            items.append({"kind": "card", "tone": "warn", "id": r["id"],
                          "idea": idea_name.get(r["idea_id"]), "text": r["change_reason"],
                          "meta": (f"≈ {round(su)} SU · " if su is not None else "")
                          + f"card {(r['card_hash'] or '')[:8]}",
                          "when": r["updated_at"], "command": f"arb approve {r['id']}"})
        tone = {"succeeded": "ok", "failed": "bad", "cancelled": "none"}
        for r in board.finished_unseen():
            items.append({"kind": "finished", "tone": tone.get(r["status"], "none"),
                          "id": r["id"], "idea": idea_name.get(r["idea_id"]),
                          "text": r["summary"] or r["change_reason"], "status": r["status"],
                          "meta": r["change_reason"], "when": r["finished_at"],
                          "command": f"arb note \"…\" --run {r['id']}"})
        for lesson in board.lessons:
            items.append({"kind": "lesson", "tone": "none", "id": lesson["id"], "idea": None,
                          "text": lesson["text"], "meta": "经验待确认", "when": lesson["created_at"],
                          "command": f"arb lesson accept {lesson['id']}"})
        todos = [{"kind": "todo", "tone": "none", "id": t["id"], "idea": t["idea"],
                  "text": t["text"], "when": t["created_at"]} for t in board.todos]
        return {"needs_you": len(items), "items": items, "todos": todos}

    def _open_idea(self, name: str) -> dict[str, Any]:
        idea = next((i for i in self._ideas() if i["name"] == name), None)
        if idea is None:
            raise ArbError(f"no idea named {name!r}")
        if idea["worktree"] is None:
            raise ArbError(f"idea {name!r} is {idea['status']} and has no worktree")
        if not idea["has_notebook"]:
            raise ArbError(f"idea {name!r} has no notebook yet; run `arb idea notebook {name}`")
        return idea

    # -- notebooks

    def notebook_open(self, idea: str) -> dict[str, Any]:
        info = self._open_idea(idea)
        with self._lock:
            nb = self._notebooks.get(idea)
            if nb is not None and nb.proc.poll() is None:
                self._notebooks.move_to_end(idea)
                return {"idea": idea, "url": nb.url, "worktree": info["worktree"]}
            if nb is not None:
                nb.stop()
                del self._notebooks[idea]
            worktree = Path(info["worktree"])
            port, token = free_port(), secrets.token_urlsafe(24)
            fd, tf = tempfile.mkstemp(prefix="arbd-token-")
            with os.fdopen(fd, "w") as fh:  # mkstemp files are 0600
                fh.write(token)
            # marimo resolves css_file against the path it opened, so open the worktree's
            # notebook.py link, not .arboria/notebooks/<idea>.py.
            proc = self._start(marimo_command(worktree / "notebook.py", port, Path(tf)),
                               worktree, child_env())
            nb = Notebook(idea, port, token, proc, Path(tf))
            try:
                self._ready(port, proc)
            except Exception:
                nb.stop()
                raise
            self._notebooks[idea] = nb
            while len(self._notebooks) > MAX_NOTEBOOKS:
                _, old = self._notebooks.popitem(last=False)
                old.stop()
            return {"idea": idea, "url": nb.url, "worktree": info["worktree"]}

    def notebook_close(self, idea: str) -> dict[str, Any]:
        with self._lock:
            nb = self._notebooks.pop(idea, None)
        if nb is not None:
            nb.stop()
        return {"idea": idea, "stopped": nb is not None}

    def shutdown(self) -> None:
        with self._lock:
            nbs = list(self._notebooks.values())
            self._notebooks.clear()
        for nb in nbs:
            nb.stop()

    # -- dispatch

    def handle(self, request: dict[str, Any]) -> dict[str, Any]:
        rid = request.get("id")
        method, params = request.get("method"), request.get("params") or {}
        methods: dict[str, Callable[[], Any]] = {
            "hello": lambda: {"protocol": PROTOCOL, "root": str(self.root)},
            "project": self.project,
            "inbox": self.inbox,
            "notebook.open": lambda: self.notebook_open(str(params["idea"])),
            "notebook.close": lambda: self.notebook_close(str(params["idea"])),
        }
        if method not in methods:
            return {"id": rid, "error": {"message": f"unknown method {method!r}"}}
        try:
            return {"id": rid, "result": methods[method]()}
        except (ArbError, KeyError, OSError) as exc:
            msg = f"missing param {exc}" if isinstance(exc, KeyError) else str(exc)
            return {"id": rid, "error": {"message": msg}}


def _handler(daemon: Daemon) -> type[socketserver.StreamRequestHandler]:
    class Handler(socketserver.StreamRequestHandler):
        def handle(self) -> None:
            while True:
                line = self.rfile.readline(MAX_LINE)
                if not line:
                    return
                try:
                    req = json.loads(line)
                    if not isinstance(req, dict):
                        raise ValueError("request must be an object")
                    reply = daemon.handle(req)
                except ValueError as exc:
                    reply = {"id": None, "error": {"message": f"bad request: {exc}"}}
                self.wfile.write((json.dumps(reply, ensure_ascii=False) + "\n").encode())
                self.wfile.flush()
    return Handler


class _Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True


def serve(socket_path: Path | str, project: Path | str, *,
          stop_on_stdin_eof: bool = True) -> None:
    """Serve until stdin closes (the app's pipe) or the server is shut down."""
    sock = Path(socket_path)
    if len(os.fsencode(sock)) >= 104:  # macOS sun_path limit
        raise ArbError(f"socket path too long for a Unix socket: {sock}")
    sock.unlink(missing_ok=True)
    daemon = Daemon(project)
    old_umask = os.umask(0o177)  # socket is 0600: only this user
    try:
        server = _Server(str(sock), _handler(daemon))
    finally:
        os.umask(old_umask)
    if stop_on_stdin_eof:
        def watch_stdin() -> None:
            try:
                while sys.stdin.buffer.read(4096):
                    pass
            finally:
                server.shutdown()
        threading.Thread(target=watch_stdin, daemon=True).start()
    try:
        print(json.dumps({"ready": True, "socket": str(sock), "protocol": PROTOCOL}), flush=True)
        server.serve_forever()
    finally:
        server.server_close()
        daemon.shutdown()
        sock.unlink(missing_ok=True)
