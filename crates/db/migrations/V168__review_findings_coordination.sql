-- Exact review-findings continuation over the existing coordination envelope.
-- Rebuild only the constrained tables, preserving their complete historical rows.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;
CREATE TEMP TABLE _v168_check(ok INTEGER NOT NULL CHECK(ok=1));
CREATE TABLE task_coordination_request_v168 (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    authorization_version   INTEGER NOT NULL CHECK (authorization_version >= 1),
    mode                    TEXT NOT NULL CHECK (mode IN ('proposal', 'prepare_and_execute')),
    brief_json              TEXT NOT NULL
                                CHECK (json_valid(brief_json)
                                       AND json_type(brief_json) = 'object'),
    objective_digest        TEXT NOT NULL CHECK (length(objective_digest) = 64),
    request_digest          TEXT NOT NULL CHECK (length(request_digest) = 64),
    idempotency_key         TEXT NOT NULL
                                CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    state                   TEXT NOT NULL
                                CHECK (state IN ('requested', 'running', 'plan_ready',
                                                 'awaiting_approval', 'materialized',
                                                 'failed', 'cancelled', 'uncertain', 'triaged')),
    wake_id                 TEXT REFERENCES orchestrator_wake(id) ON DELETE RESTRICT,
    execution_id            TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    plan_artifact_id        TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    failure_code            TEXT,
    failure_message         TEXT,
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    completed_at            TEXT,
    kind TEXT NOT NULL DEFAULT 'work_unit_plan' CHECK(kind IN ('work_unit_plan','review_findings')),
    review_report_artifact_id TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    review_report_digest TEXT,
    review_producer_execution_id TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    CHECK((kind='work_unit_plan' AND review_report_artifact_id IS NULL
        AND review_report_digest IS NULL AND review_producer_execution_id IS NULL AND state!='triaged')
      OR (kind='review_findings' AND review_report_artifact_id IS NOT NULL
        AND review_report_digest IS NOT NULL AND length(review_report_digest)=64
        AND review_producer_execution_id IS NOT NULL AND plan_artifact_id IS NULL
        AND state NOT IN ('plan_ready','awaiting_approval','materialized'))),
    UNIQUE(task_id, idempotency_key)
);
CREATE TABLE project_plan_batch_coordination_request_v168 (
 request_id TEXT PRIMARY KEY,
 project_id TEXT NOT NULL,
 batch_id TEXT NOT NULL,
 task_id TEXT NOT NULL,
 authorization_version INTEGER NOT NULL CHECK(authorization_version >= 1),
 authorized_by TEXT NOT NULL,
 expected_batch_version INTEGER NOT NULL CHECK(expected_batch_version >= 1),
 expected_graph_version INTEGER NOT NULL CHECK(expected_graph_version >= 1),
 expected_task_version INTEGER NOT NULL CHECK(expected_task_version >= 1),
 coordination_snapshot_digest TEXT NOT NULL CHECK(length(coordination_snapshot_digest)=64),
 origin_kind TEXT NOT NULL CHECK(origin_kind IN ('scheduler','quality_replan','human_replan','review_triage')),
 claim_token TEXT,
 source_artifact_id TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
 producer_execution_id TEXT REFERENCES execution(id) ON DELETE RESTRICT,
 approved_plan_artifact_id TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
 created_at TEXT NOT NULL,
 FOREIGN KEY(request_id) REFERENCES task_coordination_request(id)
  ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
 FOREIGN KEY(batch_id,task_id,authorization_version)
  REFERENCES project_plan_batch_coordination_authorization(batch_id,task_id,authorization_version)
  ON DELETE CASCADE,
 CHECK((origin_kind='scheduler' AND claim_token IS NOT NULL
        AND source_artifact_id IS NULL AND producer_execution_id IS NULL
        AND approved_plan_artifact_id IS NULL)
    OR (origin_kind IN ('quality_replan','review_triage') AND claim_token IS NULL
        AND source_artifact_id IS NOT NULL AND producer_execution_id IS NOT NULL
        AND approved_plan_artifact_id IS NULL)
    OR (origin_kind='human_replan' AND claim_token IS NULL
        AND source_artifact_id IS NULL AND producer_execution_id IS NULL
        AND approved_plan_artifact_id IS NOT NULL))
);

