ALTER TABLE task ADD COLUMN blocked_json TEXT NULL;
ALTER TABLE task ADD COLUMN failed_json TEXT NULL;

-- Reset pre-stable blocked workflow rows; blocked is now interruption metadata, so no compatibility shim is needed.
UPDATE task
SET status = 'todo',
    blocked_json = NULL,
    failed_json = NULL
WHERE status = 'blocked';
