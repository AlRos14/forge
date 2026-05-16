CREATE TABLE IF NOT EXISTS notification (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES task(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT,
    read INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notification_project_read ON notification(project_id, read);
CREATE INDEX IF NOT EXISTS idx_notification_created_at ON notification(created_at);
