"""A schema v8 ledger (written by the Rust arb, docs/adr/0002-rust-contract.md) is opened
read-only: `Ledger.write` and backups refuse, SQLite refuses writes too, and the dashboards
read it like v7. The v8 migration is the Rust one verbatim (`tests/fixtures/migration_v8.sql`,
checked by `crates/arboria-core/tests/schema.rs`)."""

from __future__ import annotations

import sqlite3
from pathlib import Path

import pytest

from arboria import db
from arboria.errors import ArbError
from arboria.ui import data

V8 = (Path(__file__).parent / "fixtures" / "migration_v8.sql").read_text(encoding="utf-8")


def to_v8(state_dir: Path) -> None:
    """Migrate a Python v7 ledger to v8 the way the Rust arb does (one transaction, FKs off)."""
    conn = sqlite3.connect(state_dir / "ledger.db", isolation_level=None)
    assert conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION
    conn.execute("PRAGMA foreign_keys = OFF")
    conn.execute("BEGIN IMMEDIATE")
    for stmt in db._split_sql(V8):
        conn.execute(stmt)
    assert conn.execute("PRAGMA foreign_key_check").fetchall() == []
    conn.execute("PRAGMA user_version = 8")
    conn.execute("COMMIT")
    conn.close()


@pytest.fixture
def v8(arb, tmp_path: Path, isolated_home: Path, initialized: Path) -> tuple[Path, Path]:
    """A project (one idea, a note) and a seed home (thinking, planted, dropped seeds and a
    seed todo), both migrated to v8. Returns (project root, home)."""
    arb(tmp_path, "seed", "new", "rank-schedule", "LoRA 的秩随层变化",
        "--hypothesis", "浅层需要更高的秩", "--metric", "val/loss")
    arb(tmp_path, "seed", "new", "xss", "一句话")
    arb(tmp_path, "seed", "new", "muon-small", "Muon 在小模型上")
    arb(tmp_path, "seed", "drop", "muon-small")
    arb(initialized, "seed", "plant", "rank-schedule")
    wt = Path(arb(initialized, "idea", "new", "exp1", "--hypothesis", "warmup helps",
                  "--metric", "val/loss").stdout.strip())
    arb(wt, "note", "looks fine")
    from arboria import lab

    scope = {"__name__": "__marimo__", "__file__": str(isolated_home / "seeds" / "xss.py"),
             "lab": lab}
    exec('lab.todo("找三篇相关论文")', scope)
    for state in (initialized / ".arboria", isolated_home / ".arboria"):
        to_v8(state)
    return initialized, isolated_home


def test_v8_opens_read_only(v8: tuple[Path, Path], arb) -> None:
    root, _ = v8
    state = root / ".arboria"
    with db.Ledger(state) as ledger:
        assert (ledger.version, ledger.read_only) == (8, True)
        assert ledger.one("SELECT COUNT(*) AS n FROM ideas WHERE stage = 'open'")["n"] == 2
        message = db.READ_ONLY_MESSAGE.format(path=state / "ledger.db", version=8)
        with pytest.raises(ArbError) as exc:
            with ledger.write():
                pass
        assert str(exc.value) == message
        with pytest.raises(ArbError):
            ledger.backup("x")
        with pytest.raises(sqlite3.OperationalError, match="readonly"):  # query_only
            ledger.conn.execute("DELETE FROM notes")
    with db.Ledger(state) as ledger:
        assert ledger.one("SELECT COUNT(*) AS n FROM notes")["n"] == 1
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == 8
    # a command that writes is refused, nothing changes
    res = arb(root, "lesson", "propose", "pin the seed", ok=False)
    assert res.exit_code == 1 and "written by the Rust arb; the Python arb (arb-py)" in res.stderr
    with db.Ledger(state) as ledger:
        assert ledger.one("SELECT COUNT(*) AS n FROM lessons")["n"] == 0
    # newer than v8 is still "upgrade arb"
    conn = sqlite3.connect(state / "ledger.db")
    conn.execute("PRAGMA user_version = 9")
    conn.commit()
    conn.close()
    with pytest.raises(ArbError, match="newer than this arb"):
        db.Ledger(state)


def test_dashboards_read_v8(v8: tuple[Path, Path]) -> None:
    root, home = v8
    board = data.load(root)
    assert sorted(i["name"] for i in board.ideas) == ["exp1", "rank-schedule"]
    assert [n["text"] for n in board.notes] == ["looks fine"]
    assert board.home_thinking == 1
    g = data.load_home(home)
    assert {s["name"]: s["status"] for s in g.seeds} == {
        "rank-schedule": "planted", "xss": "thinking", "muon-small": "dropped"}
    rs = next(s for s in g.seeds if s["name"] == "rank-schedule")
    assert [p["idea"] for p in rs["planted_to"]] == ["rank-schedule"]
    assert [(t["id"], t["seed"]) for t in g.todos] == [("t001", "xss")]
