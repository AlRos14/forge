-- Installed Tasks keep their existing limits. New Tasks receive the guided
-- defaults. Updating a limit never resets consumed work or grants new scope.
CREATE TABLE task_autonomy_limits (
    task_id TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    version INTEGER NOT NULL DEFAULT 1,
    max_rework_rounds INTEGER NOT NULL DEFAULT 16 CHECK (max_rework_rounds BETWEEN 1 AND 1000),
    max_route_requests INTEGER NOT NULL DEFAULT 16 CHECK (max_route_requests BETWEEN 1 AND 1000),
    updated_at TEXT NOT NULL
);
INSERT INTO task_autonomy_limits(task_id, max_rework_rounds, max_route_requests, updated_at)
SELECT task_id, 8, 4, updated_at FROM task_coordination;
CREATE TRIGGER task_autonomy_limits_new_task
AFTER INSERT ON task_coordination
BEGIN
    INSERT INTO task_autonomy_limits(task_id, updated_at) VALUES (NEW.task_id, NEW.updated_at);
    UPDATE task_coordination SET max_turns=32, max_quality_requests=64 WHERE task_id=NEW.task_id;
END;
CREATE TABLE task_autonomy_limit_receipt (
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    response_json TEXT NOT NULL CHECK (json_valid(response_json)),
    created_at TEXT NOT NULL,
    PRIMARY KEY(task_id, idempotency_key)
);