INSERT INTO task_coordination_request_v168 SELECT *, 'work_unit_plan',NULL,NULL,NULL FROM task_coordination_request;
INSERT INTO _v168_check SELECT NOT EXISTS(SELECT * FROM task_coordination_request EXCEPT
 SELECT id,task_id,authorization_version,mode,brief_json,objective_digest,request_digest,idempotency_key,state,
 wake_id,execution_id,plan_artifact_id,failure_code,failure_message,version,created_at,updated_at,completed_at
 FROM task_coordination_request_v168);
INSERT INTO project_plan_batch_coordination_request_v168 SELECT * FROM project_plan_batch_coordination_request;
INSERT INTO _v168_check SELECT NOT EXISTS(SELECT * FROM project_plan_batch_coordination_request EXCEPT
 SELECT * FROM project_plan_batch_coordination_request_v168);
DROP TRIGGER project_plan_incomplete_coordination_request_guard;
DROP TRIGGER project_plan_batch_coordination_request_immutable_update;
DROP TRIGGER project_plan_batch_coordination_request_immutable_delete;
DROP TRIGGER project_plan_batch_coordination_request_scope_guard;
DROP TRIGGER task_coordination_request_project_batch_scope_guard;
DROP TRIGGER task_coordination_request_project_batch_scope_match;
DROP TRIGGER task_coordination_request_batch_execution_attach;
DROP TRIGGER task_coordination_request_batch_request_task_match;

ALTER TABLE task_coordination_request RENAME TO task_coordination_request_v158;
ALTER TABLE task_coordination_request_v168 RENAME TO task_coordination_request;
DROP TABLE task_coordination_request_v158;
ALTER TABLE project_plan_batch_coordination_request RENAME TO project_plan_batch_coordination_request_v167;
ALTER TABLE project_plan_batch_coordination_request_v168 RENAME TO project_plan_batch_coordination_request;
DROP TABLE project_plan_batch_coordination_request_v167;

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
      WHERE revision.review_report_artifact_id=report.id AND line.latest_report_artifact_id IS NOT report.id);

