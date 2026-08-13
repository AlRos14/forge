-- Product Genesis admits a durable discovery as a normal task row.  The
-- historical task_type CHECK predates that workflow, so rebuild only the
-- task table while retaining every row, foreign-key relationship, index, and
-- trigger that existed before this migration.

PRAGMA foreign_keys = OFF;

CREATE TABLE task_new (
    id                      TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id                 TEXT REFERENCES repo(id) ON DELETE CASCADE,
    parent_task_id          TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type           TEXT CHECK (assignee_type IN ('agent', 'user')),
    assignee_id             TEXT,
    title                   TEXT NOT NULL,
    description             TEXT,
    task_type               TEXT NOT NULL DEFAULT 'task'
                                CHECK (task_type IN ('task', 'planning_task', 'sub_task', 'discovery')),
    status                  TEXT NOT NULL DEFAULT 'todo',
    is_automation           INTEGER NOT NULL DEFAULT 0,
    priority                INTEGER NOT NULL DEFAULT 0,
    board_position          REAL NOT NULL DEFAULT 0.0,
    subtask_order           INTEGER,
    task_state_config       TEXT DEFAULT '{}',
    merge_config            TEXT,
    metadata_json           TEXT,
    plan                    TEXT,
    error_annotation        TEXT,
    blocked_json            TEXT,
    failed_json             TEXT,
    entry_barrier_json      TEXT,
    review_passed_at        TEXT,
    archived_at             TEXT,
    deleted_at              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    CHECK (
        (assignee_type IS NULL AND assignee_id IS NULL) OR
        (assignee_type = 'agent') OR
        (assignee_type = 'user' AND assignee_id IS NOT NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id, assignee_type, assignee_id,
    title, description, task_type, status, is_automation, priority,
    board_position, subtask_order, task_state_config, merge_config,
    metadata_json, plan, error_annotation, blocked_json, failed_json,
    entry_barrier_json, review_passed_at, archived_at, deleted_at, version,
    created_at, updated_at
)
SELECT
    id, project_id, repo_id, parent_task_id, assignee_type, assignee_id,
    title, description, task_type, status, is_automation, priority,
    board_position, subtask_order, task_state_config, merge_config,
    metadata_json, plan, error_annotation, blocked_json, failed_json,
    entry_barrier_json, review_passed_at, archived_at, deleted_at, version,
    created_at, updated_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE TRIGGER task_insert_requires_assignee_id
BEFORE INSERT ON task
WHEN NEW.assignee_type IS NOT NULL
 AND NEW.assignee_id IS NULL
 AND NEW.assignee_type != 'agent'
BEGIN
    SELECT RAISE(ABORT, 'task.assignee_id required when assignee_type is set');
END;

CREATE TRIGGER task_board_revision_after_insert
AFTER INSERT ON task
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = NEW.project_id;
END;

CREATE TRIGGER task_board_revision_after_delete
AFTER DELETE ON task
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = OLD.project_id;
END;

CREATE TRIGGER task_board_revision_after_update
AFTER UPDATE OF status, board_position, deleted_at, archived_at ON task
WHEN OLD.status IS NOT NEW.status
    OR OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
    OR OLD.archived_at IS NOT NEW.archived_at
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = NEW.project_id;
END;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);
CREATE INDEX idx_task_assignee ON task(assignee_type, assignee_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_archived ON task(project_id, archived_at);
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, archived_at, deleted_at);

PRAGMA foreign_keys = ON;
