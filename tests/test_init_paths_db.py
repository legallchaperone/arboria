"""State location, `arb init`, schema and validation."""

from __future__ import annotations

import sqlite3
import threading
from pathlib import Path

import pytest

from arboria import db
from arboria.db import Ledger, validate_text
from arboria.errors import ArbError, NotARepoError
from arboria.paths import locate

from .conftest import git


def test_state_dir_same_from_every_worktree(idea: Path, initialized: Path) -> None:
    sub = idea / "a" / "b"
    sub.mkdir(parents=True)
    locs = [locate(initialized), locate(idea), locate(sub), locate(initialized / ".git")]
    assert {l.state_dir for l in locs} == {initialized / ".arboria"}
    assert locate(idea).toplevel == idea
    assert locate(sub).toplevel == idea
    assert locate(initialized).in_main and not locate(idea).in_main


def test_not_a_repo(tmp_path: Path, arb) -> None:
    with pytest.raises(NotARepoError):
        locate(tmp_path)
    res = arb(tmp_path, "status", ok=False)
    assert res.exit_code == 1 and "not inside a git repository" in res.stderr


def test_commands_require_init(repo: Path, arb) -> None:
    for args in (["status"], ["idea", "list"], ["note", "x"], ["export"],
                 ["idea", "new", "a", "--hypothesis", "h", "--metric", "m"]):
        res = arb(repo, *args, ok=False)
        assert res.exit_code == 1, args
        assert "arb init" in res.stderr
    assert not (repo / ".arboria").exists()


def test_init_creates_everything_and_is_idempotent(repo: Path, arb) -> None:
    (repo / "arboria.toml").write_text("# mine\n")
    arb(repo, "init")
    state = repo / ".arboria"
    assert (state / "ledger.db").is_file()
    assert (repo / "arboria.toml").read_text() == "# mine\n"  # not overwritten
    exclude = (repo / ".git" / "info" / "exclude").read_text()
    assert exclude.count("/.arboria/") == 1
    assert git(repo, "status", "--porcelain", "--untracked-files=all") == "?? arboria.toml"
    assert (state / "backup" / "ledger.sql").is_file()
    assert git(state, "log", "--format=%s") == "arboria backup: init"
    assert git(state, "ls-files").splitlines() == [".gitignore", "backup/ledger.sql"]

    res = arb(repo, "init")
    assert "already initialized" in res.stdout
    assert (repo / ".git" / "info" / "exclude").read_text().count("/.arboria/") == 1
    assert git(state, "rev-list", "--count", "HEAD") == "1"

    conn = sqlite3.connect(state / "ledger.db")
    assert conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION
    assert conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
    tables = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
    assert {"ideas", "runs", "decisions", "notes", "events"} <= tables


def test_init_writes_toml_template(repo: Path, arb) -> None:
    import tomllib

    arb(repo, "init")
    cfg = tomllib.loads((repo / "arboria.toml").read_text())
    assert cfg["budget"]["total_su"] == 0
    assert cfg["default_machine"] == ""
    assert len(cfg["review"]["checklist"]) == 3


def test_init_from_idea_worktree_uses_main(idea: Path, initialized: Path, arb) -> None:
    arb(idea, "init")
    assert not (idea / ".arboria").exists()


def test_busy_timeout_and_migration(initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as ledger:
        assert ledger.conn.execute("PRAGMA busy_timeout").fetchone()[0] == 5000
        assert db.migrate(ledger.conn) == db.SCHEMA_VERSION  # no-op when current
        ledger.conn.execute(f"PRAGMA user_version = {db.SCHEMA_VERSION + 1}")
        with pytest.raises(ArbError, match="newer"):
            db.migrate(ledger.conn)


def test_migration_from_empty_db(tmp_path: Path) -> None:
    conn = db.connect(tmp_path / "x.db")
    assert conn.execute("PRAGMA user_version").fetchone()[0] == 0
    assert db.migrate(conn) == db.SCHEMA_VERSION
    conn.execute("INSERT INTO events (ts, kind) VALUES ('t', 'k')")


def test_schema_rejects_bad_enums(initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as ledger:
        with pytest.raises(sqlite3.IntegrityError):
            with ledger.write() as conn:
                conn.execute(
                    "INSERT INTO ideas (name, hypothesis, metric, confidence, branch, worktree_path,"
                    " created_at) VALUES ('a', 'h', 'm', 'sure', 'b', 'w', 't')"
                )


@pytest.mark.parametrize(
    "field,limit", [("hypothesis", 300), ("metric", 200), ("change_reason", 300),
                    ("note", 1000), ("summary", 1500)]
)
def test_text_limits(field: str, limit: int) -> None:
    assert validate_text(field, "a" * limit) == "a" * limit
    with pytest.raises(ArbError, match=f"{field} is too long"):
        validate_text(field, "a" * (limit + 1))
    with pytest.raises(ArbError, match="empty"):
        validate_text(field, "   ")


def test_summary_line_limit() -> None:
    assert validate_text("summary", "\n".join(["x"] * 10))
    with pytest.raises(ArbError, match="11 lines"):
        validate_text("summary", "\n".join(["x"] * 11))


def test_rollback_on_error(initialized: Path) -> None:
    with Ledger(initialized / ".arboria") as ledger:
        with pytest.raises(RuntimeError):
            with ledger.write() as conn:
                conn.execute("INSERT INTO events (ts, kind) VALUES ('t', 'x')")
                raise RuntimeError("boom")
        assert ledger.one("SELECT COUNT(*) AS n FROM events WHERE kind = 'x'")["n"] == 0


def test_concurrent_writers_get_sequential_ids(initialized: Path) -> None:
    """The fcntl lock serializes id allocation across connections."""
    state = initialized / ".arboria"
    ids: list[str] = []
    errors: list[BaseException] = []

    def worker() -> None:
        try:
            with Ledger(state) as ledger:
                for _ in range(5):
                    with ledger.write() as conn:
                        d = ledger.next_id("decisions", "d")
                        conn.execute(
                            "INSERT INTO decisions (id, subject_type, subject_id, question, created_at)"
                            " VALUES (?, 'idea', '1', 'q', 't')", (d,))
                        ids.append(d)
        except BaseException as exc:  # pragma: no cover - surfaced below
            errors.append(exc)

    threads = [threading.Thread(target=worker) for _ in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors
    assert sorted(ids) == [f"d{i:03d}" for i in range(1, 21)]
