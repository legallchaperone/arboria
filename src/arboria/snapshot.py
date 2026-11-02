"""Snapshot a worktree's exact state (incl. uncommitted + untracked files) as a hidden commit.

The user's branch, index and working tree are never touched: we build the tree in a
throw-away index file (GIT_INDEX_FILE) and create a dangling commit with `commit-tree`;
callers anchor it under refs/arboria/...
"""

from __future__ import annotations

import os
import sqlite3
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from . import gitutil
from .errors import ArbError

MB = 1024 * 1024
DEFAULT_FILE_LIMIT = 50 * MB
DEFAULT_TOTAL_LIMIT = 200 * MB


class LargeUntrackedError(ArbError):
    """Untracked files exceed the size limits and --allow-large was not given."""


@dataclass
class SnapshotResult:
    tree: str
    commit: str
    reused: bool
    warnings: list[str] = field(default_factory=list)


def _fmt_mb(n: int) -> str:
    return f"{n / MB:.1f} MB"


def size_warnings(
    worktree: Path, paths: list[str], file_limit: int, total_limit: int
) -> list[str]:
    """Warnings for untracked files that are individually or collectively too large."""
    warnings: list[str] = []
    total = 0
    for rel in paths:
        try:
            size = os.lstat(worktree / rel).st_size
        except OSError:
            continue
        total += size
        if size > file_limit:
            warnings.append(f"large untracked file {rel} ({_fmt_mb(size)} > {_fmt_mb(file_limit)})")
    if total > total_limit:
        warnings.append(f"untracked files total {_fmt_mb(total)} (> {_fmt_mb(total_limit)})")
    return warnings


def write_tree(
    worktree: Path,
    *,
    allow_large: bool = False,
    file_limit: int | None = None,
    total_limit: int | None = None,
    warn: Callable[[str], None] | None = None,
) -> tuple[str, str | None, list[str]]:
    """Return (tree hash, HEAD commit or None, warnings) for the worktree's current state.

    Limits default to DEFAULT_FILE_LIMIT / DEFAULT_TOTAL_LIMIT (bytes).
    """
    worktree = Path(worktree)
    file_limit = DEFAULT_FILE_LIMIT if file_limit is None else file_limit
    total_limit = DEFAULT_TOTAL_LIMIT if total_limit is None else total_limit
    head = gitutil.rev_parse("HEAD", worktree)
    with tempfile.TemporaryDirectory(prefix="arboria-index-") as tmp:
        env = {"GIT_INDEX_FILE": str(Path(tmp) / "index")}
        if head:
            gitutil.run(["read-tree", head], worktree, env=env)
        else:
            gitutil.run(["read-tree", "--empty"], worktree, env=env)
        # Files `add -A` would newly add: not in HEAD and not ignored.
        listing = gitutil.run(
            ["ls-files", "-z", "--others", "--exclude-standard"], worktree, env=env, strip=False
        ).stdout
        untracked = [p for p in listing.split("\0") if p]
        warnings = size_warnings(worktree, untracked, file_limit, total_limit)
        for w in warnings:
            if warn:
                warn(w)
        if warnings and not allow_large:
            raise LargeUntrackedError(
                "refusing to snapshot: " + "; ".join(warnings)
                + ". Add them to .gitignore, or pass --allow-large."
            )
        gitutil.run(["add", "-A", "--", "."], worktree, env=env)
        tree = gitutil.out(["write-tree"], worktree, env=env)
    return tree, head, warnings


def find_existing_commit(conn: sqlite3.Connection, tree: str, worktree: Path) -> str | None:
    """A previously recorded snapshot commit with the same tree, if it still exists."""
    rows = conn.execute(
        "SELECT snapshot_commit FROM runs WHERE tree_hash = ? ORDER BY created_at, id", (tree,)
    ).fetchall()
    for (commit,) in rows:
        if gitutil.ok(["cat-file", "-e", f"{commit}^{{commit}}"], worktree):
            return commit
    return None


def commit_tree(worktree: Path, tree: str, parent: str | None, message: str) -> str:
    args = ["commit-tree", "--no-gpg-sign"]
    if parent:
        args += ["-p", parent]
    args += ["-m", message, tree]
    return gitutil.out(args, worktree, env=gitutil.identity_env(worktree))


def snapshot(
    worktree: Path,
    conn: sqlite3.Connection,
    message: str,
    *,
    allow_large: bool = False,
    file_limit: int | None = None,
    total_limit: int | None = None,
    warn: Callable[[str], None] | None = None,
) -> SnapshotResult:
    """Snapshot `worktree`, reusing an earlier run's commit when the tree is identical."""
    tree, head, warnings = write_tree(
        worktree,
        allow_large=allow_large,
        file_limit=file_limit,
        total_limit=total_limit,
        warn=warn,
    )
    existing = find_existing_commit(conn, tree, worktree)
    if existing:
        return SnapshotResult(tree=tree, commit=existing, reused=True, warnings=warnings)
    commit = commit_tree(worktree, tree, head, message)
    return SnapshotResult(tree=tree, commit=commit, reused=False, warnings=warnings)


def update_ref(worktree: Path, ref: str, commit: str, reason: str = "arboria") -> None:
    gitutil.run(["update-ref", "-m", reason, ref, commit], worktree)
