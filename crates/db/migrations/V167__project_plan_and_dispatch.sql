-- Optional project planning. TaskLifecycle and task_dependency remain canonical.
CREATE TABLE project_plan (
 project_id TEXT PRIMARY KEY REFERENCES project(id) ON DELETE CASCADE,
 version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
 document_json TEXT NOT NULL DEFAULT '{"nodes":[],"edges":[],"sources":[]}' CHECK(json_valid(document_json)),
 updated_at TEXT NOT NULL
);
INSERT INTO project_plan(project_id, updated_at) SELECT id, updated_at FROM project;
CREATE TRIGGER project_plan_project_insert AFTER INSERT ON project BEGIN
 INSERT INTO project_plan(project_id,updated_at) VALUES(NEW.id,NEW.updated_at);
END;
CREATE TABLE project_plan_proposal (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 base_version INTEGER NOT NULL, document_json TEXT NOT NULL CHECK(json_valid(document_json)),
 digest TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('proposed','applied')),
 applied_version INTEGER, requested_by TEXT NOT NULL, idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL, UNIQUE(project_id,idempotency_key)
);
CREATE TABLE project_plan_import_task (
 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 import_key TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
 PRIMARY KEY(project_id,import_key), UNIQUE(project_id,task_id)
);
CREATE TABLE project_plan_proposal_task (
 proposal_id TEXT NOT NULL REFERENCES project_plan_proposal(id) ON DELETE CASCADE,
 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 node_key TEXT NOT NULL, task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
 import_key TEXT,
 created_at TEXT NOT NULL,
 PRIMARY KEY(proposal_id,node_key), UNIQUE(proposal_id,task_id)
);
-- Suppress a Task-node version bump only for the exact import key and Task ID
-- while TaskService creates that Task in the same write transaction.
CREATE TABLE project_plan_import_in_progress (
 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 task_id TEXT NOT NULL,
 import_key TEXT NOT NULL,
 PRIMARY KEY(project_id,task_id),
 UNIQUE(project_id,import_key)
);
CREATE TRIGGER project_plan_task_insert_version AFTER INSERT ON task
WHEN NOT EXISTS(SELECT 1 FROM project_plan_import_in_progress WHERE project_id=NEW.project_id AND task_id=NEW.id)
BEGIN
 UPDATE project_plan SET version=version+1,updated_at=NEW.updated_at WHERE project_id=NEW.project_id;
END;
CREATE TRIGGER project_plan_task_visibility_version AFTER UPDATE OF deleted_at ON task
WHEN OLD.deleted_at IS NOT NEW.deleted_at
BEGIN
 UPDATE project_plan SET version=version+1,updated_at=NEW.updated_at WHERE project_id=NEW.project_id;
