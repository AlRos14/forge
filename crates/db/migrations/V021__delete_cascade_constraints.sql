PRAGMA foreign_keys = OFF;

CREATE TABLE task_new (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id             TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
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
    subtask_order       INTEGER,
    board_position      REAL NOT NULL DEFAULT 0.0,
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
    assignee_type, assignee_id, reviewer_type, reviewer_id,
    title, description, status, priority, merge_config, metadata_json,
    plan, task_state_config, error_annotation, review_passed_at,
    deleted_at, version, created_at, updated_at, subtask_order, board_position
)
SELECT
    id, project_id, repo_id, parent_task_id,
    assignee_type, assignee_id, reviewer_type, reviewer_id,
    title, description, status, priority, merge_config, metadata_json,
    plan, task_state_config, error_annotation, review_passed_at,
    deleted_at, version, created_at, updated_at, subtask_order, board_position
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
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);

CREATE TABLE workspace_new (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL UNIQUE REFERENCES task(id) ON DELETE CASCADE,
    repo_id         TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
    worktree_path   TEXT NOT NULL,
    branch          TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('creating', 'ready', 'error', 'cleaning', 'cleaned')),
    before_sha      TEXT,
    error           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    cleanup_after   TEXT
);

INSERT INTO workspace_new (
    id, task_id, repo_id, worktree_path, branch, status,
    before_sha, error, created_at, updated_at, cleanup_after
)
SELECT
    id, task_id, repo_id, worktree_path, branch, status,
    before_sha, error, created_at, updated_at, cleanup_after
FROM workspace;

DROP TABLE workspace;
ALTER TABLE workspace_new RENAME TO workspace;

CREATE INDEX idx_workspace_task ON workspace(task_id);

CREATE TABLE execution_new (
    id                              TEXT PRIMARY KEY,
    task_id                         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    agent_id                        TEXT REFERENCES agent(id) ON DELETE SET NULL,
    role                            TEXT NOT NULL DEFAULT 'executor',
    status                          TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    parent_execution_id             TEXT REFERENCES execution(id),
    agent_session_id                TEXT,
    agent_message_id                TEXT,
    summary                         TEXT,
    logs_path                       TEXT,
    before_sha                      TEXT,
    after_sha                       TEXT,
    error                           TEXT,
    executor_config_snapshot_json   TEXT,
    workspace_id                    TEXT REFERENCES workspace(id) ON DELETE SET NULL,
    created_at                      TEXT NOT NULL,
    updated_at                      TEXT NOT NULL,
    stop_reason                     TEXT,
    stopped_by                      TEXT,
    resume_policy                   TEXT,
    stopped_at                      TEXT,
    prompt                          TEXT
);

INSERT INTO execution_new (
    id, task_id, agent_id, role, status, parent_execution_id,
    agent_session_id, agent_message_id, summary, logs_path, before_sha,
    after_sha, error, executor_config_snapshot_json, workspace_id,
    created_at, updated_at, stop_reason, stopped_by, resume_policy,
    stopped_at, prompt
)
SELECT
    id, task_id, agent_id, role, status, parent_execution_id,
    agent_session_id, agent_message_id, summary, logs_path, before_sha,
    after_sha, error, executor_config_snapshot_json, workspace_id,
    created_at, updated_at, stop_reason, stopped_by, resume_policy,
    stopped_at, prompt
FROM execution;

DROP TABLE execution;
ALTER TABLE execution_new RENAME TO execution;

CREATE INDEX idx_execution_task ON execution(task_id);
CREATE INDEX idx_execution_agent ON execution(agent_id);
CREATE INDEX idx_execution_session ON execution(agent_session_id);

CREATE TABLE transition_log_new (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    from_state          TEXT NOT NULL,
    to_state            TEXT NOT NULL,
    triggered_by        TEXT NOT NULL,
    trigger_reason      TEXT NOT NULL,
    hook_results_json   TEXT,
    rejection           INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL
);

INSERT INTO transition_log_new (
    id, task_id, from_state, to_state, triggered_by,
    trigger_reason, hook_results_json, rejection, created_at
)
SELECT
    id, task_id, from_state, to_state, triggered_by,
    trigger_reason, hook_results_json, rejection, created_at
FROM transition_log;

DROP TABLE transition_log;
ALTER TABLE transition_log_new RENAME TO transition_log;

CREATE INDEX idx_transition_log_task ON transition_log(task_id, created_at);
CREATE INDEX idx_transition_log_rejection ON transition_log(task_id, from_state, rejection);
CREATE INDEX idx_transition_log_merge_failed ON transition_log(task_id, to_state, created_at);

PRAGMA foreign_keys = ON;
