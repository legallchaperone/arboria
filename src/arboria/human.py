"""Checks that a human, in their own terminal, is the one running an approval.

`arb approve` and `arb lesson accept` call `require_human` before anything is
recorded. It refuses inside Claude Code (which sets CLAUDECODE / CLAUDE_CODE_ENTRYPOINT
for its tools) and without a TTY on stdin, then asks the human to type the id of the
thing being approved. The returned provenance (process chain, author) goes into the
Decision payload.

This is a guard rail, not a sandbox: the hard guarantee for the cluster stays
`submission._guarded_submit`, which refuses without a human approval Decision.

Tests replace `stdin_is_tty` / `read_confirmation` (module attributes, looked up at
call time) and control the environment with monkeypatch.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator, Mapping

from .errors import ArbError

CLAUDE_ENV_VARS = ("CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT")
REFUSAL = "批准只能在你自己的终端里做：请在终端运行 {command}"
MISMATCH = "输入的 {got!r} 与 {expected} 不符，已取消（没有做任何改动）"
MAX_ANCESTORS = 6


IN_COMMAND = ("{command} 不能在 notebook 命令（lab.{name}）里运行：命令代码是 Claude 写的；"
              "请在你自己的终端里运行 {command}")

# Notebook commands (lab.<name>) currently executing in this process. Their code is
# written by Claude and runs in the human's kernel, so approvals refuse while one runs.
# A guard rail, not a sandbox: such code could undo it, which is why the human reviews
# command diffs (README) and `_guarded_submit` still needs a recorded human Decision.
_commands_running: dict[int, str] = {}
_commands_lock = threading.Lock()


class NotHumanError(ArbError):
    """Refused: this does not look like the human's own terminal."""


@contextmanager
def running_command(name: str) -> Iterator[None]:
    """Mark a notebook command as executing (see `require_not_claude`)."""
    key = id(object())
    with _commands_lock:
        _commands_running[key] = name
    try:
        yield
    finally:
        with _commands_lock:
            _commands_running.pop(key, None)


def command_running() -> str | None:
    with _commands_lock:
        return next(iter(_commands_running.values()), None)


def in_claude_code(env: Mapping[str, str] | None = None) -> str | None:
    """The Claude Code variable present in `env` (default: os.environ), if any."""
    env = os.environ if env is None else env
    for var in CLAUDE_ENV_VARS:
        if var in env:
            return var
    return None


def stdin_is_tty() -> bool:
    try:
        return sys.stdin is not None and sys.stdin.isatty()
    except (ValueError, OSError):  # closed / detached stdin
        return False


def read_confirmation(prompt: str) -> str:
    """Prompt on stderr (stdout stays clean) and read one line from stdin."""
    sys.stderr.write(prompt)
    sys.stderr.flush()
    line = sys.stdin.readline()
    if not line:
        raise ArbError("no confirmation typed (end of input); nothing was changed")
    return line.strip()


def _proc_parent(pid: int) -> tuple[int, str] | None:
    """(parent pid, command name) of `pid` from /proc, falling back to `ps`."""
    stat = Path(f"/proc/{pid}/stat")
    try:
        text = stat.read_text()
        # "pid (comm) state ppid ..." where comm may itself contain spaces or ')'
        comm = text[text.index("(") + 1:text.rindex(")")]
        ppid = int(text[text.rindex(")") + 2:].split()[1])
        return ppid, comm
    except (OSError, ValueError, IndexError):
        pass
    try:
        proc = subprocess.run(["ps", "-o", "ppid=,comm=", "-p", str(pid)], capture_output=True,
                              text=True, timeout=5, stdin=subprocess.DEVNULL)
        ppid_s, _, comm = proc.stdout.strip().partition(" ")
        return int(ppid_s), comm.strip()
    except (OSError, ValueError, subprocess.SubprocessError):
        return None


def process_chain(pid: int | None = None, limit: int = MAX_ANCESTORS) -> list[dict[str, Any]]:
    """This process and up to `limit` ancestors as [{pid, comm}], best effort."""
    pid = os.getpid() if pid is None else pid
    chain: list[dict[str, Any]] = []
    try:
        for _ in range(limit + 1):
            info = _proc_parent(pid)
            if info is None:
                chain.append({"pid": pid, "comm": None})
                break
            ppid, comm = info
            chain.append({"pid": pid, "comm": comm})
            if ppid <= 0 or ppid == pid:
                break
            pid = ppid
    except Exception:  # never let provenance break an approval
        pass
    return chain


def require_not_claude(command: str) -> None:
    """Refuse inside Claude Code (env check) or while a notebook command runs."""
    if in_claude_code():
        raise NotHumanError(REFUSAL.format(command=command))
    name = command_running()
    if name is not None:
        raise NotHumanError(IN_COMMAND.format(command=command, name=name))


def require_terminal(command: str) -> None:
    """Refuse inside Claude Code or without a TTY on stdin."""
    require_not_claude(command)
    if not stdin_is_tty():
        raise NotHumanError(REFUSAL.format(command=command))


def confirm_typed(expected: str) -> None:
    """The human types `expected` (a run or lesson id) to confirm."""
    got = read_confirmation(f"输入 {expected} 确认：")
    if got != expected:
        raise ArbError(MISMATCH.format(got=got, expected=expected))


def provenance() -> dict[str, Any]:
    """What is recorded in the Decision payload."""
    return {"author": "human", "pid": os.getpid(), "process_chain": process_chain(),
            "confirmed_by": "typed id on a TTY"}


def require_human(command: str, expected: str) -> dict[str, Any]:
    """Terminal + env checks, then the typed confirmation. Returns the provenance."""
    require_terminal(command)
    confirm_typed(expected)
    return provenance()
