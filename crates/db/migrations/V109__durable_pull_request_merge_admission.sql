-- A pull-request merge admission remains durable while the provider owns the
-- open PR. It does not hold the local exclusive operation slot after the
-- momentary PublishPr operation completes.
ALTER TABLE task_integration_operation
    ADD COLUMN remote_waiting INTEGER NOT NULL DEFAULT 0
        CHECK (remote_waiting IN (0, 1));
ALTER TABLE task_integration_operation
    ADD COLUMN parent_operation_id TEXT REFERENCES task_integration_operation(id) ON DELETE RESTRICT;
ALTER TABLE task_integration_operation
    ADD COLUMN result_event_id TEXT REFERENCES domain_event(id) ON DELETE RESTRICT;

ALTER TABLE pr_metadata
    ADD COLUMN task_merge_operation_id TEXT REFERENCES task_integration_operation(id) ON DELETE RESTRICT;
ALTER TABLE pr_metadata
    ADD COLUMN publish_operation_id TEXT REFERENCES task_integration_operation(id) ON DELETE RESTRICT;

DROP INDEX idx_task_integration_operation_active_task;
CREATE UNIQUE INDEX idx_task_integration_operation_active_task
    ON task_integration_operation(task_id)
    WHERE status = 'running' AND remote_waiting = 0;
CREATE UNIQUE INDEX idx_task_integration_operation_publish_parent
    ON task_integration_operation(parent_operation_id)
    WHERE kind = 'publish_pr';

CREATE TRIGGER task_integration_operation_remote_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN (NEW.remote_waiting = 1 AND (
        NEW.kind != 'task_merge'
        OR NEW.parent_operation_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1 FROM task t JOIN repo r ON r.id = t.repo_id
            WHERE t.id = NEW.task_id AND r.work_mode = 'pull_request'
        )
     ))
  OR (NEW.kind = 'task_merge' AND NEW.remote_waiting = 0 AND EXISTS (
        SELECT 1 FROM task t JOIN repo r ON r.id = t.repo_id
        WHERE t.id = NEW.task_id AND r.work_mode = 'pull_request'
     ))
  OR (NEW.kind = 'publish_pr' AND (
        NEW.parent_operation_id IS NULL
        OR NOT EXISTS (
            SELECT 1 FROM task_integration_operation merge_op
            JOIN task_lifecycle lifecycle ON lifecycle.task_id = merge_op.task_id
            WHERE merge_op.id = NEW.parent_operation_id
              AND merge_op.task_id = NEW.task_id
              AND merge_op.kind = 'task_merge'
              AND merge_op.status = 'running'
              AND merge_op.remote_waiting = 1
              AND merge_op.gate_evaluation_id = NEW.gate_evaluation_id
              AND lifecycle.state = 'merging'
              AND lifecycle.reason_ref = merge_op.id
        )
     ))
  OR (NEW.kind != 'publish_pr' AND NEW.parent_operation_id IS NOT NULL)
  OR (NEW.remote_waiting = 1 AND EXISTS (
        SELECT 1 FROM task_integration_operation active
        WHERE active.task_id = NEW.task_id AND active.status = 'running'
          AND active.remote_waiting = 0
     ))
  OR (NEW.remote_waiting = 0 AND EXISTS (
        SELECT 1 FROM task_integration_operation merge_op
        WHERE merge_op.task_id = NEW.task_id AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running' AND merge_op.remote_waiting = 1
          AND NOT (NEW.kind = 'publish_pr' AND NEW.parent_operation_id = merge_op.id)
     ))
BEGIN
    SELECT RAISE(ABORT, 'pull-request operation is missing its exact durable TaskMerge admission');
END;

CREATE TRIGGER task_integration_operation_remote_identity_immutable
BEFORE UPDATE OF remote_waiting, parent_operation_id ON task_integration_operation
WHEN NEW.remote_waiting IS NOT OLD.remote_waiting
  OR NEW.parent_operation_id IS NOT OLD.parent_operation_id
BEGIN
    SELECT RAISE(ABORT, 'Task integration operation admission identity is immutable');
END;

