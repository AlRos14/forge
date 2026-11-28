PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;
CREATE TEMP TABLE _v185_check(ok INTEGER NOT NULL CHECK(ok = 1));

-- 1. Review independence. The result-subject branch of V162 is unchanged; range
-- review WorkUnits are governed by the dedicated triggers below.
DROP TRIGGER work_unit_review_independent_execution;
CREATE TRIGGER work_unit_review_independent_execution
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM work_unit_review_range r
                 WHERE r.review_work_unit_id = NEW.work_unit_id)
 AND (
     EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         WHERE s.review_work_unit_id = NEW.work_unit_id
     )
     OR EXISTS (
     SELECT 1 FROM work_unit review
     JOIN task_role role ON role.task_id = review.task_id AND role.role = review.role
     JOIN task_role_capability_configuration configuration
       ON configuration.task_role_id = role.id
     WHERE review.id = NEW.work_unit_id
       AND EXISTS (
           SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
           WHERE type = 'text' AND value = 'formal_review'
       )
     )
 )
 AND (
     NOT EXISTS (
         SELECT 1 FROM work_unit review
         JOIN task_role role ON role.task_id = review.task_id AND role.role = review.role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE review.id = NEW.work_unit_id
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
     )
     OR
     NOT EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN execution result ON result.id = s.result_execution_id
         WHERE s.review_work_unit_id = NEW.work_unit_id
           AND s.reviewed_work_unit_version IS NOT NULL
           AND result.task_id = s.task_id
           AND result.work_unit_id = s.reviewed_work_unit_id
           AND result.work_unit_version = s.reviewed_work_unit_version
           AND result.status = 'completed'
           AND (s.result_workspace_id IS NULL OR s.workspace_snapshot_digest IS NOT NULL)
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit reviewed ON reviewed.id = s.reviewed_work_unit_id
         WHERE s.review_work_unit_id = NEW.work_unit_id
           AND NEW.actor_kind = 'human'
           AND NEW.actor_kind = reviewed.assigned_actor_kind
           AND NEW.actor_id = reviewed.assigned_actor_id
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN execution contributor ON contributor.work_unit_id = s.reviewed_work_unit_id
         WHERE s.review_work_unit_id = NEW.work_unit_id
           AND NEW.actor_kind = 'human'
           AND NEW.actor_kind = contributor.actor_kind
           AND NEW.actor_id = contributor.actor_id
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit review ON review.id = s.review_work_unit_id
         JOIN task_role role ON role.task_id = review.task_id AND role.role = review.role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE s.reviewed_work_unit_id = NEW.work_unit_id
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
           AND NEW.actor_kind = 'human'
           AND review.assigned_actor_kind = NEW.actor_kind
           AND review.assigned_actor_id = NEW.actor_id
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution violates exact review independence or readiness');
END;

CREATE TRIGGER work_unit_range_review_independent_execution
BEFORE INSERT ON execution
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.work_unit_id)
 AND (
     NOT EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_checkpoint c ON c.id = r.checkpoint_id AND c.state IN ('pending', 'in_review')
         JOIN work_unit w ON w.id = r.review_work_unit_id AND w.task_id = NEW.task_id
         JOIN task_role role ON role.task_id = w.task_id AND role.role = w.role
         JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
         WHERE r.review_work_unit_id = NEW.work_unit_id
           AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                       WHERE type = 'text' AND value = 'formal_review'))
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_range_contributor x ON x.range_id = r.range_id
         WHERE r.review_work_unit_id = NEW.work_unit_id
           AND x.actor_kind = NEW.actor_kind AND x.actor_id = NEW.actor_id
           AND (NEW.actor_kind = 'human' OR NEW.harness_session_id IS NULL)))
BEGIN SELECT RAISE(ABORT, 'WorkUnit Execution violates exact range review independence or readiness'); END;

