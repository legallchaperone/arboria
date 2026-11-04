//! The ledger schema: Python's migrations v1–v7 verbatim, and v8 (the Rust core's).
//! Ports the SQL constants of `arboria.db`.
//!
//! Invariants:
//! - `MIGRATIONS[i]` upgrades `user_version` i → i+1. Never edit a released migration;
//!   v1–v7 are Python's text byte for byte (`tests/schema.rs` compares them with
//!   `tests/fixtures/migrations_v1_v7.sql`, generated from `src/arboria/db.py`), so a
//!   ledger made by either implementation has the same schema.
//! - v8 is additive (ADR 0001 "只加不拆"): no column is removed and no table is dropped
//!   except the old copy in a table rebuild (SQLite cannot ALTER a CHECK or a NOT NULL);
//!   rebuilt tables keep their v7 columns in order and append the new ones. v7 columns
//!   keep their meaning, so Python's read-only queries still run on a v8 ledger.
//!
//! # v8 delta
//!
//! `ideas` (rebuilt; seeds merged in):
//! - `hypothesis`, `metric`, `branch`, `worktree_path`: now nullable — a seed has no repo
//!   yet and may have no hypothesis / metric. Ideas with a branch still need both.
//! - `stage` TEXT NOT NULL ∈ seed | open | merged | dropped: the one lifecycle. v7 rows get
//!   `stage = status`.
//! - `status` (kept for Python readers): `CASE stage WHEN 'seed' THEN 'open' ELSE stage END`,
//!   enforced by a CHECK.
//! - `gist` TEXT: a seed's one line (≤300, human only); was `seeds.gist`.
//! - `updated_at` TEXT NOT NULL: last change of gist / hypothesis / metric / stage /
//!   confidence; backfilled with `COALESCE(closed_at, created_at)` (seeds: their
//!   `updated_at`).
//! - `legacy_seed_id` INTEGER UNIQUE: the `seeds.id` a row was migrated from, NULL otherwise.
//! - Every `seeds` row becomes an `ideas` row: id = seed id + max(ideas.id) (so a home
//!   ledger, whose ideas table was empty, keeps its ids), stage `seed` (thinking or planted;
//!   planted-ness is `EXISTS plantings`) or `dropped` (closed_at = updated_at).
//!
//! `seeds`: kept as it was at the migration, frozen. v8 writers never touch it.
//!
//! `plantings` (rebuilt): + `seed_idea_id` INTEGER NOT NULL → ideas(id), the seed's row;
//! `seed_id` becomes nullable (legacy, set on migrated rows only).
//!
//! `tasks` (rebuilt; the agent queue):
//! - `idea_id` NOT NULL: a seed's todo now points at the seed's ideas row; `seed_id` is
//!   legacy (migrated rows keep it, so Python's joins on `seeds` still work).
//! - `status` ∈ open | queued | running | check_failed | needs_human | done | dropped;
//!   the CHECK is now `done_at IS NOT NULL` iff status ∈ (done, dropped).
//! - `attempts` INTEGER NOT NULL DEFAULT 0: background `claude -p` attempts so far.
//! - `session_id` TEXT: the latest attempt's session (for `--resume`).
//! - `model` TEXT: the latest attempt's model.
//! - `usage_json` TEXT: usage summed over attempts (tokens, cost, turns), `ledger_json`.
//! - `started_at`, `finished_at` TEXT: the latest attempt.
//! - `block_id` TEXT: the notebook block (```` ```arb {#id} ````) that asked for it.
//! - index `tasks_queue (status, created_at)`.
//! - `kind` also ∈ probe_new | probe_revise (ADR 0003: a todo asking the agent to write or
//!   change a probe; details in `payload_json`, like the command todos).
//!
//! `runs` (ALTER ADD): `block_id` TEXT (the notebook block that submitted it), `nonce`
//! TEXT (the `--repeat` nonce, backfilled from `request_json` `$.ask.repeat`; the card
//! hash still reads it from the request).
//!
//! `decisions` (rebuilt for its CHECK; columns kept in order):
//! - `subject_type` also ∈ `probe`: the human's hash-bound enable / disable of a probe
//!   (ADR 0003). `subject_id` = the probe key `<scope>:<name>` (`project:deltaai_credits`,
//!   `global:x`), `options_json` = `["enable", "disable"]`, `choice` ∈ enable | disable,
//!   `decided_by = 'human'`, `payload_json` = what the human was shown (machine, argv,
//!   every, kind, api, path) plus the provenance.
//! - `bound_hash` TEXT: the sha256 hex of the probe file's bytes the decision is bound to;
//!   required (CHECK) for a probe `enable`, NULL for every other decision (run approvals
//!   bind to `runs.card_hash` through the run row). The helper runs a probe only when the
//!   file's current hash equals the `bound_hash` of the latest decided probe decision and
//!   that decision is `enable` ([`crate::probe::enabled_hash`]).
//! - index `decisions_subject (subject_type, subject_id)`.
//!
//! `probe_results` (new): one row per probe run that produced a valid result (ADR 0003).
//! - `machine` TEXT: the profile name the probe ran against; `probe` TEXT: the probe key;
//! - `at` TEXT: when (ISO-8601 UTC, [`crate::db::now`]);
//! - `input_hash` TEXT: sha256 hex of the (truncated) stdout handed to the probe;
//! - `result_json` TEXT: the validated [`crate::probe::ProbeResult`], `ledger_json`;
//! - `probe_hash` TEXT: the enabled file hash that produced it (NULL for built-ins);
//! - index `probe_results_latest (machine, probe, at)`. Heartbeat-like: no event, no
//!   backup commit; written only by [`crate::probe::set_probe_result`].
//!
//! Python compatibility: Python's `db.migrate` refuses a ledger newer than v7 and the
//! dashboard requires exactly v7, so reading a v8 ledger from Python needs a small Python
//! change (accept 8 read-only). Python must never *write* a v8 ledger.

