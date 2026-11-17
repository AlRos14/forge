-- A remote PR close does not cancel same-Task rework. D3' keeps the exact
-- supersession and lifecycle receipt, then resumes the Task so a later
-- publication can create a fresh PR. V175 accidentally restricted the
-- Blocked -> Active receipt to an `open` supersession, making that documented
-- remote-close path impossible.
DROP TRIGGER task_lifecycle_pr_feedback_rework_guard;
CREATE TRIGGER task_lifecycle_pr_feedback_rework_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.reason_kind = 'pr_feedback_rework'
 AND NOT (
     (NEW.from_state = 'merging' AND NEW.to_state = 'blocked'
      AND NEW.cause_kind = 'merge_operation'
      AND EXISTS (
          SELECT 1 FROM pr_rework_supersession s
          JOIN task_integration_operation op ON op.id = s.task_merge_operation_id
          WHERE s.id = NEW.reason_ref AND s.task_id = NEW.task_id AND s.state = 'open'
            AND op.id = NEW.cause_ref AND op.task_id = NEW.task_id
            AND op.kind = 'task_merge' AND op.status = 'abandoned'))
  OR (NEW.from_state = 'blocked' AND NEW.to_state = 'active'
      AND NEW.cause_kind = 'domain_event'
      AND EXISTS (
          SELECT 1 FROM pr_rework_supersession s
          JOIN task_lifecycle l ON l.task_id = s.task_id
          JOIN task_coordination_request r ON r.id = s.request_id AND r.task_id = s.task_id
          WHERE s.id = NEW.reason_ref AND s.task_id = NEW.task_id
            AND s.state IN ('open', 'remote_closed')
            AND s.result_event_id = NEW.cause_ref
            AND l.state = 'blocked' AND l.reason_kind = 'pr_feedback_rework'
            AND l.reason_ref = s.id
            AND r.kind = 'external_feedback' AND r.state = 'triaged'
            AND EXISTS (
                SELECT 1 FROM task_external_feedback_disposition d
                WHERE d.request_id = r.id AND d.task_id = r.task_id
                  AND d.disposition = 'extend_task')))
 )
BEGIN
    SELECT RAISE(ABORT, 'pr_feedback_rework lifecycle receipt lacks its exact supersession authority');
END;