CREATE TRIGGER range_review_session_isolation_insert
BEFORE INSERT ON execution
WHEN NEW.actor_kind = 'agent' AND NEW.harness_session_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.harness_session_id
        UNION
        SELECT s.predecessor_session_id FROM harness_session s JOIN ancestors a ON a.id = s.id
        WHERE s.predecessor_session_id IS NOT NULL)
    SELECT 1 FROM work_unit_review_range r
    JOIN review_range_contributor x ON x.range_id = r.range_id
    JOIN execution produced ON produced.id = x.execution_id
    JOIN ancestors a ON a.id = produced.harness_session_id
    WHERE r.review_work_unit_id = NEW.work_unit_id)
BEGIN SELECT RAISE(ABORT, 'Range review requires a session without the producer context'); END;

CREATE TRIGGER range_review_session_isolation_update
BEFORE UPDATE OF harness_session_id ON execution
WHEN NEW.harness_session_id IS NOT OLD.harness_session_id
 AND NEW.actor_kind = 'agent' AND NEW.harness_session_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.harness_session_id
        UNION
        SELECT s.predecessor_session_id FROM harness_session s JOIN ancestors a ON a.id = s.id
        WHERE s.predecessor_session_id IS NOT NULL)
    SELECT 1 FROM work_unit_review_range r
    JOIN review_range_contributor x ON x.range_id = r.range_id
    JOIN execution produced ON produced.id = x.execution_id
    JOIN ancestors a ON a.id = produced.harness_session_id
    WHERE r.review_work_unit_id = NEW.work_unit_id)
BEGIN SELECT RAISE(ABORT, 'Range review requires a session without the producer context'); END;

CREATE TRIGGER work_unit_range_review_allocation
BEFORE UPDATE OF role, assigned_actor_kind, assigned_actor_id ON work_unit
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.id)
 AND (
     NOT EXISTS (
         SELECT 1 FROM task_role role
         JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
         WHERE role.task_id = NEW.task_id AND role.role = NEW.role
           AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                       WHERE type = 'text' AND value = 'formal_review'))
     OR (NEW.assigned_actor_kind = 'human' AND EXISTS (
         SELECT 1 FROM work_unit_review_range r
         JOIN review_range_contributor x ON x.range_id = r.range_id
         WHERE r.review_work_unit_id = NEW.id
           AND x.actor_kind = 'human' AND x.actor_id = NEW.assigned_actor_id)))
BEGIN SELECT RAISE(ABORT, 'WorkUnit allocation violates exact range review independence'); END;

-- 2. Reviewer workspace admission: a read-only WorkspaceGroup pinned at the
-- range head plus pinned context repositories.
CREATE TRIGGER work_unit_range_review_admission_guard
BEFORE INSERT ON execution
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.work_unit_id)
 AND (
     NEW.workspace_id IS NOT NULL
     OR NOT EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN review_range r ON r.id = rr.range_id
         JOIN workspace_scope s ON s.work_unit_id = rr.review_work_unit_id
           AND s.task_id = rr.task_id AND s.scope_kind = 'work_unit' AND s.repo_id = rr.repo_id
         JOIN workspace_group g ON g.id = s.workspace_group_id
           AND g.work_unit_id = rr.review_work_unit_id
         JOIN workspace w ON w.id = s.workspace_id AND w.status = 'ready'
           AND w.before_sha IS r.head_commit
         WHERE rr.review_work_unit_id = NEW.work_unit_id)
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN work_unit_repository_set_revision pin
           ON pin.work_unit_id = rr.review_work_unit_id AND pin.task_id = rr.task_id
         JOIN task_repository_set_revision_repo m
           ON m.task_id = pin.task_id AND m.revision = pin.revision AND m.role = 'context'
         WHERE rr.review_work_unit_id = NEW.work_unit_id AND NOT EXISTS (
             SELECT 1 FROM workspace_scope s
             JOIN workspace w ON w.id = s.workspace_id AND w.status = 'ready'
             JOIN task_repository_context_snapshot snap
               ON snap.task_id = rr.task_id AND snap.repo_id = m.repo_id
              AND snap.commit_sha IS w.before_sha
             WHERE s.work_unit_id = rr.review_work_unit_id AND s.scope_kind = 'work_unit'
               AND s.repo_id = m.repo_id))
     OR EXISTS (
         SELECT 1 FROM work_unit_review_range rr
         JOIN workspace_scope s ON s.work_unit_id = rr.review_work_unit_id
           AND s.scope_kind = 'work_unit'
         JOIN workspace w ON w.id = s.workspace_id
         JOIN work_unit_repository_set_revision pin
           ON pin.work_unit_id = rr.review_work_unit_id AND pin.task_id = rr.task_id
         LEFT JOIN task_repository_set_revision_repo m
           ON m.task_id = pin.task_id AND m.revision = pin.revision AND m.repo_id = s.repo_id
         WHERE rr.review_work_unit_id = NEW.work_unit_id
           AND (w.status != 'ready' OR m.repo_id IS NULL
                OR (s.repo_id != rr.repo_id AND m.role != 'context'))))