-- Current effect authority is separate from immutable admission versions:
-- pause/resume changes batch state/version, while preserving its same grant.
CREATE VIEW task_review_coordination_request_current AS
 SELECT request.id AS request_id
 FROM task_coordination_request request
 JOIN task_review_coordination_source_current source
   ON source.report_id=request.review_report_artifact_id
    AND source.task_id=request.task_id AND source.report_digest=request.review_report_digest
    AND source.producer_execution_id=request.review_producer_execution_id
    AND source.authorization_version=request.authorization_version
 JOIN task_coordination c ON c.task_id=request.task_id
 WHERE request.kind='review_findings' AND request.mode=c.mode
  AND request.state IN ('requested','running','uncertain')
 AND (
  (NOT EXISTS(SELECT 1 FROM project_plan_batch_coordination_request scope WHERE scope.request_id=request.id)
   AND NOT EXISTS(SELECT 1 FROM project_plan_batch_coordination_authorization a
                  WHERE a.task_id=request.task_id AND a.authorization_version=request.authorization_version)
   AND NOT EXISTS(SELECT 1 FROM project_plan_batch_task bt JOIN project_plan_batch b ON b.id=bt.batch_id
                  WHERE bt.task_id=request.task_id AND b.state IN ('running','paused')))
  OR EXISTS(
   SELECT 1 FROM project_plan_batch_coordination_request scope
   JOIN project_plan_batch b ON b.id=scope.batch_id AND b.project_id=scope.project_id
   JOIN project_plan p ON p.project_id=b.project_id AND p.version=b.graph_version
   JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=request.task_id
   JOIN task t ON t.id=request.task_id AND t.project_id=b.project_id AND t.deleted_at IS NULL
   JOIN project_plan_batch_coordination_authorization a
     ON a.batch_id=b.id AND a.task_id=t.id AND a.authorization_version=c.authorization_version
   JOIN project_member member ON member.project_id=b.project_id AND member.user_id=b.authorized_by
     AND member.role IN ('owner','admin')
   JOIN project project ON project.id=b.project_id
   JOIN project_member owner_member ON owner_member.project_id=project.id
     AND owner_member.user_id=project.owner_id AND owner_member.role='owner'
   JOIN project_plan_batch_execution producer ON producer.execution_id=source.producer_execution_id
     AND producer.batch_id=b.id AND producer.task_id=t.id
     AND producer.authorization_version=c.authorization_version
   WHERE scope.request_id=request.id AND scope.task_id=t.id
    AND scope.authorization_version=request.authorization_version
    AND scope.origin_kind='review_triage' AND scope.source_artifact_id=source.report_id
    AND scope.producer_execution_id=source.producer_execution_id
    AND scope.authorized_by=b.authorized_by AND b.state='running' AND b.preparation_complete=1
    AND scope.expected_graph_version=b.graph_version AND bt.state='started'
    AND scope.coordination_snapshot_digest=bt.coordination_snapshot_digest
    AND c.mode='prepare_and_execute' AND c.requested_by=b.authorized_by
    AND c.team_generation=a.team_generation AND c.team_digest=a.team_digest
    AND c.envelope_digest=a.envelope_digest AND c.coordinator_task_role_id=a.role_id
    AND c.coordinator_actor_kind=a.actor_kind AND c.coordinator_actor_id=a.actor_id
    AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
    AND a.expected_graph_version=b.graph_version AND a.mode=c.mode
    AND a.task_title=t.title AND a.task_description IS t.description
    AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
  )
 );
CREATE INDEX idx_task_coordination_request_task_created
    ON task_coordination_request(task_id, created_at DESC, id DESC);
CREATE INDEX idx_task_coordination_request_wake
    ON task_coordination_request(wake_id);
CREATE INDEX idx_task_coordination_request_execution
    ON task_coordination_request(execution_id);
CREATE INDEX idx_task_coordination_request_state
    ON task_coordination_request(task_id, state, created_at);
CREATE INDEX project_plan_batch_coordination_request_batch
 ON project_plan_batch_coordination_request(batch_id,task_id,created_at,request_id);
CREATE TRIGGER project_plan_incomplete_coordination_request_guard BEFORE INSERT ON task_coordination_request
WHEN EXISTS(
 SELECT 1 FROM project_plan_import_task i
 WHERE i.task_id=NEW.task_id AND NOT EXISTS(
  SELECT 1 FROM project_plan_proposal_task pt JOIN project_plan_proposal p ON p.id=pt.proposal_id
  WHERE pt.project_id=i.project_id AND pt.task_id=i.task_id AND pt.import_key=i.import_key
   AND p.state='applied' AND p.applied_version IS NOT NULL
   AND i.import_key='proposal:'||p.id||':node:'||pt.node_key
 )
)
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_APPLY_INCOMPLETE');
END;
CREATE TRIGGER project_plan_batch_coordination_request_immutable_update
 BEFORE UPDATE ON project_plan_batch_coordination_request BEGIN
 SELECT RAISE(ABORT,'Project batch coordination request origin is immutable');
END;
CREATE TRIGGER project_plan_batch_coordination_request_immutable_delete
 BEFORE DELETE ON project_plan_batch_coordination_request
 WHEN NOT EXISTS(SELECT 1 FROM project p JOIN project_deletion_guard g ON g.project_id=p.id
                WHERE p.id=OLD.project_id) BEGIN
 SELECT RAISE(ABORT,'Project batch coordination request history is immutable');
