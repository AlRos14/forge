CREATE TABLE execution_follow_up_receipt (
    idempotency_key TEXT PRIMARY KEY NOT NULL
        CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    parent_execution_id TEXT NOT NULL
        REFERENCES execution(id) ON DELETE RESTRICT,
    request_digest TEXT NOT NULL
        CHECK (length(request_digest) = 64 AND request_digest NOT GLOB '*[^0-9a-f]*'),
    execution_id TEXT NOT NULL UNIQUE
        REFERENCES execution(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    UNIQUE (parent_execution_id, idempotency_key)
);

CREATE INDEX idx_execution_follow_up_receipt_parent
    ON execution_follow_up_receipt(parent_execution_id, created_at, idempotency_key);

CREATE TRIGGER trg_execution_follow_up_receipt_exact_child_insert
BEFORE INSERT ON execution_follow_up_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM execution child
    WHERE child.id = NEW.execution_id
      AND child.parent_execution_id = NEW.parent_execution_id
)
BEGIN
    SELECT RAISE(ABORT, 'follow-up receipt requires its exact child Execution');
END;
CREATE TRIGGER trg_execution_follow_up_receipt_immutable_update
BEFORE UPDATE ON execution_follow_up_receipt
BEGIN
    SELECT RAISE(ABORT, 'follow-up Execution receipts are immutable');
END;

CREATE TRIGGER trg_execution_follow_up_receipt_immutable_delete
BEFORE DELETE ON execution_follow_up_receipt
WHEN NOT EXISTS (
    SELECT 1
    FROM execution parent
    JOIN task ON task.id = parent.task_id
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE parent.id = OLD.parent_execution_id
)
BEGIN
    SELECT RAISE(ABORT, 'follow-up Execution receipts are immutable outside guarded Project teardown');
END;