BEGIN SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_REVIEW_GROUP_UNAVAILABLE'); END;

CREATE TRIGGER work_unit_review_range_reviewer_role_immutable
BEFORE UPDATE OF role ON work_unit
WHEN EXISTS (SELECT 1 FROM work_unit_review_range r WHERE r.review_work_unit_id = NEW.id)
 AND NOT EXISTS (
     SELECT 1 FROM task_role role
     JOIN task_role_capability_configuration cfg ON cfg.task_role_id = role.id
     WHERE role.task_id = NEW.task_id AND role.role = NEW.role
       AND EXISTS (SELECT 1 FROM json_each(cfg.configuration_json, '$.capabilities')
                   WHERE type = 'text' AND value = 'formal_review'))
BEGIN SELECT RAISE(ABORT, 'review WorkUnit requires a TaskRole with formal_review capability'); END;

DROP TRIGGER workspace_scope_insert_guard;
CREATE TRIGGER workspace_scope_insert_guard
BEFORE INSERT ON workspace_scope
WHEN (NEW.scope_kind = 'integration' AND EXISTS (
          SELECT 1 FROM workspace_scope existing
          WHERE existing.task_id = NEW.task_id AND existing.repo_id = NEW.repo_id
            AND existing.scope_kind = 'integration'
      ))
  OR NOT EXISTS (
      SELECT 1 FROM workspace ws
      JOIN task t ON t.id = ws.task_id
      JOIN repo r ON r.id = ws.repo_id AND r.project_id = t.project_id
      WHERE ws.id = NEW.workspace_id AND ws.task_id = NEW.task_id
        AND ws.repo_id = NEW.repo_id
        AND (
            (NEW.scope_kind = 'integration' AND NEW.work_unit_id IS NULL
             AND NEW.workspace_group_id IS NULL AND EXISTS (
                 SELECT 1 FROM task_repository_set_revision revision
                 JOIN task_repository_set_revision_repo repository
                   ON repository.task_id = revision.task_id
                  AND repository.revision = revision.revision
                 WHERE revision.task_id = t.id AND revision.state = 'current'
                   AND repository.repo_id = ws.repo_id
                   AND repository.role IN ('primary', 'affected')
             ))
            OR (NEW.scope_kind = 'work_unit' AND NEW.work_unit_id IS NOT NULL
                AND NEW.workspace_group_id IS NOT NULL AND EXISTS (
                    SELECT 1 FROM work_unit w
                    JOIN work_unit_repository_set_revision pin
                      ON pin.work_unit_id = w.id AND pin.task_id = w.task_id
                    JOIN task_repository_set_revision_repo repository
                      ON repository.task_id = pin.task_id AND repository.revision = pin.revision
                     AND repository.repo_id = ws.repo_id
                    JOIN workspace_group g
                      ON g.id = NEW.workspace_group_id
                     AND g.task_id = w.task_id AND g.work_unit_id = w.id
                    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
                      AND (
                          (repository.role = 'context' AND w.requires_integration = 1
                           AND EXISTS (
                               SELECT 1 FROM task_repository_context_snapshot snapshot
                               WHERE snapshot.task_id = t.id AND snapshot.repo_id = ws.repo_id
                                 AND snapshot.commit_sha IS ws.before_sha
                           ))
                          OR (repository.role IN ('primary', 'affected')
                              AND w.requires_integration = 1
                              AND (NOT EXISTS (
                                  SELECT 1 FROM work_unit_repository selected
                                  WHERE selected.work_unit_id = w.id
                              ) OR EXISTS (
                                  SELECT 1 FROM work_unit_repository selected
                                  WHERE selected.work_unit_id = w.id
                                    AND selected.repo_id = ws.repo_id
                              )))
                          OR EXISTS (
                              SELECT 1 FROM work_unit_review_subject_repository subject
                              WHERE subject.review_work_unit_id = w.id
                                AND subject.task_id = t.id AND subject.repo_id = ws.repo_id
                                AND subject.result_execution_id IS NOT NULL
                                AND subject.result_commit_sha IS ws.before_sha
                          )
                          OR EXISTS (
                              SELECT 1 FROM work_unit_review_range rr
                              JOIN review_range range_row ON range_row.id = rr.range_id
                              WHERE rr.review_work_unit_id = w.id AND rr.task_id = t.id
                                AND ((range_row.repo_id = ws.repo_id
                                      AND repository.role IN ('primary', 'affected')
                                      AND range_row.head_commit IS ws.before_sha)
                                  OR (repository.role = 'context' AND EXISTS (
                                      SELECT 1 FROM task_repository_context_snapshot snapshot
                                      WHERE snapshot.task_id = t.id AND snapshot.repo_id = ws.repo_id
                                        AND snapshot.commit_sha IS ws.before_sha)))
                          )
                      )
                ))
        )
  )
  OR (NEW.scope_kind = 'work_unit' AND EXISTS (
      SELECT 1 FROM workspace current_workspace
      JOIN workspace_scope current_scope
        ON current_scope.workspace_id = current_workspace.id
       AND current_scope.task_id = current_workspace.task_id
      WHERE current_scope.work_unit_id = NEW.work_unit_id
        AND current_scope.scope_kind = 'work_unit'
        AND current_scope.repo_id = NEW.repo_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'Workspace scope is duplicate, cross-Task, cross-repository, or outside its exact pinned group');
END;

-- 3. Coordinator triage admits `defer` for range findings only.
CREATE TABLE task_review_triage_v185 (
    review_report_artifact_id TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    report_digest TEXT NOT NULL,
    execution_id TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    authorization_version INTEGER NOT NULL,
    disposition TEXT NOT NULL CHECK(disposition IN ('correct','recheck','human','defer')),
    rationale TEXT NOT NULL,
    instructions TEXT NOT NULL,
    decision_digest TEXT NOT NULL,
    review_work_unit_id TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL
);
INSERT INTO task_review_triage_v185 SELECT * FROM task_review_triage;
INSERT INTO _v185_check SELECT NOT EXISTS(
    SELECT * FROM task_review_triage EXCEPT SELECT * FROM task_review_triage_v185);
ALTER TABLE task_review_triage RENAME TO task_review_triage_v164;
ALTER TABLE task_review_triage_v185 RENAME TO task_review_triage;
DROP TABLE task_review_triage_v164;

CREATE TRIGGER task_review_triage_immutable
BEFORE UPDATE ON task_review_triage
BEGIN
    SELECT RAISE(ABORT, 'Review triage is immutable evidence, not a review verdict');
END;
CREATE TRIGGER task_review_triage_immutable_delete
BEFORE DELETE ON task_review_triage
WHEN NOT EXISTS(SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id=t.project_id WHERE t.id=OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'Review triage is preserved outside Project teardown');
END;
CREATE TRIGGER task_review_triage_current_request_guard
 BEFORE INSERT ON task_review_triage
 WHEN NOT EXISTS(
  SELECT 1 FROM task_review_coordination_request_current current
  JOIN task_coordination_request request ON request.id=current.request_id
  WHERE request.task_id=NEW.task_id AND request.execution_id=NEW.execution_id
   AND request.authorization_version=NEW.authorization_version
   AND request.review_report_artifact_id=NEW.review_report_artifact_id
   AND request.review_report_digest=NEW.report_digest AND request.state IN ('running','uncertain'))
 BEGIN SELECT RAISE(ABORT,'FORGE_COORDINATION_REVIEW_SOURCE_STALE'); END;

CREATE TRIGGER task_review_triage_defer_range_only
BEFORE INSERT ON task_review_triage
WHEN NEW.disposition = 'defer' AND NOT EXISTS (
    SELECT 1 FROM review_range_report p WHERE p.report_artifact_id = NEW.review_report_artifact_id)
BEGIN SELECT RAISE(ABORT, 'defer is only valid for range review findings'); END;

CREATE TRIGGER micro_fix_receipt_range_review_guard
BEFORE INSERT ON micro_fix_receipt
WHEN NEW.source_kind = 'review_findings' AND EXISTS (
    SELECT 1 FROM task_coordination_request q
    JOIN review_range_report p ON p.report_artifact_id = q.review_report_artifact_id
    WHERE q.id = NEW.request_id)
BEGIN SELECT RAISE(ABORT, 'FORGE_MICRO_FIX_NOT_AUTHORIZED'); END;

-- 4. Coordination source: the V168 result branch plus the range branch.
DROP VIEW task_review_coordination_source_current;
CREATE VIEW task_review_coordination_source_current AS
 SELECT report.id AS report_id,report.task_id,report.digest AS report_digest,
        producer.execution_id AS producer_execution_id,c.authorization_version
 FROM artifact report
 JOIN artifact_execution_producer producer ON producer.artifact_id=report.id AND producer.task_id=report.task_id
 JOIN execution reviewer ON reviewer.id=producer.execution_id AND reviewer.task_id=report.task_id
 JOIN work_unit_review_subject subject ON subject.review_work_unit_id=reviewer.work_unit_id AND subject.task_id=report.task_id
 JOIN task_rework_automation_intent intent ON intent.review_report_artifact_id=report.id
  AND intent.task_id=report.task_id AND intent.source_execution_id=reviewer.id
  AND intent.reviewed_work_unit_id=subject.reviewed_work_unit_id
  AND intent.reviewed_work_unit_version=subject.reviewed_work_unit_version AND intent.state='pending'
 JOIN task_coordination c ON c.task_id=report.task_id
 JOIN task t ON t.id=c.task_id AND t.deleted_at IS NULL
 JOIN task_lifecycle lifecycle ON lifecycle.task_id=t.id AND lifecycle.state NOT IN ('done','cancelled','merging')
 JOIN task_dispatch dispatch ON dispatch.task_id=t.id AND dispatch.state='running'
 WHERE report.kind='review_report' AND reviewer.status='completed'
  AND json_extract(report.content,'$.verdict')='request_changes'
  AND c.mode!='manual' AND c.revoked_at IS NULL AND c.team_stale=0
  AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0)=1
  AND NOT EXISTS(SELECT 1 FROM task_review_triage triage WHERE triage.review_report_artifact_id=report.id)
  AND NOT EXISTS(SELECT 1 FROM work_unit_review_line_revision revision
      JOIN work_unit_review_line line ON line.id=revision.line_id
      WHERE revision.review_report_artifact_id=report.id AND line.latest_report_artifact_id IS NOT report.id)
