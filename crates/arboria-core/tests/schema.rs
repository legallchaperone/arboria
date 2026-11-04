//! The ledger schema and the real parts of `db`: v1–v7 are Python's byte for byte, a
//! fresh ledger reaches v8, a Python-made v7 ledger migrates to v8 without losing rows,
//! writes are locked and transactional, `data_version` sees other connections' commits.

mod common;

use arboria_core::db::{self, Ledger};
use arboria_core::model::{EventKind, RunId, TaskId};
use arboria_core::probe::{self, ProbeKey, ProbeResult, ProbeScope, TextResult};
use arboria_core::schema::{self, MIGRATIONS, SCHEMA_VERSION};
use common::{fixture, TempDir};
use rusqlite::Connection;

fn master(conn: &Connection) -> String {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })
        .unwrap();
    rows.map(|r| {
        let (t, n, tb, s) = r.unwrap();
        let s = s
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        format!("{t}\t{n}\t{tb}\t{s}\n")
    })
    .collect()
}

/// The Python iterdump of a v7 ledger. Its tables come in name order (plantings before
/// seeds), so foreign keys are off while loading (rusqlite's bundled SQLite turns them on
/// by default).
fn load_v7(conn: &Connection) {
    conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    conn.execute_batch(&fixture("ledger_v7.sql")).unwrap();
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn v1_to_v7_are_pythons_text() {
    let text = fixture("migrations_v1_v7.sql");
    let parts: Vec<&str> = text.split("-- @@ v").skip(1).collect();
    assert_eq!(parts.len(), schema::PYTHON_SCHEMA_VERSION as usize);
    for (i, part) in parts.iter().enumerate() {
        let (num, sql) = part.split_once('\n').unwrap();
        assert_eq!(num, (i + 1).to_string());
        assert_eq!(
            MIGRATIONS[i].trim(),
            sql.trim(),
            "migration v{} differs from Python's",
            i + 1
        );
    }
}

#[test]
fn v7_schema_equals_a_python_ledger() {
    let conn = Connection::open_in_memory().unwrap();
    for sql in &MIGRATIONS[..7] {
        conn.execute_batch(sql).unwrap();
    }
    assert_eq!(master(&conn), fixture("schema_v7.txt"));
}

#[test]
fn fresh_ledger_is_v8() {
    let dir = TempDir::new("fresh");
    let ledger = Ledger::open(dir.path()).unwrap();
    assert_eq!(db::schema_version(ledger.conn()).unwrap(), SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 8);
    let mode: String = ledger
        .conn()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let fk: i64 = ledger
        .conn()
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    let tables = master(ledger.conn());
    for t in [
        "\tprobe_results\t",
        "\tideas\t",
        "\ttasks\t",
        "\tdecisions\t",
        "\tseeds\t",
    ] {
        assert!(tables.contains(t), "{t}");
    }
    assert!(dir.path().join(db::LOCK_NAME).is_file());
    drop(ledger);
    // reopening is a no-op
    let again = Ledger::open(dir.path()).unwrap();
    assert_eq!(db::schema_version(again.conn()).unwrap(), 8);
}

#[test]
fn newer_ledger_is_refused() {
    let dir = TempDir::new("newer");
    {
        let conn = Connection::open(dir.path().join(db::DB_NAME)).unwrap();
        conn.execute_batch("PRAGMA user_version = 9").unwrap();
    }
    let err = Ledger::open(dir.path()).unwrap_err().to_string();
    assert_eq!(
        err,
        "ledger schema version 9 is newer than this arb (8); upgrade arb"
    );
}

#[test]
fn python_v7_ledger_migrates_to_v8() {
    let dir = TempDir::new("v7");
    {
        let conn = Connection::open(dir.path().join(db::DB_NAME)).unwrap();
        load_v7(&conn);
        assert_eq!(db::schema_version(&conn).unwrap(), 7);
    }
    let ledger = Ledger::open(dir.path()).unwrap();
    let c = ledger.conn();
    assert_eq!(db::schema_version(c).unwrap(), 8);

    // ideas: the two v7 ideas keep their ids; the three seeds follow (id + max ideas.id)
    // one line per row: id name stage status legacy_seed_id has_branch
    let ideas: Vec<String> = {
        let mut st = c
            .prepare(
                "SELECT id || ' ' || name || ' ' || stage || ' ' || status || ' '
                        || COALESCE(legacy_seed_id, '-') || ' ' || (branch IS NOT NULL)
                 FROM ideas ORDER BY id",
            )
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        ideas,
        [
            "1 base open open - 1",
            "2 wide-lr dropped dropped - 1",
            "3 sparse seed open 1 0",
            "4 planted-one seed open 2 0",
            "5 gone dropped dropped 3 0",
        ]
    );
    let (gist, closed): (Option<String>, Option<String>) = c
        .query_row(
            "SELECT gist, closed_at FROM ideas WHERE name = 'gone'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (gist, closed.as_deref()),
        (None, Some("2026-11-02T00:00:00Z"))
    );
    let gist: String = c
        .query_row("SELECT gist FROM ideas WHERE name = 'sparse'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(gist, "一句话");

    // plantings point at the seed's new ideas row; the legacy seed id stays
    let (seed_idea, seed_id): (i64, i64) = c
        .query_row(
            "SELECT seed_idea_id, seed_id FROM plantings WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((seed_idea, seed_id), (4, 2));

    // tasks: every todo has an idea row now; queue columns default
    let t002: (i64, Option<i64>, String, i64) = c
        .query_row(
            "SELECT idea_id, seed_id, status, attempts FROM tasks WHERE id = 't002'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(t002, (3, Some(1), "done".to_string(), 0));
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM tasks WHERE idea_id IS NULL"),
        0
    );
    assert_eq!(count(c, "SELECT COUNT(*) FROM tasks"), 3);

    // runs: nonce backfilled from request_json; decisions kept, bound_hash empty
    let nonce: Option<String> = c
        .query_row("SELECT nonce FROM runs WHERE id = 'r001'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nonce.as_deref(), Some("n0nce"));
    let nonce: Option<String> = c
        .query_row("SELECT nonce FROM runs WHERE id = 'r002'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(nonce, None);
    assert_eq!(
        count(
            c,
            "SELECT COUNT(*) FROM decisions WHERE id = 'd001' AND bound_hash IS NULL"
        ),
        1
    );
    assert_eq!(count(c, "SELECT COUNT(*) FROM events"), 1);
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM seeds"),
        3,
        "seeds stays, frozen"
    );

    // integrity
    let fk: i64 = c
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk, 0);
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
}

#[test]
fn v8_checks_hold() {
    let dir = TempDir::new("checks");
    let ledger = Ledger::open(dir.path()).unwrap();
    let c = ledger.conn();
    let ts = "2026-11-04T00:00:00Z";
    // a seed: no branch, no hypothesis needed
    c.execute(
        "INSERT INTO ideas (name, stage, status, created_at, updated_at) VALUES ('s', 'seed', 'open', ?1, ?1)",
        [ts],
    )
    .unwrap();
    // status must follow stage
    assert!(c
        .execute(
            "INSERT INTO ideas (name, stage, status, created_at, updated_at) VALUES ('s2', 'seed', 'dropped', ?1, ?1)",
            [ts],
        )
        .is_err());
    // an open idea needs a branch, and then hypothesis + metric
    assert!(c
        .execute(
            "INSERT INTO ideas (name, stage, status, created_at, updated_at) VALUES ('o', 'open', 'open', ?1, ?1)",
            [ts],
        )
        .is_err());
    assert!(c
        .execute(
            "INSERT INTO ideas (name, stage, status, branch, worktree_path, created_at, updated_at) VALUES ('o', 'open', 'open', 'idea/o', '/w', ?1, ?1)",
            [ts],
        )
        .is_err());
    // done_at iff done / dropped
    assert!(c
        .execute(
            "INSERT INTO tasks (id, idea_id, text, status, created_at, done_at) VALUES ('t001', 1, 'x', 'queued', ?1, ?1)",
            [ts],
        )
        .is_err());
    c.execute(
        "INSERT INTO tasks (id, idea_id, text, status, created_at, kind) VALUES ('t001', 1, 'x', 'needs_human', ?1, 'probe_new')",
        [ts],
    )
    .unwrap();
    // a probe enable must carry its hash
    assert!(c
        .execute(
            "INSERT INTO decisions (id, subject_type, subject_id, question, choice, decided_by, created_at) VALUES ('d001', 'probe', 'project:x', 'q', 'enable', 'human', ?1)",
            [ts],
        )
        .is_err());
    assert!(c
        .execute(
            "INSERT INTO decisions (id, subject_type, subject_id, question, choice, created_at) VALUES ('d001', 'probe', 'project:x', 'q', 'approve', ?1)",
            [ts],
        )
        .is_err());
}

#[test]
fn writes_commit_roll_back_and_number_ids() {
    let dir = TempDir::new("write");
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let id: TaskId = ledger.write(|tx| tx.next_id()).unwrap();
    assert_eq!(id.as_str(), "t001");
    ledger
        .write(|tx| {
            tx.execute(
                "INSERT INTO ideas (name, stage, status, created_at, updated_at) VALUES ('s', 'seed', 'open', 'x', 'x')",
                [],
            )?;
            for t in ["t001", "t009", "t099", "tx12"] {
                tx.execute(
                    "INSERT INTO tasks (id, idea_id, text, created_at) VALUES (?1, 1, 'x', 'x')",
                    [t],
                )?;
            }
            // a non-backup event: no backup commit is attempted
            tx.event(EventKind::TodoAttempt, "t001", &serde_json::json!({"b": 1, "a": "é"}))?;
            Ok(())
        })
        .unwrap();
    let id: TaskId = ledger.write(|tx| tx.next_id()).unwrap();
    assert_eq!(id.as_str(), "t100");
    let r: RunId = ledger.write(|tx| tx.next_id()).unwrap();
    assert_eq!(r.as_str(), "r001");
    let payload: String = ledger
        .conn()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'todo_attempt'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(payload, r#"{"a": "é", "b": 1}"#);

    // a failing closure leaves nothing behind
    let err = ledger
        .write(|tx| {
            tx.execute("DELETE FROM tasks", [])?;
            Err::<(), _>(arboria_core::Error::arb("boom"))
        })
        .unwrap_err();
    assert_eq!(err.to_string(), "boom");
    assert_eq!(count(ledger.conn(), "SELECT COUNT(*) FROM tasks"), 4);
    // the lock was released: another writer gets through
    let mut other = Ledger::open(dir.path()).unwrap();
    other
        .write(|tx| Ok(tx.execute("DELETE FROM tasks WHERE id = 'tx12'", [])?))
        .unwrap();
}

#[test]
fn write_is_begin_immediate() {
    let dir = TempDir::new("immediate");
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let other = db::connect(&dir.path().join(db::DB_NAME)).unwrap();
    other
        .busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();
    ledger
        .write(|_tx| {
            // the write lock is taken at BEGIN, before any statement
            let e = other.execute_batch("BEGIN IMMEDIATE");
            assert!(e.is_err(), "a second writer must be locked out");
            Ok(())
        })
        .unwrap();
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
}

#[test]
fn data_version_sees_other_connections() {
    let dir = TempDir::new("dv");
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let reader = db::open_read_only(&dir.path().join(db::DB_NAME)).unwrap();
    let v0 = db::data_version(&reader).unwrap();
    assert_eq!(
        db::data_version(&reader).unwrap(),
        v0,
        "stable without commits"
    );
    let own0 = ledger.data_version().unwrap();
    ledger
        .write(|tx| {
            tx.execute(
                "INSERT INTO ideas (name, stage, status, created_at, updated_at) VALUES ('s', 'seed', 'open', 'x', 'x')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_ne!(
        db::data_version(&reader).unwrap(),
        v0,
        "changes after another connection commits"
    );
    assert_eq!(
        ledger.data_version().unwrap(),
        own0,
        "own commits do not change it"
    );
    // the read-only connection cannot write
    assert!(reader.execute("DELETE FROM ideas", []).is_err());
}

#[test]
fn read_only_refuses_other_versions() {
    let dir = TempDir::new("ro");
    let path = dir.path().join(db::DB_NAME);
    {
        let conn = Connection::open(&path).unwrap();
        load_v7(&conn);
    }
    let err = db::open_read_only(&path).unwrap_err().to_string();
    assert!(err.contains("ledger schema v7"), "{err}");
    assert!(db::open_read_only(&dir.path().join("missing.db")).is_err());
}

#[test]
fn probe_results_and_enabled_hash() {
    let dir = TempDir::new("probe");
    let mut ledger = Ledger::open(dir.path()).unwrap();
    let key = ProbeKey::new(ProbeScope::Project, "deltaai_credits").unwrap();
    let h1 = "a".repeat(64);
    let h2 = "b".repeat(64);
    assert_eq!(probe::enabled_hash(ledger.conn(), &key).unwrap(), None);
    let ins = |c: &Connection,
               id: &str,
               choice: Option<&str>,
               by: Option<&str>,
               hash: Option<&str>| {
        c.execute(
            "INSERT INTO decisions (id, subject_type, subject_id, question, options_json, choice, decided_by, created_at, bound_hash) VALUES (?1, 'probe', 'project:deltaai_credits', '启用这个 probe 吗？', '[\"enable\", \"disable\"]', ?2, ?3, 'x', ?4)",
            rusqlite::params![id, choice, by, hash],
        )
        .unwrap();
    };
    ins(
        ledger.conn(),
        "d001",
        Some("enable"),
        Some("human"),
        Some(&h1),
    );
    assert_eq!(
        probe::enabled_hash(ledger.conn(), &key).unwrap(),
        Some(h1.clone())
    );
    ins(ledger.conn(), "d002", None, None, None); // an open one does not count
    assert_eq!(
        probe::enabled_hash(ledger.conn(), &key).unwrap(),
        Some(h1.clone())
    );
    ins(
        ledger.conn(),
        "d010",
        Some("enable"),
        Some("agent"),
        Some(&h2),
    ); // not the human's
    assert_eq!(probe::enabled_hash(ledger.conn(), &key).unwrap(), None);
    ins(
        ledger.conn(),
        "d011",
        Some("enable"),
        Some("human"),
        Some(&h2),
    );
    assert_eq!(
        probe::enabled_hash(ledger.conn(), &key).unwrap(),
        Some(h2.clone())
    );
    ins(ledger.conn(), "d012", Some("disable"), Some("human"), None);
    assert_eq!(probe::enabled_hash(ledger.conn(), &key).unwrap(), None);

    let result = ProbeResult::Text(TextResult {
        label: None,
        text: "ok".into(),
    });
    let id =
        probe::set_probe_result(&mut ledger, "deltaai", &key, &h1, Some(&h2), &result).unwrap();
    let (probe_key, json, events): (String, String, i64) = ledger
        .conn()
        .query_row(
            "SELECT probe, result_json, (SELECT COUNT(*) FROM events) FROM probe_results WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(probe_key, "project:deltaai_credits");
    assert_eq!(json, r#"{"kind": "text", "text": "ok"}"#);
    assert_eq!(events, 0, "results are heartbeat-like: no event");
    // invalid inputs are refused before any write
    let bad = ProbeResult::Text(TextResult {
        label: None,
        text: "<b>x</b>".into(),
    });
    assert!(probe::set_probe_result(&mut ledger, "deltaai", &key, &h1, Some(&h2), &bad).is_err());
    assert!(
        probe::set_probe_result(&mut ledger, "deltaai", &key, "XYZ", Some(&h2), &result).is_err()
    );
    assert!(probe::set_probe_result(&mut ledger, "deltaai", &key, &h1, None, &result).is_err());
    assert!(
        probe::set_probe_result(&mut ledger, "Delta AI", &key, &h1, Some(&h2), &result).is_err()
    );
    assert_eq!(
        count(ledger.conn(), "SELECT COUNT(*) FROM probe_results"),
        1
    );
}
