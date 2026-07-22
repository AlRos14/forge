ALTER TABLE project ADD COLUMN board_revision INTEGER NOT NULL DEFAULT 0;

CREATE TABLE task_move_operation (
    operation_id       TEXT PRIMARY KEY,
    project_id         TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    task_id            TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    request_hash       TEXT NOT NULL,
    state              TEXT NOT NULL CHECK (state IN ('processing', 'committed', 'completed')),
    direct_result_json TEXT,
    result_json        TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE INDEX idx_task_move_operation_project_created
    ON task_move_operation(project_id, created_at);
CREATE INDEX idx_task_move_operation_task_created
    ON task_move_operation(task_id, created_at);

CREATE TRIGGER task_board_revision_after_insert
AFTER INSERT ON task
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = NEW.project_id;
END;

CREATE TRIGGER task_board_revision_after_delete
AFTER DELETE ON task
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = OLD.project_id;
END;

CREATE TRIGGER task_board_revision_after_update
AFTER UPDATE OF status, board_position, deleted_at, archived_at ON task
WHEN OLD.status IS NOT NEW.status
    OR OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
    OR OLD.archived_at IS NOT NEW.archived_at
BEGIN
    UPDATE project
    SET board_revision = board_revision + 1
    WHERE id = NEW.project_id;
END;
