-- schema::MIGRATION_V8 of crates/arboria-core/src/schema.rs, verbatim (regenerate: ARBORIA_REGEN_FIXTURES=1 cargo test -p arboria-core --test schema)
CREATE TABLE ideas_v8 (
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
CREATE INDEX probe_results_latest ON probe_results(machine, probe, at);
