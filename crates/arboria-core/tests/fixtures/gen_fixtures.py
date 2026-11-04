"""Regenerate the schema fixtures the Rust tests compare against (reads the Python db.py).

    uv run python crates/arboria-core/tests/fixtures/gen_fixtures.py

- migrations_v1_v7.sql: Python's MIGRATIONS[0..7] verbatim, separated by `-- @@ vN` lines
  (src/schema.rs embeds the same text; `schema_matches_python` checks they agree).
- schema_v7.txt: sqlite_master of a fresh Python ledger at v7 (type, name, tbl_name, sql).
- ledger_v7.sql: a small Python-made v7 home+project-like ledger (ideas, seeds,
  plantings, tasks of both owners, runs, a decision), as `PRAGMA user_version` + iterdump,
  for the v7 -> v8 migration test.
"""

from __future__ import annotations

import sqlite3
import tempfile
from pathlib import Path

from arboria import db

HERE = Path(__file__).parent


def master(conn: sqlite3.Connection) -> str:
    rows = conn.execute("SELECT type, name, tbl_name, sql FROM sqlite_master"
                        " WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name").fetchall()
    return "".join(f"{t}\t{n}\t{tb}\t{' '.join((s or '').split())}\n" for t, n, tb, s in rows)


def main() -> None:
    parts = []
    for i, sql in enumerate(db.MIGRATIONS, 1):
        parts.append(f"-- @@ v{i}\n{sql.strip()}\n")
    (HERE / "migrations_v1_v7.sql").write_text("".join(parts), encoding="utf-8")

    with tempfile.TemporaryDirectory() as tmp:
        ledger = db.Ledger(Path(tmp))
        conn = ledger.conn
        assert conn.execute("PRAGMA user_version").fetchone()[0] == 7
        (HERE / "schema_v7.txt").write_text(master(conn), encoding="utf-8")
        ts = "2026-11-01T10:00:00Z"
        conn.executescript(f"""
        BEGIN;
        INSERT INTO ideas (id, name, hypothesis, metric, branch, worktree_path, created_at)
            VALUES (1, 'base', 'h', 'm', 'idea/base', '/w/base', '{ts}');
        INSERT INTO ideas (id, name, hypothesis, metric, parent_id, confidence, status, branch,
                           worktree_path, created_at, closed_at)
            VALUES (2, 'wide-lr', 'h2', 'm2', 1, 'prelim', 'dropped', 'idea/wide-lr',
                    '/w/wide-lr', '{ts}', '{ts}');
        INSERT INTO seeds (id, name, gist, hypothesis, metric, status, created_at, updated_at)
            VALUES (1, 'sparse', '一句话', NULL, NULL, 'thinking', '{ts}', '{ts}'),
                   (2, 'planted-one', 'g', 'h', 'm', 'planted', '{ts}', '{ts}'),
                   (3, 'gone', NULL, NULL, NULL, 'dropped', '{ts}', '2026-11-02T00:00:00Z');
        INSERT INTO plantings (id, seed_id, repo, project_id, idea, created_at)
            VALUES (1, 2, '/repos/x', 'x-1a2b', 'planted-one', '{ts}');
        INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, status,
                          created_at, updated_at, request_json, card_hash)
            VALUES ('r001', 1, 'c1', 't1', 'first', 'awaiting_approval', '{ts}', '{ts}',
                    '{{"ask": {{"repeat": "n0nce"}}}}', 'h1'),
                   ('r002', 1, 'c1', 't1', 'second', 'draft', '{ts}', '{ts}', NULL, NULL);
        INSERT INTO decisions (id, subject_type, subject_id, question, options_json, created_at)
            VALUES ('d001', 'run', 'r001', 'q', '["approve", "reject", "revise"]', '{ts}');
        INSERT INTO tasks (id, idea_id, seed_id, text, status, created_at, done_at, result, kind)
            VALUES ('t001', 1, NULL, 'look', 'open', '{ts}', NULL, NULL, 'plain'),
                   ('t002', NULL, 1, 'think', 'done', '{ts}', '{ts}', 'ok', 'plain'),
                   ('t003', 2, NULL, 'gone', 'dropped', '{ts}', '{ts}', NULL, 'inspect');
        INSERT INTO events (ts, kind, subject, payload_json)
            VALUES ('{ts}', 'idea_new', 'base', '{{}}');
        COMMIT;
        """)
        lines = ["PRAGMA user_version = 7;", *conn.iterdump()]
        (HERE / "ledger_v7.sql").write_text("\n".join(lines) + "\n", encoding="utf-8")
        ledger.close()
    print("wrote fixtures to", HERE)


if __name__ == "__main__":
    main()
