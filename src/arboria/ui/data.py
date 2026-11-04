"""Everything the dashboard shows, read in one pass from the ledger and a few files.

Read-only by construction: the ledger is opened with `file:...?mode=ro` (plus
`PRAGMA query_only`), and the only other sources are `.arboria/notifications.log`,
`.arboria/runs/<run>/out/` listings, `arboria.toml`, `clusters.toml` (to recompute
approval cards) and the notebooks dir. No subprocess, no network. Cluster state is
whatever `arb watch` / `arb poll` last wrote into the ledger.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .. import cards
from ..config import project_config
from ..errors import ArbError
from ..paths import CONFIG_NAME, DB_NAME, STATE_DIR_NAME, Home, RepoContext, home

# Copies of db constants (importing db would import the ledger's write path);
# tests/test_ui.py checks they match.
SCHEMA_VERSION = 7
RUN_FINAL = ("succeeded", "failed", "cancelled")
RUN_ACTIVE = ("approved", "queued", "running")
JOB_TERMINAL = ("completed", "failed", "cancelled", "timeout")

NOTIFICATIONS = "notifications.log"
NOTIFICATIONS_SHOWN = 10
NOTIFICATIONS_TAIL_BYTES = 64 * 1024
OUT_FILES_SHOWN = 8


class ReadOnlyLedger:
    """The ledger opened read-only. Same `one` / `all` helpers as `db.Ledger`, no `write`."""

    def __init__(self, state_dir: Path):
        self.path = Path(state_dir) / DB_NAME
        if not self.path.is_file():
            raise ArbError(f"no ledger at {self.path}; run `arb init` in the main worktree")
        uri = self.path.resolve().as_uri() + "?mode=ro"
        self.conn = sqlite3.connect(uri, uri=True, timeout=5.0)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA query_only = ON")
        self.conn.execute("PRAGMA busy_timeout = 5000")

    def version(self) -> int:
        return self.conn.execute("PRAGMA user_version").fetchone()[0]

    def one(self, sql: str, params: tuple = ()) -> dict[str, Any] | None:
        row = self.conn.execute(sql, params).fetchone()
        return dict(row) if row is not None else None

    def all(self, sql: str, params: tuple = ()) -> list[dict[str, Any]]:
        return [dict(r) for r in self.conn.execute(sql, params).fetchall()]

    def close(self) -> None:
        self.conn.close()

    def __enter__(self) -> "ReadOnlyLedger":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()


def parse_ts(ts: str | None) -> datetime | None:
    """A ledger timestamp (`2026-11-02T21:24:00Z`, UTC) as an aware datetime."""
    if not ts:
        return None
    try:
        return datetime.strptime(ts, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError:
        try:
            dt = datetime.fromisoformat(ts.replace("Z", "+00:00"))
        except ValueError:
            return None
        return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)


@dataclass
class Board:
    """One read of the project: the dashboard renders only from this."""

    root: Path
    loaded_at: datetime  # aware, UTC
    project_id: str | None = None
    total_su: float = 0.0
    config_error: str | None = None
    ideas: list[dict[str, Any]] = field(default_factory=list)
    runs: list[dict[str, Any]] = field(default_factory=list)
    notes: list[dict[str, Any]] = field(default_factory=list)
    todos: list[dict[str, Any]] = field(default_factory=list)  # open, of open ideas
    lessons: list[dict[str, Any]] = field(default_factory=list)  # proposed
    notifications: list[tuple[datetime | None, str]] = field(default_factory=list)  # newest first
    cards: dict[str, dict[str, Any]] = field(default_factory=dict)  # awaiting runs
    home_thinking: int | None = None  # seeds 思考中 in the home (None: no home)

    @property
    def state_dir(self) -> Path:
        return self.root / STATE_DIR_NAME

    # -- lookups

    def idea(self, name: str) -> dict[str, Any] | None:
        return next((i for i in self.ideas if i["name"] == name), None)

    def idea_by_id(self, idea_id: int | None) -> dict[str, Any] | None:
        return next((i for i in self.ideas if i["id"] == idea_id), None)

    def run(self, run_id: str | None) -> dict[str, Any] | None:
        return next((r for r in self.runs if r["id"] == run_id), None)

    def runs_of(self, idea_id: int) -> list[dict[str, Any]]:
        return [r for r in self.runs if r["idea_id"] == idea_id]

    def human_notes(self, idea_id: int | None = None) -> list[dict[str, Any]]:
        """Notes written by the human (author NULL predates v4: those were human too)."""
        return [n for n in self.notes if (n.get("author") in (None, "human"))
                and (idea_id is None or n["idea_id"] == idea_id)]

    # -- what needs the human

    def awaiting(self) -> list[dict[str, Any]]:
        return [r for r in self.runs if r["status"] == "awaiting_approval"]

    def finished_unseen(self) -> list[dict[str, Any]]:
        """Finished runs of open ideas that ended after the idea's last human note."""
        last: dict[int, datetime] = {}
        for n in self.human_notes():
            t = parse_ts(n["created_at"])
            if t and (n["idea_id"] not in last or t > last[n["idea_id"]]):
                last[n["idea_id"]] = t
        out = []
        for r in self.runs:
            idea = self.idea_by_id(r["idea_id"])
            if r["status"] not in RUN_FINAL or idea is None or idea["status"] != "open":
                continue
            ended = parse_ts(r["finished_at"])
            seen = last.get(r["idea_id"])
            if seen is None or (ended is not None and ended > seen):
                out.append(r)
        return sorted(out, key=lambda r: r["finished_at"] or "", reverse=True)

    def needs_you(self) -> int:
        """Items only the human can act on (todos wait for Claude, not for you)."""
        return len(self.awaiting()) + len(self.finished_unseen()) + len(self.lessons)

    # -- money

    def su_used(self, idea_id: int | None = None) -> float:
        total = 0.0
        for r in self.runs:
            if idea_id is None or r["idea_id"] == idea_id:
                total += sum(j["billed_su"] or 0 for j in r["jobs"])
        return round(total, 2)

    def active_jobs(self) -> list[dict[str, Any]]:
        out = []
        for r in self.runs:
            for j in r["jobs"]:
                if j["state"] not in JOB_TERMINAL:
                    out.append(j | {"run": r})
        return out

    def su_timeline(self) -> list[tuple[datetime, float, str]]:
        """(time, cumulative SU, run id): each job's billed SU at its last ledger update."""
        points = []
        for r in self.runs:
            for j in r["jobs"]:
                t = parse_ts(j["updated_at"])
                if j["billed_su"] and t:
                    points.append((t, float(j["billed_su"]), r["id"]))
        points.sort(key=lambda p: p[0])
        out, total = [], 0.0
        for t, su, run_id in points:
            total += su
            out.append((t, round(total, 4), run_id))
        return out

    def last_active(self, idea: dict[str, Any]) -> str:
        """Latest timestamp touching the idea (created, runs, notes)."""
        stamps = [idea["created_at"], idea["closed_at"] or ""]
        stamps += [r["updated_at"] for r in self.runs_of(idea["id"])]
        stamps += [n["created_at"] for n in self.notes if n["idea_id"] == idea["id"]]
        return max(s for s in stamps if s)


