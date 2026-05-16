PRAGMA foreign_keys = OFF;

INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
SELECT
    'migrated-reviewer-' || t.reviewer_type || '-' || t.id,
    t.id,
    'reviewer',
    t.reviewer_type,
    t.reviewer_id,
    t.updated_at,
    t.updated_at
FROM task t
WHERE t.reviewer_type IN ('agent', 'user')
  AND t.reviewer_id IS NOT NULL
  AND NOT EXISTS (
    SELECT 1 FROM task_role_assignment tra
    WHERE tra.task_id = t.id AND tra.role_name = 'reviewer'
  )
ON CONFLICT DO NOTHING;

INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
SELECT
    'migrated-reviewer-user-' || t.id,
    t.id,
    'reviewer',
    'user',
    t.reviewer_id,
    t.updated_at,
    t.updated_at
FROM task t
WHERE t.reviewer_type = 'user'
  AND t.reviewer_id IS NOT NULL
  AND NOT EXISTS (
    SELECT 1 FROM task_role_assignment tra
    WHERE tra.task_id = t.id AND tra.role_name = 'reviewer'
  )
ON CONFLICT DO NOTHING;

CREATE TABLE task_new (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
    parent_task_id      TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type       TEXT CHECK (assignee_type IN ('agent', 'user')),
    assignee_id         TEXT,
    title               TEXT NOT NULL,
    description         TEXT,
    status              TEXT NOT NULL DEFAULT 'todo',
    priority            INTEGER NOT NULL DEFAULT 0,
    merge_config        TEXT,
    metadata_json       TEXT,
    plan                TEXT,
    task_state_config   TEXT DEFAULT '{}',
    error_annotation    TEXT,
    review_passed_at    TEXT NULL,
    deleted_at          TEXT,
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    subtask_order       INTEGER,
    board_position      REAL NOT NULL DEFAULT 0.0,
    blocked_json        TEXT NULL,
    failed_json         TEXT NULL,
    entry_barrier_json  TEXT NULL,
    archived_at         TEXT,
    CHECK (
        (assignee_type IS NULL AND assignee_id IS NULL) OR
        (assignee_type = 'agent') OR
        (assignee_type = 'user' AND assignee_id IS NOT NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id,
    assignee_type, assignee_id,
    title, description, status, priority, merge_config, metadata_json,
    plan, task_state_config, error_annotation, review_passed_at,
    deleted_at, version, created_at, updated_at, subtask_order, board_position,
    blocked_json, failed_json, entry_barrier_json, archived_at
)
SELECT
    id, project_id, repo_id, parent_task_id,
    assignee_type, assignee_id,
    title, description, status, priority, merge_config, metadata_json,
    plan, task_state_config, error_annotation, review_passed_at,
    deleted_at, version, created_at, updated_at, subtask_order, board_position,
    blocked_json, failed_json, entry_barrier_json, archived_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE TRIGGER task_insert_requires_assignee_id
BEFORE INSERT ON task
WHEN NEW.assignee_type IS NOT NULL AND NEW.assignee_id IS NULL AND NEW.assignee_type != 'agent'
BEGIN
    SELECT RAISE(ABORT, 'task.assignee_id required when assignee_type is set');
END;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);
CREATE INDEX idx_task_assignee ON task(assignee_type, assignee_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_archived ON task(project_id, archived_at);

PRAGMA foreign_keys = ON;