CREATE TRIGGER task_integration_operation_provider_result_guard
BEFORE UPDATE OF status, result_event_id ON task_integration_operation
WHEN (NEW.result_event_id IS NOT OLD.result_event_id
      OR (NEW.status IS NOT OLD.status AND (
          (NEW.kind = 'task_merge' AND NEW.remote_waiting = 1
           AND NEW.status IN ('succeeded', 'failed'))
          OR (NEW.kind = 'publish_pr' AND NEW.status = 'failed')
      )))
 AND (
    OLD.result_event_id IS NOT NULL
    OR NEW.status NOT IN ('succeeded', 'failed')
    OR NOT EXISTS (
        SELECT 1 FROM domain_event event
        JOIN pr_metadata metadata ON metadata.task_id = NEW.task_id
        WHERE event.id = NEW.result_event_id
          AND (
              (NEW.kind = 'task_merge'
               AND NEW.remote_waiting = 1
               AND metadata.task_merge_operation_id = NEW.id)
              OR (NEW.kind = 'publish_pr'
                  AND NEW.status = 'failed'
                  AND metadata.publish_operation_id = NEW.id
                  AND metadata.task_merge_operation_id = NEW.parent_operation_id)
          )
          AND event.event_type = 'pr.status_changed'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task'
          AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
          AND json_extract(event.payload_json, '$.pr_metadata_id') = metadata.id
          AND json_extract(event.payload_json, '$.task_merge_operation_id') =
              CASE WHEN NEW.kind = 'task_merge' THEN NEW.id ELSE NEW.parent_operation_id END
          AND json_extract(event.payload_json, '$.publish_operation_id') =
              CASE WHEN NEW.kind = 'publish_pr' THEN NEW.id ELSE metadata.publish_operation_id END
          AND (
              (NEW.status = 'succeeded' AND NEW.kind = 'task_merge'
               AND json_extract(event.payload_json, '$.status') = 'merged')
              OR (NEW.status = 'failed' AND NEW.kind = 'task_merge'
                  AND json_extract(event.payload_json, '$.status') IN ('closed', 'publication_failed'))
              OR (NEW.status = 'failed' AND NEW.kind = 'publish_pr'
                  AND json_extract(event.payload_json, '$.status') = 'publication_failed')
          )
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskMerge provider result is missing exact PR status provenance');
END;

DROP TRIGGER task_publish_pr_gate_admission_guard;
CREATE TRIGGER task_publish_pr_gate_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind = 'publish_pr' AND (
    NEW.gate_evaluation_id IS NULL
    OR NEW.parent_operation_id IS NULL
    OR NOT EXISTS (
        SELECT 1 FROM task_integration_operation merge_op
        JOIN gate_evaluation evaluation
          ON evaluation.id = merge_op.gate_evaluation_id
         AND evaluation.task_id = merge_op.task_id
        JOIN gate ON gate.id = evaluation.gate_id AND gate.task_id = evaluation.task_id
        JOIN task_lifecycle lifecycle ON lifecycle.task_id = merge_op.task_id
        WHERE merge_op.id = NEW.parent_operation_id
          AND merge_op.task_id = NEW.task_id
          AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.remote_waiting = 1
          AND merge_op.gate_evaluation_id = NEW.gate_evaluation_id
          AND evaluation.outcome = 'satisfied'
          AND gate.gate_kind = 'merge_readiness'
          AND gate.scope_kind = 'task' AND gate.scope_id = NEW.task_id
          AND lifecycle.state = 'merging'
          AND lifecycle.reason_ref = merge_op.id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'PR publication requires its exact durable TaskMerge admission');
END;

CREATE TRIGGER pr_metadata_merge_admission_guard_insert
BEFORE INSERT ON pr_metadata
WHEN NEW.task_merge_operation_id IS NULL
  OR NEW.publish_operation_id IS NULL
  OR NOT EXISTS (
      SELECT 1 FROM task_integration_operation merge_op
      JOIN task_integration_operation publish_op
        ON publish_op.parent_operation_id = merge_op.id
       AND publish_op.task_id = merge_op.task_id
      WHERE merge_op.id = NEW.task_merge_operation_id
        AND merge_op.task_id = NEW.task_id
        AND merge_op.kind = 'task_merge'
        AND merge_op.status = 'running'
        AND merge_op.remote_waiting = 1
        AND publish_op.id = NEW.publish_operation_id
        AND publish_op.kind = 'publish_pr'
        AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
  )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata requires its exact durable TaskMerge and PublishPr operations');
END;

CREATE TRIGGER pr_metadata_merge_admission_guard_update
BEFORE UPDATE OF task_merge_operation_id, publish_operation_id ON pr_metadata
WHEN (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
      OR NEW.publish_operation_id IS NOT OLD.publish_operation_id)
 AND (
    NEW.task_merge_operation_id IS NULL
    OR NEW.publish_operation_id IS NULL
    OR (OLD.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM task_integration_operation old_merge
        WHERE old_merge.id = OLD.task_merge_operation_id
          AND old_merge.task_id = OLD.task_id
          AND old_merge.kind = 'task_merge'
          AND old_merge.status IN ('failed', 'conflict', 'abandoned', 'succeeded')
          AND OLD.merge_status IN ('publication_failed', 'closed_without_merge')
    ))
    OR NOT EXISTS (
        SELECT 1 FROM task_integration_operation merge_op
        JOIN task_integration_operation publish_op
          ON publish_op.parent_operation_id = merge_op.id
         AND publish_op.task_id = merge_op.task_id
        WHERE merge_op.id = NEW.task_merge_operation_id
          AND merge_op.task_id = NEW.task_id
          AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.remote_waiting = 1
          AND publish_op.id = NEW.publish_operation_id
          AND publish_op.kind = 'publish_pr'
          AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata merge admission binding is immutable or invalid');
END;

CREATE TRIGGER task_merge_closed_provider_transition_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.from_state = 'merging' AND NEW.to_state = 'blocked'
 AND NEW.reason_kind = 'pr_closed_without_merge'
 AND NOT EXISTS (
     SELECT 1 FROM task_integration_operation merge_op
     JOIN pr_metadata metadata ON metadata.task_merge_operation_id = merge_op.id
     JOIN domain_event event ON event.id = merge_op.result_event_id
     WHERE merge_op.id = NEW.cause_ref
       AND merge_op.task_id = NEW.task_id
       AND merge_op.kind = 'task_merge'
       AND merge_op.status = 'failed'
       AND merge_op.result_event_id = NEW.reason_ref
       AND event.event_type = 'pr.status_changed'
       AND event.entity_type = 'pr_metadata'
       AND event.entity_id = metadata.id
       AND event.scope_type = 'task'
       AND event.scope_id = NEW.task_id
       AND json_valid(event.payload_json)
       AND json_extract(event.payload_json, '$.status') = 'closed'
 )
BEGIN
    SELECT RAISE(ABORT, 'closed PR transition requires its exact admitted TaskMerge provider result');
END;

-- Pre-V109 PR rows have no durable TaskMerge admission. Preserve their
-- provider metadata, but keep them out of automatic reconciliation and never
-- reinterpret a provider result as a retrospective merge authorization.
UPDATE pr_metadata
SET merge_status = 'legacy_unadmitted'
WHERE task_merge_operation_id IS NULL OR publish_operation_id IS NULL;
