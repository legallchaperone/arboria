"""State location, `arb init`, schema and validation."""

from __future__ import annotations

import re
import sqlite3
import threading
from pathlib import Path

import pytest

from arboria import config, db
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
    text = (repo / "arboria.toml").read_text()
    assert text.startswith("# mine\n\n[project]\n")  # not overwritten; only the id is added
    exclude = (repo / ".git" / "info" / "exclude").read_text()
    assert exclude.count("/.arboria/") == 1
    assert git(repo, "status", "--porcelain", "--untracked-files=all") == "?? arboria.toml"
    assert (state / "backup" / "ledger.sql").is_file()
    assert git(state, "log", "--format=%s") == "arboria backup: init"
    assert git(state, "ls-files").splitlines() == [".gitignore", "backup/ledger.sql"]

    res = arb(repo, "init")
    assert "already initialized" in res.stdout
    assert (repo / "arboria.toml").read_text() == text
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
    assert re.fullmatch(r"proj-[0-9a-f]{4}", cfg["project"]["id"])
    assert cfg["watch"]["observation_minutes"] == 10
    assert tuple(cfg["watch"]["fail_patterns"]) == config.DEFAULT_FAIL_PATTERNS
    ctx = locate(repo)
    pc = config.project_config(ctx)
    assert pc.project_id == cfg["project"]["id"] == config.derive_project_id(ctx)
    assert pc.watch.observation_minutes == 10 and len(pc.watch.fail_patterns) == 4


OLD_TOML = """\
# hand-edited, keep me exactly
default_machine = "deltaai"   # trailing comment

[budget]
total_su = 500
[claudemd]
autocommit = false"""


def test_init_adds_project_id_and_preserves_the_rest(repo: Path, arb) -> None:
    import tomllib

    (repo / "arboria.toml").write_text(OLD_TOML)
    res = arb(repo, "init")
    pid = config.derive_project_id(locate(repo))
    assert f'added [project] id = "{pid}"' in res.stdout
    text = (repo / "arboria.toml").read_text()
    assert text.startswith(OLD_TOML + "\n\n[project]\n")  # byte for byte, then appended
    cfg = tomllib.loads(text)
    assert cfg["project"]["id"] == pid and cfg["budget"]["total_su"] == 500
    assert cfg["claudemd"]["autocommit"] is False
    arb(repo, "init")
    arb(repo, "init")
    assert (repo / "arboria.toml").read_text() == text  # idempotent

    # an existing [project] table without id: the id goes right under its header
    body = 'default_machine = ""\n[project] # mine\nname = "x"\n\n[budget]\ntotal_su = 1\n'
    (repo / "arboria.toml").write_text(body)
    arb(repo, "init")
    text = (repo / "arboria.toml").read_text()
    assert text.startswith('default_machine = ""\n[project] # mine\n# ')
    assert text.endswith(f'id = "{pid}"\nname = "x"\n\n[budget]\ntotal_su = 1\n')
    assert tomllib.loads(text)["project"] == {"id": pid, "name": "x"}


def test_derive_project_id(tmp_path: Path, repo: Path) -> None:
    from arboria.paths import RepoContext

    def ctx(main: Path) -> RepoContext:
        return RepoContext(cwd=main, common_dir=main / ".git", main=main, toplevel=main)

    a = config.derive_project_id(ctx(tmp_path / "a" / "My Project_v2"))
    b = config.derive_project_id(ctx(tmp_path / "b" / "My Project_v2"))
    assert a.startswith("my-project-v2-") and b.startswith("my-project-v2-") and a != b
    long = config.derive_project_id(ctx(tmp_path / ("x" * 25 + "--" + "y" * 20)))
    assert config.PROJECT_ID_RE.match(long) and len(long) <= 31 and "--" not in long
    assert config.derive_project_id(ctx(tmp_path / "实验")).startswith("proj-")
    assert config.derive_project_id(ctx(repo)) == config.derive_project_id(ctx(repo))