UNION ALL
SELECT report.id, report.task_id, report.digest, producer.execution_id, c.authorization_version
 FROM artifact report
 JOIN artifact_execution_producer producer
   ON producer.artifact_id = report.id AND producer.task_id = report.task_id
 JOIN execution reviewer
   ON reviewer.id = producer.execution_id AND reviewer.task_id = report.task_id
 JOIN review_range_report rr ON rr.report_artifact_id = report.id AND rr.task_id = report.task_id
  AND rr.execution_id = reviewer.id AND rr.report_digest = report.digest
  AND rr.verdict = 'request_changes'
 JOIN review_checkpoint cp ON cp.id = rr.checkpoint_id AND cp.state = 'reviewed'
 JOIN task_coordination c ON c.task_id = report.task_id
 JOIN task t ON t.id = c.task_id AND t.deleted_at IS NULL
 JOIN task_lifecycle lifecycle ON lifecycle.task_id = t.id
  AND lifecycle.state NOT IN ('done','cancelled','merging')
 JOIN task_dispatch dispatch ON dispatch.task_id = t.id AND dispatch.state = 'running'
 WHERE report.kind = 'review_report' AND reviewer.status = 'completed'
  AND c.mode != 'manual' AND c.revoked_at IS NULL AND c.team_stale = 0
  AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0) = 1
  AND NOT EXISTS(SELECT 1 FROM task_review_triage x WHERE x.review_report_artifact_id = report.id)
  AND NOT EXISTS(SELECT 1 FROM review_rework_round x WHERE x.report_artifact_id = report.id)
  AND NOT EXISTS(SELECT 1 FROM review_checkpoint newer WHERE newer.range_id = cp.range_id
       AND newer.attempt > cp.attempt AND newer.state = 'reviewed');

