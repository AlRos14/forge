-- TaskRole/RoleMembership is the sole assignment authority. Archive exact
-- residual Task-level values, then rebuild Task without its singleton fields.
-- Keep this rebuild atomic: a crash before COMMIT restores the original table,
-- indexes, triggers, and migration receipt; a restart after COMMIT sees V128.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_task_assignment_expected (legacy_rows INTEGER NOT NULL);
INSERT INTO _pr13_task_assignment_expected
SELECT COUNT(*) FROM task
WHERE assignee_type IS NOT NULL OR assignee_id IS NOT NULL;

CREATE TABLE pr13_task_assignment_projection_history (
    task_id         TEXT PRIMARY KEY,
    assignee_type   TEXT,
    assignee_id     TEXT,
    task_version    INTEGER NOT NULL,
    task_created_at TEXT NOT NULL,
    task_updated_at TEXT NOT NULL,
    captured_at     TEXT NOT NULL
);

INSERT INTO pr13_task_assignment_projection_history (
    task_id, assignee_type, assignee_id, task_version,
    task_created_at, task_updated_at, captured_at
)
SELECT id, assignee_type, assignee_id, version, created_at, updated_at,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task
WHERE assignee_type IS NOT NULL OR assignee_id IS NOT NULL;

CREATE TRIGGER pr13_task_assignment_history_no_insert
BEFORE INSERT ON pr13_task_assignment_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 task assignment provenance is migration-only');
END;
CREATE TRIGGER pr13_task_assignment_history_no_update
BEFORE UPDATE ON pr13_task_assignment_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 task assignment provenance is immutable');
END;
CREATE TRIGGER pr13_task_assignment_history_no_delete
BEFORE DELETE ON pr13_task_assignment_projection_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 task assignment provenance is immutable');
END;

DROP TRIGGER task_insert_requires_assignee_id;
DROP INDEX idx_task_assignee;

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
    status            TEXT NOT NULL DEFAULT 'todo',
    is_automation     INTEGER NOT NULL DEFAULT 0,
    priority          INTEGER NOT NULL DEFAULT 0,
    board_position    REAL NOT NULL DEFAULT 0.0,
    subtask_order     INTEGER,
    task_state_config TEXT DEFAULT '{}',
    merge_config      TEXT,
    metadata_json     TEXT,
    error_annotation  TEXT,
    blocked_json      TEXT,
    failed_json       TEXT,
    entry_barrier_json TEXT,
    review_passed_at  TEXT,
    archived_at       TEXT,
    deleted_at        TEXT,
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

INSERT INTO task (
    id, project_id, repo_id, parent_task_id, title, description, task_type,
    status, is_automation, priority, board_position, subtask_order,
    task_state_config, merge_config, metadata_json, error_annotation,
    blocked_json, failed_json, entry_barrier_json, review_passed_at,
    archived_at, deleted_at, version, created_at, updated_at
)
SELECT id, project_id, repo_id, parent_task_id, title, description, task_type,
       status, is_automation, priority, board_position, subtask_order,
       task_state_config, merge_config, metadata_json, error_annotation,
       blocked_json, failed_json, entry_barrier_json, review_passed_at,
       archived_at, deleted_at, version, created_at, updated_at
FROM task_pr13_old;

DROP TABLE task_pr13_old;

CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_archived ON task(project_id, archived_at);
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, archived_at, deleted_at);
CREATE INDEX idx_task_repo ON task(repo_id);
CREATE INDEX idx_task_status_project ON task(status, project_id);

CREATE TRIGGER pr6_task_block_state_event
AFTER UPDATE OF blocked_json, failed_json ON task
WHEN NEW.deleted_at IS NULL
 AND OLD.status IS NEW.status
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
AFTER UPDATE OF status, board_position, deleted_at, archived_at ON task
WHEN OLD.status IS NOT NEW.status
    OR OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
    OR OLD.archived_at IS NOT NEW.archived_at
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;

CREATE TRIGGER task_lifecycle_insert_for_new_task
AFTER INSERT ON task
BEGIN
    INSERT INTO task_lifecycle (
        task_id, state, version, reason_kind, reason_ref, created_at, updated_at
    ) VALUES (
        NEW.id,
        CASE NEW.status
            WHEN 'backlog' THEN 'backlog'
            WHEN 'todo' THEN 'ready'
            WHEN 'ready' THEN 'ready'
            WHEN 'planning' THEN 'active'
            WHEN 'in_progress' THEN 'active'
            WHEN 'working' THEN 'active'
            WHEN 'blocked' THEN 'blocked'
            WHEN 'done' THEN 'done'
            WHEN 'cancelled' THEN 'cancelled'
            ELSE 'blocked'
        END,
        1,
        CASE
            WHEN NEW.status = 'review' THEN 'ambiguous_review'
            WHEN NEW.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'blocked', 'done', 'cancelled')
                THEN 'unknown_legacy_state'
            ELSE NULL
        END,
        CASE
            WHEN NEW.status = 'review'
              OR NEW.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'blocked', 'done', 'cancelled')
                THEN NEW.id
            ELSE NULL
        END,
        NEW.created_at,
        NEW.updated_at
    );
    UPDATE task SET status = CASE (
        SELECT state FROM task_lifecycle WHERE task_id = NEW.id
    )
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER task_lifecycle_status_projection_guard
BEFORE UPDATE OF status ON task
WHEN EXISTS (SELECT 1 FROM task_lifecycle WHERE task_id = OLD.id)
 AND NEW.status IS NOT CASE (
        SELECT state FROM task_lifecycle WHERE task_id = OLD.id
    )
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END
BEGIN
    SELECT RAISE(ABORT, 'task.status is a projection of task_lifecycle');
END;

CREATE TEMP TABLE _pr13_task_assignment_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_task_assignment_check
SELECT (SELECT COUNT(*) FROM pr13_task_assignment_projection_history)
    = (SELECT legacy_rows FROM _pr13_task_assignment_expected);
INSERT INTO _pr13_task_assignment_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('task')
    WHERE name IN ('assignee_type', 'assignee_id')
);
INSERT INTO _pr13_task_assignment_check
SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _pr13_task_assignment_check
SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';
DROP TABLE _pr13_task_assignment_check;
DROP TABLE _pr13_task_assignment_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (128, 'retire_task_singleton_assignee_projection', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