@pytest.mark.parametrize("body,msg", [
    ('[project]\nid = "Bad_ID"\n', "must match"),
    ('[project]\nid = "' + "a" * 32 + '"\n', "must match"),
    ("project = 3\n", r"\[project\] must be a table"),
    ("[watch]\nfail_patterns = ['loss (nan']\n", r"fail_patterns\[0\] 'loss \(nan' is not a valid"),
    ("[watch]\nfail_patterns = 'nan'\n", "must be a list"),
    ("[watch]\nobservation_minutes = 0\n", "observation_minutes must be an integer >= 1"),
])
def test_project_config_validation(repo: Path, body: str, msg: str) -> None:
    (repo / "arboria.toml").write_text(body)
    with pytest.raises(ArbError, match=msg):
        config.project_config(locate(repo))


def test_require_project_id(repo: Path) -> None:
    (repo / "arboria.toml").write_text('default_machine = ""\n')
    with pytest.raises(ArbError, match="no \\[project\\] id.*run `arb init`"):
        config.require_project_id(locate(repo))


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


def _make_v1_db(path: Path) -> None:
    conn = db.connect(path)
    for stmt in db._split_sql(db.MIGRATIONS[0]):
        conn.execute(stmt)
    conn.execute("PRAGMA user_version = 1")
    conn.execute(
        "INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
        " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')"
    )
    conn.execute(
        "INSERT INTO decisions (id, subject_type, subject_id, question, options_json, choice,"
        " decided_by, created_at) VALUES ('d001', 'idea', 'exp1', 'go?', '[\"yes\", \"no\"]', 'yes',"
        " 'human', 't1')"
    )
    conn.execute(
        "INSERT INTO decisions (id, subject_type, subject_id, question, created_at)"
        " VALUES ('d002', 'run', 'r001', 'submit?', 't2')"
    )
    conn.execute("INSERT INTO notes (idea_id, text, created_at) VALUES (1, 'a note', 't')")
    with pytest.raises(sqlite3.IntegrityError):  # v1 does not know lessons
        conn.execute("INSERT INTO decisions (id, subject_type, subject_id, question, created_at)"
                     " VALUES ('d003', 'lesson', 'l001', 'q', 't')")
    conn.close()


def test_migration_v1_to_current_preserves_data(tmp_path: Path) -> None:
    state = tmp_path / "state"
    state.mkdir()
    _make_v1_db(state / "ledger.db")
    with Ledger(state) as ledger:  # opening migrates in place
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION == 5
        decs = ledger.all("SELECT * FROM decisions ORDER BY id")
        assert [(d["id"], d["subject_type"], d["subject_id"], d["question"], d["options_json"],
                 d["choice"], d["decided_by"], d["created_at"]) for d in decs] == [
            ("d001", "idea", "exp1", "go?", '["yes", "no"]', "yes", "human", "t1"),
            ("d002", "run", "r001", "submit?", "[]", None, None, "t2"),
        ]
        assert ledger.one("SELECT name FROM ideas")["name"] == "exp1"
        assert ledger.one("SELECT text FROM notes")["text"] == "a note"
        with ledger.write() as conn:
            conn.execute("INSERT INTO decisions (id, subject_type, subject_id, question, created_at)"
                         " VALUES ('d003', 'lesson', 'l001', 'q', 't')")
            conn.execute("INSERT INTO lessons (id, text, source_type, source_id, created_at)"
                         " VALUES ('l001', 'x', 'idea', 'exp1', 't')")
        assert ledger.one("SELECT status FROM lessons")["status"] == "proposed"
        with pytest.raises(sqlite3.IntegrityError):
            with ledger.write() as conn:
                conn.execute("INSERT INTO decisions (id, subject_type, subject_id, question,"
                             " created_at) VALUES ('d004', 'bogus', 'x', 'q', 't')")
        with pytest.raises(sqlite3.IntegrityError):
            with ledger.write() as conn:
                conn.execute("INSERT INTO lessons (id, text, status, created_at)"
                             " VALUES ('l002', 'x', 'maybe', 't')")
        with pytest.raises(sqlite3.IntegrityError):  # source_id requires a source_type
            with ledger.write() as conn:
                conn.execute("INSERT INTO lessons (id, text, source_id, created_at)"
                             " VALUES ('l003', 'x', 'exp1', 't')")
    # reopening is a no-op
    with Ledger(state) as ledger:
        assert ledger.one("SELECT COUNT(*) AS n FROM decisions")["n"] == 3


