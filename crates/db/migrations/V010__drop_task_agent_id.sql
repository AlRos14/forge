PRAGMA foreign_keys = OFF;

CREATE TABLE task_new (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id           TEXT NOT NULL REFERENCES repo(id),
    parent_task_id    TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type     TEXT CHECK (assignee_type IN ('agent', 'user')),
    user_handle       TEXT,
    title             TEXT NOT NULL,
    description       TEXT,
    status            TEXT NOT NULL DEFAULT 'todo',
    priority          INTEGER NOT NULL DEFAULT 0,
    merge_config      TEXT,
    metadata_json     TEXT,
    plan              TEXT,
    task_state_config TEXT DEFAULT '{}',
    error_annotation  TEXT,
    deleted_at        TEXT,
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    CHECK (
        (assignee_type IS NULL AND user_handle IS NULL) OR
        (assignee_type = 'user' AND user_handle IS NOT NULL) OR
        (assignee_type = 'agent' AND user_handle IS NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id, assignee_type, user_handle,
    title, description, status, priority, merge_config, metadata_json, plan,
    task_state_config, error_annotation, deleted_at, version, created_at, updated_at
)
SELECT
    id, project_id, repo_id, parent_task_id, assignee_type, user_handle,
    title, description, status, priority, merge_config, metadata_json, plan,
    task_state_config, error_annotation, deleted_at, version, created_at, updated_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);

PRAGMA foreign_keys = ON;
