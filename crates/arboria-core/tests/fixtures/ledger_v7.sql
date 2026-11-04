PRAGMA user_version = 7;
BEGIN TRANSACTION;
CREATE TABLE "decisions" (
    id           TEXT PRIMARY KEY,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('run', 'idea', 'lesson')),
    subject_id   TEXT NOT NULL,
    question     TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    choice       TEXT,
    decided_by   TEXT,
    created_at   TEXT NOT NULL
, payload_json TEXT);
INSERT INTO "decisions" VALUES('d001','run','r001','q','["approve", "reject", "revise"]',NULL,NULL,'2026-11-01T10:00:00Z',NULL);
CREATE TABLE events (
    id           INTEGER PRIMARY KEY,
    ts           TEXT NOT NULL,
    kind         TEXT NOT NULL,
    subject      TEXT,
    payload_json TEXT
);
INSERT INTO "events" VALUES(1,'2026-11-01T10:00:00Z','idea_new','base','{}');
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
INSERT INTO "ideas" VALUES(1,'base','h','m',NULL,'guess','open','idea/base','/w/base','2026-11-01T10:00:00Z',NULL);
INSERT INTO "ideas" VALUES(2,'wide-lr','h2','m2',1,'prelim','dropped','idea/wide-lr','/w/wide-lr','2026-11-01T10:00:00Z','2026-11-01T10:00:00Z');
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
CREATE TABLE notes (
    id         INTEGER PRIMARY KEY,
    idea_id    INTEGER NOT NULL REFERENCES ideas(id),
    run_id     TEXT REFERENCES runs(id),
    text       TEXT NOT NULL,
    confidence TEXT CHECK (confidence IS NULL OR confidence IN ('guess', 'prelim', 'confirmed', 'rejected')),
    created_at TEXT NOT NULL
, author TEXT);
CREATE TABLE plantings (
    id          INTEGER PRIMARY KEY,
    seed_id     INTEGER NOT NULL REFERENCES seeds(id),
    repo        TEXT NOT NULL,
    project_id  TEXT,
    idea        TEXT NOT NULL,
    created_at  TEXT NOT NULL
);
INSERT INTO "plantings" VALUES(1,2,'/repos/x','x-1a2b','planted-one','2026-11-01T10:00:00Z');
CREATE TABLE "runs" (
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
, parent_run TEXT REFERENCES runs(id));
INSERT INTO "runs" VALUES('r001',1,'c1','t1',NULL,'first',NULL,NULL,NULL,NULL,NULL,NULL,'awaiting_approval',NULL,NULL,NULL,'2026-11-01T10:00:00Z','2026-11-01T10:00:00Z','{"ask": {"repeat": "n0nce"}}','h1',NULL);
INSERT INTO "runs" VALUES('r002',1,'c1','t1',NULL,'second',NULL,NULL,NULL,NULL,NULL,NULL,'draft',NULL,NULL,NULL,'2026-11-01T10:00:00Z','2026-11-01T10:00:00Z',NULL,NULL,NULL);
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
INSERT INTO "seeds" VALUES(1,'sparse','一句话',NULL,NULL,'thinking','2026-11-01T10:00:00Z','2026-11-01T10:00:00Z');
INSERT INTO "seeds" VALUES(2,'planted-one','g','h','m','planted','2026-11-01T10:00:00Z','2026-11-01T10:00:00Z');
INSERT INTO "seeds" VALUES(3,'gone',NULL,NULL,NULL,'dropped','2026-11-01T10:00:00Z','2026-11-02T00:00:00Z');
CREATE TABLE "tasks" (
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
INSERT INTO "tasks" VALUES('t001',1,NULL,'look','open','2026-11-01T10:00:00Z',NULL,NULL,'plain',NULL);
INSERT INTO "tasks" VALUES('t002',NULL,1,'think','done','2026-11-01T10:00:00Z','2026-11-01T10:00:00Z','ok','plain',NULL);
INSERT INTO "tasks" VALUES('t003',2,NULL,'gone','dropped','2026-11-01T10:00:00Z','2026-11-01T10:00:00Z',NULL,'inspect',NULL);
CREATE INDEX notes_idea ON notes(idea_id);
CREATE INDEX lessons_status ON lessons(status);
CREATE INDEX runs_tree_hash ON runs(tree_hash);
CREATE INDEX runs_idea ON runs(idea_id);
CREATE INDEX runs_status ON runs(status);
CREATE INDEX jobs_run ON jobs(run_id);
CREATE INDEX runs_card_hash ON runs(idea_id, card_hash);
CREATE INDEX plantings_seed ON plantings(seed_id);
CREATE INDEX tasks_idea ON tasks(idea_id, status);
CREATE INDEX tasks_seed ON tasks(seed_id, status);
CREATE INDEX inspections_key ON inspections(idea_id, question, view, target);
COMMIT;