def _requests(run: dict[str, Any]) -> dict[str, Any]:
    try:
        return json.loads(run["request_json"]) if run.get("request_json") else {}
    except ValueError:
        return {}


def _notifications(state_dir: Path) -> list[tuple[datetime | None, str]]:
    path = state_dir / NOTIFICATIONS
    if not path.is_file():
        return []
    with open(path, "rb") as fh:
        size = fh.seek(0, 2)
        fh.seek(max(0, size - NOTIFICATIONS_TAIL_BYTES))
        text = fh.read().decode("utf-8", errors="replace")
    lines = [ln for ln in text.splitlines() if ln.strip()]
    if size > NOTIFICATIONS_TAIL_BYTES and lines:
        lines = lines[1:]  # probably cut in the middle
    out = []
    for line in reversed(lines[-NOTIFICATIONS_SHOWN:]):
        stamp, _, rest = line.partition(" ")
        t = parse_ts(stamp)
        out.append((t, rest) if t else (None, line))
    return out


def out_files(board: Board, run_id: str) -> tuple[int, list[str]]:
    """(count, first names) of the fetched ARBORIA_OUT in `.arboria/runs/<run>/out/`."""
    path = board.state_dir / "runs" / run_id / "out"
    try:
        names = sorted(p.name + ("/" if p.is_dir() else "") for p in path.iterdir())
    except OSError:
        return 0, []
    return len(names), names[:OUT_FILES_SHOWN]


