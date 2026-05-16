PRAGMA foreign_keys = OFF;

ALTER TABLE task ADD COLUMN reviewer_type TEXT CHECK (reviewer_type IN ('agent', 'user'));
ALTER TABLE task ADD COLUMN reviewer_agent_id TEXT REFERENCES agent(id) ON DELETE SET NULL;
ALTER TABLE task ADD COLUMN reviewer_user_handle TEXT;

CREATE TABLE task_new (
    id                      TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id                 TEXT NOT NULL REFERENCES repo(id),
    parent_task_id          TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type           TEXT CHECK (assignee_type IN ('agent', 'user')),
    agent_id                TEXT REFERENCES agent(id) ON DELETE SET NULL,
    user_handle             TEXT,
    reviewer_type           TEXT CHECK (reviewer_type IN ('agent', 'user')),
    reviewer_agent_id       TEXT REFERENCES agent(id) ON DELETE SET NULL,
    reviewer_user_handle    TEXT,
    title                   TEXT NOT NULL,
    description             TEXT,
    type                    TEXT NOT NULL DEFAULT 'task' CHECK (type IN ('task', 'planning_task', 'sub_task')),
    status                  TEXT NOT NULL DEFAULT 'todo' CHECK (status IN ('todo', 'in_progress', 'review', 'merging', 'merge_failed', 'done', 'cancelled', 'blocked')),
    priority                INTEGER NOT NULL DEFAULT 0,
    review_config           TEXT,
    merge_config            TEXT,
    metadata_json           TEXT,
    plan                    TEXT,
    review_attempt_count    INTEGER NOT NULL DEFAULT 0,
    fix_attempt_count       INTEGER NOT NULL DEFAULT 0,
    error_annotation        TEXT,
    deleted_at              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    CHECK (
        (assignee_type IS NULL AND agent_id IS NULL AND user_handle IS NULL) OR
        (assignee_type = 'agent' AND agent_id IS NOT NULL AND user_handle IS NULL) OR
        (assignee_type = 'user' AND user_handle IS NOT NULL AND agent_id IS NULL)
    ),
    CHECK (
        (reviewer_type IS NULL AND reviewer_agent_id IS NULL AND reviewer_user_handle IS NULL) OR
        (reviewer_type = 'agent' AND reviewer_agent_id IS NOT NULL AND reviewer_user_handle IS NULL) OR
        (reviewer_type = 'user' AND reviewer_user_handle IS NOT NULL AND reviewer_agent_id IS NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id, assignee_type, agent_id, user_handle,
    reviewer_type, reviewer_agent_id, reviewer_user_handle,
    title, description, type, status, priority, review_config, merge_config,
    metadata_json, plan, review_attempt_count, fix_attempt_count, error_annotation,
    deleted_at, version, created_at, updated_at
)
SELECT
    id, project_id, repo_id, parent_task_id, assignee_type, agent_id, user_handle,
    reviewer_type, reviewer_agent_id, reviewer_user_handle,
    title, description, type, status, priority, review_config, merge_config,
    metadata_json, plan, review_attempt_count, fix_attempt_count, error_annotation,
    deleted_at, version, created_at, updated_at
FROM task;

DROP TABLE task;
ALTER TABLE task_new RENAME TO task;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_agent ON task(agent_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);

CREATE TABLE review_new (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    attempt_number      INTEGER NOT NULL,
    status              TEXT NOT NULL CHECK (status IN ('running', 'awaiting_human', 'passed', 'failed', 'cancelled')),
    step_results_json   TEXT NOT NULL DEFAULT '[]',
    started_at          TEXT NOT NULL,
    finished_at         TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE(task_id, attempt_number)
);

INSERT INTO review_new (
    id, task_id, execution_id, attempt_number, status, step_results_json,
    started_at, finished_at, created_at, updated_at
)
SELECT
    id, task_id, execution_id, attempt_number, status, step_results_json,
    started_at, finished_at, created_at, updated_at
FROM review;

DROP TABLE review;
ALTER TABLE review_new RENAME TO review;
CREATE INDEX idx_review_task_attempt ON review(task_id, attempt_number);

CREATE TABLE task_comment (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    author_type TEXT NOT NULL CHECK (author_type IN ('user', 'agent', 'system')),
    author_id   TEXT,
    author_name TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX idx_task_comment_task_id ON task_comment(task_id, created_at);

PRAGMA foreign_keys = ON;