END;
-- An imported Task is not part of the applied graph until the final apply
-- transaction writes its exact proposal/node mapping. A crash between imports
-- leaves a durable mapping row, so these guards fail closed after restart.
CREATE TRIGGER project_plan_incomplete_execution_guard BEFORE INSERT ON execution
WHEN NEW.status='running' AND EXISTS(
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
CREATE TRIGGER project_plan_incomplete_work_unit_guard BEFORE INSERT ON work_unit
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
CREATE TRIGGER project_plan_incomplete_coordination_receipt_guard BEFORE INSERT ON task_coordination_receipt
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
-- Keep the origin of Task dependency edges. Project plan application may
-- replace only edges it previously wrote; older/manual edges remain intact.
CREATE TABLE project_plan_manual_dependency (
 task_id TEXT NOT NULL, depends_on_id TEXT NOT NULL,
 PRIMARY KEY(task_id,depends_on_id),
 FOREIGN KEY(task_id,depends_on_id) REFERENCES task_dependency(task_id,depends_on_id) ON DELETE CASCADE
);
INSERT INTO project_plan_manual_dependency(task_id,depends_on_id)
 SELECT task_id,depends_on_id FROM task_dependency;
CREATE TABLE project_plan_owned_dependency (
 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 task_id TEXT NOT NULL, depends_on_id TEXT NOT NULL,
 predecessor_key TEXT NOT NULL, successor_key TEXT NOT NULL,
 edge_json TEXT NOT NULL CHECK(json_valid(edge_json)),
 PRIMARY KEY(task_id,depends_on_id),
 FOREIGN KEY(task_id,depends_on_id) REFERENCES task_dependency(task_id,depends_on_id) ON DELETE CASCADE
);
CREATE TRIGGER project_plan_dependency_manual_insert AFTER INSERT ON task_dependency BEGIN
 INSERT OR IGNORE INTO project_plan_manual_dependency(task_id,depends_on_id)
 VALUES(NEW.task_id,NEW.depends_on_id);
END;
CREATE TABLE project_plan_receipt (
 project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 idempotency_key TEXT NOT NULL, request_digest TEXT NOT NULL,
 result_json TEXT NOT NULL CHECK(json_valid(result_json)), created_at TEXT NOT NULL,
 PRIMARY KEY(project_id,idempotency_key)
);
CREATE TABLE project_plan_batch (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
 graph_version INTEGER NOT NULL, version INTEGER NOT NULL DEFAULT 1,
 state TEXT NOT NULL CHECK(state IN ('running','paused','revoked','completed')),
 preparation_complete INTEGER NOT NULL DEFAULT 0 CHECK(preparation_complete IN (0,1)),
 max_parallel_tasks INTEGER NOT NULL CHECK(max_parallel_tasks BETWEEN 1 AND 64),
 authorized_by TEXT NOT NULL, idempotency_key TEXT NOT NULL,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(project_id,idempotency_key)
);
CREATE TABLE project_plan_batch_task (
 batch_id TEXT NOT NULL REFERENCES project_plan_batch(id) ON DELETE CASCADE,
 task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','started','completed','blocked')),
 claim_expires_at TEXT, claim_token TEXT, coordination_request_id TEXT, blocker TEXT,
 authorization_baseline_json TEXT NOT NULL CHECK(json_valid(authorization_baseline_json)),
 coordination_snapshot_json TEXT CHECK(coordination_snapshot_json IS NULL OR json_valid(coordination_snapshot_json)),
 coordination_snapshot_digest TEXT CHECK(coordination_snapshot_digest IS NULL OR length(coordination_snapshot_digest)=64),
 PRIMARY KEY(batch_id,task_id)
);
CREATE TRIGGER project_plan_batch_task_authority_baseline_immutable
 BEFORE UPDATE OF authorization_baseline_json ON project_plan_batch_task
 WHEN NEW.authorization_baseline_json IS NOT OLD.authorization_baseline_json BEGIN
 SELECT RAISE(ABORT,'Project batch Task authority baseline is immutable');
END;
CREATE INDEX project_plan_batch_task_task ON project_plan_batch_task(task_id);
-- Exact grant origin. This row is inserted in the same TaskCoordination write
-- that advances authorization_version; it must never be inferred from a
-- version delta or reconstructed from the current batch projection.
CREATE TABLE project_plan_batch_coordination_authorization (
 batch_id TEXT NOT NULL,
 project_id TEXT NOT NULL,
 task_id TEXT NOT NULL,
 authorization_version INTEGER NOT NULL CHECK(authorization_version >= 1),
 authorized_by TEXT NOT NULL,
 expected_batch_version INTEGER NOT NULL CHECK(expected_batch_version >= 1),
 expected_graph_version INTEGER NOT NULL CHECK(expected_graph_version >= 1),
 task_version INTEGER NOT NULL CHECK(task_version >= 1),
 task_objective_digest TEXT NOT NULL CHECK(length(task_objective_digest)=64),
 task_title TEXT NOT NULL,
 task_description TEXT,
 task_repo_id TEXT,
 task_parent_task_id TEXT,
 team_generation INTEGER NOT NULL CHECK(team_generation >= 1),
 team_digest TEXT NOT NULL CHECK(length(team_digest)=64),
 envelope_digest TEXT NOT NULL CHECK(length(envelope_digest)=64),
 unconfigured_default_refresh INTEGER NOT NULL CHECK(unconfigured_default_refresh IN (0,1)),
 mode TEXT NOT NULL CHECK(mode='prepare_and_execute'),
 role_id TEXT NOT NULL,
 actor_kind TEXT NOT NULL CHECK(actor_kind='agent'),
 actor_id TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(batch_id,task_id,authorization_version),
 FOREIGN KEY(batch_id,task_id) REFERENCES project_plan_batch_task(batch_id,task_id) ON DELETE CASCADE
);
CREATE INDEX project_plan_batch_coordination_authorization_task
 ON project_plan_batch_coordination_authorization(task_id,authorization_version);
CREATE TRIGGER project_plan_batch_coordination_authorization_scope_guard
 BEFORE INSERT ON project_plan_batch_coordination_authorization
 WHEN NOT EXISTS(
  SELECT 1 FROM project_plan_batch b
  JOIN project_plan p ON p.project_id=b.project_id
  JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=NEW.task_id
  JOIN task t ON t.id=NEW.task_id AND t.project_id=b.project_id AND t.deleted_at IS NULL
  JOIN task_coordination c ON c.task_id=t.id
  JOIN project_member m ON m.project_id=b.project_id AND m.user_id=NEW.authorized_by
   AND m.role IN ('owner','admin')
  JOIN project owner_project ON owner_project.id=b.project_id
  JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
   AND owner_membership.user_id=owner_project.owner_id AND owner_membership.role='owner'
  WHERE b.id=NEW.batch_id AND b.project_id=NEW.project_id
   AND b.authorized_by=NEW.authorized_by AND b.version=NEW.expected_batch_version
   AND b.graph_version=NEW.expected_graph_version AND p.version=b.graph_version
   AND c.authorization_version=NEW.authorization_version
   AND NEW.authorization_version=json_extract(bt.authorization_baseline_json,'$.authorization_version')+1
   AND c.mode=NEW.mode AND c.requested_by=NEW.authorized_by
   AND c.team_generation=NEW.team_generation AND c.team_digest=NEW.team_digest
   AND c.envelope_digest=NEW.envelope_digest
   AND c.coordinator_task_role_id=NEW.role_id
   AND c.coordinator_actor_kind=NEW.actor_kind AND c.coordinator_actor_id=NEW.actor_id
   AND (json_extract(bt.authorization_baseline_json,'$.team_digest') IS NEW.team_digest
     OR (NEW.unconfigured_default_refresh=1
         AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=1
         AND json_extract(bt.authorization_baseline_json,'$.mode')='manual'
         AND json_extract(bt.authorization_baseline_json,'$.requested_by') IS NULL
         AND json_extract(bt.authorization_baseline_json,'$.revoked_at') IS NULL))
   AND (json_extract(bt.authorization_baseline_json,'$.envelope_digest') IS NEW.envelope_digest
     OR (NEW.unconfigured_default_refresh=1
         AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=1
         AND json_extract(bt.authorization_baseline_json,'$.mode')='manual'
         AND json_extract(bt.authorization_baseline_json,'$.requested_by') IS NULL
         AND json_extract(bt.authorization_baseline_json,'$.revoked_at') IS NULL))
   AND json_extract(bt.authorization_baseline_json,'$.role_id') IS NEW.role_id
   AND json_extract(bt.authorization_baseline_json,'$.actor_kind') IS NEW.actor_kind
   AND json_extract(bt.authorization_baseline_json,'$.actor_id') IS NEW.actor_id
   AND c.revoked_at IS NULL AND c.team_stale=0
   AND t.version=NEW.task_version AND t.title=NEW.task_title
   AND t.description IS NEW.task_description AND t.repo_id IS NEW.task_repo_id
   AND t.parent_task_id IS NEW.task_parent_task_id
   AND json_extract(bt.authorization_baseline_json,'$.team_generation')=NEW.team_generation
   AND json_extract(bt.authorization_baseline_json,'$.unconfigured_manual_default')=NEW.unconfigured_default_refresh
   AND json_extract(bt.authorization_baseline_json,'$.task_objective_digest')=NEW.task_objective_digest
   AND ((b.state IN ('paused','running') AND b.preparation_complete=0
         AND json_extract(bt.authorization_baseline_json,'$.task_version')=NEW.task_version
         AND json_extract(bt.authorization_baseline_json,'$.lifecycle_state') IN ('backlog','ready'))
     OR (b.state='running' AND b.preparation_complete=1 AND bt.state='blocked'
         AND bt.blocker='lifecycle:blocked'
         AND json_extract(bt.authorization_baseline_json,'$.lifecycle_state')='blocked'
         AND json_extract(bt.authorization_baseline_json,'$.task_version')+1=NEW.task_version))
   AND NOT EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status!='cancelled')
   AND NOT EXISTS(SELECT 1 FROM task_coordination_plan plan
                  WHERE plan.task_id=t.id AND plan.state IN ('approved','materialized'))
 )
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE');
END;
CREATE TRIGGER project_plan_batch_coordination_authorization_immutable_update
 BEFORE UPDATE ON project_plan_batch_coordination_authorization BEGIN
 SELECT RAISE(ABORT,'Project batch coordination authorization origin is immutable');
