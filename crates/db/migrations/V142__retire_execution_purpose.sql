-- Execution purpose is retired as a runtime/public classification. The exact
-- historical values remain available as migration provenance; new Executions
-- are identified by their exact Actor, Role, WorkUnit, subject, and outputs.
--
-- Purpose-based authorization is replaced by the same checks over durable
-- records: formal Review is the reviewer Role plus its exact ReviewReport
-- output, and orchestration is the orchestrator Role bound to its exact wake.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _execution_purpose_expected (legacy_rows INTEGER NOT NULL);
INSERT INTO _execution_purpose_expected
SELECT COUNT(*) FROM execution WHERE purpose IS NOT NULL;

CREATE TABLE pr13_execution_purpose_history (
    execution_id          TEXT PRIMARY KEY,
    task_id               TEXT NOT NULL,
    actor_kind            TEXT,
    actor_id              TEXT,
    role                  TEXT NOT NULL,
    purpose               TEXT NOT NULL,
    work_unit_id          TEXT,
    work_unit_version     INTEGER,
    workspace_id          TEXT,
    execution_created_at  TEXT NOT NULL,
    execution_updated_at  TEXT NOT NULL,
    captured_at           TEXT NOT NULL
);

INSERT INTO pr13_execution_purpose_history (
    execution_id, task_id, actor_kind, actor_id, role, purpose,
    work_unit_id, work_unit_version, workspace_id,
    execution_created_at, execution_updated_at, captured_at
)
SELECT id, task_id, actor_kind, actor_id, role, purpose,
       work_unit_id, work_unit_version, workspace_id,
       created_at, updated_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM execution
WHERE purpose IS NOT NULL;

CREATE TRIGGER pr13_execution_purpose_history_no_update
BEFORE UPDATE ON pr13_execution_purpose_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution purpose provenance is immutable');
END;
CREATE TRIGGER pr13_execution_purpose_history_no_insert
BEFORE INSERT ON pr13_execution_purpose_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution purpose provenance is migration-only');
END;
CREATE TRIGGER pr13_execution_purpose_history_no_delete
BEFORE DELETE ON pr13_execution_purpose_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 execution purpose provenance is immutable');
END;

DROP TRIGGER pr9_gate_current_inputs_lifecycle_guard;
DROP TRIGGER pr9_task_merge_current_inputs_guard;
DROP VIEW gate_evaluation_currentness;
DROP VIEW gate_evaluation_stale_input;
DROP TRIGGER completed_review_execution_requires_report;
DROP TRIGGER execution_actor_role_purpose_immutable;
DROP TRIGGER execution_artifact_output_guard_insert;
DROP TRIGGER execution_artifact_output_review_guard_insert;
DROP TRIGGER execution_purpose_guard_insert;
DROP TRIGGER execution_purpose_guard_update;
DROP TRIGGER gate_evaluation_input_same_task_insert;
DROP TRIGGER orchestrator_action_execution_guard;
DROP TRIGGER orchestrator_execution_authority_guard;
DROP TRIGGER orchestrator_execution_authority_guard_update;
DROP TRIGGER plan_artifact_output_required_insert;
DROP TRIGGER pr6_orchestrator_action_effect_authority_guard;
DROP TRIGGER review_execution_subject_execution_match_update;
DROP TRIGGER review_execution_subject_guard_insert;
DROP TRIGGER review_report_artifact_required_insert;
DROP TRIGGER reviewer_execution_purpose_guard_insert;
DROP TRIGGER reviewer_execution_purpose_guard_update;
DROP TRIGGER work_unit_execution_admission_guard;
DROP TRIGGER work_unit_execution_binding_immutable;
DROP INDEX idx_execution_purpose;
ALTER TABLE execution DROP COLUMN purpose;

