-- The exact HarnessSession is the only current authority for external session
-- continuity. Preserve every old scalar projection verbatim as provenance,
-- including values whose Agent/session relationship cannot be proven.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_session_expected (legacy_rows INTEGER NOT NULL);
INSERT INTO _pr13_session_expected
SELECT COUNT(*) FROM execution WHERE agent_session_id IS NOT NULL;

CREATE TABLE pr13_execution_session_projection_history (
    execution_id                  TEXT PRIMARY KEY,
    task_id                       TEXT NOT NULL,
    agent_id                      TEXT,
    actor_kind                    TEXT,
    actor_id                      TEXT,
    harness_session_id            TEXT,
    workspace_id                  TEXT,
    external_session_id           TEXT NOT NULL,
    executor_config_snapshot_json TEXT,
    execution_created_at          TEXT NOT NULL,
    execution_updated_at          TEXT NOT NULL,
    captured_at                   TEXT NOT NULL
);

INSERT INTO pr13_execution_session_projection_history (
    execution_id, task_id, agent_id, actor_kind, actor_id,
    harness_session_id, workspace_id, external_session_id,
    executor_config_snapshot_json, execution_created_at,
    execution_updated_at, captured_at
)
SELECT id, task_id, agent_id, actor_kind, actor_id,
       harness_session_id, workspace_id, agent_session_id,
       executor_config_snapshot_json, created_at, updated_at,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM execution
WHERE agent_session_id IS NOT NULL;

CREATE TRIGGER pr13_execution_session_history_no_update
BEFORE UPDATE ON pr13_execution_session_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution session provenance is immutable');
END;
CREATE TRIGGER pr13_execution_session_history_no_insert
BEFORE INSERT ON pr13_execution_session_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution session provenance is migration-only');
END;
CREATE TRIGGER pr13_execution_session_history_no_delete
BEFORE DELETE ON pr13_execution_session_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution session provenance is immutable');
END;

DROP TRIGGER execution_harness_projection_guard_insert;
DROP TRIGGER execution_harness_projection_guard_update;
DROP TRIGGER harness_session_projection_update;
DROP INDEX idx_execution_session;
ALTER TABLE execution DROP COLUMN agent_session_id;

CREATE TEMP TABLE _pr13_session_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_session_check
SELECT (SELECT COUNT(*) FROM pr13_execution_session_projection_history)
    = (SELECT legacy_rows FROM _pr13_session_expected);
INSERT INTO _pr13_session_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('execution') WHERE name = 'agent_session_id'
);
INSERT INTO _pr13_session_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE _pr13_session_check;
DROP TABLE _pr13_session_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (124, 'remove_execution_session_projection', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
