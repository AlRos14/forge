CREATE TABLE task_plan_revision (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    revision            INTEGER NOT NULL,
    checkpoint          TEXT NOT NULL CHECK (checkpoint IN ('planner_ready', 'approved', 'execution_update', 'final')),
    markdown            TEXT NOT NULL,
    content_digest      TEXT NOT NULL,
    checklist_json      TEXT NOT NULL CHECK (json_valid(checklist_json)),
    warnings_json       TEXT NOT NULL CHECK (json_valid(warnings_json)),
    source_execution_id TEXT REFERENCES execution(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL,
    UNIQUE(task_id, revision),
    UNIQUE(task_id, checkpoint, content_digest)
);

CREATE INDEX idx_task_plan_revision_latest
    ON task_plan_revision(task_id, revision DESC);

CREATE TABLE task_plan_approval (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    plan_revision_id    TEXT NOT NULL REFERENCES task_plan_revision(id) ON DELETE RESTRICT,
    content_digest      TEXT NOT NULL,
    principal_type      TEXT NOT NULL CHECK (principal_type IN ('user', 'system')),
    principal_id        TEXT NOT NULL,
    decision            TEXT NOT NULL CHECK (decision IN ('approved', 'rejected')),
    reason              TEXT,
    created_at          TEXT NOT NULL,
    UNIQUE(task_id, plan_revision_id, decision)
);

CREATE TRIGGER task_plan_revision_immutable
BEFORE UPDATE ON task_plan_revision BEGIN
    SELECT RAISE(ABORT, 'Task plan revisions are immutable');
END;

CREATE TRIGGER task_plan_approval_immutable
BEFORE UPDATE ON task_plan_approval BEGIN
    SELECT RAISE(ABORT, 'Task plan approvals are immutable');
END;
