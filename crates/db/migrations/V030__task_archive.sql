ALTER TABLE task ADD COLUMN archived_at TEXT;
CREATE INDEX idx_task_project_archived ON task(project_id, archived_at);
