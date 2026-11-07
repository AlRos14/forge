-- Task launch is a durable dispatch intent owned by TaskLifecycle authority.
-- The Human requests a Task-level state; the dispatcher performs exact WorkUnit
-- admissions. Executions remain the historical attempt records and are no
-- longer created by an ordinary public launch request.

CREATE TABLE task_dispatch (
    task_id            TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    state              TEXT NOT NULL CHECK (state IN ('idle', 'running', 'paused', 'cancelled')),
    version            INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    requested_by_kind  TEXT NOT NULL CHECK (requested_by_kind IN ('human', 'agent', 'system')),
    requested_by_id    TEXT,
    reason             TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    CHECK (requested_by_kind = 'system' OR requested_by_id IS NOT NULL)
);

CREATE TABLE task_dispatch_receipt (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    operation         TEXT NOT NULL CHECK (operation IN ('launch', 'pause', 'resume', 'cancel')),
    idempotency_key   TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    request_digest    TEXT NOT NULL CHECK (length(request_digest) = 64),
    intent_version    INTEGER NOT NULL CHECK (intent_version >= 1),
    created_at        TEXT NOT NULL,
    UNIQUE(task_id, idempotency_key)
);

CREATE INDEX idx_task_dispatch_receipt_task_created
    ON task_dispatch_receipt(task_id, created_at DESC, id DESC);
CREATE INDEX idx_task_dispatch_state_version
    ON task_dispatch(state, version, task_id);

INSERT INTO task_dispatch (
    task_id, state, version, requested_by_kind, requested_by_id, reason, created_at, updated_at
)
SELECT id, 'idle', 1, 'system', NULL, NULL, created_at, updated_at FROM task;

CREATE TRIGGER task_dispatch_insert_for_new_task
AFTER INSERT ON task
BEGIN
    INSERT INTO task_dispatch (
        task_id, state, version, requested_by_kind, requested_by_id, reason,
        created_at, updated_at
    ) VALUES (
        NEW.id, 'idle', 1, 'system', NULL, NULL, NEW.created_at, NEW.updated_at
    );
END;

CREATE TRIGGER task_dispatch_update_guard
BEFORE UPDATE ON task_dispatch
WHEN NEW.task_id IS NOT OLD.task_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.version != OLD.version + 1
  OR NOT EXISTS (
      SELECT 1 FROM task_dispatch_receipt r
      WHERE r.task_id = OLD.task_id AND r.intent_version = NEW.version
  )
BEGIN
    SELECT RAISE(ABORT, 'Task dispatch changes require one exact receipt and version advance');
END;

CREATE TRIGGER task_dispatch_receipt_insert_guard
BEFORE INSERT ON task_dispatch_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task_dispatch d
    WHERE d.task_id = NEW.task_id AND d.version = NEW.intent_version - 1
)
BEGIN
    SELECT RAISE(ABORT, 'Task dispatch receipt does not match its exact intent version');
END;

CREATE TRIGGER task_dispatch_receipt_immutable_update
BEFORE UPDATE ON task_dispatch_receipt
BEGIN
    SELECT RAISE(ABORT, 'Task dispatch receipts are immutable');
END;

CREATE TRIGGER task_dispatch_receipt_immutable_delete
BEFORE DELETE ON task_dispatch_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task dispatch receipts are immutable outside Project teardown');
END;

CREATE TRIGGER task_dispatch_immutable_delete
BEFORE DELETE ON task_dispatch
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task dispatch intent is immutable outside Project teardown');
END;
