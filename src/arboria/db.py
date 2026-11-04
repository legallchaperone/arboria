"""SQLite ledger: schema, migrations, validation, locking and git backup."""

from __future__ import annotations

import fcntl
import json
import os
import re
import sqlite3
import sys
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterator

from . import gitutil
from .errors import ArbError

CONFIDENCE = ("guess", "prelim", "confirmed", "rejected")
IDEA_STATUS = ("open", "merged", "dropped")
RUN_STATUS = (
    "draft",
    "awaiting_approval",
    "approved",
    "queued",
    "running",
    "succeeded",
    "failed",
    "cancelled",
    "rejected",
)
# Run statuses before schema v3 (the v1 CHECK constraint).
RUN_STATUS_V1 = RUN_STATUS[:-1]
# Runs whose jobs may still be on the cluster.
RUN_ACTIVE = ("approved", "queued", "running")
RUN_FINAL = ("succeeded", "failed", "cancelled")
JOB_STATES = ("submitting", "pending", "running", "completed", "failed", "cancelled", "timeout",
              "unknown")
JOB_TERMINAL = ("completed", "failed", "cancelled", "timeout")
SUBJECT_TYPES = ("run", "idea", "lesson")
LESSON_STATUS = ("proposed", "accepted", "retired")
LESSON_SOURCES = ("idea", "run", "none")
TASK_STATUS = ("open", "done", "dropped")
# plain: a free-text todo; command_new / command_revise: write or change a notebook command
# (the details live in tasks.payload_json, see commands.py); inspect: explain code for the
# human's notebook (`lab.inspect`, see inspection.py).
TASK_KINDS_V5 = ("plain", "command_new", "command_revise")
TASK_KINDS = (*TASK_KINDS_V5, "inspect")
# What `lab.inspect` can ask for: annotated source excerpts, pseudocode, simplified code,
# an architecture diagram (Mermaid).
INSPECT_VIEWS = ("code", "pseudo", "simple", "map")
SEED_STATUS = ("thinking", "planted", "dropped")
# Runs that an identical re-submit returns instead of creating a new one.
RUN_NOT_REUSABLE = ("rejected", "cancelled", "failed")

# Events after which the db is dumped to .arboria/backup/ledger.sql and committed.
BACKUP_EVENTS = frozenset({
    "idea_new", "idea_close", "note", "run_new", "decision",
    "lesson_propose", "lesson_accept", "lesson_reject", "lesson_retire",
    "run_submit", "run_approve", "run_queued", "run_reject", "run_cancel", "run_final",
    "run_config", "todo_new", "todo_done", "todo_drop", "idea_confidence", "idea_metric",
    "notebook_new", "seed_new", "seed_set", "seed_drop", "seed_revive",
    "seed_plant", "command_promote", "inspect_done",
})

# Limits for the free-text fields an LLM is allowed to write.
LIMITS = {"hypothesis": 300, "metric": 200, "change_reason": 300, "note": 1000, "summary": 1500,
          "lesson": 200, "result": 600, "todo": 1000, "gist": 300,
          # `lab.inspect` (inspection.py): the human's question, and what the agent delivers
          "question": 300, "inspect_title": 80, "inspect_summary": 600, "span_note": 600,
          "line_note": 200, "inspect_text": 8000, "mermaid": 8000}
SUMMARY_MAX_LINES = 10

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")


def _in(values: tuple[str, ...]) -> str:
    return "(" + ", ".join(f"'{v}'" for v in values) + ")"