/// The schema version this build writes (`PRAGMA user_version`).
pub const SCHEMA_VERSION: u32 = 8;

/// The last version written by the Python implementation.
pub const PYTHON_SCHEMA_VERSION: u32 = 7;

/// `MIGRATIONS[i]` upgrades `user_version` i → i+1.
pub const MIGRATIONS: [&str; SCHEMA_VERSION as usize] = [
    SCHEMA_V1,
    MIGRATION_V2,
    MIGRATION_V3,
    MIGRATION_V4,
    MIGRATION_V5,
    MIGRATION_V6,
    MIGRATION_V7,
    MIGRATION_V8,
];

/// v7 → v8 (see the module docs for every column).
pub const MIGRATION_V8: &str = r#"CREATE TABLE ideas_v8 (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL UNIQUE,
    hypothesis     TEXT,
    metric         TEXT,
    parent_id      INTEGER REFERENCES ideas(id),
    confidence     TEXT NOT NULL DEFAULT 'guess' CHECK (confidence IN ('guess', 'prelim', 'confirmed', 'rejected')),
    status         TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'merged', 'dropped')),
    branch         TEXT,
    worktree_path  TEXT,
    created_at     TEXT NOT NULL,
    closed_at      TEXT,
    stage          TEXT NOT NULL DEFAULT 'open' CHECK (stage IN ('seed', 'open', 'merged', 'dropped')),
    gist           TEXT,
    updated_at     TEXT NOT NULL,
    legacy_seed_id INTEGER UNIQUE,
    CHECK (status = CASE stage WHEN 'seed' THEN 'open' ELSE stage END),
    CHECK ((branch IS NULL) = (worktree_path IS NULL)),
    CHECK (branch IS NULL OR (hypothesis IS NOT NULL AND metric IS NOT NULL)),
    CHECK (stage <> 'seed' OR branch IS NULL),
    CHECK (stage NOT IN ('open', 'merged') OR branch IS NOT NULL)
);
INSERT INTO ideas_v8 (id, name, hypothesis, metric, parent_id, confidence, status, branch,
                      worktree_path, created_at, closed_at, stage, updated_at)
    SELECT id, name, hypothesis, metric, parent_id, confidence, status, branch,
           worktree_path, created_at, closed_at, status, COALESCE(closed_at, created_at)
    FROM ideas;