CREATE VIEW gate_evaluation_stale_input AS
SELECT i.evaluation_id, i.ordinal
FROM gate_evaluation_input AS i
WHERE
    (i.input_kind = 'review_report' AND NOT EXISTS (
        SELECT 1
        FROM artifact AS a
        JOIN artifact_execution_producer AS p
          ON p.artifact_id = a.id AND p.task_id = a.task_id
        JOIN execution AS x ON x.id = p.execution_id AND x.task_id = p.task_id
        WHERE a.id = i.input_id AND a.task_id = i.task_id
          AND a.kind = 'review_report' AND a.digest = i.input_digest
          AND p.execution_id = i.producer_ref
          AND x.role = 'reviewer' AND x.status = 'completed'
          AND json_extract(a.content, '$.verdict') = i.status
    ))
 OR (i.input_kind = 'validation_run' AND NOT EXISTS (
        SELECT 1 FROM validation_run AS v
        WHERE v.id = i.input_id AND v.task_id = i.task_id
          AND v.status = i.status
          AND v.check_identity = json_extract(i.subject_json, '$.check_identity')
          AND v.config_digest = json_extract(i.subject_json, '$.config_digest')
          AND v.workspace_id = json_extract(i.subject_json, '$.workspace_id')
          AND v.commit_sha = json_extract(i.subject_json, '$.commit_sha')
          AND v.workspace_snapshot_digest = json_extract(i.subject_json, '$.workspace_snapshot_digest')
    ))
 OR (i.input_kind = 'evidence' AND NOT EXISTS (
        SELECT 1 FROM evidence AS e
        JOIN evidence_validation_run_producer AS p
          ON p.evidence_id = e.id AND p.task_id = e.task_id
        WHERE e.id = i.input_id AND e.task_id = i.task_id
          AND e.digest = i.input_digest AND p.validation_run_id = i.producer_ref
    ))
 OR (i.input_kind = 'decision' AND NOT EXISTS (
        SELECT 1
        FROM decision AS d
        JOIN proposal AS p ON p.id = d.proposal_id AND p.task_id = d.task_id
        WHERE d.id = i.input_id AND d.task_id = i.task_id
          AND d.proposal_version = i.input_version AND d.outcome = i.status
          AND p.content_version = d.proposal_version AND p.status = 'resolved'
          AND d.proposal_id = json_extract(i.subject_json, '$.proposal_id')
          AND d.policy_ref IS json_extract(i.subject_json, '$.policy_ref')
          AND d.policy_version IS json_extract(i.subject_json, '$.policy_version')
          AND d.policy_digest IS json_extract(i.subject_json, '$.policy_digest')
    ))
 OR (i.input_kind = 'work_unit' AND NOT EXISTS (
        SELECT 1 FROM work_unit AS w
        WHERE w.id = i.input_id AND w.task_id = i.task_id
          AND w.version = i.input_version AND w.status = i.status
          AND (SELECT COUNT(*) FROM json_each(i.subject_json, '$.dependencies')) =
              (SELECT COUNT(*) FROM work_unit_dependency d
               WHERE d.task_id = i.task_id AND d.work_unit_id = i.input_id)
          AND NOT EXISTS (
              SELECT 1 FROM json_each(i.subject_json, '$.dependencies') expected
              WHERE NOT EXISTS (
                  SELECT 1
                  FROM work_unit_dependency d
                  JOIN work_unit prerequisite
                    ON prerequisite.id = d.depends_on_work_unit_id
                   AND prerequisite.task_id = d.task_id
                  WHERE d.task_id = i.task_id AND d.work_unit_id = i.input_id
                    AND d.work_unit_id = json_extract(expected.value, '$.work_unit_id')
                    AND d.depends_on_work_unit_id =
                        json_extract(expected.value, '$.depends_on_work_unit_id')
                    AND CASE WHEN prerequisite.status = 'completed'
                                  AND (prerequisite.requires_integration = 0 OR EXISTS (
                                      SELECT 1 FROM work_unit_integration integration
                                      WHERE integration.work_unit_id = prerequisite.id
                                        AND integration.task_id = prerequisite.task_id
                                        AND integration.outcome = 'success'
                                  ))
                             THEN 1 ELSE 0 END =
                        json_extract(expected.value, '$.satisfied')
              )
          )
    ))
 OR (i.input_kind = 'execution' AND NOT EXISTS (
        SELECT 1 FROM execution AS x
        WHERE x.id = i.input_id AND x.task_id = i.task_id
          AND x.status = i.status
          AND x.work_unit_id IS json_extract(i.subject_json, '$.work_unit_id')
          AND x.work_unit_version IS json_extract(i.subject_json, '$.work_unit_version')
          AND x.after_sha IS json_extract(i.subject_json, '$.result_sha')
    ))
 OR (i.input_kind = 'work_unit_integration' AND NOT EXISTS (
        SELECT 1 FROM work_unit_integration AS integration
        WHERE integration.id = i.input_id AND integration.task_id = i.task_id
          AND integration.version = i.input_version AND integration.outcome = i.status
          AND integration.work_unit_id = json_extract(i.subject_json, '$.work_unit_id')
          AND integration.execution_id = i.producer_ref
          AND integration.source_sha = json_extract(i.subject_json, '$.source_sha')
          AND integration.target_before_sha IS json_extract(i.subject_json, '$.target_before_sha')
          AND integration.target_workspace_id = json_extract(i.subject_json, '$.workspace_id')
          AND integration.target_after_sha IS json_extract(i.subject_json, '$.commit_sha')
    ))
 OR (i.input_kind = 'task_role_snapshot' AND NOT EXISTS (
        SELECT 1 FROM task_role AS r
        WHERE r.id = i.input_id AND r.task_id = i.task_id AND r.version = i.input_version
    ))
 OR (i.input_kind = 'merge_operation' AND NOT EXISTS (
        SELECT 1 FROM task_integration_operation AS op
        WHERE op.id = i.input_id AND op.task_id = i.task_id
          AND op.kind = json_extract(i.subject_json, '$.kind')
          AND op.owner_id = json_extract(i.subject_json, '$.owner_id')
          AND op.version = i.input_version AND op.status = i.status
          AND op.gate_evaluation_id IS json_extract(i.subject_json, '$.gate_evaluation_id')
    ))
 OR (i.input_kind = 'lifecycle_operation' AND NOT EXISTS (
        SELECT 1 FROM task_lifecycle_transition AS tr
        WHERE tr.id = i.input_id AND tr.task_id = i.task_id
          AND tr.from_state = json_extract(i.subject_json, '$.from_state')
          AND tr.to_state = i.status
          AND tr.from_version = json_extract(i.subject_json, '$.from_version')
          AND tr.to_version = i.input_version
          AND tr.cause_kind = json_extract(i.subject_json, '$.cause_kind')
          AND tr.cause_ref IS json_extract(i.subject_json, '$.cause_ref')
          AND tr.gate_evaluation_id IS json_extract(i.subject_json, '$.gate_evaluation_id')
    ));

