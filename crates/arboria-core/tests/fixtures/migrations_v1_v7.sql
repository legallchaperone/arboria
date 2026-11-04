-- @@ v1
CREATE TABLE ideas (
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
);
-- @@ v2
CREATE TABLE lessons (
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
ALTER TABLE decisions_v2 RENAME TO decisions;
-- @@ v3
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
CREATE INDEX jobs_run ON jobs(run_id);
-- @@ v4
ALTER TABLE runs ADD COLUMN parent_run TEXT REFERENCES runs(id);
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
CREATE INDEX tasks_idea ON tasks(idea_id, status);
-- @@ v5
ALTER TABLE tasks ADD COLUMN kind TEXT NOT NULL DEFAULT 'plain' CHECK (kind IN ('plain', 'command_new', 'command_revise'));
ALTER TABLE tasks ADD COLUMN payload_json TEXT;
-- @@ v6
CREATE TABLE seeds (
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
CREATE INDEX tasks_seed ON tasks(seed_id, status);
-- @@ v7
CREATE TABLE tasks_v7 (
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
CREATE INDEX inspections_key ON inspections(idea_id, question, view, target);
