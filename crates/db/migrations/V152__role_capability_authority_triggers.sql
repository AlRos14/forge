-- Replace active reviewer/orchestrator name checks with immutable
-- Execution snapshots and current TaskRole capabilities. Historical migration
-- files remain unchanged.


DROP TRIGGER completed_review_execution_requires_report;

DROP TRIGGER execution_artifact_output_review_guard_insert;

DROP TRIGGER gate_evaluation_input_same_task_insert;

DROP TRIGGER review_execution_subject_execution_match_update;

DROP TRIGGER review_execution_subject_guard_insert;

DROP TRIGGER review_report_artifact_required_insert;

DROP TRIGGER orchestrator_action_execution_guard;

DROP TRIGGER orchestrator_execution_authority_guard;

DROP TRIGGER orchestrator_execution_authority_guard_update;

DROP TRIGGER pr6_orchestrator_action_effect_authority_guard;

DROP TRIGGER orchestrator_wake_authority_guard_insert;

CREATE TRIGGER completed_review_execution_requires_report
BEFORE UPDATE OF status ON execution
WHEN OLD.status = 'running' AND NEW.status = 'completed'
 AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = NEW.id AND capability.type = 'text' AND capability.value = 'formal_review')
 AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN artifact a ON a.id = p.artifact_id AND a.task_id = p.task_id
    WHERE o.execution_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = 'review_report' AND a.kind = 'review_report'
 )
BEGIN
    SELECT RAISE(ABORT, 'Completed Review Execution requires its exact ReviewReport Artifact');
END;

CREATE TRIGGER execution_artifact_output_review_guard_insert
BEFORE INSERT ON execution_artifact_output
WHEN NEW.kind = 'review_report' AND NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = e.id AND capability.type = 'text' AND capability.value = 'formal_review')
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
      AND (e.workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM review_execution_subject s
          WHERE s.execution_id = e.id AND s.task_id = e.task_id
            AND s.workspace_id = e.workspace_id
            AND s.base_commit_sha = e.before_sha
            AND s.head_commit_sha = e.after_sha
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'ReviewReport output requires a real reviewer Execution with its exact Workspace subject');
END;