CREATE VIEW gate_evaluation_currentness AS
SELECT e.id AS evaluation_id,
       NOT EXISTS (
           SELECT 1 FROM gate_evaluation_stale_input stale
           WHERE stale.evaluation_id = e.id
       ) AS inputs_current
FROM gate_evaluation AS e;

CREATE TRIGGER pr9_gate_current_inputs_lifecycle_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.gate_evaluation_id IS NOT NULL
 AND NOT EXISTS (
     SELECT 1 FROM gate_evaluation_currentness currentness
     WHERE currentness.evaluation_id = NEW.gate_evaluation_id
       AND currentness.inputs_current = 1
 )
BEGIN
    SELECT RAISE(ABORT, 'GateEvaluation inputs changed before lifecycle effect');
END;

CREATE TRIGGER pr9_task_merge_current_inputs_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind IN ('task_merge', 'publish_pr')
 AND NOT EXISTS (
     SELECT 1 FROM gate_evaluation_currentness currentness
     WHERE currentness.evaluation_id = NEW.gate_evaluation_id
       AND currentness.inputs_current = 1
 )
BEGIN
    SELECT RAISE(ABORT, 'merge admission requires current exact GateEvaluation inputs');
END;

CREATE TRIGGER completed_review_execution_requires_report
BEFORE UPDATE OF status ON execution
WHEN OLD.status = 'running' AND NEW.status = 'completed'
 AND NEW.role = 'reviewer'
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