SCHEMA_V1 = f"""
CREATE TABLE ideas (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    hypothesis    TEXT NOT NULL,
    metric        TEXT NOT NULL,
    parent_id     INTEGER REFERENCES ideas(id),
    confidence    TEXT NOT NULL DEFAULT 'guess' CHECK (confidence IN {_in(CONFIDENCE)}),
    status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN {_in(IDEA_STATUS)}),
    branch        TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    closed_at     TEXT
);
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    idea_id         INTEGER NOT NULL REFERENCES ideas(id),
    snapshot_commit TEXT NOT NULL,
    tree_hash       TEXT NOT NULL,
    config_json     TEXT,
    change_reason   TEXT NOT NULL,
    machine         TEXT,
    "partition"     TEXT,
    gpus            INTEGER,
    walltime        TEXT,
    est_cost        REAL,
    actual_cost     REAL,
    status          TEXT NOT NULL DEFAULT 'draft' CHECK (status IN {_in(RUN_STATUS_V1)}),
    review_json     TEXT,
    wandb_url       TEXT,
    summary         TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX runs_tree_hash ON runs(tree_hash);
CREATE INDEX runs_idea ON runs(idea_id);
CREATE TABLE decisions (
    id           TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('run', 'idea')),
    subject_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    choice       TEXT,
    decided_by   TEXT,
    created_at   TEXT NOT NULL
);
CREATE TABLE notes (
    id         INTEGER PRIMARY KEY,
    idea_id    INTEGER NOT NULL REFERENCES ideas(id),
    run_id     TEXT REFERENCES runs(id),
    text       TEXT NOT NULL,
    confidence TEXT CHECK (confidence IS NULL OR confidence IN {_in(CONFIDENCE)}),
    created_at TEXT NOT NULL
);
CREATE INDEX notes_idea ON notes(idea_id);
CREATE TABLE events (
    id           INTEGER PRIMARY KEY,
    ts           TEXT NOT NULL,
    kind         TEXT NOT NULL,
    subject      TEXT,
    payload_json TEXT
);
"""

# v2: lessons, and decisions may be about a lesson. SQLite cannot ALTER a CHECK
# constraint, so `decisions` is rebuilt with its rows copied over.
MIGRATION_V2 = f"""
CREATE TABLE lessons (
    id          TEXT PRIMARY KEY,
    text        TEXT NOT NULL,
    source_type TEXT NOT NULL DEFAULT 'none' CHECK (source_type IN {_in(LESSON_SOURCES)}),
    source_id   TEXT,
    status      TEXT NOT NULL DEFAULT 'proposed' CHECK (status IN {_in(LESSON_STATUS)}),
    created_at  TEXT NOT NULL,
    decided_at  TEXT,
    CHECK ((source_type = 'none') = (source_id IS NULL))
);
CREATE INDEX lessons_status ON lessons(status);
CREATE TABLE decisions_v2 (
    id           TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN {_in(SUBJECT_TYPES)}),
    subject_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    choice       TEXT,
    decided_by   TEXT,
    created_at   TEXT NOT NULL
);
INSERT INTO decisions_v2 (id, subject_type, subject_id, question, options_json, choice,
                          decided_by, created_at)
    SELECT id, subject_type, subject_id, question, options_json, choice, decided_by, created_at
    FROM decisions;
DROP TABLE decisions;
ALTER TABLE decisions_v2 RENAME TO decisions;
"""

_RUN_COLUMNS_V1 = (
    "id, idea_id, snapshot_commit, tree_hash, config_json, change_reason, machine, \"partition\","
    " gpus, walltime, est_cost, actual_cost, status, review_json, wandb_url, summary,"
    " created_at, updated_at"
)

# v3: cluster submission. runs.status gains 'rejected' (table rebuilt, rows kept; foreign
# keys are switched off during migrations so `notes.run_id` survives the swap), runs get
# the submitted request + approval card hash, and `jobs` holds one row per Slurm segment.
MIGRATION_V3 = f"""
CREATE TABLE runs_v3 (
    id              TEXT PRIMARY KEY,
    idea_id         INTEGER NOT NULL REFERENCES ideas(id),
    snapshot_commit TEXT NOT NULL,
    tree_hash       TEXT NOT NULL,
    config_json     TEXT,
    change_reason   TEXT NOT NULL,
    machine         TEXT,
    "partition"     TEXT,
    gpus            INTEGER,
    walltime        TEXT,
    est_cost        REAL,
    actual_cost     REAL,
    status          TEXT NOT NULL DEFAULT 'draft' CHECK (status IN {_in(RUN_STATUS)}),
    review_json     TEXT,
    wandb_url       TEXT,
    summary         TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    request_json    TEXT,
    card_hash       TEXT
);
INSERT INTO runs_v3 ({_RUN_COLUMNS_V1}) SELECT {_RUN_COLUMNS_V1} FROM runs;
DROP TABLE runs;
ALTER TABLE runs_v3 RENAME TO runs;
CREATE INDEX runs_tree_hash ON runs(tree_hash);
CREATE INDEX runs_idea ON runs(idea_id);
CREATE INDEX runs_status ON runs(status);
CREATE TABLE jobs (
    id           INTEGER PRIMARY KEY,
    run_id       TEXT NOT NULL REFERENCES runs(id),
    segment      INTEGER NOT NULL CHECK (segment >= 1),
    job_name     TEXT NOT NULL,
    job_id       TEXT,
    state        TEXT NOT NULL DEFAULT 'submitting' CHECK (state IN {_in(JOB_STATES)}),
    elapsed_s    INTEGER,
    billed_su    REAL,
    submitted_at TEXT,
    updated_at   TEXT NOT NULL,
    UNIQUE (run_id, segment)
);
CREATE INDEX jobs_run ON jobs(run_id);
"""

