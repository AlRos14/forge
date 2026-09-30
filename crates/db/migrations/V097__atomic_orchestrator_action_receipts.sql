-- V096 makes an orchestrator action effect and its domain_event one guarded
-- transaction. Record the action receipt in that same transaction as well.
--
-- First repair receipts left reserved by a V096 process crash after the effect
-- transaction committed but before the old Rust completion update ran. Only
-- events timestamped at or after V096's recorded application can be trusted
-- here: that migration's BEFORE INSERT guard admitted them only while the
-- TaskRole snapshot and active membership were current.
WITH verified_receipts AS (
    SELECT
        action.execution_id,
        action.action_index,
        MIN(effect_event.sequence) AS event_sequence
    FROM orchestrator_action AS action
    JOIN execution AS execution
      ON execution.id = action.execution_id
    JOIN orchestrator_wake_execution AS attempt
      ON attempt.execution_id = execution.id
    JOIN orchestrator_wake AS wake
      ON wake.id = attempt.wake_id
    JOIN domain_event AS effect_event
      ON effect_event.entity_id = action.result_id
    JOIN _migration AS v096
      ON v096.version = 96
     AND effect_event.created_at >= v096.applied_at
    JOIN domain_event AS started
      ON started.id = effect_event.causation_id
    WHERE action.state = 'reserved'
      AND execution.role = 'orchestrator'
      AND execution.purpose = 'orchestrate'
      AND execution.status = 'completed'
      AND execution.task_id = wake.task_id
      AND execution.actor_kind = wake.actor_kind
      AND execution.actor_id = wake.actor_id
      AND (
          (action.action_type = 'message'
           AND effect_event.event_type = 'message.created'
           AND effect_event.entity_type = 'message')
          OR (action.action_type = 'handoff'
              AND effect_event.event_type = 'handoff.created'
              AND effect_event.entity_type = 'handoff')
          OR (action.action_type = 'work_unit'
              AND effect_event.event_type = 'work_unit.created'
              AND effect_event.entity_type = 'work_unit')
          OR (action.action_type = 'proposal'
              AND effect_event.event_type = 'proposal.created'
              AND effect_event.entity_type = 'proposal')
      )
      AND started.event_type = 'execution.started'
      AND started.entity_type = 'execution'
      AND started.entity_id = execution.id
      AND started.actor_type = execution.actor_kind
      AND started.actor_id = execution.actor_id
      AND started.scope_type = 'task'
      AND started.scope_id = execution.task_id
      AND started.correlation_id = wake.correlation_id
      AND started.causation_id = wake.event_id
      AND wake.causation_depth < 16
      AND started.causation_depth = wake.causation_depth + 1
      AND effect_event.actor_type = execution.actor_kind
      AND effect_event.actor_id = execution.actor_id
      AND effect_event.scope_type = 'task'
      AND effect_event.scope_id = execution.task_id
      AND effect_event.correlation_id = started.correlation_id
      AND effect_event.causation_depth = started.causation_depth + 1
      AND CASE action.action_type
          WHEN 'message' THEN EXISTS (
              SELECT 1 FROM message AS effect
              WHERE effect.id = action.result_id
                AND effect.task_id = execution.task_id
                AND effect.sender_actor_kind = execution.actor_kind
                AND effect.sender_actor_id = execution.actor_id
                AND (
                    wake.work_unit_id IS NULL
                    OR EXISTS (
                        SELECT 1 FROM message_work_unit AS scope
                        WHERE scope.message_id = effect.id
                          AND scope.task_id = effect.task_id
                          AND scope.work_unit_id = wake.work_unit_id
                    )
                )
          )
          WHEN 'handoff' THEN EXISTS (
              SELECT 1 FROM handoff AS effect
              WHERE effect.id = action.result_id
                AND effect.task_id = execution.task_id
                AND effect.created_by_actor_kind = execution.actor_kind
                AND effect.created_by_actor_id = execution.actor_id
                AND effect.parent_execution_id = execution.id
                AND (
                    wake.work_unit_id IS NULL
                    OR EXISTS (
                        SELECT 1 FROM handoff_work_unit AS scope
                        WHERE scope.handoff_id = effect.id
                          AND scope.task_id = effect.task_id
                          AND scope.work_unit_id = wake.work_unit_id
                    )
                )
          )
          WHEN 'work_unit' THEN EXISTS (
              SELECT 1 FROM work_unit AS effect
              WHERE effect.id = action.result_id
                AND effect.task_id = execution.task_id
                AND effect.created_by_kind = execution.actor_kind
                AND effect.created_by_id = execution.actor_id
                AND (
                    wake.work_unit_id IS NULL
                    OR effect.parent_work_unit_id = wake.work_unit_id
                )
          )
          WHEN 'proposal' THEN EXISTS (
              SELECT 1 FROM proposal AS effect
              WHERE effect.id = action.result_id
                AND effect.task_id = execution.task_id
                AND effect.proposer_actor_kind = execution.actor_kind
                AND effect.proposer_actor_id = execution.actor_id
                AND (
                    wake.work_unit_id IS NULL
                    OR (
                        effect.target_kind = 'work_unit'
                        AND effect.target_id = wake.work_unit_id
                    )
                )
          )
          ELSE 0
      END
    GROUP BY action.execution_id, action.action_index
)
UPDATE orchestrator_action
SET state = 'completed',
    updated_at = (
        SELECT effect_event.created_at
        FROM verified_receipts AS receipt
        JOIN domain_event AS effect_event
          ON effect_event.sequence = receipt.event_sequence
        WHERE receipt.execution_id = orchestrator_action.execution_id
          AND receipt.action_index = orchestrator_action.action_index
    )
WHERE state = 'reserved'
  AND EXISTS (
      SELECT 1 FROM verified_receipts AS receipt
      WHERE receipt.execution_id = orchestrator_action.execution_id
        AND receipt.action_index = orchestrator_action.action_index
  );

CREATE TRIGGER pr6_orchestrator_action_effect_receipt
AFTER INSERT ON domain_event
WHEN EXISTS (
    SELECT 1 FROM orchestrator_action AS action
    WHERE action.result_id = NEW.entity_id
      AND action.state = 'reserved'
      AND action.action_type = CASE
          WHEN NEW.event_type = 'message.created'
           AND NEW.entity_type = 'message' THEN 'message'
          WHEN NEW.event_type = 'handoff.created'
           AND NEW.entity_type = 'handoff' THEN 'handoff'
          WHEN NEW.event_type = 'work_unit.created'
           AND NEW.entity_type = 'work_unit' THEN 'work_unit'
          WHEN NEW.event_type = 'proposal.created'
           AND NEW.entity_type = 'proposal' THEN 'proposal'
          ELSE ''
      END
)
BEGIN
    UPDATE orchestrator_action
    SET state = 'completed', updated_at = NEW.created_at
    WHERE result_id = NEW.entity_id
      AND state = 'reserved'
      AND action_type = CASE
          WHEN NEW.event_type = 'message.created'
           AND NEW.entity_type = 'message' THEN 'message'
          WHEN NEW.event_type = 'handoff.created'
           AND NEW.entity_type = 'handoff' THEN 'handoff'
          WHEN NEW.event_type = 'work_unit.created'
           AND NEW.entity_type = 'work_unit' THEN 'work_unit'
          WHEN NEW.event_type = 'proposal.created'
           AND NEW.entity_type = 'proposal' THEN 'proposal'
          ELSE ''
      END;
END;
