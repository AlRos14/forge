-- Split WorkUnit integration failures from final TaskMerge failures. Existing
-- merge_failed receipts are retained as immutable historical facts; new code
-- emits only the two domain-specific kinds.
DROP TRIGGER task_failure_retry_receipt_immutable_update;
DROP TRIGGER task_failure_retry_receipt_immutable_delete;
-- SQLite rewrites trigger bodies when the receipt table is renamed. Recreate
-- both lifecycle fences below so they bind to the replacement table.
DROP TRIGGER task_lifecycle_retry_exhaustion_guard;
DROP TRIGGER pr9_ready_to_merge_demotion_guard;
DROP INDEX idx_task_failure_retry_receipt_task_kind;

ALTER TABLE task_failure_retry_receipt
    RENAME TO task_failure_retry_receipt_v101;

CREATE TABLE task_failure_retry_receipt (
    id               TEXT PRIMARY KEY,
    task_id          TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    failure_kind     TEXT NOT NULL CHECK (failure_kind IN (
                         'review_request_changes', 'validation_failed',
                         'execution_failed', 'work_unit_integration_failed',
                         'task_merge_failed', 'merge_failed'
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
    retry_epoch      INTEGER NOT NULL DEFAULT 0 CHECK (retry_epoch >= 0),
    UNIQUE(task_id, failure_kind, failure_ref),
    UNIQUE(task_id, failure_kind, retry_epoch, attempt_number),
    CHECK ((disposition = 'rework' AND attempt_number <= retry_budget)
        OR (disposition = 'exhausted' AND attempt_number > retry_budget))
);

INSERT INTO task_failure_retry_receipt (
    id, task_id, failure_kind, failure_ref, source_event_id,
    attempt_number, retry_budget, disposition, policy_ref,
    policy_version, policy_digest, receipt_event_id, created_at, retry_epoch
)
SELECT id, task_id, failure_kind, failure_ref, source_event_id,
       attempt_number, retry_budget, disposition, policy_ref,
       policy_version, policy_digest, receipt_event_id, created_at, 0
FROM task_failure_retry_receipt_v101;

DROP TABLE task_failure_retry_receipt_v101;

CREATE INDEX idx_task_failure_retry_receipt_task_kind_epoch
    ON task_failure_retry_receipt(task_id, failure_kind, retry_epoch, attempt_number);

CREATE TRIGGER task_failure_retry_receipt_immutable_update
BEFORE UPDATE ON task_failure_retry_receipt
BEGIN
    SELECT RAISE(ABORT, 'Task failure retry receipts are immutable');
END;

CREATE TRIGGER task_failure_retry_receipt_immutable_delete
BEFORE DELETE ON task_failure_retry_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task retry receipts are immutable outside Project teardown');
END;

CREATE TRIGGER task_lifecycle_retry_exhaustion_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.to_state IN ('ready', 'active')
 AND EXISTS (
     SELECT 1 FROM task_failure_retry_receipt receipt
     WHERE receipt.task_id = NEW.task_id AND receipt.disposition = 'exhausted'
 )
BEGIN
    SELECT RAISE(ABORT, 'Task retry budget is exhausted; runnable lifecycle states are fenced');
END;

CREATE TRIGGER pr9_ready_to_merge_demotion_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.from_state = 'ready_to_merge' AND NEW.to_state = 'active'
 AND NOT (
     NEW.cause_kind = 'gate_evaluation'
     OR (
         NEW.cause_kind = 'domain_event'
         AND EXISTS (
             SELECT 1
             FROM domain_event event
             JOIN task_failure_retry_receipt receipt
               ON receipt.receipt_event_id = event.id
             WHERE event.id = NEW.cause_ref
               AND event.event_type = 'task.rework_requested'
               AND event.entity_type = 'task'
               AND event.entity_id = NEW.task_id
               AND event.scope_type = 'task'
               AND event.scope_id = NEW.task_id
               AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
               AND json_extract(event.payload_json, '$.source_event_id') = receipt.source_event_id
               AND json_extract(event.payload_json, '$.failure_kind') = receipt.failure_kind
               AND json_extract(event.payload_json, '$.failure_ref') = receipt.failure_ref
               AND json_extract(event.payload_json, '$.attempt_number') = receipt.attempt_number
               AND json_extract(event.payload_json, '$.retry_budget') = receipt.retry_budget
               AND json_extract(event.payload_json, '$.disposition') = 'rework'
               AND json_extract(event.payload_json, '$.policy_ref') = receipt.policy_ref
               AND json_extract(event.payload_json, '$.policy_version') = receipt.policy_version
               AND json_extract(event.payload_json, '$.policy_digest') = receipt.policy_digest
               AND receipt.task_id = NEW.task_id
               AND receipt.disposition = 'rework'
               AND receipt.attempt_number <= receipt.retry_budget
               AND receipt.policy_ref = 'forge.task_failure_retry'
               AND receipt.policy_version IN (1, 2)
               AND NEW.reason_ref = receipt.failure_ref
               AND NOT EXISTS (
                   SELECT 1
                   FROM task_lifecycle_transition later_transition
                   JOIN domain_event later_event
                     ON later_event.id = later_transition.domain_event_id
                   JOIN domain_event retry_event
                     ON retry_event.id = receipt.receipt_event_id
                   WHERE later_transition.task_id = receipt.task_id
                     AND later_event.sequence > retry_event.sequence
               )
         )
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'ready_to_merge rework requires a new exact GateEvaluation or retry receipt');
END;
