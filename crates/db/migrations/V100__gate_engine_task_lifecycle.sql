-- Plan PR9 adds an aggregate Task lifecycle and deterministic, immutable Gates.
-- Legacy Task state is retained as a one-way compatibility projection.

CREATE TABLE task_lifecycle (
    task_id       TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    state         TEXT NOT NULL CHECK (state IN (
                      'backlog', 'ready', 'active', 'blocked',
                      'ready_to_merge', 'merging', 'done', 'cancelled'
                  )),
    version       INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    reason_kind   TEXT,
    reason_ref    TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    CHECK ((reason_kind IS NULL) = (reason_ref IS NULL))
);

CREATE TABLE task_lifecycle_migration_audit (
    task_id                   TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    legacy_state              TEXT NOT NULL,
    mapped_state              TEXT NOT NULL CHECK (mapped_state IN (
                                  'backlog', 'ready', 'active', 'blocked',
                                  'ready_to_merge', 'merging', 'done', 'cancelled'
                              )),
    mapping_status            TEXT NOT NULL,
    reason_kind               TEXT,
    reason_ref                TEXT,
    legacy_task_state_config  TEXT,
    workflow_definition       TEXT NOT NULL,
    details_json              TEXT NOT NULL CHECK (json_valid(details_json)),
    created_at                TEXT NOT NULL
);

INSERT INTO task_lifecycle_migration_audit (
    task_id, legacy_state, mapped_state, mapping_status, reason_kind, reason_ref,
    legacy_task_state_config, workflow_definition, details_json, created_at
)
SELECT t.id, t.status,
       CASE
           WHEN t.status = 'backlog' THEN 'backlog'
           WHEN t.status = 'todo' THEN 'ready'
           WHEN t.status = 'ready' THEN 'ready'
           WHEN t.status IN ('planning', 'in_progress', 'working') THEN 'active'
           WHEN t.status IN ('done', 'cancelled', 'blocked') THEN t.status
           WHEN t.status = 'merging' AND EXISTS (
               SELECT 1 FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge' AND op.status = 'running'
           ) THEN 'merging'
           ELSE 'blocked'
       END,
       CASE
           WHEN t.status IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'done', 'cancelled', 'blocked')
               THEN 'mapped'
           WHEN t.status = 'review' THEN 'ambiguous_review'
           WHEN t.status = 'merging' AND EXISTS (
               SELECT 1 FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge' AND op.status = 'running'
           ) THEN 'active_merge_operation'
           WHEN t.status = 'merging' THEN 'ambiguous_merge'
           WHEN t.status = 'merge_failed' THEN 'merge_failed'
           ELSE 'unknown_custom_state'
       END,
       CASE
           WHEN t.status = 'review' THEN 'legacy_review_without_gate_proof'
           WHEN t.status = 'merging' AND EXISTS (
               SELECT 1 FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge' AND op.status = 'running'
           ) THEN 'legacy_merge_operation'
           WHEN t.status = 'merging' THEN 'legacy_merge_without_operation'
           WHEN t.status = 'merge_failed' THEN 'legacy_merge_failure'
           WHEN t.status = 'blocked' THEN 'legacy_blocked'
           WHEN t.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'done', 'cancelled', 'blocked')
               THEN 'unknown_legacy_state'
           ELSE NULL
       END,
       CASE
           WHEN t.status = 'merging' AND EXISTS (
               SELECT 1 FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge' AND op.status = 'running'
           ) THEN (
               SELECT op.id FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge' AND op.status = 'running'
               ORDER BY op.created_at DESC, op.id DESC LIMIT 1
           )
           WHEN t.status = 'merge_failed' THEN COALESCE((
               SELECT log.id FROM transition_log log
               WHERE log.task_id = t.id AND log.to_state = 'merge_failed'
               ORDER BY log.created_at DESC, log.id DESC LIMIT 1
           ), t.id)
           WHEN t.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'done', 'cancelled')
               THEN t.id
           ELSE NULL
       END,
       t.task_state_config,
       p.workflow_definition,
       json_object(
           'legacy_blocked_json', t.blocked_json,
           'legacy_failed_json', t.failed_json,
           'legacy_entry_barrier_json', t.entry_barrier_json,
           'legacy_review_passed_at', t.review_passed_at,
           'legacy_error_annotation', t.error_annotation,
           'merge_operation_count', (
               SELECT COUNT(*) FROM task_integration_operation op
               WHERE op.task_id = t.id AND op.kind = 'task_merge'
           )
       ),
       t.updated_at
