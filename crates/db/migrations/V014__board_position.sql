ALTER TABLE task ADD COLUMN board_position REAL NOT NULL DEFAULT 0.0;

UPDATE task
SET board_position = sub.rn
FROM (
  SELECT id, ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY created_at ASC, id ASC) AS rn
  FROM task
) AS sub
WHERE task.id = sub.id;
