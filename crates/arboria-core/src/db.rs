//! The SQLite ledger: open, migrate, locked writes, change detection, git backup.
//! Ports `arboria.db` (`connect`, `migrate`, `Ledger`, `now`, the backup repo helpers).
//!
//! Invariants:
//! - Every write goes through [`Ledger::write`]: an exclusive `flock(2)` on
//!   `<state>/ledger.lock` (the same lock Python's `fcntl.flock` takes, so both
//!   implementations exclude each other) plus a `BEGIN IMMEDIATE` transaction. A failing
//!   closure rolls back; nesting is impossible (`&mut self`).
//! - Connections use WAL, `busy_timeout` 5000 ms and `foreign_keys = ON`; migrations run
//!   with foreign keys off (SQLite table rebuilds), re-checked with `foreign_key_check`
//!   before each commit, one transaction per version.
//! - A backup commit happens only after a write that recorded an event in
//!   [`EventKind::BACKUP`]; heartbeat-like updates never trigger one. A failed backup only
//!   warns (the ledger itself is committed).
//! - Change notification is `PRAGMA data_version` ([`Ledger::data_version`]): it changes
//!   when *another* connection commits; the app polls it instead of watching files.
//!
//! Real in phase 1: [`now`], [`connect`], [`open_read_only`], [`migrate`],
//! [`Ledger::open`], [`Ledger::write`], [`WriteTx::event`], [`WriteTx::next_id`],
//! [`Ledger::data_version`], the backup repo and its dump ([`dump`]).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::Value;

use crate::model::EventKind;
use crate::schema::{MIGRATIONS, SCHEMA_VERSION};
use crate::{Error, Result};

/// The ledger file inside a state dir.
pub const DB_NAME: &str = "ledger.db";
/// The lock file next to it.
pub const LOCK_NAME: &str = "ledger.lock";
/// `PRAGMA busy_timeout` (Python: 5000 ms).
pub const BUSY_TIMEOUT_MS: u32 = 5000;

// ---------------------------------------------------------------- time

/// Current time as ISO-8601 UTC with second resolution, e.g. `2026-11-02T21:24:00Z`
/// (Python `db.now`).
pub fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_utc(secs)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (proleptic Gregorian, UTC).
pub fn format_utc(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

// ---------------------------------------------------------------- connections

/// Open the db read-write (no migration): WAL, busy timeout, foreign keys on.
pub fn connect(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        tracing::warn!(%mode, path = %path.display(), "ledger is not in WAL mode");
    }
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(conn)
}

/// Open an existing ledger read-only (the dashboard / app views): never creates,
/// migrates or writes. Refuses a schema other than [`SCHEMA_VERSION`].
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
    let v = schema_version(&conn)?;
    if v != SCHEMA_VERSION {
        return Err(Error::Arb(format!(
            "ledger schema v{v}, this arb reads v{SCHEMA_VERSION}: run any arb command in the project once to upgrade it"
        )));
    }
    Ok(conn)
}

/// `PRAGMA user_version`.
pub fn schema_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// `PRAGMA data_version`: changes when another connection has committed since this
/// connection last looked (this connection's own commits do not change it).
pub fn data_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA data_version", [], |r| r.get(0))?)
}

/// Apply pending migrations; return the resulting version (Python `db.migrate`).
pub fn migrate(conn: &mut Connection) -> Result<u32> {
    let version = schema_version(conn)?;
    if version > SCHEMA_VERSION {
        return Err(Error::Arb(format!(
            "ledger schema version {version} is newer than this arb ({SCHEMA_VERSION}); upgrade arb"
        )));
    }
    if version == SCHEMA_VERSION {
        return Ok(SCHEMA_VERSION);
    }
    // Table rebuilds need foreign keys off; it cannot change inside a transaction.
    let fk_was_on: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let result = (|| -> Result<()> {
        for target in version + 1..=SCHEMA_VERSION {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(MIGRATIONS[target as usize - 1])?;
            let bad: Vec<String> = {
                let mut stmt = tx.prepare("PRAGMA foreign_key_check")?;
                let rows = stmt.query_map([], |r| {
                    Ok(format!(
                        "({}, {:?}, {}, {})",
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?
                    ))
                })?;
                rows.take(5).collect::<rusqlite::Result<_>>()?
            };
            if !bad.is_empty() {
                return Err(Error::Arb(format!(
                    "migration to schema v{target} broke foreign keys: [{}]",
                    bad.join(", ")
                )));
            }
            tx.pragma_update(None, "user_version", target)?;
            tx.commit()?;
        }
        Ok(())
    })();
    if fk_was_on {
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
    }
    result.map(|()| SCHEMA_VERSION)
}