# v4: notebooks. Runs may be derived from a parent run (same snapshot, new overrides),
# decisions carry a payload (who approved, from which process chain), notes record
# their author, and `tasks` holds the todos a human writes in a notebook.
MIGRATION_V4 = f"""
ALTER TABLE runs ADD COLUMN parent_run TEXT REFERENCES runs(id);
ALTER TABLE decisions ADD COLUMN payload_json TEXT;
ALTER TABLE notes ADD COLUMN author TEXT;
CREATE INDEX runs_card_hash ON runs(idea_id, card_hash);
CREATE TABLE tasks (
    id         TEXT PRIMARY KEY,
    idea_id    INTEGER NOT NULL REFERENCES ideas(id),
    text       TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'open' CHECK (status IN {_in(TASK_STATUS)}),
    created_at TEXT NOT NULL,
    done_at    TEXT,
    result     TEXT,
    CHECK ((status = 'open') = (done_at IS NULL))
);
CREATE INDEX tasks_idea ON tasks(idea_id, status);
"""

# v5: notebook commands. A todo may ask Claude to write or revise a command; what the
# notebook call looked like (name, scope, args, description, idea) is in payload_json.
MIGRATION_V5 = f"""
ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN {_in(TASK_KINDS_V5)});
ALTER TABLE tasks ADD COLUMN payload_json TEXT;
"""

# v6: seeds, the repo-less ideas of the global home (paths.Home). One schema for every
# ledger: a project's has the (empty) seeds tables, a home's the (empty) idea tables.
# A seed may be planted into several repos (`plantings`, one row each). A todo belongs to
# an idea or to a seed, so `tasks` is rebuilt with idea_id nullable and a seed_id.
MIGRATION_V6 = f"""
CREATE TABLE seeds (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    gist        TEXT,
    hypothesis  TEXT,
    metric      TEXT,
    status      TEXT NOT NULL DEFAULT 'thinking' CHECK (status IN {_in(SEED_STATUS)}),
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE TABLE plantings (
    id          INTEGER PRIMARY KEY,
    seed_id     INTEGER NOT NULL REFERENCES seeds(id),
    repo        TEXT NOT NULL,
    project_id  TEXT,
    idea        TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX plantings_seed ON plantings(seed_id);
CREATE TABLE tasks_v6 (
    id           TEXT PRIMARY KEY,
    idea_id      INTEGER REFERENCES ideas(id),
    seed_id      INTEGER REFERENCES seeds(id),
    text         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN {_in(TASK_STATUS)}),
    created_at   TEXT NOT NULL,
    done_at      TEXT,
    result       TEXT,
    kind         TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN {_in(TASK_KINDS_V5)}),
    payload_json TEXT,
    CHECK ((status = 'open') = (done_at IS NULL)),
    CHECK ((idea_id IS NULL) != (seed_id IS NULL))
);
INSERT INTO tasks_v6 (id, idea_id, text, status, created_at, done_at, result, kind, payload_json)
    SELECT id, idea_id, text, status, created_at, done_at, result, kind, payload_json FROM tasks;
DROP TABLE tasks;
ALTER TABLE tasks_v6 RENAME TO tasks;
CREATE INDEX tasks_idea ON tasks(idea_id, status);
CREATE INDEX tasks_seed ON tasks(seed_id, status);
"""