END;
CREATE TRIGGER project_plan_batch_coordination_authorization_immutable_delete
 BEFORE DELETE ON project_plan_batch_coordination_authorization
 WHEN NOT EXISTS(SELECT 1 FROM project p JOIN project_deletion_guard g ON g.project_id=p.id
                WHERE p.id=OLD.project_id) BEGIN
 SELECT RAISE(ABORT,'Project batch coordination authorization history is immutable');
END;
CREATE TABLE project_plan_handoff (
 batch_id TEXT NOT NULL, task_id TEXT NOT NULL,
 predecessor_task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
 context_json TEXT NOT NULL CHECK(json_valid(context_json)), digest TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(batch_id,task_id,predecessor_task_id),
 FOREIGN KEY(batch_id,task_id) REFERENCES project_plan_batch_task(batch_id,task_id) ON DELETE CASCADE
);
CREATE TABLE project_plan_batch_execution (
 execution_id TEXT PRIMARY KEY REFERENCES execution(id) ON DELETE CASCADE,
 batch_id TEXT NOT NULL,
 task_id TEXT NOT NULL,
 authorization_version INTEGER NOT NULL CHECK(authorization_version >= 1),
 FOREIGN KEY(batch_id,task_id) REFERENCES project_plan_batch_task(batch_id,task_id) ON DELETE CASCADE,
 FOREIGN KEY(batch_id,task_id,authorization_version)
  REFERENCES project_plan_batch_coordination_authorization(batch_id,task_id,authorization_version)
  ON DELETE CASCADE
);
CREATE INDEX project_plan_batch_execution_batch ON project_plan_batch_execution(batch_id,task_id);
-- One Task may have multiple exact coordinator turns in one authorized phase.
-- The deferred request FK allows this provenance receipt to be prepared first,
-- so the generic request trigger can reject unscoped inserts atomically.
CREATE TABLE project_plan_batch_coordination_request (
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
 origin_kind TEXT NOT NULL CHECK(origin_kind IN ('scheduler','quality_replan','human_replan')),
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
    OR (origin_kind='quality_replan' AND claim_token IS NULL
        AND source_artifact_id IS NOT NULL AND producer_execution_id IS NOT NULL
        AND approved_plan_artifact_id IS NULL)
    OR (origin_kind='human_replan' AND claim_token IS NULL
        AND source_artifact_id IS NULL AND producer_execution_id IS NULL
        AND approved_plan_artifact_id IS NOT NULL))
);
CREATE INDEX project_plan_batch_coordination_request_batch
 ON project_plan_batch_coordination_request(batch_id,task_id,created_at,request_id);
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
    OR (NEW.origin_kind IN ('quality_replan','human_replan') AND bt.state='started'
        AND bt.coordination_request_id IS NOT NULL))
   AND (NEW.origin_kind!='quality_replan' OR EXISTS(
       SELECT 1 FROM artifact_execution_producer producer
       JOIN project_plan_batch_execution exec_link
        ON exec_link.execution_id=producer.execution_id AND exec_link.batch_id=b.id
         AND exec_link.task_id=t.id AND exec_link.authorization_version=NEW.authorization_version
       WHERE producer.artifact_id=NEW.source_artifact_id
        AND producer.execution_id=NEW.producer_execution_id AND producer.task_id=t.id))
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
-- A completed predecessor is required, with all repository scopes integrated.
CREATE VIEW project_task_dependency_readiness AS
 SELECT d.task_id,d.depends_on_id,
 CASE WHEN l.state='done' AND t.deleted_at IS NULL
  AND NOT EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status NOT IN ('completed','cancelled'))
  AND NOT EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.requires_integration=1 AND w.status!='cancelled'
   AND (w.status!='completed' OR NOT EXISTS(SELECT 1 FROM work_unit_integration i
    JOIN execution e ON e.id=i.execution_id AND e.task_id=i.task_id
    WHERE i.work_unit_id=w.id AND i.task_id=w.task_id AND i.outcome='success'
     AND e.status='completed' AND e.work_unit_version=w.version-1
     AND e.after_sha=i.source_sha AND i.target_after_sha IS NOT NULL)))
 THEN 1 ELSE 0 END AS satisfied
 FROM task_dependency d JOIN task t ON t.id=d.depends_on_id
 LEFT JOIN task_lifecycle l ON l.task_id=t.id;