// ---------------------------------------------------------------- ledger

/// The ledger of one state dir (a project's `.arboria/` or the home's).
#[derive(Debug)]
pub struct Ledger {
    state_dir: PathBuf,
    conn: Connection,
}

/// Open `<state_dir>/ledger.db`, creating the dir and the db, and migrate it to
/// [`SCHEMA_VERSION`] (under the ledger lock).
pub fn open(state_dir: &Path) -> Result<Ledger> {
    Ledger::open(state_dir)
}

impl Ledger {
    /// See [`open`].
    pub fn open(state_dir: &Path) -> Result<Ledger> {
        std::fs::create_dir_all(state_dir)?;
        let lock = lock_file(state_dir)?;
        let result = (|| {
            let mut conn = connect(&state_dir.join(DB_NAME))?;
            migrate(&mut conn)?;
            Ok(Ledger {
                state_dir: state_dir.to_path_buf(),
                conn,
            })
        })();
        lock.unlock()?;
        result
    }

    /// The state dir.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// The db file.
    pub fn path(&self) -> PathBuf {
        self.state_dir.join(DB_NAME)
    }

    /// The connection, for reads. Writes go through [`Ledger::write`].
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// See [`data_version`].
    pub fn data_version(&self) -> Result<i64> {
        data_version(&self.conn)
    }

    /// Run `f` under the ledger lock in a `BEGIN IMMEDIATE` transaction; commit when it
    /// returns Ok, roll back otherwise. After the commit, a backup commit is made if `f`
    /// recorded a backup event (a failed backup only warns).
    pub fn write<T>(&mut self, f: impl FnOnce(&mut WriteTx<'_>) -> Result<T>) -> Result<T> {
        let lock = lock_file(&self.state_dir)?;
        let result = (|| -> Result<(T, Vec<(EventKind, String)>)> {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut w = WriteTx {
                tx,
                events: Vec::new(),
            };
            let value = f(&mut w)?;
            let WriteTx { tx, events } = w;
            tx.commit()?;
            Ok((value, events))
        })();
        let result = result.map(|(value, events)| {
            let backup: Vec<String> = events
                .iter()
                .filter(|(k, _)| k.triggers_backup())
                .map(|(k, s)| format!("{k} {s}").trim().to_string())
                .collect();
            // still under the lock, like Python: two backups never race on the git index
            if !backup.is_empty() {
                if let Err(e) = self.backup_unlocked(&backup.join("; ")) {
                    tracing::warn!("ledger backup failed: {e}");
                    eprintln!("warning: ledger backup failed: {e}");
                }
            }
            value
        });
        lock.unlock()?;
        result
    }

    /// Dump the db to `backup/ledger.sql` and commit it in the state dir's git repo
    /// (`arboria backup: <message>`); returns whether a commit was made. Idempotent.
    /// Takes the ledger lock (call it outside [`Ledger::write`]).
    ///
    /// The dump is Python 3.13's `sqlite3.Connection.iterdump` ([`dump`]), so the backup
    /// history stays readable across implementations.
    pub fn backup(&self, message: &str) -> Result<bool> {
        let lock = lock_file(&self.state_dir)?;
        let result = self.backup_unlocked(message);
        lock.unlock()?;
        result
    }

    fn backup_unlocked(&self, message: &str) -> Result<bool> {
        require_backup_repo(&self.state_dir)?;
        let backup_dir = self.state_dir.join("backup");
        std::fs::create_dir_all(&backup_dir)?;
        let version = schema_version(&self.conn)?;
        let mut text = format!("PRAGMA user_version = {version};\n");
        for line in dump(&self.conn)? {
            text.push_str(&line);
            text.push('\n');
        }
        let target = backup_dir.join("ledger.sql");
        let tmp = backup_dir.join("ledger.sql.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &target)?;
        commit_backup_unlocked(&self.state_dir, message)
    }
}

fn lock_file(state_dir: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(state_dir.join(LOCK_NAME))?;
    // std's File::lock is flock(LOCK_EX) on Linux and macOS, like Python's fcntl.flock.
    f.lock()?;
    Ok(f)
}

/// A write transaction (see [`Ledger::write`]). Derefs to the connection.
pub struct WriteTx<'a> {
    tx: rusqlite::Transaction<'a>,
    events: Vec<(EventKind, String)>,
}

impl std::ops::Deref for WriteTx<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.tx
    }
}