END;
CREATE TRIGGER project_plan_batch_coordination_request_scope_guard
 BEFORE INSERT ON project_plan_batch_coordination_request BEGIN
 SELECT CASE WHEN NOT EXISTS(
  SELECT 1 FROM project_plan_batch b
  JOIN project_plan p ON p.project_id=b.project_id
  JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=NEW.task_id
  JOIN task t ON t.id=NEW.task_id AND t.project_id=b.project_id AND t.deleted_at IS NULL
  JOIN task_coordination c ON c.task_id=t.id
  JOIN project_plan_batch_coordination_authorization a
   ON a.batch_id=b.id AND a.task_id=t.id AND a.authorization_version=NEW.authorization_version
  JOIN project_member m ON m.project_id=b.project_id AND m.user_id=NEW.authorized_by
   AND m.role IN ('owner','admin')
  JOIN project owner_project ON owner_project.id=b.project_id
  JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
   AND owner_membership.user_id=owner_project.owner_id AND owner_membership.role='owner'
  WHERE b.id=NEW.batch_id AND b.project_id=NEW.project_id
   AND b.authorized_by=NEW.authorized_by AND b.state='running'
   AND b.preparation_complete=1 AND b.version=NEW.expected_batch_version
   AND b.graph_version=NEW.expected_graph_version AND p.version=b.graph_version
   AND bt.coordination_snapshot_json IS NOT NULL
   AND bt.coordination_snapshot_digest=NEW.coordination_snapshot_digest
   AND t.version=NEW.expected_task_version
   AND c.authorization_version=NEW.authorization_version
   AND c.mode='prepare_and_execute' AND c.requested_by=NEW.authorized_by
   AND c.revoked_at IS NULL AND c.team_stale=0
   AND c.team_generation=a.team_generation AND c.team_digest=a.team_digest
   AND c.envelope_digest=a.envelope_digest
   AND c.coordinator_task_role_id=a.role_id AND c.coordinator_actor_kind=a.actor_kind
   AND c.coordinator_actor_id=a.actor_id
   AND a.project_id=b.project_id AND a.authorized_by=NEW.authorized_by
   AND a.expected_graph_version=b.graph_version
   AND a.mode=c.mode AND a.role_id=c.coordinator_task_role_id
   AND a.actor_kind=c.coordinator_actor_kind AND a.actor_id=c.coordinator_actor_id
   AND a.task_title=t.title AND a.task_description IS t.description
   AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
   AND ((NEW.origin_kind='scheduler' AND bt.state='claimed'
         AND bt.claim_token=NEW.claim_token AND bt.claim_expires_at IS NOT NULL
         AND bt.claim_expires_at>=strftime('%Y-%m-%dT%H:%M:%fZ','now'))
    OR (NEW.origin_kind IN ('quality_replan','human_replan','review_triage') AND bt.state='started'
        AND bt.coordination_request_id IS NOT NULL))
   AND (NEW.origin_kind!='quality_replan' OR EXISTS(
       SELECT 1 FROM artifact_execution_producer producer
       JOIN project_plan_batch_execution exec_link
        ON exec_link.execution_id=producer.execution_id AND exec_link.batch_id=b.id
         AND exec_link.task_id=t.id AND exec_link.authorization_version=NEW.authorization_version
       WHERE producer.artifact_id=NEW.source_artifact_id
        AND producer.execution_id=NEW.producer_execution_id AND producer.task_id=t.id))
   AND (NEW.origin_kind!='review_triage' OR EXISTS(
       SELECT 1 FROM task_review_coordination_source_current source
       JOIN project_plan_batch_execution link
         ON link.execution_id=source.producer_execution_id AND link.batch_id=b.id
          AND link.task_id=t.id AND link.authorization_version=c.authorization_version
       WHERE source.report_id=NEW.source_artifact_id
        AND source.producer_execution_id=NEW.producer_execution_id
        AND source.task_id=t.id AND source.authorization_version=NEW.authorization_version))
   AND (NEW.origin_kind!='human_replan' OR EXISTS(
       SELECT 1 FROM task_coordination_plan plan
       JOIN project_plan_batch_coordination_request previous
        ON previous.request_id=plan.request_id AND previous.batch_id=b.id
         AND previous.task_id=t.id
         AND previous.authorization_version=NEW.authorization_version
       JOIN artifact plan_artifact
        ON plan_artifact.id=plan.artifact_id AND plan_artifact.task_id=plan.task_id
         AND plan_artifact.digest=plan.plan_digest
       WHERE plan.artifact_id=NEW.approved_plan_artifact_id AND plan.task_id=t.id
        AND plan.authorization_version=NEW.authorization_version
        AND ((plan.state='approved' AND NOT EXISTS(
              SELECT 1 FROM task_coordination_materialization receipt
              WHERE receipt.plan_artifact_id=plan.artifact_id))
          OR (plan.state='materialized' AND EXISTS(
              SELECT 1 FROM task_coordination_materialization receipt
              WHERE receipt.plan_artifact_id=plan.artifact_id
                AND receipt.task_id=plan.task_id
                AND receipt.request_id=plan.request_id
                AND receipt.plan_digest=plan.plan_digest
                AND receipt.authorization_version=plan.authorization_version)))))
  ) THEN RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE') END;