def notebook_path(board: Board, idea_name: str) -> Path:
    return board.state_dir / "notebooks" / f"{idea_name}.py"


def _context(root: Path) -> RepoContext:
    """A context for config/card reads without running git (the root is known)."""
    return RepoContext(cwd=root, common_dir=root / ".git", main=root, toplevel=root)


def _card(ctx: RepoContext, ledger: ReadOnlyLedger, run: dict[str, Any],
          idea: dict[str, Any]) -> dict[str, Any]:
    """The approval card as `arb card` shows it, recomputed now (local files only)."""
    try:
        profile, plan = cards.replan(run)
        card = cards.build_card(ctx, ledger, run, idea, profile, plan)
    except (ArbError, OSError, KeyError, ValueError, TypeError) as exc:
        return {"text": None, "matches": None, "error": str(exc)}
    return {"text": card.text(), "matches": card.card_hash == run["card_hash"], "error": None,
            "est_su": card.est_su}


def load(root: Path | str, now: datetime | None = None) -> Board:
    """Read the project at `root` (its main worktree)."""
    root = Path(root).resolve()
    board = Board(root=root, loaded_at=now or datetime.now(timezone.utc))
    ctx = _context(root)
    try:
        cfg = project_config(ctx)
        board.project_id, board.total_su = cfg.project_id, cfg.total_su
    except ArbError as exc:
        board.config_error = f"{CONFIG_NAME}: {exc}"
    with ReadOnlyLedger(board.state_dir) as ledger:
        version = ledger.version()
        if version != SCHEMA_VERSION:
            raise ArbError(f"ledger schema v{version}, the dashboard reads v{SCHEMA_VERSION}: "
                           "run any arb command (e.g. `arb status`) once to upgrade it, or "
                           "upgrade arb")
        events = ledger.all("SELECT kind, subject, ts, payload_json FROM events"
                            " WHERE kind IN ('idea_new', 'run_final') ORDER BY id")
        from_run: dict[str, str] = {}
        finished: dict[str, str] = {}
        for e in events:
            if e["kind"] == "run_final":
                finished[e["subject"]] = e["ts"]
                continue
            try:
                fr = json.loads(e["payload_json"] or "{}").get("from_run")
            except ValueError:
                fr = None
            if fr:
                from_run[e["subject"]] = fr
        board.ideas = ledger.all(
            "SELECT i.*, p.name AS parent_name FROM ideas i"
            " LEFT JOIN ideas p ON p.id = i.parent_id ORDER BY i.id")
        for i in board.ideas:
            i["from_run"] = from_run.get(i["name"])
        jobs: dict[str, list[dict[str, Any]]] = {}
        for j in ledger.all("SELECT * FROM jobs ORDER BY run_id, segment"):
            jobs.setdefault(j["run_id"], []).append(j)
        board.runs = ledger.all(
            "SELECT id, idea_id, parent_run, status, change_reason, snapshot_commit, machine,"
            " \"partition\", gpus, walltime, est_cost, actual_cost, wandb_url, summary,"
            " request_json, card_hash, created_at, updated_at FROM runs ORDER BY id")
        for r in board.runs:
            r["jobs"] = jobs.get(r["id"], [])
            r["request"] = _requests(r)
            r["finished_at"] = (finished.get(r["id"]) or r["updated_at"]) \
                if r["status"] in RUN_FINAL else None
        board.notes = ledger.all("SELECT * FROM notes ORDER BY id DESC")
        board.todos = ledger.all(
            "SELECT t.*, i.name AS idea FROM tasks t JOIN ideas i ON i.id = t.idea_id"
            " WHERE t.status = 'open' AND i.status = 'open' ORDER BY t.id")
        board.lessons = ledger.all(
            "SELECT * FROM lessons WHERE status = 'proposed' ORDER BY id")
        for r in board.awaiting():
            board.cards[r["id"]] = _card(ctx, ledger, r, board.idea_by_id(r["idea_id"]))
    board.notifications = _notifications(board.state_dir)
    board.home_thinking = home_thinking(home())
    return board