CREATE TEMP TABLE v8_seed_offset AS SELECT COALESCE(MAX(id), 0) AS n FROM ideas;
DROP TABLE ideas;
ALTER TABLE ideas_v8 RENAME TO ideas;
INSERT INTO ideas (id, name, gist, hypothesis, metric, status, stage, created_at, updated_at,
                   closed_at, legacy_seed_id)
    SELECT s.id + (SELECT n FROM v8_seed_offset), s.name, s.gist, s.hypothesis, s.metric,
           CASE s.status WHEN 'dropped' THEN 'dropped' ELSE 'open' END,
           CASE s.status WHEN 'dropped' THEN 'dropped' ELSE 'seed' END,
           s.created_at, s.updated_at,
           CASE s.status WHEN 'dropped' THEN s.updated_at END,
           s.id
    FROM seeds s ORDER BY s.id;
DROP TABLE v8_seed_offset;
CREATE INDEX ideas_stage ON ideas(stage);
CREATE TABLE plantings_v8 (
    id           INTEGER PRIMARY KEY,
    seed_id      INTEGER REFERENCES seeds(id),
    repo         TEXT NOT NULL,
    project_id   TEXT,
    idea         TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    seed_idea_id INTEGER NOT NULL REFERENCES ideas(id)
);
INSERT INTO plantings_v8 (id, seed_id, repo, project_id, idea, created_at, seed_idea_id)
    SELECT p.id, p.seed_id, p.repo, p.project_id, p.idea, p.created_at,
           (SELECT i.id FROM ideas i WHERE i.legacy_seed_id = p.seed_id)
    FROM plantings p;
DROP TABLE plantings;
ALTER TABLE plantings_v8 RENAME TO plantings;
CREATE INDEX plantings_seed ON plantings(seed_idea_id);
CREATE TABLE tasks_v8 (
    id           TEXT PRIMARY KEY,
    idea_id      INTEGER NOT NULL REFERENCES ideas(id),
    seed_id      INTEGER REFERENCES seeds(id),
    text         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'queued', 'running', 'check_failed', 'needs_human', 'done', 'dropped')),
    created_at   TEXT NOT NULL,
    done_at      TEXT,
    result       TEXT,
    kind         TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN ('plain', 'command_new', 'command_revise', 'inspect', 'probe_new', 'probe_revise')),
    payload_json TEXT,
    attempts     INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    session_id   TEXT,
    model        TEXT,
    usage_json   TEXT,
    started_at   TEXT,
    finished_at  TEXT,
    block_id     TEXT,
    CHECK ((status IN ('done', 'dropped')) = (done_at IS NOT NULL))
);
INSERT INTO tasks_v8 (id, idea_id, seed_id, text, status, created_at, done_at, result, kind,
                      payload_json)
    SELECT t.id,
           COALESCE(t.idea_id, (SELECT i.id FROM ideas i WHERE i.legacy_seed_id = t.seed_id)),
           t.seed_id, t.text, t.status, t.created_at, t.done_at, t.result, t.kind, t.payload_json
    FROM tasks t;
DROP TABLE tasks;
ALTER TABLE tasks_v8 RENAME TO tasks;
CREATE INDEX tasks_idea ON tasks(idea_id, status);
CREATE INDEX tasks_seed ON tasks(seed_id, status);
CREATE INDEX tasks_queue ON tasks(status, created_at);
ALTER TABLE runs ADD COLUMN block_id TEXT;
ALTER TABLE runs ADD COLUMN nonce TEXT;
UPDATE runs SET nonce = json_extract(request_json, '$.ask.repeat')
    WHERE request_json IS NOT NULL AND json_valid(request_json);
