ALTER TABLE task ADD COLUMN assignee_type TEXT CHECK (assignee_type IN ('agent', 'user'));
ALTER TABLE task ADD COLUMN user_handle TEXT;

CREATE TABLE IF NOT EXISTS task_dependency (
    task_id        TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    depends_on_id  TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    created_at     TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on_id)
);
CREATE INDEX IF NOT EXISTS idx_task_dependency_depends_on ON task_dependency(depends_on_id);
