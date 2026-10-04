-- Gate evaluations pin exact immutable facts and exact snapshots of mutable
-- facts. Recheck the latter at the SQLite effect boundary, not only in a
-- service preflight that can race a concurrent write.

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
          AND x.role = 'reviewer' AND x.purpose = 'review' AND x.status = 'completed'
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

CREATE TRIGGER pr9_ready_to_merge_demotion_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.from_state = 'ready_to_merge' AND NEW.to_state = 'active'
 AND NEW.cause_kind != 'gate_evaluation'
BEGIN
    SELECT RAISE(ABORT, 'ready_to_merge can return to active only from a new exact GateEvaluation');
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