CREATE TRIGGER project_task_dependency_insert_guard BEFORE INSERT ON task_dependency BEGIN
 SELECT CASE WHEN NEW.task_id=NEW.depends_on_id OR NOT EXISTS(
  SELECT 1 FROM task a JOIN task b ON b.id=NEW.depends_on_id
  WHERE a.id=NEW.task_id AND a.project_id=b.project_id AND a.deleted_at IS NULL AND b.deleted_at IS NULL
 ) THEN RAISE(ABORT,'FORGE_PROJECT_DEPENDENCY_INVALID') END;
 SELECT CASE WHEN EXISTS(WITH RECURSIVE reachable(id) AS (
  SELECT depends_on_id FROM task_dependency WHERE task_id=NEW.depends_on_id UNION
  SELECT d.depends_on_id FROM task_dependency d JOIN reachable r ON d.task_id=r.id
 ) SELECT 1 FROM reachable WHERE id=NEW.task_id)
 THEN RAISE(ABORT,'FORGE_PROJECT_DEPENDENCY_CYCLE') END;
END;
CREATE TRIGGER project_task_dependency_insert_version AFTER INSERT ON task_dependency BEGIN
 UPDATE project_plan SET version=version+1,updated_at=NEW.created_at
 WHERE project_id=(SELECT project_id FROM task WHERE id=NEW.task_id);