CREATE INDEX runs_block ON runs(block_id);
CREATE TABLE decisions_v8 (
    id           TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('run', 'idea', 'lesson', 'probe')),
    subject_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    choice       TEXT,
    decided_by   TEXT,
    created_at   TEXT NOT NULL,
    payload_json TEXT,
    bound_hash   TEXT,
    CHECK (subject_type <> 'probe' OR choice IS NULL OR choice IN ('enable', 'disable')),
    CHECK (NOT (subject_type = 'probe' AND choice = 'enable' AND bound_hash IS NULL))
);
INSERT INTO decisions_v8 (id, subject_type, subject_id, question, options_json, choice,
                          decided_by, created_at, payload_json)
    SELECT id, subject_type, subject_id, question, options_json, choice, decided_by,
           created_at, payload_json
    FROM decisions;
DROP TABLE decisions;
ALTER TABLE decisions_v8 RENAME TO decisions;
CREATE INDEX decisions_subject ON decisions(subject_type, subject_id);
CREATE TABLE probe_results (
    id          INTEGER PRIMARY KEY,
    machine     TEXT NOT NULL,
    probe       TEXT NOT NULL,
    at          TEXT NOT NULL,
    input_hash  TEXT NOT NULL,
    result_json TEXT NOT NULL,
    probe_hash  TEXT
);
CREATE INDEX probe_results_latest ON probe_results(machine, probe, at);"#;

// ---------------------------------------------------------------- v1–v7 (Python, verbatim)

/// Python `db.SCHEMA_V1`, verbatim.
pub const SCHEMA_V1: &str = r#"CREATE TABLE ideas (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    hypothesis    TEXT NOT NULL,
    metric        TEXT NOT NULL,
    parent_id     INTEGER REFERENCES ideas(id),
    confidence    TEXT NOT NULL DEFAULT 'guess' CHECK (confidence IN ('guess', 'prelim', 'confirmed', 'rejected')),
    status        TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'merged', 'dropped')),
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
    status          TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'awaiting_approval', 'approved', 'queued', 'running', 'succeeded', 'failed', 'cancelled')),
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
    confidence TEXT CHECK (confidence IS NULL OR confidence IN ('guess', 'prelim', 'confirmed', 'rejected')),
    created_at TEXT NOT NULL
);
CREATE INDEX notes_idea ON notes(idea_id);
CREATE TABLE events (
    id           INTEGER PRIMARY KEY,
    ts           TEXT NOT NULL,
    kind         TEXT NOT NULL,
    subject      TEXT,
    payload_json TEXT
);"#;

/// Python `db.MIGRATION_V2`, verbatim.
pub const MIGRATION_V2: &str = r#"CREATE TABLE lessons (
    id          TEXT PRIMARY KEY,
    text        TEXT NOT NULL,
    source_type TEXT NOT NULL DEFAULT 'none' CHECK (source_type IN ('idea', 'run', 'none')),
    source_id   TEXT,
    status      TEXT NOT NULL DEFAULT 'proposed' CHECK (status IN ('proposed', 'accepted', 'retired')),
    created_at  TEXT NOT NULL,
    decided_at  TEXT,
    CHECK ((source_type = 'none') = (source_id IS NULL))
);
CREATE INDEX lessons_status ON lessons(status);
CREATE TABLE decisions_v2 (
    id           TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('run', 'idea', 'lesson')),
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
ALTER TABLE decisions_v2 RENAME TO decisions;"#;