# ---------------------------------------------------------------- the seed home (种子园)


def _iso(ts: float) -> str:
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


@dataclass
class Garden:
    """One read of the seed home (`arb ui --home`)."""

    root: Path
    loaded_at: datetime  # aware, UTC
    seeds: list[dict[str, Any]] = field(default_factory=list)  # most recently edited first
    todos: list[dict[str, Any]] = field(default_factory=list)  # open, of live seeds

    def with_status(self, status: str) -> list[dict[str, Any]]:
        return [s for s in self.seeds if s["status"] == status]

    def count(self, status: str) -> int:
        return len(self.with_status(status))


def _check_version(ledger: ReadOnlyLedger) -> None:
    version = ledger.version()
    if version != SCHEMA_VERSION:
        raise ArbError(f"ledger schema v{version}, the dashboard reads v{SCHEMA_VERSION}: "
                       "run any arb command (e.g. `arb idea list`) once to upgrade it, or "
                       "upgrade arb")


def load_home(root: Path | str, now: datetime | None = None) -> Garden:
    """Read the seed home at `root`: its ledger (read-only) and its notebooks' mtimes."""
    h = Home(Path(root).resolve())
    garden = Garden(root=h.root, loaded_at=now or datetime.now(timezone.utc))
    with ReadOnlyLedger(h.state_dir) as ledger:
        _check_version(ledger)
        planted: dict[int, list[dict[str, Any]]] = {}
        for p in ledger.all("SELECT * FROM plantings ORDER BY id"):
            planted.setdefault(p["seed_id"], []).append(p)
        seeds = ledger.all("SELECT * FROM seeds ORDER BY id")
        garden.todos = ledger.all(
            "SELECT t.*, s.name AS seed FROM tasks t JOIN seeds s ON s.id = t.seed_id"
            " WHERE t.status = 'open' AND s.status != 'dropped' ORDER BY t.id")
    for s in seeds:
        nb = h.notebook(s["name"])
        try:
            edited = max(s["updated_at"], _iso(nb.stat().st_mtime))
        except OSError:
            edited = s["updated_at"]
        s.update(notebook=nb, notebook_exists=nb.is_file(), last_edited=edited,
                 planted_to=planted.get(s["id"], []))
    garden.seeds = sorted(seeds, key=lambda s: (s["last_edited"], s["id"]), reverse=True)
    return garden


def home_thinking(h: Home) -> int | None:
    """How many seeds are 思考中 (for the project dashboard's header); None without a
    home, or when it cannot be read."""
    if not h.initialized:
        return None
    try:
        with ReadOnlyLedger(h.state_dir) as ledger:
            if ledger.version() != SCHEMA_VERSION:
                return None
            return ledger.one("SELECT COUNT(*) AS n FROM seeds WHERE status = 'thinking'")["n"]
    except (ArbError, sqlite3.Error, OSError):
        return None
