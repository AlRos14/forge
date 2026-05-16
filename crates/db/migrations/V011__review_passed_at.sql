ALTER TABLE task ADD COLUMN review_passed_at TEXT NULL;

CREATE INDEX idx_transition_log_merge_failed ON transition_log(task_id, to_state, created_at);

PRAGMA foreign_keys = OFF;

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
    workspace_id                    TEXT REFERENCES workspace(id),
    created_at                      TEXT NOT NULL,
    updated_at                      TEXT NOT NULL
);

INSERT INTO execution_new (
    id, task_id, agent_id, role, status, parent_execution_id, agent_session_id,
    agent_message_id, summary, logs_path, before_sha, after_sha, error,
    executor_config_snapshot_json, workspace_id, created_at, updated_at
)
SELECT
    id, task_id, agent_id, role, status, parent_execution_id, agent_session_id,
    agent_message_id, summary, logs_path, before_sha, after_sha, error,
    executor_config_snapshot_json, workspace_id, created_at, updated_at
FROM execution;

DROP TABLE execution;
ALTER TABLE execution_new RENAME TO execution;

CREATE INDEX idx_execution_task ON execution(task_id);
CREATE INDEX idx_execution_agent ON execution(agent_id);
CREATE INDEX idx_execution_session ON execution(agent_session_id);

PRAGMA foreign_keys = ON;