END;
CREATE TRIGGER task_coordination_request_project_batch_scope_guard
 BEFORE INSERT ON task_coordination_request
 WHEN (EXISTS(SELECT 1 FROM project_plan_batch_coordination_authorization a
               WHERE a.task_id=NEW.task_id
                AND a.authorization_version=NEW.authorization_version)
    OR EXISTS(SELECT 1 FROM project_plan_batch_task bt
              JOIN project_plan_batch b ON b.id=bt.batch_id
              WHERE bt.task_id=NEW.task_id AND b.state IN ('running','paused')))
  AND NOT EXISTS(SELECT 1 FROM project_plan_batch_coordination_request scope
                 WHERE scope.request_id=NEW.id AND scope.task_id=NEW.task_id
                  AND scope.authorization_version=NEW.authorization_version)
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE');
END;
CREATE TRIGGER task_coordination_request_project_batch_scope_match
 AFTER INSERT ON task_coordination_request
 WHEN EXISTS(SELECT 1 FROM project_plan_batch_coordination_request scope
             WHERE scope.request_id=NEW.id AND (scope.task_id!=NEW.task_id
              OR scope.authorization_version!=NEW.authorization_version))
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE');
END;
CREATE TRIGGER task_coordination_request_batch_execution_attach
 AFTER UPDATE OF execution_id ON task_coordination_request
 WHEN NEW.execution_id IS NOT NULL AND OLD.execution_id IS NOT NEW.execution_id
BEGIN
 INSERT OR IGNORE INTO project_plan_batch_execution(execution_id,batch_id,task_id,authorization_version)
 SELECT NEW.execution_id,scope.batch_id,scope.task_id,scope.authorization_version
 FROM project_plan_batch_coordination_request scope WHERE scope.request_id=NEW.id;
END;
CREATE TRIGGER task_coordination_request_batch_request_task_match
 BEFORE INSERT ON project_plan_batch_coordination_request
 WHEN EXISTS(SELECT 1 FROM task_coordination_request request
             WHERE request.id=NEW.request_id AND (request.task_id!=NEW.task_id
              OR request.authorization_version!=NEW.authorization_version))
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE');
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
CREATE TRIGGER task_coordination_request_identity_immutable
 BEFORE UPDATE ON task_coordination_request
 WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.authorization_version IS NOT OLD.authorization_version OR NEW.mode IS NOT OLD.mode
  OR NEW.brief_json IS NOT OLD.brief_json OR NEW.objective_digest IS NOT OLD.objective_digest
  OR NEW.request_digest IS NOT OLD.request_digest OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.kind IS NOT OLD.kind OR NEW.review_report_artifact_id IS NOT OLD.review_report_artifact_id
  OR NEW.review_report_digest IS NOT OLD.review_report_digest
  OR NEW.review_producer_execution_id IS NOT OLD.review_producer_execution_id
  OR NEW.created_at IS NOT OLD.created_at
 BEGIN SELECT RAISE(ABORT,'Coordination request source and identity are immutable'); END;
