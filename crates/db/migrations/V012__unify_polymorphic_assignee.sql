PRAGMA foreign_keys = OFF;

CREATE TABLE task_role_assignment_new (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    role_name       TEXT NOT NULL,
    assignee_type   TEXT CHECK (assignee_type IN ('agent', 'user')),
    assignee_id     TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(task_id, role_name),
    CHECK (
        (assignee_type IS NULL AND assignee_id IS NULL) OR
        (assignee_type = 'agent') OR
        (assignee_type = 'user' AND assignee_id IS NOT NULL)
    )
);

INSERT INTO task_role_assignment_new (
    id,
    task_id,
    role_name,
    assignee_type,
    assignee_id,
    created_at,
    updated_at
)
SELECT
    id,
    task_id,
    role_name,
    assignment_type,
    COALESCE(agent_id, user_handle),
    created_at,
    updated_at
FROM task_role_assignment;

DROP TABLE task_role_assignment;
ALTER TABLE task_role_assignment_new RENAME TO task_role_assignment;

CREATE INDEX idx_task_role_task ON task_role_assignment(task_id);
CREATE INDEX idx_task_role_assignee ON task_role_assignment(assignee_type, assignee_id);

CREATE TRIGGER task_role_assignment_insert_requires_assignee_id
BEFORE INSERT ON task_role_assignment
WHEN NEW.assignee_type IS NOT NULL AND NEW.assignee_id IS NULL
BEGIN
    SELECT RAISE(ABORT, 'task_role_assignment assignee_id is required on insert');
END;

CREATE TABLE task_new (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id             TEXT NOT NULL REFERENCES repo(id),
    parent_task_id      TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type       TEXT CHECK (assignee_type IN ('agent', 'user')),
    assignee_id         TEXT,
    reviewer_type       TEXT CHECK (reviewer_type IN ('agent', 'user')),
    reviewer_id         TEXT,
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
    CHECK (
        (assignee_type IS NULL AND assignee_id IS NULL) OR
        (assignee_type = 'agent') OR
        (assignee_type = 'user' AND assignee_id IS NOT NULL)
    ),
    CHECK (
        (reviewer_type IS NULL AND reviewer_id IS NULL) OR
        (reviewer_type = 'agent') OR
        (reviewer_type = 'user' AND reviewer_id IS NOT NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id,
    assignee_type, assignee_id,
    reviewer_type, reviewer_id,
    title, description, status, priority,
    merge_config, metadata_json, plan, task_state_config, error_annotation,
    review_passed_at, deleted_at, version, created_at, updated_at
)
SELECT
    id, project_id, repo_id, parent_task_id,
    assignee_type, user_handle,
    NULL, NULL,
    title, description, status, priority,
    merge_config, metadata_json, plan, task_state_config, error_annotation,
    review_passed_at, deleted_at, version, created_at, updated_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE TRIGGER task_insert_requires_assignee_id
BEFORE INSERT ON task
WHEN NEW.assignee_type IS NOT NULL AND NEW.assignee_id IS NULL AND NEW.assignee_type != 'agent'
BEGIN
    SELECT RAISE(ABORT, 'task.assignee_id required when assignee_type is set');
END;

CREATE TRIGGER task_insert_requires_reviewer_id
BEFORE INSERT ON task
WHEN NEW.reviewer_type IS NOT NULL AND NEW.reviewer_id IS NULL AND NEW.reviewer_type != 'agent'
BEGIN
    SELECT RAISE(ABORT, 'task.reviewer_id required when reviewer_type is set');
END;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);
CREATE INDEX idx_task_assignee ON task(assignee_type, assignee_id);
CREATE INDEX idx_task_reviewer ON task(reviewer_type, reviewer_id);

PRAGMA foreign_keys = ON;