-- 5. Proposals may target review findings, bases and commits.
DROP TRIGGER proposal_actor_target_guard_insert;
CREATE TRIGGER proposal_actor_target_guard_insert
BEFORE INSERT ON proposal
WHEN NEW.target_kind NOT IN ('task', 'execution', 'workspace', 'work_unit', 'pr_external_action',
                             'review_finding', 'review_base', 'review_commit')
  OR (NEW.proposer_actor_kind = 'human' AND NOT EXISTS (
          SELECT 1 FROM user u WHERE u.id = NEW.proposer_actor_id
      ))
  OR (NEW.proposer_actor_kind = 'agent' AND NOT EXISTS (
          SELECT 1 FROM agent_identity ai WHERE ai.id = NEW.proposer_actor_id
      ))
  OR (NEW.target_kind = 'task' AND NEW.target_id != NEW.task_id)
  OR (NEW.target_kind = 'execution' AND NOT EXISTS (
          SELECT 1 FROM execution e WHERE e.id = NEW.target_id AND e.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'workspace' AND NOT EXISTS (
          SELECT 1 FROM workspace w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'work_unit' AND NOT EXISTS (
          SELECT 1 FROM work_unit w WHERE w.id = NEW.target_id AND w.task_id = NEW.task_id
      ))
  OR (NEW.target_kind = 'pr_external_action' AND NOT EXISTS (
          SELECT 1 FROM pr_external_action a
          WHERE a.id = NEW.target_id AND a.task_id = NEW.task_id AND a.state = 'proposed'
      ))
  OR (NEW.target_kind = 'review_finding' AND NOT EXISTS (
          SELECT 1 FROM review_finding f
          WHERE f.id = NEW.target_id AND f.task_id = NEW.task_id AND f.severity != 'P0'))
  OR (NEW.target_kind IN ('review_base', 'review_commit') AND NOT EXISTS (
          SELECT 1 FROM task_review_coverage_state s
          WHERE s.task_id = NEW.task_id AND s.repo_id = NEW.target_id))
  OR (NEW.supersedes_proposal_id IS NOT NULL AND NOT EXISTS (
          SELECT 1 FROM proposal p
          WHERE p.id = NEW.supersedes_proposal_id
            AND p.task_id = NEW.task_id AND p.status = 'superseded'
      ))
BEGIN
    SELECT RAISE(ABORT, 'Proposal ActorRef, target, or supersedes scope is invalid');
END;

-- 6. Gate inputs admit `review_coverage`; every historical row is preserved.
DROP VIEW gate_evaluation_stale_input;
DROP TRIGGER gate_evaluation_input_same_task_insert;
DROP TRIGGER gate_evaluation_input_immutable_update;
DROP TRIGGER gate_evaluation_input_immutable_delete;
DROP TRIGGER gate_evaluation_input_review_line_guard;
DROP INDEX idx_gate_evaluation_input_ref;

CREATE TABLE gate_evaluation_input_v185 (
    evaluation_id   TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
    input_kind      TEXT NOT NULL CHECK (input_kind IN (
                        'review_report', 'validation_run', 'evidence', 'decision',
                        'work_unit', 'execution', 'work_unit_integration',
                        'task_role_snapshot', 'merge_operation', 'lifecycle_operation',
                        'review_line', 'review_coverage'
                    )),
    input_id        TEXT NOT NULL CHECK (length(trim(input_id)) > 0),
    input_version   INTEGER NOT NULL CHECK (input_version >= 1),
    input_digest    TEXT NOT NULL CHECK (length(input_digest) = 64),
    producer_ref    TEXT,
    subject_json    TEXT NOT NULL CHECK (json_valid(subject_json) AND json_type(subject_json) = 'object'),
    status          TEXT NOT NULL CHECK (length(trim(status)) BETWEEN 1 AND 64),
    PRIMARY KEY(evaluation_id, ordinal),
    UNIQUE(evaluation_id, input_kind, input_id),
    FOREIGN KEY (evaluation_id, task_id)
        REFERENCES gate_evaluation(id, task_id) ON DELETE CASCADE
);

INSERT INTO gate_evaluation_input_v185 (
    evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
    input_digest, producer_ref, subject_json, status
)
SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
       input_digest, producer_ref, subject_json, status
FROM gate_evaluation_input;
INSERT INTO _v185_check SELECT NOT EXISTS(
    SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
           input_digest, producer_ref, subject_json, status FROM gate_evaluation_input
    EXCEPT
    SELECT evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
           input_digest, producer_ref, subject_json, status FROM gate_evaluation_input_v185);

ALTER TABLE gate_evaluation_input RENAME TO gate_evaluation_input_v155;
ALTER TABLE gate_evaluation_input_v185 RENAME TO gate_evaluation_input;
DROP TABLE gate_evaluation_input_v155;
CREATE INDEX idx_gate_evaluation_input_ref
    ON gate_evaluation_input(input_kind, input_id, task_id);

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
        AND EXISTS (
            SELECT 1 FROM execution_task_role_authority authority
            JOIN json_each(authority.capabilities_json) capability
              ON capability.type = 'text' AND capability.value = 'formal_review'
            WHERE authority.execution_id = x.id
        ) AND x.status = 'completed'
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
  OR (NEW.input_kind = 'review_coverage' AND NOT EXISTS (
      SELECT 1 FROM task_review_coverage_state s
      JOIN task_review_policy_revision p ON p.task_id = s.task_id AND p.state = 'current'
      WHERE s.task_id = NEW.task_id AND s.repo_id = NEW.input_id
        AND s.revision = NEW.input_version
        AND p.revision = json_extract(NEW.subject_json, '$.review_policy_revision')
        AND p.digest = json_extract(NEW.subject_json, '$.review_policy_digest')
        AND NEW.input_digest = json_extract(NEW.subject_json, '$.snapshot_digest')))
BEGIN
    SELECT RAISE(ABORT, 'Gate input is missing, stale, cross-Task, or has invalid provenance');
END;

CREATE TRIGGER gate_evaluation_input_immutable_update
BEFORE UPDATE ON gate_evaluation_input
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluation inputs are immutable');
END;

CREATE TRIGGER gate_evaluation_input_immutable_delete
BEFORE DELETE ON gate_evaluation_input
WHEN NOT EXISTS (
    SELECT 1 FROM gate_evaluation e
    JOIN task t ON t.id = e.task_id
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE e.id = OLD.evaluation_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluation inputs are immutable outside Project teardown');
END;

CREATE TRIGGER gate_evaluation_input_review_line_guard
BEFORE INSERT ON gate_evaluation_input
WHEN NEW.input_kind = 'review_line' AND NOT EXISTS (
    SELECT 1 FROM work_unit_review_line line
    WHERE line.id = NEW.input_id AND line.task_id = NEW.task_id
      AND line.revision = NEW.input_version
      AND COALESCE(line.latest_verdict, 'pending') IS NEW.status
      AND NEW.input_digest = json_extract(NEW.subject_json, '$.line_digest')
      AND json_extract(NEW.subject_json, '$.revision') = line.revision
      AND json_extract(NEW.subject_json, '$.current_work_unit_id') = line.current_work_unit_id
      AND json_extract(NEW.subject_json, '$.current_result_execution_id') IS line.current_result_execution_id
      AND json_extract(NEW.subject_json, '$.latest_report_artifact_id') IS line.latest_report_artifact_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate review-line input must pin its exact current revision and verdict');
END;

-- 7. Currentness: the V169 view plus the coverage branch.
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
          AND EXISTS (SELECT 1 FROM execution_task_role_authority authority JOIN json_each(authority.capabilities_json) capability WHERE authority.execution_id = x.id AND capability.type = 'text' AND capability.value = 'formal_review') AND x.status = 'completed'
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
    ))
 OR (i.input_kind = 'review_coverage' AND NOT EXISTS (
        SELECT 1 FROM task_review_coverage_state s
        JOIN task_review_policy_revision p ON p.task_id = s.task_id AND p.state = 'current'
        WHERE s.task_id = i.task_id AND s.repo_id = i.input_id AND s.revision = i.input_version
          AND p.revision = json_extract(i.subject_json, '$.review_policy_revision')
          AND p.digest = json_extract(i.subject_json, '$.review_policy_digest')
    ));

-- 8. Fail closed: a Task with a current review policy admits merge readiness only
-- through a v4 Gate carrying that exact policy and satisfied coverage inputs.
CREATE VIEW gate_evaluation_review_coverage_missing AS
SELECT e.id AS evaluation_id
FROM gate_evaluation e
JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id AND g.gate_kind = 'merge_readiness'
JOIN task_review_policy_revision p ON p.task_id = e.task_id AND p.state = 'current'
JOIN gate_policy_revision r ON r.gate_id = e.gate_id AND r.revision = e.policy_revision
WHERE r.schema_version != 4
   OR json_extract(r.policy_json, '$.review_coverage.review_policy_revision') IS NOT p.revision
   OR json_extract(r.policy_json, '$.review_coverage.review_policy_digest') IS NOT p.digest
   OR EXISTS (SELECT 1 FROM gate_evaluation_input i
        WHERE i.evaluation_id = e.id AND i.input_kind = 'review_coverage'
          AND i.status NOT IN ('satisfied', 'not_required', 'unchanged'))
   OR EXISTS (SELECT 1 FROM work_unit_repository_integration w
        JOIN task_repository_set_revision v ON v.task_id = w.task_id AND v.state = 'current'
        JOIN task_repository_set_revision_repo m ON m.task_id = v.task_id
          AND m.revision = v.revision AND m.repo_id = w.repo_id
          AND m.role IN ('primary', 'affected')
        WHERE w.task_id = e.task_id AND w.outcome = 'success' AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i WHERE i.evaluation_id = e.id
              AND i.input_kind = 'review_coverage' AND i.input_id = w.repo_id));

CREATE TRIGGER review_coverage_lifecycle_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.to_state = 'ready_to_merge' AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;

CREATE TRIGGER review_coverage_integration_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind IN ('task_merge', 'publish_pr') AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;

CREATE TRIGGER review_coverage_push_update_guard
BEFORE INSERT ON pr_external_action
WHEN NEW.kind = 'push_update' AND EXISTS (
    SELECT 1 FROM gate_evaluation_review_coverage_missing x
    WHERE x.evaluation_id = NEW.gate_evaluation_id)
BEGIN SELECT RAISE(ABORT, 'gate.review_coverage_missing'); END;

INSERT INTO _v185_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _v185_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';
DROP TABLE _v185_check;
INSERT INTO _migration(version, name, applied_at)
VALUES (185, 'review_range_authority', strftime('%Y-%m-%dT%H:%M:%fZ','now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
