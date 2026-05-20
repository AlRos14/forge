ALTER TABLE task ADD COLUMN is_automation INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, archived_at, deleted_at);