END;
CREATE TRIGGER project_task_dependency_delete_version AFTER DELETE ON task_dependency BEGIN
 UPDATE project_plan SET version=version+1 WHERE project_id=(SELECT project_id FROM task WHERE id=OLD.task_id);
END;
CREATE TRIGGER project_task_dependency_update_guard BEFORE UPDATE ON task_dependency BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_DEPENDENCY_IMMUTABLE');
END;
CREATE TRIGGER project_task_execution_dependency_guard BEFORE INSERT ON execution
WHEN NEW.status='running' BEGIN
 SELECT CASE WHEN EXISTS(SELECT 1 FROM project_task_dependency_readiness WHERE task_id=NEW.task_id AND satisfied=0)
 THEN RAISE(ABORT,'FORGE_WU_ADMISSION_PROJECT_DEPENDENCY_UNSATISFIED') END;
 SELECT CASE WHEN EXISTS(SELECT 1 FROM project_plan_batch_task bt JOIN project_plan_batch b ON b.id=bt.batch_id
 JOIN project_plan p ON p.project_id=b.project_id
 WHERE bt.task_id=NEW.task_id AND b.state IN ('running','paused') AND (
  b.state!='running' OR b.graph_version!=p.version OR bt.state NOT IN ('claimed','started') OR
  NOT EXISTS(SELECT 1 FROM project_member m WHERE m.project_id=b.project_id AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')) OR
  NOT EXISTS(SELECT 1 FROM project_member owner_membership WHERE owner_membership.project_id=b.project_id
   AND owner_membership.user_id=(SELECT owner_id FROM project WHERE id=b.project_id) AND owner_membership.role='owner') OR
  NOT EXISTS(SELECT 1 FROM task t WHERE t.id=bt.task_id AND t.project_id=b.project_id
   AND t.title=json_extract(bt.coordination_snapshot_json,'$.task_title')
   AND t.description IS json_extract(bt.coordination_snapshot_json,'$.task_description')
   AND t.repo_id IS json_extract(bt.coordination_snapshot_json,'$.task_repo_id')
   AND t.parent_task_id IS json_extract(bt.coordination_snapshot_json,'$.task_parent_task_id')) OR
  NOT EXISTS(SELECT 1 FROM task_coordination c WHERE c.task_id=bt.task_id
   AND bt.coordination_snapshot_json IS NOT NULL AND c.mode='prepare_and_execute'
   AND c.revoked_at IS NULL AND c.team_stale=0
   AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
   AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
   AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
   AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
   AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
   AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
   AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
   AND c.requested_by=b.authorized_by)))
 THEN RAISE(ABORT,'FORGE_WU_ADMISSION_PROJECT_BATCH_INACTIVE') END;
 SELECT CASE WHEN EXISTS(
  SELECT 1 FROM task_coordination c
  JOIN project_plan_batch_coordination_authorization prior
   ON prior.task_id=c.task_id AND prior.authorization_version=c.authorization_version
  WHERE c.task_id=NEW.task_id AND NOT EXISTS(
   SELECT 1 FROM project_plan_batch b
   JOIN project_plan p ON p.project_id=b.project_id
   JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=NEW.task_id
   JOIN project_plan_batch_coordination_authorization active_grant
    ON active_grant.batch_id=b.id AND active_grant.task_id=bt.task_id
     AND active_grant.authorization_version=c.authorization_version
   WHERE b.id=prior.batch_id AND b.state='running' AND b.preparation_complete=1
    AND b.graph_version=p.version AND bt.state IN ('claimed','started')
    AND bt.coordination_snapshot_json IS NOT NULL
    AND bt.coordination_snapshot_digest IS NOT NULL
    AND c.mode='prepare_and_execute' AND c.revoked_at IS NULL AND c.team_stale=0
    AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
    AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
    AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
    AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
    AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
    AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
    AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
    AND c.requested_by=b.authorized_by
  )
 ) THEN RAISE(ABORT,'FORGE_WU_ADMISSION_PROJECT_BATCH_INACTIVE') END;
END;
CREATE TRIGGER project_plan_batch_execution_attach AFTER INSERT ON execution
WHEN NEW.status='running' AND NEW.work_unit_id IS NOT NULL BEGIN
 INSERT OR IGNORE INTO project_plan_batch_execution(execution_id,batch_id,task_id,authorization_version)
 SELECT NEW.id,b.id,bt.task_id,c.authorization_version
 FROM work_unit w
 JOIN project_plan_batch_task bt ON bt.task_id=w.task_id
 JOIN project_plan_batch b ON b.id=bt.batch_id
 JOIN project_plan p ON p.project_id=b.project_id
 JOIN task t ON t.id=w.task_id AND t.project_id=b.project_id AND t.deleted_at IS NULL
 JOIN task_coordination c ON c.task_id=bt.task_id
 JOIN project_plan_batch_coordination_authorization a
  ON a.batch_id=b.id AND a.task_id=bt.task_id
   AND a.authorization_version=c.authorization_version
 WHERE NEW.work_unit_id=w.id AND NEW.task_id=w.task_id
  AND bt.state='started' AND b.state='running' AND b.preparation_complete=1
  AND b.graph_version=p.version
  AND EXISTS(SELECT 1 FROM project_member m
             WHERE m.project_id=b.project_id AND m.user_id=b.authorized_by
               AND m.role IN ('owner','admin'))
  AND EXISTS(SELECT 1 FROM project_member owner_membership
             WHERE owner_membership.project_id=b.project_id
               AND owner_membership.user_id=(SELECT owner_id FROM project WHERE id=b.project_id)
               AND owner_membership.role='owner')
  AND t.title=json_extract(bt.coordination_snapshot_json,'$.task_title')
  AND t.description IS json_extract(bt.coordination_snapshot_json,'$.task_description')
  AND t.repo_id IS json_extract(bt.coordination_snapshot_json,'$.task_repo_id')
  AND t.parent_task_id IS json_extract(bt.coordination_snapshot_json,'$.task_parent_task_id')
  AND c.mode='prepare_and_execute' AND c.revoked_at IS NULL AND c.team_stale=0
  AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
  AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
  AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
  AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
  AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
  AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
  AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
  AND c.requested_by=b.authorized_by
  AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
  AND a.expected_graph_version=b.graph_version AND a.mode=c.mode
  AND a.task_title=t.title AND a.task_description IS t.description
  AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
  AND (
   EXISTS(
    WITH RECURSIVE source_chain(work_unit_id,provenance_kind,provenance_id,depth) AS (
     SELECT w.id,w.provenance_kind,w.provenance_id,0
     UNION ALL
     SELECT parent.id,parent.provenance_kind,parent.provenance_id,source_chain.depth+1
     FROM source_chain
     JOIN work_unit parent ON source_chain.provenance_kind='work_unit'
       AND parent.id=source_chain.provenance_id AND parent.task_id=w.task_id
     WHERE source_chain.depth<32
    )
    SELECT 1 FROM source_chain
    WHERE source_chain.provenance_kind='artifact' AND (
     EXISTS(
      SELECT 1 FROM task_coordination_plan plan
      JOIN project_plan_batch_coordination_request plan_origin
       ON plan_origin.request_id=plan.request_id AND plan_origin.batch_id=b.id
        AND plan_origin.task_id=w.task_id
        AND plan_origin.authorization_version=c.authorization_version
      WHERE plan.artifact_id=source_chain.provenance_id
       AND plan.task_id=w.task_id AND plan.state='materialized'
     ) OR EXISTS(
      SELECT 1 FROM artifact_execution_producer producer
      JOIN project_plan_batch_execution producer_execution
       ON producer_execution.execution_id=producer.execution_id
        AND producer_execution.batch_id=b.id
        AND producer_execution.task_id=w.task_id
        AND producer_execution.authorization_version=c.authorization_version
      WHERE producer.artifact_id=source_chain.provenance_id
       AND producer.task_id=w.task_id
     )
    )
   )
   OR EXISTS(
    SELECT 1 FROM task_quality_request quality
    WHERE quality.work_unit_id=w.id AND quality.task_id=w.task_id
     AND quality.state IN ('work_created','running','resolved')
     AND (quality.source_kind!='review_report' OR EXISTS(
      SELECT 1 FROM artifact_execution_producer producer
      JOIN project_plan_batch_execution producer_execution
       ON producer_execution.execution_id=producer.execution_id
        AND producer_execution.batch_id=b.id
        AND producer_execution.task_id=w.task_id
        AND producer_execution.authorization_version=c.authorization_version
      WHERE producer.artifact_id=quality.source_id
       AND producer.task_id=quality.task_id
     ))
   )
  );
END;
CREATE TRIGGER project_plan_materialization_guard BEFORE INSERT ON work_unit
WHEN NEW.provenance_kind='artifact' AND EXISTS(
 SELECT 1 FROM task_coordination_plan plan
 JOIN task_coordination_request request ON request.id=plan.request_id
 JOIN project_plan_batch_coordination_request scope ON scope.request_id=request.id
  AND scope.task_id=NEW.task_id
 JOIN project_plan_batch_task bt ON bt.batch_id=scope.batch_id AND bt.task_id=NEW.task_id
 JOIN project_plan_batch b ON b.id=bt.batch_id
 JOIN project_plan p ON p.project_id=b.project_id
 WHERE plan.artifact_id=NEW.provenance_id AND (
  b.state!='running' OR b.graph_version!=p.version OR bt.state NOT IN ('claimed','started')
  OR NOT EXISTS(SELECT 1 FROM project_member m WHERE m.project_id=b.project_id AND m.user_id=b.authorized_by AND m.role IN ('owner','admin'))
  OR NOT EXISTS(SELECT 1 FROM project_member owner_membership WHERE owner_membership.project_id=b.project_id
   AND owner_membership.user_id=(SELECT owner_id FROM project WHERE id=b.project_id) AND owner_membership.role='owner')
  OR NOT EXISTS(SELECT 1 FROM task t WHERE t.id=bt.task_id AND t.project_id=b.project_id
   AND t.title=json_extract(bt.coordination_snapshot_json,'$.task_title')
   AND t.description IS json_extract(bt.coordination_snapshot_json,'$.task_description')
   AND t.repo_id IS json_extract(bt.coordination_snapshot_json,'$.task_repo_id')
   AND t.parent_task_id IS json_extract(bt.coordination_snapshot_json,'$.task_parent_task_id'))
  OR NOT EXISTS(SELECT 1 FROM task_coordination c WHERE c.task_id=bt.task_id
   AND bt.coordination_snapshot_json IS NOT NULL AND c.mode='prepare_and_execute'
   AND c.revoked_at IS NULL AND c.team_stale=0
   AND c.authorization_version=scope.authorization_version
   AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
   AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
   AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
   AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
   AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
   AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
   AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
   AND c.requested_by=b.authorized_by)))
BEGIN
 SELECT RAISE(ABORT,'FORGE_PROJECT_PLAN_BATCH_INACTIVE');
END;
