ALTER TABLE execution ADD COLUMN last_activity_at TEXT;

UPDATE execution
SET last_activity_at = created_at
WHERE last_activity_at IS NULL
  AND status = 'running';