CREATE TRIGGER execution_actor_role_immutable
BEFORE UPDATE OF actor_kind, actor_id, role ON execution
WHEN OLD.actor_kind IS NOT NEW.actor_kind
  OR OLD.actor_id IS NOT NEW.actor_id
  OR OLD.role IS NOT NEW.role
BEGIN
    SELECT RAISE(ABORT, 'Execution Actor and role are immutable');
END;

CREATE TRIGGER execution_artifact_output_guard_insert
BEFORE INSERT ON execution_artifact_output
WHEN (NEW.kind = 'plan' AND NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
)) OR EXISTS (
    SELECT 1 FROM artifact a
    WHERE a.id = NEW.artifact_id
      AND (a.task_id != NEW.task_id OR a.kind != NEW.kind OR a.digest IS NOT NEW.digest)
)
BEGIN
    SELECT RAISE(ABORT, 'Execution output Artifact kind, digest, or Task is invalid');
END;

CREATE TRIGGER execution_artifact_output_review_guard_insert
BEFORE INSERT ON execution_artifact_output
WHEN NEW.kind = 'review_report' AND NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND e.role = 'reviewer'
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
        AND x.role = 'reviewer' AND x.status = 'completed'
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
      AND execution.role = 'orchestrator'
      AND execution.status = 'completed'
      AND role.task_id = wake.task_id
      AND role.role = 'orchestrator'
      AND role.version = wake.task_role_version
      AND role.policy_json = wake.task_role_policy_json
      AND role.coordination_mode IS wake.coordination_mode
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action requires completed Execution and unchanged current TaskRole policy');
END;

CREATE TRIGGER orchestrator_execution_authority_guard
BEFORE INSERT ON execution
WHEN NEW.role = 'orchestrator'
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
              AND role.role = 'orchestrator'
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
WHEN NEW.role = 'orchestrator'
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
              AND role.role = 'orchestrator'
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

CREATE TRIGGER plan_artifact_output_required_insert
BEFORE INSERT ON artifact
WHEN NEW.kind = 'plan' AND NOT EXISTS (
    SELECT 1 FROM execution_artifact_output o
    JOIN artifact_execution_producer p
      ON p.artifact_id = o.artifact_id
     AND p.execution_id = o.execution_id
     AND p.task_id = o.task_id
    JOIN execution e ON e.id = p.execution_id AND e.task_id = p.task_id
    WHERE o.artifact_id = NEW.id AND o.task_id = NEW.task_id
      AND o.kind = NEW.kind AND o.digest IS NEW.digest
      AND e.actor_kind IN ('human', 'agent') AND e.actor_id IS NOT NULL
      AND ((e.actor_kind = 'human' AND EXISTS (SELECT 1 FROM user u WHERE u.id = e.actor_id))
        OR (e.actor_kind = 'agent' AND EXISTS (SELECT 1 FROM agent_identity ai WHERE ai.id = e.actor_id)))
)
BEGIN
    SELECT RAISE(ABORT, 'Plan Artifact requires its exact attributed Execution output binding');
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
          AND execution.role = 'orchestrator'
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
      AND NEW.role = 'reviewer'
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
      AND e.role = 'reviewer' AND e.status = 'running'
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
      AND e.role = 'reviewer'
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