FROM task t
JOIN project p ON p.id = t.project_id;

INSERT INTO task_lifecycle (
    task_id, state, version, reason_kind, reason_ref, created_at, updated_at
)
SELECT task_id, mapped_state, 1, reason_kind, reason_ref, created_at, created_at
FROM task_lifecycle_migration_audit;

-- Normalize the legacy status column to the documented, lossy compatibility
-- projection. The exact original value remains in task_lifecycle_migration_audit.
UPDATE task
SET status = CASE (SELECT state FROM task_lifecycle WHERE task_id = task.id)
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END,
    entry_barrier_json = NULL,
    version = version + 1
WHERE status IS NOT CASE (SELECT state FROM task_lifecycle WHERE task_id = task.id)
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END
   OR entry_barrier_json IS NOT NULL;

CREATE TRIGGER task_lifecycle_insert_for_new_task
AFTER INSERT ON task
BEGIN
    INSERT INTO task_lifecycle (
        task_id, state, version, reason_kind, reason_ref, created_at, updated_at
    ) VALUES (
        NEW.id,
        CASE NEW.status
            WHEN 'backlog' THEN 'backlog'
            WHEN 'todo' THEN 'ready'
            WHEN 'ready' THEN 'ready'
            WHEN 'planning' THEN 'active'
            WHEN 'in_progress' THEN 'active'
            WHEN 'working' THEN 'active'
            WHEN 'blocked' THEN 'blocked'
            WHEN 'done' THEN 'done'
            WHEN 'cancelled' THEN 'cancelled'
            ELSE 'blocked'
        END,
        1,
        CASE
            WHEN NEW.status = 'review' THEN 'ambiguous_review'
            WHEN NEW.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'blocked', 'done', 'cancelled')
                THEN 'unknown_legacy_state'
            ELSE NULL
        END,
        CASE
            WHEN NEW.status = 'review'
              OR NEW.status NOT IN ('backlog', 'todo', 'ready', 'planning', 'in_progress', 'working', 'blocked', 'done', 'cancelled')
                THEN NEW.id
            ELSE NULL
        END,
        NEW.created_at,
        NEW.updated_at
    );
    UPDATE task SET status = CASE (
        SELECT state FROM task_lifecycle WHERE task_id = NEW.id
    )
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER task_lifecycle_status_projection_guard
BEFORE UPDATE OF status ON task
WHEN EXISTS (SELECT 1 FROM task_lifecycle WHERE task_id = OLD.id)
 AND NEW.status IS NOT CASE (
        SELECT state FROM task_lifecycle WHERE task_id = OLD.id
    )
        WHEN 'backlog' THEN 'backlog'
        WHEN 'ready' THEN 'todo'
        WHEN 'active' THEN 'in_progress'
        WHEN 'blocked' THEN 'blocked'
        WHEN 'ready_to_merge' THEN 'in_progress'
        WHEN 'merging' THEN 'in_progress'
        WHEN 'done' THEN 'done'
        WHEN 'cancelled' THEN 'cancelled'
    END
BEGIN
    SELECT RAISE(ABORT, 'task.status is a projection of task_lifecycle');
END;

CREATE TABLE task_lifecycle_transition (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    idempotency_key          TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    request_digest           TEXT NOT NULL CHECK (length(request_digest) = 64),
    expected_task_version    INTEGER NOT NULL CHECK (expected_task_version >= 1),
    from_state               TEXT NOT NULL CHECK (from_state IN (
                                 'backlog', 'ready', 'active', 'blocked',
                                 'ready_to_merge', 'merging', 'done', 'cancelled'
                             )),
    to_state                 TEXT NOT NULL CHECK (to_state IN (
                                 'backlog', 'ready', 'active', 'blocked',
                                 'ready_to_merge', 'merging', 'done', 'cancelled'
                             )),
    from_version             INTEGER NOT NULL CHECK (from_version >= 1),
    to_version               INTEGER NOT NULL CHECK (to_version = from_version + 1),
    cause_kind               TEXT NOT NULL CHECK (cause_kind IN (
                                 'actor', 'gate_evaluation', 'execution', 'validation_run',
                                 'work_unit', 'merge_operation', 'domain_event', 'system'
                             )),
    cause_ref                TEXT,
    gate_evaluation_id       TEXT REFERENCES gate_evaluation(id) ON DELETE RESTRICT,
    reason_kind              TEXT,
    reason_ref               TEXT,
    domain_event_id          TEXT NOT NULL UNIQUE REFERENCES domain_event(id) ON DELETE RESTRICT,
    created_at               TEXT NOT NULL,
    UNIQUE(task_id, idempotency_key),
    CHECK ((reason_kind IS NULL) = (reason_ref IS NULL)),
    CHECK ((cause_kind = 'gate_evaluation' OR to_state = 'merging') = (gate_evaluation_id IS NOT NULL)),
    CHECK (
        gate_evaluation_id IS NULL
        OR (cause_kind = 'gate_evaluation' AND cause_ref = gate_evaluation_id)
        OR (to_state = 'merging' AND cause_kind = 'merge_operation' AND cause_ref IS NOT NULL)
    )
);
CREATE INDEX idx_task_lifecycle_transition_task_version
    ON task_lifecycle_transition(task_id, to_version);

