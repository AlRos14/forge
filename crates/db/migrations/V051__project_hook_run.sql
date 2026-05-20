CREATE TABLE project_hook_run (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    rule_id             TEXT NOT NULL,
    trigger_type        TEXT NOT NULL,
    dedupe_key          TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'queued',
    source_task_id      TEXT REFERENCES task(id) ON DELETE SET NULL,
    source_execution_id TEXT,
    automation_task_id  TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id        TEXT,
    agent_id            TEXT REFERENCES agent(id) ON DELETE SET NULL,
    reason              TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    completed_at        TEXT,
    UNIQUE(project_id, rule_id, dedupe_key)
);

CREATE INDEX idx_project_hook_run_project_created ON project_hook_run(project_id, created_at DESC);
