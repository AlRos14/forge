-- A merge-ready Task can be reopened by a new exact GateEvaluation or by the
-- durable retry policy's exact rework receipt. Actor/UI transitions cannot
-- clear merge readiness directly.
DROP TRIGGER pr9_ready_to_merge_demotion_guard;

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
               AND receipt.policy_version = 1
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
