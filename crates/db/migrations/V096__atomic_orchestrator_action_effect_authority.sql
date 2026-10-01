-- Plan PR6 closes the policy-check/write race at the shared effect transaction
-- boundary. Collaboration and WorkUnit writers insert their entity row before
-- its domain_event in one SQLite transaction, so aborting this event insert
-- rolls both records back together.
CREATE TRIGGER pr6_orchestrator_action_effect_authority_guard
BEFORE INSERT ON domain_event
WHEN EXISTS (
        SELECT 1 FROM orchestrator_action AS action
        WHERE action.result_id = NEW.entity_id
    )
 AND (
        NEW.event_type IN (
            'message.created', 'handoff.created',
            'work_unit.created', 'proposal.created'
        )
        OR NEW.entity_type IN ('message', 'handoff', 'work_unit', 'proposal')
    )
 AND NOT EXISTS (
        SELECT 1
        FROM orchestrator_action AS action
        JOIN execution AS execution
          ON execution.id = action.execution_id
        JOIN orchestrator_wake_execution AS attempt
          ON attempt.execution_id = execution.id
        JOIN orchestrator_wake AS wake
          ON wake.id = attempt.wake_id
        JOIN task_role AS role
          ON role.id = wake.task_role_id
        JOIN role_membership AS member
          ON member.task_role_id = wake.task_role_id
         AND member.actor_kind = wake.actor_kind
         AND member.actor_id = wake.actor_id
         AND member.status = 'active'
        JOIN domain_event AS started
          ON started.id = NEW.causation_id
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
          AND execution.role = 'orchestrator'
          AND execution.purpose = 'orchestrate'
          AND execution.status = 'completed'
          AND execution.task_id = wake.task_id
          AND execution.actor_kind = wake.actor_kind
          AND execution.actor_id = wake.actor_id
          AND wake.current_attempt = attempt.attempt_number
          AND wake.state IN ('leased', 'running', 'uncertain')
          AND attempt.state IN ('start_requested', 'running', 'uncertain')
          AND role.task_id = wake.task_id
          AND role.role = 'orchestrator'
          AND role.version = wake.task_role_version
          AND role.policy_json = wake.task_role_policy_json
          AND role.coordination_mode IS wake.coordination_mode
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
          AND NEW.actor_type = execution.actor_kind
          AND NEW.actor_id = execution.actor_id
          AND NEW.scope_type = 'task'
          AND NEW.scope_id = execution.task_id
          AND NEW.correlation_id = started.correlation_id
          AND NEW.causation_depth = started.causation_depth + 1
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
    )
BEGIN
    SELECT RAISE(ABORT, 'PR6_STALE_ORCHESTRATOR_ACTION');
END;