CREATE TRIGGER gate_evaluation_input_same_task_insert
BEFORE INSERT ON gate_evaluation_input
WHEN NOT EXISTS (
    SELECT 1 FROM gate_evaluation e
    WHERE e.id = NEW.evaluation_id AND e.task_id = NEW.task_id
)
  OR (NEW.input_kind = 'review_report' AND NOT EXISTS (
      SELECT 1 FROM artifact a
      JOIN artifact_execution_producer p ON p.artifact_id = a.id AND p.task_id = a.task_id
      JOIN execution x ON x.id = p.execution_id AND x.task_id = p.task_id
      WHERE a.id = NEW.input_id AND a.task_id = NEW.task_id AND a.kind = 'review_report'
        AND a.digest = NEW.input_digest AND p.execution_id = NEW.producer_ref
        AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = x.id AND capability.type = 'text' AND capability.value = 'formal_review') AND x.status = 'completed'
        AND json_extract(a.content, '$.subject.task_id') = NEW.task_id
        AND json_extract(a.content, '$.subject.review_execution_id') = x.id
        AND json_extract(a.content, '$.verdict') = NEW.status
  ))
  OR (NEW.input_kind = 'validation_run' AND NOT EXISTS (
      SELECT 1 FROM validation_run v
      WHERE v.id = NEW.input_id AND v.task_id = NEW.task_id
        AND v.config_digest = json_extract(NEW.subject_json, '$.config_digest')
        AND v.workspace_id = json_extract(NEW.subject_json, '$.workspace_id')
        AND v.commit_sha = json_extract(NEW.subject_json, '$.commit_sha')
        AND v.workspace_snapshot_digest = json_extract(NEW.subject_json, '$.workspace_snapshot_digest')
        AND v.status = NEW.status
  ))
  OR (NEW.input_kind = 'evidence' AND NOT EXISTS (
      SELECT 1 FROM evidence e
      JOIN evidence_validation_run_producer p ON p.evidence_id = e.id AND p.task_id = e.task_id
      WHERE e.id = NEW.input_id AND e.task_id = NEW.task_id
        AND e.digest = NEW.input_digest AND p.validation_run_id = NEW.producer_ref
  ))
  OR (NEW.input_kind = 'decision' AND NOT EXISTS (
      SELECT 1 FROM decision d WHERE d.id = NEW.input_id AND d.task_id = NEW.task_id
        AND d.proposal_id = json_extract(NEW.subject_json, '$.proposal_id')
        AND d.proposal_version = NEW.input_version AND d.outcome = NEW.status
        AND d.policy_ref IS json_extract(NEW.subject_json, '$.policy_ref')
        AND d.policy_version IS json_extract(NEW.subject_json, '$.policy_version')
        AND d.policy_digest IS json_extract(NEW.subject_json, '$.policy_digest')
  ))
  OR (NEW.input_kind = 'work_unit' AND NOT EXISTS (
      SELECT 1 FROM work_unit w WHERE w.id = NEW.input_id AND w.task_id = NEW.task_id
        AND w.version = NEW.input_version AND w.status = NEW.status
  ))
  OR (NEW.input_kind = 'execution' AND NOT EXISTS (
      SELECT 1 FROM execution x WHERE x.id = NEW.input_id AND x.task_id = NEW.task_id
        AND x.work_unit_id IS json_extract(NEW.subject_json, '$.work_unit_id')
        AND x.status = NEW.status
  ))
  OR (NEW.input_kind = 'work_unit_integration' AND NOT EXISTS (
      SELECT 1 FROM work_unit_integration i WHERE i.id = NEW.input_id AND i.task_id = NEW.task_id
        AND i.work_unit_id = json_extract(NEW.subject_json, '$.work_unit_id')
        AND i.execution_id = NEW.producer_ref
        AND i.version = NEW.input_version AND i.outcome = NEW.status
  ))
  OR (NEW.input_kind = 'task_role_snapshot' AND NOT EXISTS (
      SELECT 1 FROM task_role r WHERE r.id = NEW.input_id AND r.task_id = NEW.task_id
        AND r.version = NEW.input_version
  ))
  OR (NEW.input_kind = 'merge_operation' AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation op
      WHERE op.id = NEW.input_id AND op.task_id = NEW.task_id
        AND op.kind = 'task_merge' AND op.version = NEW.input_version AND op.status = NEW.status
  ))
  OR (NEW.input_kind = 'lifecycle_operation' AND NOT EXISTS (
      SELECT 1 FROM task_lifecycle_transition tr
      WHERE tr.id = NEW.input_id AND tr.task_id = NEW.task_id
        AND tr.to_version = NEW.input_version
  ))
BEGIN
    SELECT RAISE(ABORT, 'Gate input is missing, stale, cross-Task, or has invalid provenance');
END;

CREATE TRIGGER review_execution_subject_execution_match_update
BEFORE UPDATE OF task_id, workspace_id, before_sha, after_sha, role ON execution
WHEN EXISTS (
    SELECT 1 FROM review_execution_subject s WHERE s.execution_id = OLD.id
) AND NOT EXISTS (
    SELECT 1 FROM review_execution_subject s
    WHERE s.execution_id = NEW.id AND s.task_id = NEW.task_id
      AND s.workspace_id = NEW.workspace_id
      AND s.base_commit_sha = NEW.before_sha
      AND s.head_commit_sha = NEW.after_sha
      AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = NEW.id AND capability.type = 'text' AND capability.value = 'formal_review')
)
BEGIN
    SELECT RAISE(ABORT, 'Review Execution identity cannot diverge from its frozen subject');
END;

