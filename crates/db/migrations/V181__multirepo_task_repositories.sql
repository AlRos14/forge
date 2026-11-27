CREATE TABLE task_repository (
    task_id     TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    repo_id     TEXT NOT NULL REFERENCES repo(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('primary', 'affected', 'context')),
    position    INTEGER NOT NULL CHECK (position >= 0),
    created_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id),
    UNIQUE (task_id, position),
    CHECK ((role = 'primary' AND position = 0)
        OR (role IN ('affected', 'context') AND position > 0))
);
CREATE UNIQUE INDEX idx_task_repository_one_primary
    ON task_repository(task_id) WHERE role = 'primary';
CREATE INDEX idx_task_repository_repo
    ON task_repository(repo_id, task_id);

CREATE TRIGGER task_repository_insert_guard
BEFORE INSERT ON task_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN repo r ON r.id = NEW.repo_id
    WHERE t.id = NEW.task_id AND r.project_id = t.project_id
      AND (NEW.role != 'primary' OR t.repo_id = NEW.repo_id)
)
BEGIN
    SELECT RAISE(ABORT, 'Task repository must belong to its Project and primary must mirror task.repo_id');
END;

CREATE TRIGGER task_repository_update_guard
BEFORE UPDATE ON task_repository
WHEN NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NOT EXISTS (
      SELECT 1 FROM task t JOIN repo r ON r.id = NEW.repo_id
      WHERE t.id = NEW.task_id AND r.project_id = t.project_id
        AND (NEW.role != 'primary' OR t.repo_id = NEW.repo_id)
        AND (OLD.role != 'primary' OR NEW.role = 'primary'
             OR t.repo_id IS NOT OLD.repo_id)
  )
BEGIN
    SELECT RAISE(ABORT, 'Task repository identity is immutable and primary must mirror task.repo_id');
END;

CREATE TRIGGER task_repository_delete_guard
BEFORE DELETE ON task_repository
WHEN OLD.role = 'primary'
 AND EXISTS (SELECT 1 FROM task t WHERE t.id = OLD.task_id AND t.repo_id = OLD.repo_id)
 AND NOT EXISTS (
     SELECT 1 FROM task t
     JOIN project_deletion_guard guard ON guard.project_id = t.project_id
     WHERE t.id = OLD.task_id
 )
BEGIN
    SELECT RAISE(ABORT, 'change task.repo_id before removing its primary association');
END;

CREATE TRIGGER task_repository_mirror_after_task_insert
AFTER INSERT ON task
WHEN NEW.repo_id IS NOT NULL
BEGIN
    INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
    VALUES (NEW.id, NEW.repo_id, 'primary', 0, NEW.created_at);
END;

CREATE TRIGGER task_repository_mirror_after_task_repo_update
AFTER UPDATE OF repo_id ON task
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    UPDATE task_repository
       SET role = 'affected',
           position = (SELECT COALESCE(MAX(position), 0) + 1
                       FROM task_repository WHERE task_id = NEW.id)
     WHERE task_id = NEW.id AND role = 'primary'
       AND repo_id IS NOT NEW.repo_id;
    UPDATE task_repository
       SET role = 'primary', position = 0
     WHERE task_id = NEW.id AND repo_id = NEW.repo_id;
    INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
    SELECT NEW.id, NEW.repo_id, 'primary', 0, NEW.updated_at
     WHERE NEW.repo_id IS NOT NULL
       AND NOT EXISTS (SELECT 1 FROM task_repository
                       WHERE task_id = NEW.id AND repo_id = NEW.repo_id);
END;

CREATE TRIGGER task_primary_repo_project_guard_update
BEFORE UPDATE OF repo_id ON task
WHEN NEW.repo_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1 FROM repo r
     WHERE r.id = NEW.repo_id AND r.project_id = NEW.project_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Task primary repository must belong to its Project');
END;

INSERT INTO task_repository(task_id, repo_id, role, position, created_at)
SELECT id, repo_id, 'primary', 0, created_at
FROM task
WHERE repo_id IS NOT NULL;

CREATE TABLE work_unit_repository (
    work_unit_id  TEXT NOT NULL,
    task_id       TEXT NOT NULL,
    repo_id       TEXT NOT NULL,
    created_at    TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT
);
CREATE INDEX idx_work_unit_repository_repo
    ON work_unit_repository(task_id, repo_id, work_unit_id);

CREATE TRIGGER work_unit_repository_insert_guard
BEFORE INSERT ON work_unit_repository
WHEN NOT EXISTS (
    SELECT 1
    FROM work_unit w
    JOIN task_repository tr
      ON tr.task_id = w.task_id AND tr.repo_id = NEW.repo_id
    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
      AND tr.role IN ('primary', 'affected')
)
 OR EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.work_unit_id)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository scope must be writable and fixed before its first Execution');
END;

CREATE TRIGGER work_unit_repository_immutable_update
BEFORE UPDATE ON work_unit_repository
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository scope is immutable');
END;

-- Tasks repo-less remain without a task_repository row.
-- An empty work_unit_repository means all writable Task repos;
-- context repositories cannot be selected.