CREATE TRIGGER task_lifecycle_update_guard
BEFORE UPDATE ON task_lifecycle
WHEN NEW.task_id IS NOT OLD.task_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.state IS OLD.state
  OR NEW.version != OLD.version + 1
  OR NOT EXISTS (
      SELECT 1 FROM task_lifecycle_transition tr
      WHERE tr.task_id = OLD.task_id
        AND tr.from_state = OLD.state AND tr.to_state = NEW.state
        AND tr.from_version = OLD.version AND tr.to_version = NEW.version
        AND tr.reason_kind IS NEW.reason_kind AND tr.reason_ref IS NEW.reason_ref
        AND tr.created_at = NEW.updated_at
  )
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle changes require one exact transition receipt');
END;
CREATE TRIGGER task_lifecycle_immutable_delete
BEFORE DELETE ON task_lifecycle
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle is immutable outside Project teardown');
END;

CREATE TABLE gate (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    gate_kind                TEXT NOT NULL CHECK (length(trim(gate_kind)) BETWEEN 1 AND 64),
    scope_kind               TEXT NOT NULL CHECK (scope_kind IN (
                                 'task', 'work_unit', 'merge_operation', 'lifecycle_operation'
                             )),
    scope_id                 TEXT NOT NULL CHECK (length(trim(scope_id)) > 0),
    active_policy_revision   INTEGER,
    created_at               TEXT NOT NULL,
    UNIQUE(task_id, gate_kind, scope_kind, scope_id),
    UNIQUE(id, task_id)
);
CREATE INDEX idx_gate_task ON gate(task_id, scope_kind, scope_id);