def test_lesson_text_validation() -> None:
    assert validate_text("lesson", "  eval 每 500 步一次  ") == "eval 每 500 步一次"
    assert validate_text("lesson", "x" * 200)
    with pytest.raises(ArbError, match="lesson is too long"):
        validate_text("lesson", "x" * 201)
    with pytest.raises(ArbError, match="single line"):
        validate_text("lesson", "a\nb")
    with pytest.raises(ArbError, match="comment markers"):
        validate_text("lesson", "<!-- arboria:end -->")


def _make_db_at(path: Path, version: int) -> sqlite3.Connection:
    conn = db.connect(path)
    for script in db.MIGRATIONS[:version]:
        for stmt in db._split_sql(script):
            conn.execute(stmt)
    conn.execute(f"PRAGMA user_version = {version}")
    return conn


@pytest.mark.parametrize("version", [1, 2])
def test_migration_to_v3_keeps_runs_and_notes(tmp_path: Path, version: int) -> None:
    state = tmp_path / "state"
    state.mkdir()
    conn = _make_db_at(state / "ledger.db", version)
    conn.execute("INSERT INTO ideas (name, hypothesis, metric, branch, worktree_path, created_at)"
                 " VALUES ('exp1', 'h', 'm', 'idea/exp1', '/w', 't')")
    for rid, status in (("r001", "draft"), ("r002", "succeeded")):
        conn.execute(
            "INSERT INTO runs (id, idea_id, snapshot_commit, tree_hash, change_reason, machine,"
            " \"partition\", gpus, walltime, est_cost, actual_cost, status, summary, created_at,"
            " updated_at) VALUES (?, 1, 'c' || ?, 't' || ?, 'why ' || ?, 'm', 'p', 4, '1h', 1.5,"
            " 2.5, ?, 'sum', 't1', 't2')", (rid, rid, rid, rid, status))
    conn.execute("INSERT INTO notes (idea_id, run_id, text, created_at) VALUES (1, 'r002', 'n', 't')")
    with pytest.raises(sqlite3.IntegrityError):  # 'rejected' is new in v3
        conn.execute("UPDATE runs SET status = 'rejected' WHERE id = 'r001'")
    conn.close()

    with Ledger(state) as ledger:
        assert ledger.conn.execute("PRAGMA user_version").fetchone()[0] == db.SCHEMA_VERSION
        assert ledger.conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
        runs = ledger.all("SELECT * FROM runs ORDER BY id")
        assert [(r["id"], r["status"], r["snapshot_commit"], r["change_reason"], r["gpus"],
                 r["est_cost"], r["actual_cost"], r["summary"], r["request_json"], r["card_hash"])
                for r in runs] == [
            ("r001", "draft", "cr001", "why r001", 4, 1.5, 2.5, "sum", None, None),
            ("r002", "succeeded", "cr002", "why r002", 4, 1.5, 2.5, "sum", None, None),
        ]
        assert ledger.one("SELECT run_id FROM notes")["run_id"] == "r002"
        assert ledger.conn.execute("PRAGMA foreign_key_check").fetchall() == []
        indexes = {r[0] for r in ledger.conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'runs'")}
        assert {"runs_tree_hash", "runs_idea", "runs_status"} <= indexes
        with ledger.write() as c:
            c.execute("UPDATE runs SET status = 'rejected' WHERE id = 'r001'")
            c.execute("INSERT INTO jobs (run_id, segment, job_name, updated_at)"
                      " VALUES ('r002', 1, 'arboria-r002-s1', 't')")
        job = ledger.one("SELECT * FROM jobs")
        assert job["state"] == "submitting" and job["job_id"] is None
        for bad in ("INSERT INTO jobs (run_id, segment, job_name, updated_at)"
                    " VALUES ('r002', 1, 'dup', 't')",  # UNIQUE(run_id, segment)
                    "INSERT INTO jobs (run_id, segment, job_name, state, updated_at)"
                    " VALUES ('r002', 2, 'x', 'exploded', 't')",
                    "INSERT INTO jobs (run_id, segment, job_name, updated_at)"
                    " VALUES ('r999', 1, 'x', 't')"):  # foreign key
            with pytest.raises(sqlite3.IntegrityError):
                with ledger.write() as c:
                    c.execute(bad)
