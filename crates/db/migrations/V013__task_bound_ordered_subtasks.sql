ALTER TABLE task ADD COLUMN subtask_order INTEGER;

WITH ranked AS (
    SELECT
        id,
        ROW_NUMBER() OVER (
            PARTITION BY parent_task_id
            ORDER BY created_at ASC, id ASC
        ) - 1 AS subtask_order
    FROM task
    WHERE parent_task_id IS NOT NULL
)
UPDATE task
SET subtask_order = (
    SELECT ranked.subtask_order
    FROM ranked
    WHERE ranked.id = task.id
)
WHERE parent_task_id IS NOT NULL;

CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);

-- workspace.task_id UNIQUE and workspace.cleanup_after are preserved; no workspace schema changes are made here.