CREATE TRIGGER review_execution_subject_guard_insert
BEFORE INSERT ON review_execution_subject
WHEN NOT EXISTS (
    SELECT 1 FROM execution e
    JOIN workspace w ON w.id = NEW.workspace_id AND w.task_id = NEW.task_id
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = e.id AND capability.type = 'text' AND capability.value = 'formal_review') AND e.status = 'running'
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
      AND e.workspace_id = NEW.workspace_id
      AND e.before_sha = NEW.base_commit_sha AND e.after_sha = NEW.head_commit_sha
      AND NOT EXISTS (
          SELECT 1 FROM execution_artifact_output o
          WHERE o.execution_id = e.id AND o.kind = 'review_report'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Review subject requires its exact Running reviewer Execution and Workspace');
END;

CREATE TRIGGER review_report_artifact_required_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'review_report' AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = NEW.kind AND o.digest = NEW.digest
      AND NEW.storage_kind = 'inline'
      AND json_valid(NEW.content) AND json_type(NEW.content) = 'object'
      AND json_extract(NEW.content, '$.kind') = 'review_report'
      AND json_extract(NEW.content, '$.verdict') IN ('pass', 'request_changes', 'questions')
      AND length(trim(COALESCE(json_extract(NEW.content, '$.summary'), ''))) > 0
      AND json_type(NEW.content, '$.criteria') = 'array'
      AND json_type(NEW.content, '$.findings') = 'array'
      AND json_type(NEW.content, '$.questions') = 'array'
      AND json_type(NEW.content, '$.evidence_considered') = 'array'
      AND json_extract(NEW.content, '$.subject.task_id') = e.task_id
      AND json_extract(NEW.content, '$.subject.review_execution_id') = e.id
      AND json_extract(NEW.content, '$.subject.workspace_id') IS e.workspace_id
      AND json_extract(NEW.content, '$.subject.base_commit_sha') IS e.before_sha
      AND json_extract(NEW.content, '$.subject.head_commit_sha') IS e.after_sha
      AND (
          (e.workspace_id IS NULL
           AND json_extract(NEW.content, '$.subject.workspace_snapshot_digest') IS NULL
           AND json_type(NEW.content, '$.subject.workspace_snapshot_digest') = 'null'
           AND NOT EXISTS (
               SELECT 1 FROM review_execution_subject s WHERE s.execution_id = e.id
           ))
          OR EXISTS (
              SELECT 1 FROM review_execution_subject s
              WHERE s.execution_id = e.id AND s.task_id = e.task_id
                AND s.workspace_id = e.workspace_id
                AND s.base_commit_sha = e.before_sha
                AND s.head_commit_sha = e.after_sha
                AND json_extract(NEW.content, '$.subject.workspace_snapshot_digest')
                    = s.workspace_snapshot_digest
          )
      )
      AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = e.id AND capability.type = 'text' AND capability.value = 'formal_review')
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
      AND NOT EXISTS (
          SELECT 1 FROM json_each(NEW.content, '$.evidence_considered') ref
          WHERE (
              (json_type(ref.value, '$.artifact_id') IS NOT NULL
               AND json_type(ref.value, '$.evidence_id') IS NOT NULL)
              OR (json_type(ref.value, '$.artifact_id') IS NULL
                  AND json_type(ref.value, '$.evidence_id') IS NULL)
              OR (json_type(ref.value, '$.artifact_id') IS NOT NULL AND NOT EXISTS (
                  SELECT 1 FROM execution_artifact_input i
                  JOIN artifact input ON input.id = i.artifact_id AND input.task_id = i.task_id
                  WHERE i.execution_id = e.id AND i.task_id = e.task_id
                    AND i.artifact_id = json_extract(ref.value, '$.artifact_id')
                    AND i.digest IS json_extract(ref.value, '$.digest')
                    AND input.kind = json_extract(ref.value, '$.kind')
              ))
              OR (json_type(ref.value, '$.evidence_id') IS NOT NULL AND NOT EXISTS (
                  SELECT 1 FROM execution_evidence_input i
                  JOIN evidence input ON input.id = i.evidence_id AND input.task_id = i.task_id
                  JOIN evidence_validation_run_producer p
                    ON p.evidence_id = input.id AND p.task_id = input.task_id
                  WHERE i.execution_id = e.id AND i.task_id = e.task_id
                    AND i.evidence_id = json_extract(ref.value, '$.evidence_id')
                    AND i.digest = json_extract(ref.value, '$.digest')
                    AND input.kind = json_extract(ref.value, '$.kind')
                    AND p.validation_run_id = json_extract(ref.value, '$.validation_run_id')
              ))
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'ReviewReport requires its exact attributed reviewer output binding');
END;

CREATE TRIGGER orchestrator_action_execution_guard
BEFORE INSERT ON orchestrator_action
WHEN NOT EXISTS (
    SELECT 1 FROM execution AS execution
    JOIN orchestrator_wake_execution AS attempt
      ON attempt.execution_id = execution.id
    JOIN orchestrator_wake AS wake ON wake.id = attempt.wake_id
    JOIN task_role AS role ON role.id = wake.task_role_id
    JOIN role_membership AS member
      ON member.task_role_id = wake.task_role_id
     AND member.actor_kind = wake.actor_kind
     AND member.actor_id = wake.actor_id
     AND member.status = 'active'
    WHERE execution.id = NEW.execution_id
      AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = execution.id AND capability.type = 'text' AND capability.value = 'orchestration')
      AND execution.status = 'completed'
      AND role.task_id = wake.task_id
      AND EXISTS (SELECT 1 FROM task_role_capability_configuration configuration JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE configuration.task_role_id = role.id AND capability.type = 'text' AND capability.value = 'orchestration')
      AND role.version = wake.task_role_version
      AND role.policy_json = wake.task_role_policy_json
      AND role.coordination_mode IS wake.coordination_mode
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action requires completed Execution and unchanged current TaskRole policy');
END;

CREATE TRIGGER orchestrator_execution_authority_guard
BEFORE INSERT ON execution
WHEN EXISTS (SELECT 1 FROM task_role role JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE role.task_id = NEW.task_id AND role.role = NEW.role AND capability.type = 'text' AND capability.value = 'orchestration')
  AND (
        NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake AS wake
            JOIN orchestrator_wake_execution AS attempt
              ON attempt.wake_id = wake.id AND attempt.execution_id = NEW.id
            JOIN task_role AS role ON role.id = wake.task_role_id
            JOIN role_membership AS member
              ON member.task_role_id = wake.task_role_id
             AND member.actor_kind = wake.actor_kind
             AND member.actor_id = wake.actor_id
             AND member.status = 'active'
            WHERE wake.task_id = NEW.task_id
              AND wake.actor_kind = NEW.actor_kind
              AND wake.actor_id = NEW.actor_id
              AND role.task_id = NEW.task_id
              AND EXISTS (SELECT 1 FROM task_role_capability_configuration configuration JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE configuration.task_role_id = role.id AND capability.type = 'text' AND capability.value = 'orchestration')
              AND role.version = wake.task_role_version
              AND role.policy_json = wake.task_role_policy_json
              AND role.coordination_mode IS wake.coordination_mode
              AND wake.state = 'leased'
              AND attempt.state = 'reserved'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrator Execution requires its exact leased wake, current TaskRole policy, active Actor, and reserved attempt');
END;

CREATE TRIGGER orchestrator_execution_authority_guard_update
BEFORE UPDATE OF task_id, actor_kind, actor_id, role, workspace_id ON execution
WHEN EXISTS (SELECT 1 FROM task_role role JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE role.task_id = NEW.task_id AND role.role = NEW.role AND capability.type = 'text' AND capability.value = 'orchestration')
  AND (
        NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake AS wake
            JOIN orchestrator_wake_execution AS attempt
              ON attempt.wake_id = wake.id AND attempt.execution_id = NEW.id
            JOIN task_role AS role ON role.id = wake.task_role_id
            JOIN role_membership AS member
              ON member.task_role_id = wake.task_role_id
             AND member.actor_kind = wake.actor_kind
             AND member.actor_id = wake.actor_id
             AND member.status = 'active'
            WHERE wake.task_id = NEW.task_id
              AND wake.actor_kind = NEW.actor_kind
              AND wake.actor_id = NEW.actor_id
              AND role.task_id = NEW.task_id
              AND EXISTS (SELECT 1 FROM task_role_capability_configuration configuration JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE configuration.task_role_id = role.id AND capability.type = 'text' AND capability.value = 'orchestration')
              AND role.version = wake.task_role_version
              AND role.policy_json = wake.task_role_policy_json
              AND role.coordination_mode IS wake.coordination_mode
              AND wake.state IN ('leased', 'running', 'uncertain')
              AND attempt.state IN ('reserved', 'start_requested', 'running', 'uncertain')
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrator Execution updates require their exact wake and current TaskRole policy');
END;

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
          AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = execution.id AND capability.type = 'text' AND capability.value = 'orchestration')
          AND execution.status = 'completed'
          AND execution.task_id = wake.task_id
          AND execution.actor_kind = wake.actor_kind
          AND execution.actor_id = wake.actor_id
          AND wake.current_attempt = attempt.attempt_number
          AND wake.state IN ('leased', 'running', 'uncertain')
          AND attempt.state IN ('start_requested', 'running', 'uncertain')
          AND role.task_id = wake.task_id
          AND EXISTS (SELECT 1 FROM task_role_capability_configuration configuration JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE configuration.task_role_id = role.id AND capability.type = 'text' AND capability.value = 'orchestration')
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

CREATE TRIGGER orchestrator_wake_authority_guard_insert
BEFORE INSERT ON orchestrator_wake
WHEN NOT EXISTS (
        SELECT 1 FROM domain_event AS event
        WHERE event.id = NEW.event_id AND event.sequence = NEW.event_sequence
          AND event.scope_type = 'task' AND event.scope_id = NEW.task_id
    )
  OR NOT EXISTS (
        SELECT 1 FROM task_role AS role
        WHERE role.id = NEW.task_role_id AND role.task_id = NEW.task_id
          AND EXISTS (SELECT 1 FROM task_role_capability_configuration configuration JOIN json_each(configuration.configuration_json, '$.capabilities') capability WHERE configuration.task_role_id = role.id AND capability.type = 'text' AND capability.value = 'orchestration')
          AND role.coordination_mode IS NEW.coordination_mode
          AND role.version = NEW.task_role_version
          AND role.policy_json = NEW.task_role_policy_json
    )
  OR NOT EXISTS (
        SELECT 1 FROM role_membership AS member
        WHERE member.task_role_id = NEW.task_role_id
          AND member.actor_kind = NEW.actor_kind AND member.actor_id = NEW.actor_id
          AND member.status = 'active'
    )
  OR (NEW.work_unit_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM work_unit AS unit
        WHERE unit.id = NEW.work_unit_id AND unit.task_id = NEW.task_id
    ))
BEGIN
    SELECT RAISE(ABORT, 'orchestrator wake requires exact durable cause, current TaskRole policy, active Actor, and same-Task WorkUnit');
END;


-- TaskRole policy, membership, and capability edits remain durable signals for
-- every currently configured orchestration role.
DROP TRIGGER pr6_orchestrator_task_role_changed_event;
DROP TRIGGER pr6_orchestrator_membership_inserted_event;
DROP TRIGGER pr6_orchestrator_membership_updated_event;

CREATE TRIGGER pr15_orchestrator_task_role_changed_event
AFTER UPDATE OF coordination_mode, policy_json ON task_role
WHEN (OLD.coordination_mode IS NOT NEW.coordination_mode
      OR OLD.policy_json IS NOT NEW.policy_json)
 AND EXISTS (
     SELECT 1 FROM task_role_capability_configuration configuration
     JOIN json_each(configuration.configuration_json, '$.capabilities') capability
       ON capability.type = 'text' AND capability.value = 'orchestration'
     WHERE configuration.task_role_id = NEW.id
 )
 AND EXISTS (
     SELECT 1 FROM role_membership member
     WHERE member.task_role_id = NEW.id AND member.status = 'active'
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', NEW.id, 'system', NULL,
        'task', NEW.task_id, NEW.id, NULL, 0,
        'pr15-task-role:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', NEW.id,
            'task_role_version', NEW.version,
            'coordination_mode', NEW.coordination_mode,
            'policy_changed', OLD.policy_json IS NOT NEW.policy_json,
            'actor_kind', NULL, 'actor_id', NULL, 'work_unit_id', NULL
        ),
        NEW.updated_at
    WHERE NEW.coordination_mode IS NOT 'partitioned';

    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', NEW.id, 'system', NULL,
        'task', NEW.task_id, NEW.id, NULL, 0,
        'pr15-task-role:' || NEW.id || ':' || NEW.version || ':' ||
            member.actor_kind || ':' || member.actor_id || ':' || unit.id,
        json_object(
            'task_role_id', NEW.id,
            'task_role_version', NEW.version,
            'coordination_mode', NEW.coordination_mode,
            'policy_changed', OLD.policy_json IS NOT NEW.policy_json,
            'actor_kind', member.actor_kind,
            'actor_id', member.actor_id,
            'work_unit_id', unit.id
        ),
        NEW.updated_at
    FROM role_membership member
    JOIN work_unit unit
      ON unit.task_id = NEW.task_id AND unit.role = NEW.role
     AND unit.assigned_actor_kind = member.actor_kind
     AND unit.assigned_actor_id = member.actor_id
    WHERE NEW.coordination_mode = 'partitioned'
      AND member.task_role_id = NEW.id AND member.status = 'active';
END;

CREATE TRIGGER pr15_orchestrator_task_role_capability_changed_event
AFTER UPDATE OF configuration_json ON task_role_capability_configuration
WHEN OLD.configuration_json IS NOT NEW.configuration_json
 AND EXISTS (
     SELECT 1 FROM json_each(NEW.configuration_json, '$.capabilities') capability
     WHERE capability.type = 'text' AND capability.value = 'orchestration'
 )
 AND EXISTS (
     SELECT 1 FROM role_membership member
     WHERE member.task_role_id = NEW.task_role_id AND member.status = 'active'
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', role.id, 'system', NULL,
        'task', role.task_id, role.id, NULL, 0,
        'pr15-task-role-capability:' || role.id || ':' || role.version,
        json_object(
            'task_role_id', role.id,
            'task_role_version', role.version,
            'coordination_mode', role.coordination_mode,
            'policy_changed', false,
            'actor_kind', NULL, 'actor_id', NULL, 'work_unit_id', NULL
        ),
        NEW.updated_at
    FROM task_role role WHERE role.id = NEW.task_role_id
      AND role.coordination_mode IS NOT 'partitioned';

    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', role.id, 'system', NULL,
        'task', role.task_id, role.id, NULL, 0,
        'pr15-task-role-capability:' || role.id || ':' || role.version || ':' ||
            member.actor_kind || ':' || member.actor_id || ':' || unit.id,
        json_object(
            'task_role_id', role.id,
            'task_role_version', role.version,
            'coordination_mode', role.coordination_mode,
            'policy_changed', false,
            'actor_kind', member.actor_kind,
            'actor_id', member.actor_id,
            'work_unit_id', unit.id
        ),
        NEW.updated_at
    FROM task_role role
    JOIN role_membership member ON member.task_role_id = role.id AND member.status = 'active'
    JOIN work_unit unit
      ON unit.task_id = role.task_id AND unit.role = role.role
     AND unit.assigned_actor_kind = member.actor_kind
     AND unit.assigned_actor_id = member.actor_id
    WHERE role.id = NEW.task_role_id AND role.coordination_mode = 'partitioned';
END;

CREATE TRIGGER pr15_orchestrator_membership_inserted_event
AFTER INSERT ON role_membership
WHEN NEW.status = 'active'
 AND EXISTS (
     SELECT 1 FROM task_role role
     JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id
     JOIN json_each(configuration.configuration_json, '$.capabilities') capability
       ON capability.type = 'text' AND capability.value = 'orchestration'
     WHERE role.id = NEW.task_role_id
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.membership_changed', 'role_membership', NEW.id, 'system', NULL,
        'task', role.task_id, NEW.id, NULL, 0,
        'pr15-membership:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', role.id, 'actor_kind', NEW.actor_kind,
            'actor_id', NEW.actor_id, 'status', NEW.status,
            'membership_version', NEW.version
        ),
        NEW.created_at
    FROM task_role role WHERE role.id = NEW.task_role_id;
END;

CREATE TRIGGER pr15_orchestrator_membership_updated_event
AFTER UPDATE OF status, actor_kind, actor_id, task_role_id ON role_membership
WHEN (OLD.status IS NOT NEW.status OR OLD.actor_kind IS NOT NEW.actor_kind
      OR OLD.actor_id IS NOT NEW.actor_id OR OLD.task_role_id IS NOT NEW.task_role_id)
 AND EXISTS (
     SELECT 1 FROM task_role role
     JOIN task_role_capability_configuration configuration ON configuration.task_role_id = role.id
     JOIN json_each(configuration.configuration_json, '$.capabilities') capability
       ON capability.type = 'text' AND capability.value = 'orchestration'
     WHERE role.id = NEW.task_role_id
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.membership_changed', 'role_membership', NEW.id, 'system', NULL,
        'task', role.task_id, NEW.id, NULL, 0,
        'pr15-membership:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', role.id, 'actor_kind', NEW.actor_kind,
            'actor_id', NEW.actor_id, 'status', NEW.status,
            'membership_version', NEW.version
        ),
        NEW.updated_at
    FROM task_role role WHERE role.id = NEW.task_role_id;
END;