CREATE TRIGGER task_coordination_request_review_source_guard
 BEFORE INSERT ON task_coordination_request WHEN NEW.kind='review_findings'
 BEGIN
  SELECT CASE WHEN NEW.state!='requested' OR NOT EXISTS(SELECT 1 FROM task_review_coordination_source_current source
    WHERE source.report_id=NEW.review_report_artifact_id AND source.task_id=NEW.task_id
     AND source.report_digest=NEW.review_report_digest
     AND source.producer_execution_id=NEW.review_producer_execution_id
     AND source.authorization_version=NEW.authorization_version)
   THEN RAISE(ABORT,'FORGE_COORDINATION_REVIEW_SOURCE_STALE') END;
 END;
CREATE TRIGGER task_coordination_request_origin_kind_guard
 AFTER INSERT ON task_coordination_request
 WHEN EXISTS(SELECT 1 FROM project_plan_batch_coordination_request source WHERE source.request_id=NEW.id
  AND ((NEW.kind='review_findings' AND (source.origin_kind!='review_triage'
    OR source.source_artifact_id IS NOT NEW.review_report_artifact_id
    OR source.producer_execution_id IS NOT NEW.review_producer_execution_id))
    OR (NEW.kind='work_unit_plan' AND source.origin_kind='review_triage')))
 BEGIN SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE'); END;
CREATE TRIGGER task_coordination_plan_request_kind_guard
 BEFORE INSERT ON task_coordination_plan
 WHEN EXISTS(SELECT 1 FROM task_coordination_request request WHERE request.id=NEW.request_id AND request.kind!='work_unit_plan')
 BEGIN SELECT RAISE(ABORT,'Review findings cannot materialize a WorkUnit plan'); END;
CREATE TRIGGER project_plan_batch_review_request_kind_guard
 BEFORE INSERT ON project_plan_batch_coordination_request
 WHEN EXISTS(SELECT 1 FROM task_coordination_request request WHERE request.id=NEW.request_id
  AND ((request.kind='review_findings' AND (NEW.origin_kind!='review_triage'
    OR NEW.source_artifact_id IS NOT request.review_report_artifact_id
    OR NEW.producer_execution_id IS NOT request.review_producer_execution_id))
    OR (request.kind='work_unit_plan' AND NEW.origin_kind='review_triage')))
 BEGIN SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE'); END;
CREATE TRIGGER task_coordination_request_triaged_guard
 BEFORE UPDATE OF state ON task_coordination_request WHEN NEW.state='triaged'
 BEGIN
  SELECT CASE WHEN NEW.kind!='review_findings' OR NOT EXISTS(
    SELECT 1 FROM task_review_triage triage WHERE triage.task_id=NEW.task_id
     AND triage.review_report_artifact_id=NEW.review_report_artifact_id
     AND triage.report_digest=NEW.review_report_digest AND triage.execution_id=NEW.execution_id
     AND triage.authorization_version=NEW.authorization_version)
   THEN RAISE(ABORT,'FORGE_COORDINATION_REVIEW_DECISION_STALE') END;
 END;
CREATE UNIQUE INDEX idx_task_coordination_exact_review_source
 ON task_coordination_request(task_id,authorization_version,review_report_artifact_id)
 WHERE kind='review_findings';

INSERT INTO _v168_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _v168_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1)='ok';
DROP TABLE _v168_check;
INSERT INTO _migration(version,name,applied_at) VALUES(168,'review_findings_coordination',strftime('%Y-%m-%dT%H:%M:%fZ','now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
