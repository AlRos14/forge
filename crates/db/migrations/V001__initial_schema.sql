CREATE TABLE project (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    settings    TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE repo (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('local','remote')),
    local_path      TEXT,
    remote_url      TEXT,
    default_branch  TEXT NOT NULL DEFAULT 'main',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK (
        (kind = 'local'  AND local_path IS NOT NULL AND remote_url IS NULL) OR
        (kind = 'remote' AND remote_url IS NOT NULL AND local_path IS NULL)
    )
);
CREATE INDEX idx_repo_project ON repo(project_id);

CREATE TABLE agent (
    id                          TEXT PRIMARY KEY,
    name                        TEXT NOT NULL,
    executor_type               TEXT NOT NULL,
    prompt_template             TEXT,
    capabilities                TEXT NOT NULL DEFAULT '[]',
    max_concurrent_tasks        INTEGER NOT NULL DEFAULT 1,
    heartbeat_interval_seconds  INTEGER NOT NULL DEFAULT 30,
    max_missed_heartbeats       INTEGER NOT NULL DEFAULT 3,
    status                      TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle', 'busy', 'error', 'offline')),
    last_heartbeat_at           TEXT,
    version                     INTEGER NOT NULL DEFAULT 1,
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL
);

CREATE TABLE skill (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_skill_project ON skill(project_id);

CREATE TABLE agent_skill (
    agent_id    TEXT NOT NULL REFERENCES agent(id) ON DELETE CASCADE,
    skill_id    TEXT NOT NULL REFERENCES skill(id) ON DELETE CASCADE,
    PRIMARY KEY (agent_id, skill_id)
);

CREATE TABLE task (
    id                      TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id                 TEXT NOT NULL REFERENCES repo(id),
    parent_task_id          TEXT REFERENCES task(id) ON DELETE SET NULL,
    agent_id                TEXT REFERENCES agent(id) ON DELETE SET NULL,
    title                   TEXT NOT NULL,
    description             TEXT,
    type                    TEXT NOT NULL DEFAULT 'task' CHECK (type IN ('task', 'planning_task', 'sub_task')),
    status                  TEXT NOT NULL DEFAULT 'todo' CHECK (status IN ('todo', 'in_progress', 'review', 'merging', 'merge_failed', 'done', 'cancelled', 'blocked')),
    priority                INTEGER NOT NULL DEFAULT 0,
    review_config           TEXT,
    merge_config            TEXT,
    plan                    TEXT,
    review_attempt_count    INTEGER NOT NULL DEFAULT 0,
    fix_attempt_count       INTEGER NOT NULL DEFAULT 0,
    error_annotation        TEXT,
    deleted_at              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_agent ON task(agent_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);

CREATE TABLE execution (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    agent_id                TEXT REFERENCES agent(id) ON DELETE SET NULL,
    role                    TEXT NOT NULL DEFAULT 'executor' CHECK (role IN ('executor', 'reviewer', 'auditor', 'merge_fixer', 'interactive')),
    status                  TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    parent_execution_id     TEXT REFERENCES execution(id),
    agent_session_id        TEXT,
    agent_message_id        TEXT,
    summary                 TEXT,
    logs_path               TEXT,
    before_sha              TEXT,
    after_sha               TEXT,
    error                   TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);
CREATE INDEX idx_execution_task ON execution(task_id);
CREATE INDEX idx_execution_agent ON execution(agent_id);
CREATE INDEX idx_execution_session ON execution(agent_session_id);

CREATE TABLE review (
    id              TEXT PRIMARY KEY,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    execution_id    TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    attempt_number  INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'passed', 'failed', 'cancelled')),
    ci_results      TEXT,
    audit_result    TEXT,
    human_decision  TEXT CHECK (human_decision IN ('approved', 'rejected') OR human_decision IS NULL),
    feedback        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX idx_review_task ON review(task_id);
CREATE INDEX idx_review_execution ON review(execution_id);

CREATE TABLE IF NOT EXISTS _migration (
    version     INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    applied_at  TEXT NOT NULL
);