impl WriteTx<'_> {
    /// Append to `events` (ts = [`now`], payload as [`crate::canonical::ledger_json`]).
    pub fn event(&mut self, kind: EventKind, subject: &str, payload: &Value) -> Result<()> {
        let payload = match payload {
            Value::Null => crate::canonical::ledger_json(&Value::Object(Default::default())),
            v => crate::canonical::ledger_json(v),
        };
        self.tx.execute(
            "INSERT INTO events (ts, kind, subject, payload_json) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![now(), kind.as_str(), subject, payload],
        )?;
        self.events.push((kind, subject.to_string()));
        Ok(())
    }

    /// The next sequential id (`r001`, `d001`, …): max numeric suffix in its table + 1,
    /// like Python `Ledger.next_id`.
    pub fn next_id<T: SeqId>(&self) -> Result<T> {
        next_id_in(&self.tx)
    }
}

/// [`WriteTx::next_id`] on any connection (outside a write it is only a guess, e.g. the
/// snapshot commit message `arboria snapshot r004`).
pub fn next_id_in<T: SeqId>(conn: &Connection) -> Result<T> {
    let mut stmt = conn.prepare(&format!("SELECT id FROM {}", T::TABLE))?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(T::from_number(next_number(ids.iter().map(String::as_str))))
}

// ---------------------------------------------------------------- rows

/// A row as a JSON object keyed by column name (INTEGER → number, REAL → float,
/// TEXT → string, NULL → null; BLOBs are not used by the ledger and read as null).
pub fn row_map(row: &rusqlite::Row<'_>) -> rusqlite::Result<serde_json::Map<String, Value>> {
    use rusqlite::types::ValueRef;
    let stmt = row.as_ref();
    let mut map = serde_json::Map::new();
    for i in 0..stmt.column_count() {
        let v = match row.get_ref(i)? {
            ValueRef::Null | ValueRef::Blob(_) => Value::Null,
            ValueRef::Integer(n) => Value::from(n),
            ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
            ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        };
        map.insert(stmt.column_name(i)?.to_string(), v);
    }
    Ok(map)
}

/// A ledger table row type read with `SELECT *` (Python `Ledger.one` / `Ledger.all`).
pub trait LedgerRow: serde::de::DeserializeOwned {
    /// Adjust the column map before deserializing (e.g. decode a JSON column).
    fn fix(map: &mut serde_json::Map<String, Value>) -> Result<()> {
        let _ = map;
        Ok(())
    }

    /// Deserialize one row.
    fn from_row(row: &rusqlite::Row<'_>) -> Result<Self> {
        let mut map = row_map(row)?;
        Self::fix(&mut map)?;
        Ok(serde_json::from_value(Value::Object(map))?)
    }
}

