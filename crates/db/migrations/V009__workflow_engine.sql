ALTER TABLE project ADD COLUMN workflow_definition TEXT NOT NULL DEFAULT '{}';
ALTER TABLE task ADD COLUMN task_state_config TEXT DEFAULT '{}';

CREATE TABLE task_role_assignment (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    role_name TEXT NOT NULL,
    assignment_type TEXT NOT NULL CHECK (assignment_type IN ('agent', 'user')),
    agent_id TEXT REFERENCES agent(id) ON DELETE SET NULL,
    user_handle TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(task_id, role_name)
);

CREATE INDEX idx_task_role_task ON task_role_assignment(task_id);
CREATE INDEX idx_task_role_agent ON task_role_assignment(agent_id);

CREATE TABLE transition_log (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id),
    from_state TEXT NOT NULL,
    to_state TEXT NOT NULL,
    triggered_by TEXT NOT NULL,
    trigger_reason TEXT NOT NULL,
    hook_results_json TEXT,
    rejection INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_transition_log_task ON transition_log(task_id, created_at);
CREATE INDEX idx_transition_log_rejection ON transition_log(task_id, from_state, rejection);

INSERT INTO task_role_assignment (
    id,
    task_id,
    role_name,
    assignment_type,
    agent_id,
    user_handle,
    created_at,
    updated_at
)
SELECT
    lower(hex(randomblob(16))),
    id,
    'coder',
    'agent',
    agent_id,
    NULL,
    datetime('now'),
    datetime('now')
FROM task
WHERE agent_id IS NOT NULL;

INSERT INTO task_role_assignment (
    id,
    task_id,
    role_name,
    assignment_type,
    agent_id,
    user_handle,
    created_at,
    updated_at
)
SELECT
    lower(hex(randomblob(16))),
    id,
    'reviewer',
    'agent',
    reviewer_agent_id,
    NULL,
    datetime('now'),
    datetime('now')
FROM task
WHERE reviewer_agent_id IS NOT NULL;

INSERT INTO task_role_assignment (
    id,
    task_id,
    role_name,
    assignment_type,
    agent_id,
    user_handle,
    created_at,
    updated_at
)
SELECT
    lower(hex(randomblob(16))),
    id,
    'reviewer',
    'user',
    NULL,
    reviewer_user_handle,
    datetime('now'),
    datetime('now')
FROM task
WHERE reviewer_user_handle IS NOT NULL;

UPDATE task
SET task_state_config = json_object('review', json(review_config))
WHERE review_config IS NOT NULL
  AND review_config != ''
  AND (task_state_config IN ('{}', '') OR task_state_config IS NULL);

PRAGMA foreign_keys = OFF;

CREATE TABLE task_new (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id         TEXT NOT NULL REFERENCES repo(id),
    parent_task_id  TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type   TEXT CHECK (assignee_type IN ('agent', 'user')),
    agent_id        TEXT REFERENCES agent(id) ON DELETE SET NULL,
    user_handle     TEXT,
    title           TEXT NOT NULL,
    description     TEXT,
    status          TEXT NOT NULL DEFAULT 'todo',
    priority        INTEGER NOT NULL DEFAULT 0,
    merge_config    TEXT,
    metadata_json   TEXT,
    plan            TEXT,
    task_state_config TEXT DEFAULT '{}',
    error_annotation TEXT,
    deleted_at      TEXT,
    version         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK (
        (assignee_type IS NULL AND agent_id IS NULL AND user_handle IS NULL) OR
        (assignee_type = 'agent' AND agent_id IS NOT NULL AND user_handle IS NULL) OR
        (assignee_type = 'user' AND user_handle IS NOT NULL AND agent_id IS NULL)
    )
);

INSERT INTO task_new (id, project_id, repo_id, parent_task_id, assignee_type, agent_id, user_handle, title, description, status, priority, merge_config, metadata_json, plan, task_state_config, error_annotation, deleted_at, version, created_at, updated_at)
SELECT id, project_id, repo_id, parent_task_id, assignee_type, agent_id, user_handle, title, description, status, priority, merge_config, metadata_json, plan, task_state_config, error_annotation, deleted_at, version, created_at, updated_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_agent ON task(agent_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);

PRAGMA foreign_keys = ON;
