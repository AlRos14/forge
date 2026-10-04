-- Retry/rework is a lifecycle/orchestration policy over exact failure facts.
-- Each failure has one immutable receipt and one durable follow-up event.
CREATE TABLE task_failure_retry_receipt (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    failure_kind     TEXT NOT NULL CHECK (failure_kind IN (
                         'review_request_changes', 'validation_failed',
                         'execution_failed', 'merge_failed'
                     )),
    failure_ref      TEXT NOT NULL CHECK (length(trim(failure_ref)) > 0),
    source_event_id  TEXT NOT NULL REFERENCES domain_event(id) ON DELETE RESTRICT,
    attempt_number   INTEGER NOT NULL CHECK (attempt_number >= 1),
    retry_budget     INTEGER NOT NULL CHECK (retry_budget >= 0),
    disposition      TEXT NOT NULL CHECK (disposition IN ('rework', 'exhausted')),
    policy_ref       TEXT NOT NULL,
    policy_version   INTEGER NOT NULL CHECK (policy_version >= 1),
    policy_digest    TEXT NOT NULL CHECK (length(policy_digest) = 64),
    receipt_event_id TEXT NOT NULL UNIQUE REFERENCES domain_event(id) ON DELETE RESTRICT,
    created_at       TEXT NOT NULL,
    UNIQUE(task_id, failure_kind, failure_ref),
    UNIQUE(task_id, failure_kind, attempt_number),
    CHECK ((disposition = 'rework' AND attempt_number <= retry_budget)
        OR (disposition = 'exhausted' AND attempt_number > retry_budget))
);

CREATE INDEX idx_task_failure_retry_receipt_task_kind
    ON task_failure_retry_receipt(task_id, failure_kind, attempt_number);

CREATE TRIGGER task_failure_retry_receipt_immutable_update
BEFORE UPDATE ON task_failure_retry_receipt
BEGIN
    SELECT RAISE(ABORT, 'Task failure retry receipts are immutable');
END;

CREATE TRIGGER task_failure_retry_receipt_immutable_delete
BEFORE DELETE ON task_failure_retry_receipt
BEGIN
    SELECT RAISE(ABORT, 'Task failure retry receipts are immutable');
END;