impl LedgerRow for crate::model::Idea {}
impl LedgerRow for crate::model::Run {}
impl LedgerRow for crate::model::Job {}
impl LedgerRow for crate::model::Note {}
impl LedgerRow for crate::model::Lesson {}
impl LedgerRow for crate::model::Task {}
impl LedgerRow for crate::model::Event {}
impl LedgerRow for crate::model::Planting {}
impl LedgerRow for crate::model::Inspection {}
impl LedgerRow for crate::model::Decision {
    /// `options_json` → `options`.
    fn fix(map: &mut serde_json::Map<String, Value>) -> Result<()> {
        let options = match map.remove("options_json") {
            Some(Value::String(s)) if !s.is_empty() => serde_json::from_str(&s)?,
            _ => Value::Array(Vec::new()),
        };
        map.insert("options".into(), options);
        Ok(())
    }
}

/// The first row of `sql`, or None.
pub fn one<T: LedgerRow>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Option<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    match rows.next()? {
        Some(row) => Ok(Some(T::from_row(row)?)),
        None => Ok(None),
    }
}

/// Every row of `sql`.
pub fn all<T: LedgerRow>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(T::from_row(row)?);
    }
    Ok(out)
}

/// `max + 1` over the numeric suffixes of `ids` (Python `Ledger.next_id`): the suffix is
/// everything after the first character whatever that character is, and suffixes that
/// are not all digits are ignored; gaps are not filled.
pub fn next_number<'a>(ids: impl IntoIterator<Item = &'a str>) -> u64 {
    ids.into_iter()
        .filter_map(|id| {
            let rest: &str = id.char_indices().nth(1).map_or("", |(i, _)| &id[i..]);
            (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
                .then(|| rest.parse::<u64>().ok())
                .flatten()
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// Text ids handed out by [`WriteTx::next_id`].
pub trait SeqId: Sized {
    /// The table whose `id` column holds them.
    const TABLE: &'static str;
    /// `{prefix}{n:03}`.
    fn from_number(n: u64) -> Self;
}

macro_rules! seq_table {
    ($t:ty, $table:literal) => {
        impl SeqId for $t {
            const TABLE: &'static str = $table;
            fn from_number(n: u64) -> Self {
                <$t>::from_number(n)
            }
        }
    };
}
seq_table!(crate::model::RunId, "runs");
seq_table!(crate::model::DecisionId, "decisions");
seq_table!(crate::model::TaskId, "tasks");
seq_table!(crate::model::LessonId, "lessons");

// ---------------------------------------------------------------- backup repo

/// `.gitignore` of a new backup repo: only the SQL dump and the notebooks are versioned.
pub const BACKUP_GITIGNORE: &str =
    "# Only the SQL dump and the idea notebooks are versioned here.\n/*\n!/.gitignore\n!/backup/\n/backup/*\n!/backup/ledger.sql\n";
/// Appended for the notebooks.
pub const NOTEBOOKS_GITIGNORE: &str = "!/notebooks/\n/notebooks/*\n!/notebooks/*.py\n";
/// Appended for the project's notebook commands.
pub const COMMANDS_GITIGNORE: &str = "!/commands/\n/commands/*\n!/commands/*.py\n";
/// Appended in a home for the seed notebooks' copies.
pub const SEEDS_GITIGNORE: &str = "!/seeds/\n/seeds/*\n!/seeds/*.py\n";

/// Make `.arboria/` a git repo (idempotent).
pub fn init_backup_repo(state_dir: &Path) -> Result<()> {
    if !state_dir.join(".git").exists() {
        backup_git(&["init", "-q"], state_dir)?;
    }
    ensure_backup_gitignore(state_dir)
}

/// Bring the backup repo's `.gitignore` up to date (also older ones).
pub fn ensure_backup_gitignore(state_dir: &Path) -> Result<()> {
    let gi = state_dir.join(".gitignore");
    let existing = match std::fs::read_to_string(&gi) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let mut text = existing
        .clone()
        .unwrap_or_else(|| BACKUP_GITIGNORE.to_string());
    let append = |text: &mut String, block: &str| {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(block);
    };
    if !text.contains("!/notebooks/") {
        append(&mut text, NOTEBOOKS_GITIGNORE);
    }
    if !text.contains("!/commands/") {
        append(&mut text, COMMANDS_GITIGNORE);
    }
    // a home only
    if state_dir.join("seeds").is_dir() && !text.contains("!/seeds/") {
        append(&mut text, SEEDS_GITIGNORE);
    }
    if existing.as_deref() != Some(text.as_str()) {
        std::fs::write(&gi, text)?;
    }
    Ok(())
}

/// In a home: copy `<home>/seeds/*.py` into `.arboria/seeds/` (changed files only), so
/// the backup repo versions the human's seed notebooks. Copies of deleted notebooks stay
/// (the backup is history, not a sync). Projects have no `.arboria/seeds/`: no-op. A file
/// that cannot be copied only warns.
pub fn mirror_seeds(state_dir: &Path) -> Result<()> {
    let mirror = state_dir.join("seeds");
    let Some(source) = state_dir.parent().map(|p| p.join("seeds")) else {
        return Ok(());
    };
    if !mirror.is_dir() || !source.is_dir() {
        return Ok(());
    }
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&source)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "py"))
        .collect();
    // Python sorts the paths (str order = byte order for UTF-8 names)
    sources.sort();
    for src in sources {
        let copy = || -> std::io::Result<()> {
            if !src.is_file() {
                return Ok(());
            }
            let data = std::fs::read(&src)?;
            let name = src.file_name().expect("globbed file has a name");
            let dst = mirror.join(name);
            if dst.is_file() && std::fs::read(&dst)? == data {
                return Ok(());
            }
            let mut tmp_name = std::ffi::OsString::from(".");
            tmp_name.push(name);
            tmp_name.push(".arboria-tmp");
            let tmp = mirror.join(tmp_name);
            std::fs::write(&tmp, &data)?;
            std::fs::rename(&tmp, &dst)
        };
        if let Err(e) = copy() {
            eprintln!("warning: could not back up {}: {e}", src.display());
        }
    }
    Ok(())
}

/// `git -c commit.gpgsign=false -c core.hooksPath=/dev/null <args>` with the fallback
/// identity (backup commits are arboria's, not the user's).
fn backup_git(args: &[&str], state_dir: &Path) -> Result<String> {
    let mut full = vec![
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
    ];
    full.extend_from_slice(args);
    crate::gitutil::out(&full, state_dir, crate::gitutil::FALLBACK_IDENTITY)
}

/// Refuse to run git in a state dir without its own repo: git would fall back to the
/// enclosing project repo and commit the user's staged changes.
fn require_backup_repo(state_dir: &Path) -> Result<()> {
    if state_dir.join(".git").exists() {
        Ok(())
    } else {
        Err(Error::Arb(format!(
            "{} has no backup repository (.git); run `arb init` again to create it",
            state_dir.display()
        )))
    }
}

/// Commit whatever the backup repo's `.gitignore` versions, if anything changed. Takes
/// the ledger lock (backups never race on the git index) and is idempotent (ADR 0001).
pub fn commit_backup(state_dir: &Path, message: &str) -> Result<bool> {
    let lock = lock_file(state_dir)?;
    let result = commit_backup_unlocked(state_dir, message);
    lock.unlock()?;
    result
}

fn commit_backup_unlocked(state_dir: &Path, message: &str) -> Result<bool> {
    require_backup_repo(state_dir)?;
    mirror_seeds(state_dir)?;
    ensure_backup_gitignore(state_dir)?;
    backup_git(&["add", "-A", "--", "."], state_dir)?;
    if crate::gitutil::ok(&["diff", "--cached", "--quiet"], state_dir)?
        && crate::gitutil::ok(&["rev-parse", "--verify", "--quiet", "HEAD"], state_dir)?
    {
        return Ok(false);
    }
    backup_git(
        &[
            "commit",
            "-q",
            "--no-verify",
            "-m",
            &format!("arboria backup: {message}"),
        ],
        state_dir,
    )?;
    Ok(true)
}

// ---------------------------------------------------------------- dump

fn quote_name(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn quote_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The SQL dump of the db, line by line: Python 3.13's `sqlite3.Connection.iterdump()`
/// (`Lib/sqlite3/dump.py`), statement for statement. Values come from SQLite's own
/// `quote()`, as there.
pub fn dump(conn: &Connection) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut writable_schema = false;
    let violations = {
        let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        rows.next()?.is_some()
    };
    if violations {
        out.push("PRAGMA foreign_keys=OFF;".to_string());
    }
    out.push("BEGIN TRANSACTION;".to_string());
    let tables: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT \"name\", \"type\", \"sql\" FROM \"sqlite_master\" \
             WHERE \"sql\" NOT NULL AND \"type\" == 'table' ORDER BY \"name\"",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut sqlite_sequence = Vec::new();
    for (table, sql) in tables {
        if table == "sqlite_sequence" {
            sqlite_sequence.push("DELETE FROM \"sqlite_sequence\"".to_string());
            let mut stmt = conn.prepare("SELECT * FROM \"sqlite_sequence\";")?;
            let rows = stmt.query_map([], |r| {
                let name: String = r.get(0)?;
                let seq = match r.get_ref(1)? {
                    rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                    rusqlite::types::ValueRef::Null => "None".to_string(),
                    other => format!("{other:?}"),
                };
                Ok((name, seq))
            })?;
            for row in rows {
                let (name, seq) = row?;
                sqlite_sequence.push(format!(
                    "INSERT INTO \"sqlite_sequence\" VALUES({},{seq})",
                    quote_value(&name)
                ));
            }
            continue;
        } else if table == "sqlite_stat1" {
            out.push("ANALYZE \"sqlite_master\";".to_string());
        } else if table.starts_with("sqlite_") {
            continue;
        } else if sql.starts_with("CREATE VIRTUAL TABLE") {
            if !writable_schema {
                writable_schema = true;
                out.push("PRAGMA writable_schema=ON;".to_string());
            }
            out.push(format!(
                "INSERT INTO sqlite_master(type,name,tbl_name,rootpage,sql)VALUES('table',{0},{0},0,{1});",
                quote_value(&table),
                quote_value(&sql)
            ));
        } else {
            out.push(format!("{sql};"));
        }
        let ident = quote_name(&table);
        let columns: Vec<String> = {
            let mut stmt = conn.prepare(&format!("PRAGMA table_info({ident})"))?;
            let rows = stmt.query_map([], |r| {
                Ok(match r.get_ref(1)? {
                    rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    other => format!("{other:?}"),
                })
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let select = format!(
            "SELECT 'INSERT INTO {} VALUES('{}')' FROM {ident};",
            ident.replace('\'', "''"),
            columns
                .iter()
                .map(|c| format!("||quote({})||", quote_name(c)))
                .collect::<Vec<_>>()
                .join("','")
        );
        let mut stmt = conn.prepare(&select)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            out.push(format!("{};", row?));
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT \"name\", \"type\", \"sql\" FROM \"sqlite_master\" \
             WHERE \"sql\" NOT NULL AND \"type\" IN ('index', 'trigger', 'view')",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(2))?;
        for sql in rows {
            out.push(format!("{};", sql?));
        }
    }
    if writable_schema {
        out.push("PRAGMA writable_schema=OFF;".to_string());
    }
    for row in sqlite_sequence {
        out.push(format!("{row};"));
    }
    out.push("COMMIT;".to_string());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_format() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_793_655_840), "2026-11-02T21:44:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(-1), "1969-12-31T23:59:59Z");
        let n = now();
        assert_eq!(n.len(), 20);
        assert!(n.ends_with('Z') && n.as_bytes()[10] == b'T');
    }
}