/// Python `db.MIGRATION_V3`, verbatim.
pub const MIGRATION_V3: &str = r#"CREATE TABLE runs_v3 (
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
    status          TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'awaiting_approval', 'approved', 'queued', 'running', 'succeeded', 'failed', 'cancelled', 'rejected')),
    review_json     TEXT,
    wandb_url       TEXT,
    summary         TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    request_json    TEXT,
    card_hash       TEXT
);
INSERT INTO runs_v3 (id, idea_id, snapshot_commit, tree_hash, config_json, change_reason, machine, "partition", gpus, walltime, est_cost, actual_cost, status, review_json, wandb_url, summary, created_at, updated_at) SELECT id, idea_id, snapshot_commit, tree_hash, config_json, change_reason, machine, "partition", gpus, walltime, est_cost, actual_cost, status, review_json, wandb_url, summary, created_at, updated_at FROM runs;
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
    state        TEXT NOT NULL DEFAULT 'submitting' CHECK (state IN ('submitting', 'pending', 'running', 'completed', 'failed', 'cancelled', 'timeout', 'unknown')),
    elapsed_s    INTEGER,
    billed_su    REAL,
    submitted_at TEXT,
    updated_at   TEXT NOT NULL,
    UNIQUE (run_id, segment)
);
CREATE INDEX jobs_run ON jobs(run_id);"#;

/// Python `db.MIGRATION_V4`, verbatim.
pub const MIGRATION_V4: &str = r#"ALTER TABLE runs ADD COLUMN parent_run TEXT REFERENCES runs(id);
ALTER TABLE decisions ADD COLUMN payload_json TEXT;
ALTER TABLE notes ADD COLUMN author TEXT;
CREATE INDEX runs_card_hash ON runs(idea_id, card_hash);
CREATE TABLE tasks (
    id         TEXT PRIMARY KEY,
    idea_id    INTEGER NOT NULL REFERENCES ideas(id),
    text       TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done', 'dropped')),
    created_at TEXT NOT NULL,
    done_at    TEXT,
    result     TEXT,
    CHECK ((status = 'open') = (done_at IS NULL))
);
CREATE INDEX tasks_idea ON tasks(idea_id, status);"#;

/// Python `db.MIGRATION_V5`, verbatim.
pub const MIGRATION_V5: &str = r#"ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN ('plain', 'command_new', 'command_revise'));
ALTER TABLE tasks ADD COLUMN payload_json TEXT;"#;

/// Python `db.MIGRATION_V6`, verbatim.
pub const MIGRATION_V6: &str = r#"CREATE TABLE seeds (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    gist        TEXT,
    hypothesis  TEXT,
    metric      TEXT,
    status      TEXT NOT NULL DEFAULT 'thinking' CHECK (status IN ('thinking', 'planted', 'dropped')),
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
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done', 'dropped')),
    created_at   TEXT NOT NULL,
    done_at      TEXT,
    result       TEXT,
    kind         TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN ('plain', 'command_new', 'command_revise')),
    payload_json TEXT,
    CHECK ((status = 'open') = (done_at IS NULL)),
    CHECK ((idea_id IS NULL) != (seed_id IS NULL))
);
INSERT INTO tasks_v6 (id, idea_id, text, status, created_at, done_at, result, kind, payload_json)
    SELECT id, idea_id, text, status, created_at, done_at, result, kind, payload_json FROM tasks;
DROP TABLE tasks;
ALTER TABLE tasks_v6 RENAME TO tasks;
CREATE INDEX tasks_idea ON tasks(idea_id, status);
CREATE INDEX tasks_seed ON tasks(seed_id, status);"#;

/// Python `db.MIGRATION_V7`, verbatim.
pub const MIGRATION_V7: &str = r#"CREATE TABLE tasks_v7 (
    id           TEXT PRIMARY KEY,
    idea_id      INTEGER REFERENCES ideas(id),
    seed_id      INTEGER REFERENCES seeds(id),
    text         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done', 'dropped')),
    created_at   TEXT NOT NULL,
    done_at      TEXT,
    result       TEXT,
    kind         TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN ('plain', 'command_new', 'command_revise', 'inspect')),
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
    view        TEXT NOT NULL CHECK (view IN ('code', 'pseudo', 'simple', 'map')),
    target      TEXT NOT NULL,
    code_ref    TEXT NOT NULL,
    tree        TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX inspections_key ON inspections(idea_id, question, view, target);"#;
