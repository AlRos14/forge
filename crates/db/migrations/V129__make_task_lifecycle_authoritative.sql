-- TaskLifecycle replaces the one-way Task.status projection and its workflow
-- configuration. Preserve exact compatibility-column values before rebuilding
-- Task; lifecycle_migration_audit retains V100's original provenance.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_task_legacy_expected (rows INTEGER NOT NULL);
INSERT INTO _pr13_task_legacy_expected SELECT COUNT(*) FROM task;

CREATE TEMP TABLE _pr13_task_cutover_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_task_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM task t
    LEFT JOIN task_lifecycle l ON l.task_id = t.id
    WHERE l.task_id IS NULL
);

DROP TRIGGER pr13_legacy_record_history_no_insert;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task_legacy_fields', id,
       json_object('task_id', id,
                   'status_projection', status,
                   'task_state_config', task_state_config,
                   'entry_barrier_json', entry_barrier_json,
                   'review_passed_at', review_passed_at,
                   'archived_at', archived_at,
                   'task_version', version,
                   'created_at', created_at,
                   'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task;
CREATE TRIGGER pr13_legacy_record_history_no_insert
BEFORE INSERT ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is migration-only');
END;

DROP TRIGGER task_lifecycle_insert_for_new_task;
DROP TRIGGER task_lifecycle_status_projection_guard;
DROP TRIGGER pr6_task_block_state_event;
DROP TRIGGER pr6_task_created_event;
DROP TRIGGER task_board_revision_after_delete;
DROP TRIGGER task_board_revision_after_insert;
DROP TRIGGER task_board_revision_after_update;
DROP INDEX idx_task_project_archived;
DROP INDEX idx_task_project_automation;
DROP INDEX idx_task_status_project;

ALTER TABLE task RENAME TO task_pr13_old;
CREATE TABLE task (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id           TEXT REFERENCES repo(id) ON DELETE CASCADE,
    parent_task_id    TEXT REFERENCES task(id) ON DELETE SET NULL,
    title             TEXT NOT NULL,
    description       TEXT,
    task_type         TEXT NOT NULL DEFAULT 'implementation'
                          CHECK (task_type IN ('implementation', 'planning', 'discovery', 'review', 'validation')),
    is_automation     INTEGER NOT NULL DEFAULT 0,
    priority          INTEGER NOT NULL DEFAULT 0,
    board_position    REAL NOT NULL DEFAULT 0.0,
    subtask_order     INTEGER,
    merge_config      TEXT,
    metadata_json     TEXT,
    error_annotation  TEXT,
    blocked_json      TEXT,
    failed_json       TEXT,
    deleted_at        TEXT,
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

INSERT INTO task (
    id, project_id, repo_id, parent_task_id, title, description, task_type,
    is_automation, priority, board_position, subtask_order, merge_config,
    metadata_json, error_annotation, blocked_json, failed_json, deleted_at,
    version, created_at, updated_at
)
SELECT id, project_id, repo_id, parent_task_id, title, description, task_type,
       is_automation, priority, board_position, subtask_order, merge_config,
       metadata_json, error_annotation, blocked_json, failed_json, deleted_at,
       version, created_at, updated_at
FROM task_pr13_old;
DROP TABLE task_pr13_old;

CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, deleted_at);
CREATE INDEX idx_task_repo ON task(repo_id);

CREATE TRIGGER pr6_task_block_state_event
AFTER UPDATE OF blocked_json, failed_json ON task
WHEN NEW.deleted_at IS NULL
 AND (OLD.blocked_json IS NOT NEW.blocked_json
      OR OLD.failed_json IS NOT NEW.failed_json)
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    ) VALUES (
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        CASE WHEN NEW.failed_json IS NOT NULL THEN 'task.failed'
             WHEN NEW.blocked_json IS NOT NULL THEN 'task.blocked'
             ELSE 'task.unblocked' END,
        'task', NEW.id, 'system', NULL, 'task', NEW.id,
        NEW.id, NULL, 0, 'pr6-task-block-state:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_version', NEW.version,
            'blocked', NEW.blocked_json IS NOT NULL,
            'failed', NEW.failed_json IS NOT NULL
        ),
        NEW.updated_at
    );
END;

CREATE TRIGGER pr6_task_created_event
AFTER INSERT ON task
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    ) VALUES (
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'task.created', 'task', NEW.id, 'system', NULL,
        'task', NEW.id, NEW.id, NULL, 0,
        'pr6-task-created:' || NEW.id,
        json_object('task_version', NEW.version),
        NEW.created_at
    );
END;

CREATE TRIGGER task_board_revision_after_delete
AFTER DELETE ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = OLD.project_id;
END;
CREATE TRIGGER task_board_revision_after_insert
AFTER INSERT ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;
CREATE TRIGGER task_board_revision_after_update
AFTER UPDATE OF board_position, deleted_at ON task
WHEN OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;
CREATE TRIGGER task_lifecycle_board_revision_after_update
AFTER UPDATE OF state ON task_lifecycle
WHEN OLD.state IS NOT NEW.state
BEGIN
    UPDATE project SET board_revision = board_revision + 1
    WHERE id = (SELECT project_id FROM task WHERE id = NEW.task_id);
END;

INSERT INTO _pr13_task_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history
        WHERE source_table = 'task_legacy_fields')
       = (SELECT rows FROM _pr13_task_legacy_expected);
INSERT INTO _pr13_task_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('task')
    WHERE name IN ('status', 'task_state_config', 'entry_barrier_json',
                   'review_passed_at', 'archived_at', 'assignee_type', 'assignee_id', 'plan')
);
INSERT INTO _pr13_task_cutover_check
SELECT (SELECT COUNT(*) FROM task) = (SELECT COUNT(*) FROM task_lifecycle);
INSERT INTO _pr13_task_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master
    WHERE type = 'trigger' AND name IN (
        'task_lifecycle_insert_for_new_task',
        'task_lifecycle_status_projection_guard'
    )
);
INSERT INTO _pr13_task_cutover_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _pr13_task_cutover_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';
DROP TABLE _pr13_task_cutover_check;
DROP TABLE _pr13_task_legacy_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (129, 'make_task_lifecycle_authoritative', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