# v7: `lab.inspect`. A todo may ask the agent to explain code (kind 'inspect'; `tasks` is
# rebuilt for its CHECK). What the agent delivers lands in `inspections`, one row per done
# todo, keyed for the notebook's cache by (idea, question, view, target) and bound to the
# code it describes (`code_ref` commit, `tree`). The source lines shown are extracted by
# arb from git at delivery time and kept in result_json.
MIGRATION_V7 = f"""
CREATE TABLE tasks_v7 (
    id           TEXT PRIMARY KEY,
    idea_id      INTEGER REFERENCES ideas(id),
    seed_id      INTEGER REFERENCES seeds(id),
    text         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN {_in(TASK_STATUS)}),
    created_at   TEXT NOT NULL,
    done_at      TEXT,
    result       TEXT,
    kind         TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN {_in(TASK_KINDS)}),
    payload_json TEXT,
    CHECK ((status = 'open') = (done_at IS NULL)),
    CHECK ((idea_id IS NULL) != (seed_id IS NULL))
);
INSERT INTO tasks_v7 (id, idea_id, seed_id, text, status, created_at, done_at, result, kind,
                      payload_json)
    SELECT id, idea_id, seed_id, text, status, created_at, done_at, result, kind, payload_json
    FROM tasks;
DROP TABLE tasks;
ALTER TABLE tasks_v7 RENAME TO tasks;
CREATE INDEX tasks_idea ON tasks(idea_id, status);
CREATE INDEX tasks_seed ON tasks(seed_id, status);
CREATE TABLE inspections (
    task_id     TEXT PRIMARY KEY REFERENCES tasks(id),
    idea_id     INTEGER NOT NULL REFERENCES ideas(id),
    question    TEXT NOT NULL,
    view        TEXT NOT NULL CHECK (view IN {_in(INSPECT_VIEWS)}),
    target      TEXT NOT NULL,
    code_ref    TEXT NOT NULL,
    tree        TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX inspections_key ON inspections(idea_id, question, view, target);
"""

# MIGRATIONS[i] upgrades user_version i -> i+1. Never edit a released migration.
MIGRATIONS: list[str] = [SCHEMA_V1, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5,
                         MIGRATION_V6, MIGRATION_V7]
SCHEMA_VERSION = len(MIGRATIONS)


def now() -> str:
    """Current time as ISO-8601 UTC, e.g. 2026-11-02T21:24:00Z."""
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


# ---------------------------------------------------------------- validation


def validate_text(field: str, value: str | None, *, required: bool = True) -> str | None:
    """Check an LLM-writable free-text field against its length limit."""
    if value is None:
        if required:
            raise ArbError(f"{field} is required")
        return None
    value = value.strip()
    if not value:
        if required:
            raise ArbError(f"{field} must not be empty")
        return None
    limit = LIMITS[field]
    if len(value) > limit:
        raise ArbError(f"{field} is too long: {len(value)} chars (max {limit}); shorten it")
    if field == "summary":
        lines = value.count("\n") + 1
        if lines > SUMMARY_MAX_LINES:
            raise ArbError(f"summary has {lines} lines (max {SUMMARY_MAX_LINES}); shorten it")
    if field == "lesson":
        if "\n" in value or "\r" in value:
            raise ArbError("lesson must be a single line; state one pitfall per lesson")
        if "<!--" in value or "-->" in value:
            raise ArbError("lesson must not contain HTML comment markers (<!-- or -->)")
    return value


def validate_name(name: str, what: str = "idea") -> str:
    if not NAME_RE.match(name or ""):
        raise ArbError(
            f"invalid {what} name {name!r}: use lowercase letters, digits and '-' "
            "(must start with a letter or digit, max 63 chars)"
        )
    return name


def validate_choice(field: str, value: str | None, allowed: tuple[str, ...]) -> str | None:
    if value is None:
        return None
    if value not in allowed:
        raise ArbError(f"invalid {field} {value!r}; choose one of: {', '.join(allowed)}")
    return value


# ---------------------------------------------------------------- connection


def connect(path: Path) -> sqlite3.Connection:
    """Open the db in autocommit mode (we issue BEGIN/COMMIT explicitly)."""
    conn = sqlite3.connect(str(path), isolation_level=None, timeout=5.0)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA busy_timeout = 5000")
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def migrate(conn: sqlite3.Connection) -> int:
    """Apply pending migrations; return the resulting schema version."""
    version = conn.execute("PRAGMA user_version").fetchone()[0]
    if version > SCHEMA_VERSION:
        raise ArbError(
            f"ledger schema version {version} is newer than this arb ({SCHEMA_VERSION}); upgrade arb"
        )
    if version == SCHEMA_VERSION:
        return SCHEMA_VERSION
    # Table rebuilds (the documented SQLite procedure) need foreign keys off; it cannot be
    # changed inside a transaction. Integrity is re-checked before each COMMIT.
    fk_was_on = conn.execute("PRAGMA foreign_keys").fetchone()[0]
    conn.execute("PRAGMA foreign_keys = OFF")
    try:
        for target in range(version + 1, SCHEMA_VERSION + 1):
            conn.execute("BEGIN IMMEDIATE")
            try:
                for stmt in _split_sql(MIGRATIONS[target - 1]):
                    conn.execute(stmt)
                bad = conn.execute("PRAGMA foreign_key_check").fetchall()
                if bad:
                    raise ArbError(f"migration to schema v{target} broke foreign keys: "
                                   f"{[tuple(r) for r in bad][:5]}")
                conn.execute(f"PRAGMA user_version = {target}")
                conn.execute("COMMIT")
            except BaseException:
                conn.execute("ROLLBACK")
                raise
    finally:
        if fk_was_on:
            conn.execute("PRAGMA foreign_keys = ON")
    return SCHEMA_VERSION


