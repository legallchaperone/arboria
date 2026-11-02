"""Thin wrapper around the `git` executable."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path
from typing import Mapping

from .errors import ArbError

# Variables that would redirect git away from the `cwd` we pass explicitly.
_SCRUBBED_ENV = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR", "GIT_PREFIX")

# Identity used when the user has none configured (hidden refs / backup repo only).
FALLBACK_IDENTITY = {
    "GIT_AUTHOR_NAME": "arboria",
    "GIT_AUTHOR_EMAIL": "arboria@localhost",
    "GIT_COMMITTER_NAME": "arboria",
    "GIT_COMMITTER_EMAIL": "arboria@localhost",
}


class GitError(ArbError):
    """A git command exited with a non-zero status."""

    def __init__(self, args: list[str], cwd: Path, returncode: int, stderr: str):
        self.args_list = args
        self.cwd = cwd
        self.returncode = returncode
        self.stderr = stderr.strip()
        detail = f": {self.stderr}" if self.stderr else ""
        super().__init__(f"`git {' '.join(args)}` failed in {cwd} (exit {returncode}){detail}")


def _env(extra: Mapping[str, str] | None) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in _SCRUBBED_ENV}
    if extra:
        env.update(extra)
    return env


def run(
    args: list[str],
    cwd: Path | str,
    *,
    env: Mapping[str, str] | None = None,
    check: bool = True,
    strip: bool = True,
) -> subprocess.CompletedProcess[str]:
    """Run `git <args>` in `cwd`; raise GitError on failure when `check` is set."""
    cwd = Path(cwd)
    try:
        proc = subprocess.run(
            ["git", *args],
            cwd=cwd,
            env=_env(env),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="surrogateescape",
            stdin=subprocess.DEVNULL,
        )
    except FileNotFoundError as exc:  # git binary missing or cwd vanished
        raise ArbError(f"cannot run git in {cwd}: {exc}") from exc
    if check and proc.returncode != 0:
        raise GitError(args, cwd, proc.returncode, proc.stderr)
    if strip:
        proc.stdout = proc.stdout.strip()
    return proc


def out(args: list[str], cwd: Path | str, *, env: Mapping[str, str] | None = None) -> str:
    """Run git and return its stripped stdout."""
    return run(args, cwd, env=env).stdout


def ok(args: list[str], cwd: Path | str, *, env: Mapping[str, str] | None = None) -> bool:
    """Run git and return True when it exits 0 (never raises GitError)."""
    return run(args, cwd, env=env, check=False).returncode == 0


def rev_parse(rev: str, cwd: Path | str) -> str | None:
    """Resolve `rev` to a commit hash, or None if it does not resolve."""
    proc = run(["rev-parse", "--verify", "--quiet", f"{rev}^{{commit}}"], cwd, check=False)
    return proc.stdout if proc.returncode == 0 else None


def ref_exists(ref: str, cwd: Path | str) -> bool:
    return ok(["show-ref", "--verify", "--quiet", ref], cwd)


def identity_env(cwd: Path | str) -> dict[str, str]:
    """Fallback author/committer env vars if the user has no git identity configured."""
    if ok(["var", "GIT_COMMITTER_IDENT"], cwd) and ok(["var", "GIT_AUTHOR_IDENT"], cwd):
        return {}
    return dict(FALLBACK_IDENTITY)
