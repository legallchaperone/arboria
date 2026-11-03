"""Running Claude Code sessions per worktree, and posting a short note into one.

Claude Code (v2.1.224+, macOS/Linux) binds an inbox socket per session and exports its
path to hooks as `CLAUDE_CODE_MESSAGING_SOCKET`
(https://code.claude.com/docs/en/cross-session-messaging.md#the-sessions-inbox-socket).
The SessionStart hook records it in `<state dir>/sessions/<sha of worktree>.json` (0600);
when the human's notebook creates a command todo, `lab` posts one line there so the
session picks the todo up without the human copy-pasting.

Wire format (the docs only say "plain text"; Claude Code's own debug log shows the
frame): one JSON line `{"type":"user","message":{"role":"user","content":"..."}}`.

Deliberately *not* done: the per-session `CLAUDE_CODE_MESSAGING_TOKEN` is neither stored
nor sent. The docs make the auth line optional on macOS/Linux, and the token is how a
session recognises its *own child processes*; the notebook kernel is not one, so it posts
as what it is, an unverified local peer, and the session's inbound controls
(`crossSessionInbound`, permission-mode defaults) decide. Everything here is best effort:
any failure returns False and nothing is raised.
"""

from __future__ import annotations

import hashlib
import json
import os
import socket
import stat
from pathlib import Path
from typing import Any, Mapping

from .db import now

SESSIONS_DIR = "sessions"
SOCKET_ENV = "CLAUDE_CODE_MESSAGING_SOCKET"
TIMEOUT_S = 2.0
# Process names that sit between Claude Code and the hook (`sh -c "arb hook …"`, uv, …).
_WRAPPERS = {"sh", "bash", "zsh", "dash", "fish", "env", "uv", "uvx", "arb", "timeout"}


def sessions_dir(state_dir: Path) -> Path:
    return Path(state_dir) / SESSIONS_DIR


def key(worktree: Path | str) -> str:
    return hashlib.sha256(str(Path(worktree).resolve()).encode("utf-8")).hexdigest()[:16]


def session_file(state_dir: Path, worktree: Path | str) -> Path:
    return sessions_dir(state_dir) / f"{key(worktree)}.json"


# ---------------------------------------------------------------- processes


def _stat_fields(pid: int) -> tuple[str, int, int] | None:
    """(comm, ppid, start time in clock ticks) from /proc/<pid>/stat (Linux)."""
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
        comm = text[text.index("(") + 1:text.rindex(")")]
        rest = text[text.rindex(")") + 2:].split()
        return comm, int(rest[1]), int(rest[19])
    except (OSError, ValueError, IndexError):
        return None


def start_time(pid: int) -> int | None:
    fields = _stat_fields(pid)
    return None if fields is None else fields[2]


def pid_alive(pid: int, started: int | None = None) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:  # exists, another user's: not our session
        return False
    except OSError:
        return False
    if started is not None:
        now_start = start_time(pid)
        if now_start is not None and now_start != started:  # pid reused
            return False
    return True


def claude_pid(limit: int = 6) -> int | None:
    """The Claude Code process that ran this hook, best effort: the nearest ancestor named
    `claude` (or the first one that is not a shell/wrapper)."""
    from .human import _proc_parent

    pid = os.getppid()
    fallback = None
    for _ in range(limit):
        info = _proc_parent(pid)
        if info is None:
            break
        ppid, comm = info
        name = os.path.basename(comm or "").lower()
        if name.startswith("claude"):
            return pid
        if fallback is None and name not in _WRAPPERS and not name.startswith("python"):
            fallback = pid
        if ppid <= 1 or ppid == pid:
            break
        pid = ppid
    return fallback


# ---------------------------------------------------------------- the hook side


def _write_private(path: Path, data: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(data, fh, ensure_ascii=False)
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
    except BaseException:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise


def record(state_dir: Path, worktree: Path, env: Mapping[str, str] | None = None,
           pid: int | None = None) -> Path | None:
    """Remember this session's inbox socket for `worktree` (called by the SessionStart
    hook). Returns the file written, or None when the session has no inbox."""
    env = os.environ if env is None else env
    sock = env.get(SOCKET_ENV)
    if not sock or not os.path.isabs(sock):
        return None
    pid = claude_pid() if pid is None else pid
    entry = {"socket": sock, "pid": pid, "pid_start": start_time(pid) if pid else None,
             "worktree": str(Path(worktree).resolve()), "started_at": now()}
    path = session_file(state_dir, worktree)
    _write_private(path, entry)
    clean(state_dir)
    return path


def _usable_socket(path: str) -> bool:
    """A Unix socket we own, not a symlink (Claude Code refuses symlinked targets too)."""
    try:
        st = os.lstat(path)
    except OSError:
        return False
    return stat.S_ISSOCK(st.st_mode) and st.st_uid == os.getuid()


def _read(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def _live(entry: dict[str, Any] | None) -> bool:
    if not entry or not isinstance(entry.get("socket"), str):
        return False
    pid = entry.get("pid")
    if isinstance(pid, int) and pid > 0:
        started = entry.get("pid_start")
        if not pid_alive(pid, started if isinstance(started, int) else None):
            return False
    return _usable_socket(entry["socket"])


def clean(state_dir: Path) -> int:
    """Delete entries whose session is gone. Returns how many were removed."""
    removed = 0
    folder = sessions_dir(state_dir)
    if not folder.is_dir():
        return 0
    for path in folder.glob("*.json"):
        if not _live(_read(path)):
            try:
                path.unlink()
                removed += 1
            except OSError:
                pass
    return removed


def live_session(state_dir: Path, worktree: Path | str) -> dict[str, Any] | None:
    path = session_file(state_dir, worktree)
    entry = _read(path)
    if entry is None:
        return None
    if entry.get("worktree") != str(Path(worktree).resolve()) or not _live(entry):
        try:
            path.unlink()
        except OSError:
            pass
        return None
    return entry


# ---------------------------------------------------------------- the notebook side


def frame(text: str) -> bytes:
    msg = {"type": "user", "message": {"role": "user", "content": text}}
    return (json.dumps(msg, ensure_ascii=False) + "\n").encode("utf-8")


def send(sock_path: str, text: str, timeout: float = TIMEOUT_S) -> bool:
    """Post one message to an inbox socket. True if it was written."""
    if not _usable_socket(sock_path):
        return False
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            s.connect(sock_path)
            s.sendall(frame(text))
            try:
                s.shutdown(socket.SHUT_WR)
            except OSError:
                pass
        return True
    except (OSError, ValueError):
        return False


def notify(state_dir: Path, worktree: Path | str, text: str,
           timeout: float = TIMEOUT_S) -> bool:
    """Send `text` to the live session of `worktree`, if any. Never raises."""
    try:
        entry = live_session(state_dir, worktree)
        return entry is not None and send(entry["socket"], text, timeout)
    except Exception:
        return False