CREATE TRIGGER gate_scope_same_task_insert
BEFORE INSERT ON gate
WHEN (NEW.scope_kind = 'task' AND NEW.scope_id != NEW.task_id)
  OR (NEW.scope_kind = 'work_unit' AND NOT EXISTS (
      SELECT 1 FROM work_unit w WHERE w.id = NEW.scope_id AND w.task_id = NEW.task_id
  ))
  OR (NEW.scope_kind = 'merge_operation' AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation op
      WHERE op.id = NEW.scope_id AND op.task_id = NEW.task_id
        AND op.kind = 'task_merge'
  ))
  OR (NEW.scope_kind = 'lifecycle_operation' AND NOT EXISTS (
      SELECT 1 FROM task_lifecycle_transition tr
      WHERE tr.id = NEW.scope_id AND tr.task_id = NEW.task_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'Gate scope is missing or belongs to another Task');
END;

CREATE TRIGGER gate_identity_immutable
BEFORE UPDATE ON gate
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.gate_kind IS NOT OLD.gate_kind
  OR NEW.scope_kind IS NOT OLD.scope_kind OR NEW.scope_id IS NOT OLD.scope_id
  OR NEW.created_at IS NOT OLD.created_at
  OR (NEW.active_policy_revision IS NOT OLD.active_policy_revision AND (
      NEW.active_policy_revision IS NULL
      OR NEW.active_policy_revision != COALESCE(OLD.active_policy_revision, 0) + 1
      OR NOT EXISTS (
          SELECT 1 FROM gate_policy_revision r
          WHERE r.gate_id = OLD.id AND r.revision = NEW.active_policy_revision
      )
  ))
BEGIN
    SELECT RAISE(ABORT, 'Gate identity is immutable and its policy pointer only advances');
END;
CREATE TRIGGER gate_immutable_delete
BEFORE DELETE ON gate
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate identity is immutable outside Project teardown');
END;

CREATE TABLE gate_policy_revision (
    gate_id          TEXT NOT NULL,
    revision         INTEGER NOT NULL CHECK (revision >= 1),
    schema_version   INTEGER NOT NULL CHECK (schema_version >= 1),
    policy_json      TEXT NOT NULL CHECK (
                         json_valid(policy_json)
                         AND json_type(policy_json) = 'object'
                         AND length(policy_json) <= 65536
                     ),
    policy_digest    TEXT NOT NULL CHECK (length(policy_digest) = 64),
    created_at       TEXT NOT NULL,
    PRIMARY KEY(gate_id, revision),
    UNIQUE(gate_id, policy_digest),
    FOREIGN KEY (gate_id) REFERENCES gate(id) ON DELETE CASCADE
);

CREATE TRIGGER gate_policy_revision_sequential_insert
BEFORE INSERT ON gate_policy_revision
WHEN NEW.revision != COALESCE((
    SELECT active_policy_revision FROM gate WHERE id = NEW.gate_id
), 0) + 1
BEGIN
    SELECT RAISE(ABORT, 'Gate policy revisions must advance one revision at a time');
END;

CREATE TRIGGER gate_policy_revision_immutable_update
BEFORE UPDATE ON gate_policy_revision
BEGIN
    SELECT RAISE(ABORT, 'Gate policy revisions are immutable');
END;
CREATE TRIGGER gate_policy_revision_immutable_delete
BEFORE DELETE ON gate_policy_revision
WHEN NOT EXISTS (
    SELECT 1 FROM gate g JOIN project_deletion_guard guard ON guard.project_id = (
        SELECT t.project_id FROM task t WHERE t.id = g.task_id
    ) WHERE g.id = OLD.gate_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate policy revisions are immutable outside Project teardown');
END;

CREATE TABLE gate_evaluation (
    id                 TEXT PRIMARY KEY,
    gate_id            TEXT NOT NULL,
    task_id            TEXT NOT NULL,
    policy_revision    INTEGER NOT NULL,
    outcome            TEXT NOT NULL CHECK (outcome IN ('satisfied', 'unsatisfied', 'indeterminate')),
    input_digest       TEXT NOT NULL CHECK (length(input_digest) = 64),
    result_json        TEXT NOT NULL CHECK (
                           json_valid(result_json)
                           AND json_type(result_json) = 'object'
                           AND length(result_json) <= 16384
                       ),
    evaluated_at       TEXT NOT NULL,
    UNIQUE(gate_id, policy_revision, input_digest),
    UNIQUE(id, task_id),
    FOREIGN KEY (gate_id, task_id) REFERENCES gate(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (gate_id, policy_revision)
        REFERENCES gate_policy_revision(gate_id, revision) ON DELETE CASCADE
);
CREATE INDEX idx_gate_evaluation_task_created
    ON gate_evaluation(task_id, evaluated_at DESC, id DESC);

ALTER TABLE task_integration_operation
    ADD COLUMN gate_evaluation_id TEXT REFERENCES gate_evaluation(id) ON DELETE RESTRICT;

CREATE TRIGGER task_integration_operation_gate_immutable
BEFORE UPDATE OF gate_evaluation_id ON task_integration_operation
WHEN NEW.gate_evaluation_id IS NOT OLD.gate_evaluation_id
BEGIN
    SELECT RAISE(ABORT, 'Task integration operation GateEvaluation binding is immutable');
END;

CREATE TRIGGER task_merge_gate_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind = 'task_merge' AND (
    NEW.gate_evaluation_id IS NULL
    OR NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id
        JOIN task_lifecycle l ON l.task_id = e.task_id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied'
          AND g.gate_kind = 'merge_readiness'
          AND g.scope_kind = 'task' AND g.scope_id = e.task_id
          AND g.active_policy_revision = e.policy_revision
          AND l.state = 'ready_to_merge'
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation_input i
              LEFT JOIN work_unit w ON w.id = i.input_id AND w.task_id = i.task_id
              WHERE i.evaluation_id = e.id AND i.input_kind = 'work_unit'
                AND (w.id IS NULL OR w.version != i.input_version)
          )
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation_input i
              LEFT JOIN decision d ON d.id = i.input_id AND d.task_id = i.task_id
              LEFT JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
              WHERE i.evaluation_id = e.id AND i.input_kind = 'decision'
                AND (d.id IS NULL OR p.id IS NULL OR p.content_version != d.proposal_version
                     OR p.status != 'resolved')
          )
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation newer
              WHERE newer.gate_id = e.gate_id AND newer.policy_revision = e.policy_revision
                AND (newer.evaluated_at > e.evaluated_at
                     OR (newer.evaluated_at = e.evaluated_at AND newer.id > e.id))
          )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'TaskMerge admission requires an active satisfied merge-readiness GateEvaluation');