def _split_sql(script: str) -> list[str]:
    return [s.strip() for s in script.split(";") if s.strip()]


def row_dict(row: sqlite3.Row | None) -> dict[str, Any] | None:
    return dict(row) if row is not None else None


class Ledger:
    """The ledger db in a state dir. Use `with ledger.write():` for every write."""

    def __init__(self, state_dir: Path):
        self.state_dir = Path(state_dir)
        self.path = self.state_dir / "ledger.db"
        self.lock_path = self.state_dir / "ledger.lock"
        self._pending: list[tuple[str, str]] = []
        self._in_write = False
        with self._lock():
            self.conn = connect(self.path)
            migrate(self.conn)

    def close(self) -> None:
        self.conn.close()

    def __enter__(self) -> "Ledger":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- locking / transactions

    @contextmanager
    def _lock(self) -> Iterator[None]:
        self.state_dir.mkdir(parents=True, exist_ok=True)
        with open(self.lock_path, "a+") as fh:
            fcntl.flock(fh, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(fh, fcntl.LOCK_UN)

    @contextmanager
    def write(self) -> Iterator[sqlite3.Connection]:
        """Exclusive file lock + IMMEDIATE transaction; backup afterwards if needed."""
        if self._in_write:
            raise RuntimeError("nested Ledger.write()")
        with self._lock():
            self._in_write = True
            self._pending = []
            self.conn.execute("BEGIN IMMEDIATE")
            try:
                yield self.conn
                self.conn.execute("COMMIT")
            except BaseException:
                self.conn.execute("ROLLBACK")
                self._pending = []
                raise
            finally:
                self._in_write = False
            events, self._pending = self._pending, []
            backup_events = [e for e in events if e[0] in BACKUP_EVENTS]
            if backup_events:
                message = "; ".join(f"{kind} {subject}".strip() for kind, subject in backup_events)
                try:
                    self.backup(message)
                except ArbError as exc:  # the ledger itself is committed; only warn
                    print(f"warning: ledger backup failed: {exc}", file=sys.stderr)

    def event(self, kind: str, subject: str = "", payload: dict[str, Any] | None = None) -> None:
        """Append to the events table (call inside `write()`)."""
        if not self._in_write:
            raise RuntimeError("Ledger.event() outside of write()")
        self.conn.execute(
            "INSERT INTO events (ts, kind, subject, payload_json) VALUES (?, ?, ?, ?)",
            (now(), kind, subject, json.dumps(payload or {}, ensure_ascii=False, sort_keys=True)),
        )
        self._pending.append((kind, subject))

    def next_id(self, table: str, prefix: str) -> str:
        """Next sequential text id like r001 / d001 (call inside `write()`)."""
        rows = self.conn.execute(f"SELECT id FROM {table}").fetchall()
        nums = [int(r[0][len(prefix):]) for r in rows if r[0][len(prefix):].isdigit()]
        return f"{prefix}{max(nums, default=0) + 1:03d}"

    # -- backup

    def backup(self, message: str) -> bool:
        """Dump the db to backup/ledger.sql and commit it in the state-dir repo."""
        backup_dir = self.state_dir / "backup"
        backup_dir.mkdir(exist_ok=True)
        version = self.conn.execute("PRAGMA user_version").fetchone()[0]
        lines = [f"PRAGMA user_version = {version};", *self.conn.iterdump()]
        target = backup_dir / "ledger.sql"
        tmp = target.with_suffix(".sql.tmp")
        tmp.write_text("\n".join(lines) + "\n", encoding="utf-8")
        os.replace(tmp, target)
        return commit_backup(self.state_dir, message)

    # -- small query helpers

    def one(self, sql: str, params: tuple = ()) -> dict[str, Any] | None:
        return row_dict(self.conn.execute(sql, params).fetchone())

    def all(self, sql: str, params: tuple = ()) -> list[dict[str, Any]]:
        return [dict(r) for r in self.conn.execute(sql, params).fetchall()]


# ---------------------------------------------------------------- backup repo

BACKUP_GITIGNORE = (
    "# Only the SQL dump and the idea notebooks are versioned here.\n"
    "/*\n!/.gitignore\n!/backup/\n/backup/*\n!/backup/ledger.sql\n"
)
# Appended to the backup repo's .gitignore (also to older ones that lack it). marimo keeps
# session caches in notebooks/__marimo__/; only the notebook sources are versioned.
NOTEBOOKS_GITIGNORE = "!/notebooks/\n/notebooks/*\n!/notebooks/*.py\n"
# The project's notebook commands are versioned too, so every change Claude makes to one
# can be reviewed later (`git -C .arboria log -p commands/`).
COMMANDS_GITIGNORE = "!/commands/\n/commands/*\n!/commands/*.py\n"
# A home (paths.Home) versions copies of its seed notebooks, `seeds/*.py` next to the
# state dir, mirrored into `.arboria/seeds/` before each backup commit.
SEEDS_GITIGNORE = "!/seeds/\n/seeds/*\n!/seeds/*.py\n"


def ensure_backup_gitignore(state_dir: Path) -> None:
    gi = state_dir / ".gitignore"
    text = gi.read_text() if gi.exists() else BACKUP_GITIGNORE
    if "!/notebooks/" not in text:
        text += ("" if text.endswith("\n") else "\n") + NOTEBOOKS_GITIGNORE
    if "!/commands/" not in text:
        text += ("" if text.endswith("\n") else "\n") + COMMANDS_GITIGNORE
    if (state_dir / "seeds").is_dir() and "!/seeds/" not in text:  # a home only
        text += ("" if text.endswith("\n") else "\n") + SEEDS_GITIGNORE
    if not gi.exists() or gi.read_text() != text:
        gi.write_text(text)


def mirror_seeds(state_dir: Path) -> None:
    """In a home: copy `<home>/seeds/*.py` into `.arboria/seeds/` (changed files only) so
    the backup repo versions the human's seed notebooks. Copies of deleted notebooks stay
    (the backup is history, not a sync). Projects have no `.arboria/seeds/`: no-op."""
    mirror = Path(state_dir) / "seeds"
    source = Path(state_dir).parent / "seeds"
    if not mirror.is_dir() or not source.is_dir():
        return
    for src in sorted(source.glob("*.py")):
        try:
            if not src.is_file():
                continue
            data = src.read_bytes()
            dst = mirror / src.name
            if dst.is_file() and dst.read_bytes() == data:
                continue
            tmp = dst.with_name(f".{dst.name}.arboria-tmp")
            tmp.write_bytes(data)
            os.replace(tmp, dst)
        except OSError as exc:
            print(f"warning: could not back up {src}: {exc}", file=sys.stderr)


def _backup_git(args: list[str], state_dir: Path) -> str:
    cfg = ["-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"]
    return gitutil.out([*cfg, *args], state_dir, env=gitutil.FALLBACK_IDENTITY)


def init_backup_repo(state_dir: Path) -> None:
    """Make `.arboria/` a git repo (idempotent)."""
    if not (state_dir / ".git").exists():
        _backup_git(["init", "-q"], state_dir)
    ensure_backup_gitignore(state_dir)


def commit_backup(state_dir: Path, message: str) -> bool:
    """Commit backup/ledger.sql, the notebooks and the commands if anything changed. Returns True if a
    commit was made. What is versioned is decided by the backup repo's .gitignore."""
    mirror_seeds(state_dir)
    ensure_backup_gitignore(state_dir)
    _backup_git(["add", "-A", "--", "."], state_dir)
    if gitutil.ok(["diff", "--cached", "--quiet"], state_dir) and gitutil.ok(
        ["rev-parse", "--verify", "--quiet", "HEAD"], state_dir
    ):
        return False
    _backup_git(["commit", "-q", "--no-verify", "-m", f"arboria backup: {message}"], state_dir)
    return True