CREATE TRIGGER work_unit_execution_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NEW.status != 'running'
          OR NEW.work_unit_version IS NULL
          OR NEW.actor_kind IS NULL OR NEW.actor_id IS NULL
          OR (NEW.actor_kind != 'agent' AND NEW.agent_id IS NOT NULL)
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_INVALID')

        WHEN NEW.actor_kind = 'agent' AND NOT EXISTS (
            SELECT 1 FROM agent_current a
            WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_AGENT_MISSING')

        WHEN NOT EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORK_UNIT_MISSING')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.status != 'open'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORK_UNIT_NOT_OPEN')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.version != NEW.work_unit_version
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_VERSION_CONFLICT')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.requires_integration = 1
        ) AND (NEW.actor_kind != 'agent' OR NEW.agent_id != NEW.actor_id)
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_INVALID')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND (w.role != NEW.role OR
                   (w.assigned_actor_id IS NOT NULL AND
                    (w.assigned_actor_kind != NEW.actor_kind
                     OR w.assigned_actor_id != NEW.actor_id)))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_VERSION_CONFLICT')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND ((w.requires_integration = 1 AND NOT EXISTS (
                    SELECT 1 FROM workspace_scope ws
                    JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
                    WHERE ws.workspace_id = NEW.workspace_id
                      AND ws.task_id = NEW.task_id
                      AND ws.scope_kind = 'work_unit'
                      AND ws.work_unit_id = w.id
                      AND bound_workspace.status = 'ready'
                      AND bound_workspace.repo_id = (
                          SELECT t.repo_id FROM task t WHERE t.id = w.task_id
                      )
              )) OR (w.requires_integration = 0 AND NEW.workspace_id IS NOT NULL))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORKSPACE_UNAVAILABLE')

        WHEN NOT EXISTS (
            SELECT 1 FROM work_unit w
            JOIN task_role tr ON tr.task_id = w.task_id AND tr.role = w.role
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND rm.actor_kind = NEW.actor_kind
              AND rm.actor_id = NEW.actor_id
              AND rm.status = 'active'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_ROLE_MEMBERSHIP_INACTIVE')

        WHEN EXISTS (
            SELECT 1 FROM execution active
            WHERE active.work_unit_id = NEW.work_unit_id
              AND active.status = 'running'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_ALREADY_RUNNING')

        WHEN EXISTS (
            SELECT 1 FROM work_unit_dependency d
            JOIN work_unit p
              ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
            WHERE d.work_unit_id = NEW.work_unit_id AND d.task_id = NEW.task_id
              AND (p.status != 'completed' OR
                   (p.requires_integration = 1 AND NOT EXISTS (
                       SELECT 1 FROM work_unit_integration i
                       WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                         AND i.outcome = 'success'
                   )))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_DEPENDENCY_UNSATISFIED')

        WHEN NEW.actor_kind = 'agent' AND EXISTS (
            SELECT 1 FROM agent_current a
            WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
              AND (SELECT COUNT(*) FROM execution e
                   WHERE e.agent_id = a.id AND e.status = 'running')
                  >= a.max_concurrent_tasks
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_AGENT_AT_CAPACITY')
    END;
END;

CREATE TRIGGER work_unit_execution_binding_immutable
BEFORE UPDATE OF task_id, actor_kind, actor_id, role, work_unit_id,
                 work_unit_version, workspace_id ON execution
WHEN OLD.work_unit_id IS NOT NULL AND (
    NEW.task_id IS NOT OLD.task_id OR NEW.actor_kind IS NOT OLD.actor_kind
    OR NEW.actor_id IS NOT OLD.actor_id OR NEW.role IS NOT OLD.role
    OR NEW.work_unit_id IS NOT OLD.work_unit_id
    OR NEW.work_unit_version IS NOT OLD.work_unit_version
    OR NEW.workspace_id IS NOT OLD.workspace_id
)
AND NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution historical binding is immutable');
END;

CREATE TEMP TABLE _execution_purpose_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _execution_purpose_check
SELECT (SELECT COUNT(*) FROM pr13_execution_purpose_history)
    = (SELECT legacy_rows FROM _execution_purpose_expected);
INSERT INTO _execution_purpose_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('execution') WHERE name = 'purpose'
);
INSERT INTO _execution_purpose_check
SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master
    WHERE sql LIKE '%purpose%' AND type IN ('trigger', 'index', 'view')
      AND name NOT LIKE 'pr13_execution_purpose%'
);
INSERT INTO _execution_purpose_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE _execution_purpose_check;
DROP TABLE _execution_purpose_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (142, 'retire_execution_purpose', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