END;

CREATE TRIGGER task_publish_pr_gate_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN NEW.kind = 'publish_pr' AND (
    NEW.gate_evaluation_id IS NULL
    OR NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id
        JOIN task_lifecycle l ON l.task_id = e.task_id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied'
          AND g.gate_kind = 'merge_readiness'
          AND g.scope_kind = 'task' AND g.scope_id = e.task_id
          AND g.active_policy_revision = e.policy_revision
          AND l.state = 'ready_to_merge'
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation_input i
              LEFT JOIN work_unit w ON w.id = i.input_id AND w.task_id = i.task_id
              WHERE i.evaluation_id = e.id AND i.input_kind = 'work_unit'
                AND (w.id IS NULL OR w.version != i.input_version)
          )
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation_input i
              LEFT JOIN decision d ON d.id = i.input_id AND d.task_id = i.task_id
              LEFT JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
              WHERE i.evaluation_id = e.id AND i.input_kind = 'decision'
                AND (d.id IS NULL OR p.id IS NULL OR p.content_version != d.proposal_version
                     OR p.status != 'resolved')
          )
          AND NOT EXISTS (
              SELECT 1 FROM gate_evaluation newer
              WHERE newer.gate_id = e.gate_id AND newer.policy_revision = e.policy_revision
                AND (newer.evaluated_at > e.evaluated_at
                     OR (newer.evaluated_at = e.evaluated_at AND newer.id > e.id))
          )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'PR publication requires an active satisfied merge-readiness GateEvaluation');
END;

