ALTER TABLE task
    ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0
    CHECK (read_only IN (0, 1));

-- Preserve the effective access mode that legacy Task classifications implied.
-- New Tasks default to read-write; TaskType no longer selects repository access.
UPDATE task
SET read_only = 1
WHERE task_type IN ('planning', 'discovery', 'review', 'validation');