CREATE TABLE gate_evaluation_input (
    evaluation_id   TEXT NOT NULL,
    task_id         TEXT NOT NULL,
    ordinal         INTEGER NOT NULL CHECK (ordinal >= 0),
    input_kind      TEXT NOT NULL CHECK (input_kind IN (
                        'review_report', 'validation_run', 'evidence', 'decision',
                        'work_unit', 'execution', 'work_unit_integration',
                        'task_role_snapshot', 'merge_operation', 'lifecycle_operation'
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
        AND x.role = 'reviewer' AND x.purpose = 'review' AND x.status = 'completed'
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
      -- The role/member set is frozen into the immutable policy revision. The
      -- live TaskRole may advance after that revision without rewriting its
      -- historical evaluation inputs.
      SELECT 1 FROM task_role r WHERE r.id = NEW.input_id AND r.task_id = NEW.task_id
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

CREATE TRIGGER gate_evaluation_immutable_update
BEFORE UPDATE ON gate_evaluation
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluations are immutable');
END;
CREATE TRIGGER gate_evaluation_immutable_delete
BEFORE DELETE ON gate_evaluation
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Gate evaluations are immutable outside Project teardown');
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

CREATE TRIGGER task_lifecycle_transition_guard_insert
BEFORE INSERT ON task_lifecycle_transition
WHEN NOT EXISTS (
    SELECT 1 FROM task_lifecycle l
    WHERE l.task_id = NEW.task_id AND l.state = NEW.from_state AND l.version = NEW.from_version
)
  OR (NEW.to_state = 'ready_to_merge' AND NOT EXISTS (
      SELECT 1 FROM gate_evaluation e
      JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id
      WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
        AND e.outcome = 'satisfied' AND g.gate_kind = 'merge_readiness'
        AND g.scope_kind = 'task' AND g.scope_id = e.task_id
        AND g.active_policy_revision = e.policy_revision
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation newer
            WHERE newer.gate_id = e.gate_id AND newer.policy_revision = e.policy_revision
              AND (newer.evaluated_at > e.evaluated_at
                   OR (newer.evaluated_at = e.evaluated_at AND newer.id > e.id))
        )
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i
            LEFT JOIN work_unit w ON w.id = i.input_id AND w.task_id = i.task_id
            WHERE i.evaluation_id = e.id AND i.input_kind = 'work_unit'
              AND (w.id IS NULL OR w.version != i.input_version)
        )
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i
            LEFT JOIN decision d ON d.id = i.input_id AND d.task_id = i.task_id
            LEFT JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
            WHERE i.evaluation_id = e.id AND i.input_kind = 'decision'
              AND (d.id IS NULL OR p.id IS NULL OR p.content_version != d.proposal_version
                   OR p.status != 'resolved')
        )
  ))
  OR (NEW.to_state = 'merging' AND (
      NEW.cause_kind != 'merge_operation'
      OR NOT EXISTS (
          SELECT 1 FROM task_integration_operation op
          WHERE op.id = NEW.cause_ref AND op.task_id = NEW.task_id
            AND op.kind = 'task_merge' AND op.status = 'running'
            AND op.gate_evaluation_id = NEW.gate_evaluation_id
      )
      OR NOT EXISTS (
      SELECT 1 FROM gate_evaluation e
      JOIN gate g ON g.id = e.gate_id AND g.task_id = e.task_id
      WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
        AND e.outcome = 'satisfied' AND g.gate_kind = 'merge_readiness'
        AND g.scope_kind = 'task' AND g.scope_id = e.task_id
        AND g.active_policy_revision = e.policy_revision
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation newer
            WHERE newer.gate_id = e.gate_id AND newer.policy_revision = e.policy_revision
              AND (newer.evaluated_at > e.evaluated_at
                   OR (newer.evaluated_at = e.evaluated_at AND newer.id > e.id))
        )
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i
            LEFT JOIN work_unit w ON w.id = i.input_id AND w.task_id = i.task_id
            WHERE i.evaluation_id = e.id AND i.input_kind = 'work_unit'
              AND (w.id IS NULL OR w.version != i.input_version)
        )
        AND NOT EXISTS (
            SELECT 1 FROM gate_evaluation_input i
            LEFT JOIN decision d ON d.id = i.input_id AND d.task_id = i.task_id
            LEFT JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
            WHERE i.evaluation_id = e.id AND i.input_kind = 'decision'
              AND (d.id IS NULL OR p.id IS NULL OR p.content_version != d.proposal_version
                   OR p.status != 'resolved')
        )
      )
  ))
  OR (NEW.to_state = 'done' AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation op
      WHERE op.id = NEW.cause_ref AND op.task_id = NEW.task_id
        AND op.kind = 'task_merge' AND op.status = 'succeeded'
        AND NEW.cause_kind = 'merge_operation'
  ))
  OR (NEW.from_state = 'merging' AND NEW.to_state = 'blocked' AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation op
      WHERE op.id = NEW.cause_ref AND op.task_id = NEW.task_id
        AND op.kind = 'task_merge' AND op.status IN ('conflict', 'failed', 'abandoned')
        AND NEW.cause_kind = 'merge_operation'
  ))
  OR NOT (
      (NEW.from_state = 'backlog' AND NEW.to_state IN ('ready', 'cancelled'))
      OR (NEW.from_state = 'ready' AND NEW.to_state IN ('active', 'blocked', 'cancelled'))
      OR (NEW.from_state = 'active' AND NEW.to_state IN ('ready', 'blocked', 'ready_to_merge', 'cancelled'))
      OR (NEW.from_state = 'blocked' AND NEW.to_state IN ('ready', 'active', 'cancelled'))
      OR (NEW.from_state = 'ready_to_merge' AND NEW.to_state IN ('active', 'merging', 'blocked', 'cancelled'))
      OR (NEW.from_state = 'merging' AND NEW.to_state IN ('done', 'blocked'))
  )
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle transition lacks its exact authority or is stale');
END;

CREATE TRIGGER task_lifecycle_transition_immutable_update
BEFORE UPDATE ON task_lifecycle_transition
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle transition receipts are immutable');
END;
CREATE TRIGGER task_lifecycle_transition_immutable_delete
BEFORE DELETE ON task_lifecycle_transition
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle transition receipts are immutable outside Project teardown');
END;

CREATE TRIGGER task_lifecycle_migration_audit_immutable_update
BEFORE UPDATE ON task_lifecycle_migration_audit
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle migration audit is immutable');
END;
CREATE TRIGGER task_lifecycle_migration_audit_immutable_delete
BEFORE DELETE ON task_lifecycle_migration_audit
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task lifecycle migration audit is immutable outside Project teardown');
END;
