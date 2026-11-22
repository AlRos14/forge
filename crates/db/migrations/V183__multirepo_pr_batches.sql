PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;

-- Immutable repository-set history. Revision 1 records the exact V181
-- association set without inventing a digest for historical data.
CREATE TABLE task_repository_set_revision (
    task_id       TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    revision      INTEGER NOT NULL CHECK (revision >= 1),
    digest        TEXT CHECK (digest IS NULL OR length(digest) = 64),
    state         TEXT NOT NULL CHECK (state IN ('building', 'current', 'superseded')),
    created_at    TEXT NOT NULL,
    source_kind   TEXT NOT NULL CHECK (length(trim(source_kind)) > 0),
    source_ref    TEXT,
    decision_id   TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    human_decision_request_id TEXT
        REFERENCES human_decision_request(id) ON DELETE RESTRICT,
    PRIMARY KEY (task_id, revision),
    UNIQUE (task_id, revision, digest),
    CHECK (digest IS NOT NULL OR (revision = 1 AND source_kind = 'migration'))
);
CREATE UNIQUE INDEX idx_task_repository_set_revision_current
    ON task_repository_set_revision(task_id) WHERE state = 'current';

CREATE TABLE task_repository_set_revision_repo (
    task_id    TEXT NOT NULL,
    revision   INTEGER NOT NULL,
    repo_id    TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    role       TEXT NOT NULL CHECK (role IN ('primary', 'affected', 'context')),
    position   INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (task_id, revision, repo_id),
    UNIQUE (task_id, revision, position),
    FOREIGN KEY (task_id, revision)
        REFERENCES task_repository_set_revision(task_id, revision) ON DELETE RESTRICT,
    CHECK ((role = 'primary' AND position = 0)
        OR (role IN ('affected', 'context') AND position > 0))
);
CREATE INDEX idx_task_repository_set_revision_repo
    ON task_repository_set_revision_repo(repo_id, task_id, revision);

CREATE TRIGGER task_repository_set_revision_current_guard
BEFORE UPDATE OF state ON task_repository_set_revision
WHEN NEW.state = 'current'
 AND (
     NOT EXISTS (SELECT 1 FROM task t WHERE t.id = NEW.task_id)
     OR ((SELECT repo_id FROM task WHERE id = NEW.task_id) IS NULL
         AND EXISTS (
             SELECT 1 FROM task_repository_set_revision_repo r
             WHERE r.task_id = NEW.task_id AND r.revision = NEW.revision
               AND r.role = 'primary'
         ))
     OR ((SELECT repo_id FROM task WHERE id = NEW.task_id) IS NOT NULL
         AND NOT EXISTS (
             SELECT 1 FROM task t
             JOIN task_repository_set_revision_repo r
               ON r.task_id = NEW.task_id AND r.revision = NEW.revision
              AND r.role = 'primary' AND r.repo_id = t.repo_id
             WHERE t.id = NEW.task_id
         ))
     OR EXISTS (
         SELECT 1 FROM task_repository_set_revision_repo r
         WHERE r.task_id = NEW.task_id AND r.revision = NEW.revision
           AND r.role = 'primary'
         GROUP BY r.task_id, r.revision HAVING COUNT(*) != 1
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'current repository revision must mirror the Task primary repository');
END;

CREATE TRIGGER task_repository_set_revision_update_guard
BEFORE UPDATE ON task_repository_set_revision
WHEN NEW.task_id IS NOT OLD.task_id OR NEW.revision IS NOT OLD.revision
  OR NEW.digest IS NOT OLD.digest OR NEW.created_at IS NOT OLD.created_at
  OR NEW.source_kind IS NOT OLD.source_kind OR NEW.source_ref IS NOT OLD.source_ref
  OR NEW.decision_id IS NOT OLD.decision_id
  OR NEW.human_decision_request_id IS NOT OLD.human_decision_request_id
  OR NOT (
      (OLD.state = NEW.state AND OLD.state = 'building')
      OR (OLD.state = 'building' AND NEW.state = 'current')
      OR (OLD.state = 'current' AND NEW.state = 'superseded')
  )
BEGIN
    SELECT RAISE(ABORT, 'Task repository set revisions are immutable');
END;

CREATE TRIGGER task_repository_set_revision_delete_guard
BEFORE DELETE ON task_repository_set_revision
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task repository set revisions are immutable outside Project teardown');
END;

CREATE TRIGGER task_repository_set_revision_repo_insert_guard
BEFORE INSERT ON task_repository_set_revision_repo
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository_set_revision revision
    JOIN task t ON t.id = revision.task_id
    JOIN repo r ON r.id = NEW.repo_id AND r.project_id = t.project_id
    WHERE revision.task_id = NEW.task_id AND revision.revision = NEW.revision
      AND revision.state = 'building'
)
BEGIN
    SELECT RAISE(ABORT, 'repository revision member must belong to its Task Project while building');
END;

CREATE TRIGGER task_repository_set_revision_repo_update_guard
BEFORE UPDATE ON task_repository_set_revision_repo
WHEN EXISTS (
    SELECT 1 FROM task_repository_set_revision revision
    WHERE revision.task_id = OLD.task_id AND revision.revision = OLD.revision
      AND revision.state != 'building'
)
BEGIN
    SELECT RAISE(ABORT, 'current and superseded repository revision members are immutable');
END;

CREATE TRIGGER task_repository_set_revision_repo_delete_guard
BEFORE DELETE ON task_repository_set_revision_repo
WHEN EXISTS (
    SELECT 1 FROM task_repository_set_revision revision
    WHERE revision.task_id = OLD.task_id AND revision.revision = OLD.revision
      AND revision.state != 'building'
) AND NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'current and superseded repository revision members are immutable');
END;

INSERT INTO task_repository_set_revision(
    task_id, revision, digest, state, created_at, source_kind, source_ref, decision_id,
    human_decision_request_id
)
SELECT task.id, 1, NULL, 'building', task.created_at, 'migration', NULL, NULL, NULL
FROM task;
INSERT INTO task_repository_set_revision_repo(task_id, revision, repo_id, role, position)
SELECT task_id, 1, repo_id, role, position FROM task_repository;
UPDATE task_repository_set_revision SET state = 'current'
WHERE revision = 1 AND state = 'building';

CREATE VIEW task_requires_repository_merge_batch AS
SELECT task_id
FROM task_repository
WHERE role IN ('primary', 'affected')
GROUP BY task_id
HAVING COUNT(*) > 1
UNION
SELECT revision.task_id
FROM task_repository_set_revision revision
JOIN task_repository_set_revision_repo repository
  ON repository.task_id = revision.task_id AND repository.revision = revision.revision
WHERE revision.state = 'current' AND repository.role IN ('primary', 'affected')
GROUP BY revision.task_id, revision.revision
HAVING COUNT(*) > 1;

CREATE TABLE work_unit_repository_set_revision (
    work_unit_id TEXT NOT NULL,
    task_id      TEXT NOT NULL,
    revision     INTEGER NOT NULL,
    PRIMARY KEY (work_unit_id, task_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, revision)
        REFERENCES task_repository_set_revision(task_id, revision) ON DELETE RESTRICT
);
INSERT INTO work_unit_repository_set_revision(work_unit_id, task_id, revision)
SELECT work_unit.id, work_unit.task_id, 1 FROM work_unit;
CREATE TRIGGER work_unit_repository_set_revision_insert_guard
BEFORE INSERT ON work_unit_repository_set_revision
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit work_unit
    JOIN task_repository_set_revision revision
      ON revision.task_id = NEW.task_id AND revision.revision = NEW.revision
     AND revision.state = 'current'
    WHERE work_unit.id = NEW.work_unit_id AND work_unit.task_id = NEW.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit must pin the current repository revision of its Task');
END;
CREATE TRIGGER work_unit_repository_set_revision_immutable_update
BEFORE UPDATE ON work_unit_repository_set_revision
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository revision pin is immutable');
END;
CREATE TRIGGER work_unit_repository_set_revision_immutable_delete
BEFORE DELETE ON work_unit_repository_set_revision
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository revision pin is immutable outside Project teardown');
END;

-- The legacy scope identity guard also blocked the one-time repo backfill.
DROP TRIGGER workspace_scope_immutable_update;
ALTER TABLE workspace_scope ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
UPDATE workspace_scope
   SET repo_id = (SELECT workspace.repo_id FROM workspace WHERE workspace.id = workspace_scope.workspace_id);
CREATE TEMP TABLE _v183_workspace_scope_repo_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_workspace_scope_repo_guard
SELECT COUNT(*) FROM workspace_scope WHERE repo_id IS NULL;
DROP TABLE _v183_workspace_scope_repo_guard;
CREATE TRIGGER workspace_scope_immutable_update
BEFORE UPDATE ON workspace_scope
BEGIN
    SELECT RAISE(ABORT, 'Workspace scope identity is immutable');
END;
DROP INDEX IF EXISTS idx_workspace_scope_integration;
DROP INDEX IF EXISTS idx_workspace_scope_work_unit;
CREATE UNIQUE INDEX idx_workspace_scope_integration
    ON workspace_scope(task_id, repo_id) WHERE scope_kind = 'integration';
CREATE INDEX idx_workspace_scope_work_unit
    ON workspace_scope(work_unit_id, workspace_id, repo_id)
    WHERE scope_kind = 'work_unit';

CREATE TABLE work_unit_repository_integration (
    id                         TEXT PRIMARY KEY,
    work_unit_integration_id   TEXT NOT NULL
        REFERENCES work_unit_integration(id) ON DELETE RESTRICT,
    task_id                    TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    work_unit_id               TEXT NOT NULL,
    execution_id               TEXT NOT NULL,
    repository_set_revision    INTEGER NOT NULL,
    repo_id                    TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    source_workspace_id        TEXT NOT NULL,
    source_branch              TEXT NOT NULL CHECK (length(trim(source_branch)) > 0),
    source_sha                 TEXT NOT NULL CHECK (length(trim(source_sha)) > 0),
    target_workspace_id        TEXT NOT NULL,
    target_branch              TEXT NOT NULL CHECK (length(trim(target_branch)) > 0),
    target_before_sha          TEXT NOT NULL CHECK (length(trim(target_before_sha)) > 0),
    target_after_sha           TEXT,
    outcome                    TEXT NOT NULL CHECK (outcome IN
        ('running', 'success', 'unchanged', 'conflict', 'failed', 'rejected')),
    conflict_metadata_json     TEXT CHECK (conflict_metadata_json IS NULL
        OR json_valid(conflict_metadata_json)),
    version                    INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    started_at                 TEXT NOT NULL,
    finished_at                TEXT,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    UNIQUE (work_unit_integration_id, repo_id),
    UNIQUE (id, task_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (source_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (target_workspace_id, task_id)
        REFERENCES workspace(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repository_set_revision, repo_id)
        REFERENCES task_repository_set_revision_repo(task_id, revision, repo_id)
        ON DELETE RESTRICT,
    CHECK ((outcome = 'running' AND finished_at IS NULL AND target_after_sha IS NULL)
        OR (outcome != 'running' AND finished_at IS NOT NULL)),
    CHECK ((outcome IN ('success', 'unchanged') AND target_after_sha IS NOT NULL)
        OR (outcome NOT IN ('success', 'unchanged') AND target_after_sha IS NULL))
);
CREATE INDEX idx_work_unit_repository_integration_task_repo
    ON work_unit_repository_integration(task_id, repo_id, started_at DESC, id DESC);
INSERT INTO work_unit_repository_integration(
    id, work_unit_integration_id, task_id, work_unit_id, execution_id,
    repository_set_revision, repo_id, source_workspace_id, source_branch, source_sha,
    target_workspace_id, target_branch, target_before_sha, target_after_sha,
    outcome, conflict_metadata_json, version, started_at, finished_at, created_at, updated_at
)
SELECT 'legacy:' || integration.id || ':' || target_workspace.repo_id,
       integration.id, integration.task_id, integration.work_unit_id,
       integration.execution_id, pin.revision, target_workspace.repo_id,
       integration.source_workspace_id, integration.source_branch, integration.source_sha,
       integration.target_workspace_id, integration.target_branch,
       integration.target_before_sha, integration.target_after_sha,
       integration.outcome, integration.conflict_metadata_json, integration.version,
       integration.started_at, integration.finished_at, integration.created_at,
       integration.updated_at
FROM work_unit_integration integration
JOIN work_unit_repository_set_revision pin
  ON pin.work_unit_id = integration.work_unit_id AND pin.task_id = integration.task_id
JOIN task_repository_set_revision_repo repository
  ON repository.task_id = pin.task_id AND repository.revision = pin.revision
 AND repository.role IN ('primary', 'affected')
JOIN workspace source_workspace
  ON source_workspace.id = integration.source_workspace_id
 AND source_workspace.task_id = integration.task_id
JOIN workspace target_workspace
  ON target_workspace.id = integration.target_workspace_id
 AND target_workspace.task_id = integration.task_id
 AND target_workspace.repo_id = source_workspace.repo_id
 AND target_workspace.repo_id = repository.repo_id;

-- TaskMerge and PR publication are still one Task-wide serialized operation,
-- but each operation now carries its exact repository and batch receipt.
CREATE TABLE task_repository_merge_batch (
    id                       TEXT PRIMARY KEY,
    task_id                  TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    gate_evaluation_id       TEXT NOT NULL REFERENCES gate_evaluation(id) ON DELETE RESTRICT,
    repository_set_revision  INTEGER NOT NULL,
    repository_set_digest    TEXT CHECK (repository_set_digest IS NULL
        OR length(repository_set_digest) = 64),
    state                    TEXT NOT NULL DEFAULT 'integrating'
        CHECK (state IN ('integrating', 'publishing', 'open', 'completed', 'blocked', 'failed')),
    version                  INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL,
    UNIQUE (id, task_id),
    FOREIGN KEY (task_id, repository_set_revision)
        REFERENCES task_repository_set_revision(task_id, revision) ON DELETE RESTRICT
);
CREATE UNIQUE INDEX idx_task_repository_merge_batch_active
    ON task_repository_merge_batch(task_id)
    WHERE state IN ('integrating', 'publishing', 'open', 'blocked');

ALTER TABLE task_integration_operation
    ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
ALTER TABLE task_integration_operation
    ADD COLUMN repository_merge_batch_id TEXT
        REFERENCES task_repository_merge_batch(id) ON DELETE RESTRICT;
UPDATE task_integration_operation
   SET repo_id = (SELECT task.repo_id FROM task WHERE task.id = task_integration_operation.task_id);
UPDATE task_integration_operation
   SET repository_merge_batch_id = id
 WHERE kind = 'task_merge' AND status = 'running';
UPDATE task_integration_operation
   SET repository_merge_batch_id = (
       SELECT merge_op.repository_merge_batch_id
       FROM task_integration_operation merge_op
       WHERE merge_op.id = task_integration_operation.parent_operation_id
   )
 WHERE kind = 'publish_pr' AND status = 'running'
   AND parent_operation_id IS NOT NULL;
CREATE INDEX idx_task_integration_operation_repo
    ON task_integration_operation(task_id, repo_id, created_at, id);
CREATE INDEX idx_task_integration_operation_merge_batch_repo
    ON task_integration_operation(repository_merge_batch_id, repo_id, created_at, id)
    WHERE repository_merge_batch_id IS NOT NULL;

ALTER TABLE remote_pr_admission
    ADD COLUMN repo_id TEXT REFERENCES repo(id) ON DELETE RESTRICT;
UPDATE remote_pr_admission
   SET repo_id = (
       SELECT config.repo_id FROM pr_provider_config config
       WHERE config.id = remote_pr_admission.provider_config_id
   );
CREATE TEMP TABLE _v183_remote_pr_repo_backfill_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_remote_pr_repo_backfill_guard
SELECT COUNT(*) FROM remote_pr_admission WHERE repo_id IS NULL;
DROP TABLE _v183_remote_pr_repo_backfill_guard;
CREATE INDEX idx_remote_pr_admission_repo_reconcile
    ON remote_pr_admission(repo_id, state, updated_at, task_merge_operation_id);

CREATE TABLE task_repository_merge_batch_repo (
    batch_id                              TEXT NOT NULL,
    task_id                               TEXT NOT NULL,
    repository_set_revision              INTEGER NOT NULL,
    repo_id                               TEXT NOT NULL,
    position                              INTEGER NOT NULL CHECK (position >= 0),
    state                                 TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'integrating', 'integrated', 'publishing',
                         'open', 'merged', 'conflict', 'closed', 'failed')),
    work_unit_repository_integration_id   TEXT,
    task_merge_operation_id               TEXT REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    pr_metadata_id                        TEXT REFERENCES pr_metadata(id) ON DELETE RESTRICT,
    before_sha                            TEXT,
    after_sha                             TEXT,
    created_at                            TEXT NOT NULL,
    updated_at                            TEXT NOT NULL,
    PRIMARY KEY (batch_id, repo_id),
    UNIQUE (batch_id, position),
    FOREIGN KEY (batch_id, task_id)
        REFERENCES task_repository_merge_batch(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (task_id, repository_set_revision, repo_id)
        REFERENCES task_repository_set_revision_repo(task_id, revision, repo_id) ON DELETE RESTRICT,
    FOREIGN KEY (work_unit_repository_integration_id, task_id, repo_id)
        REFERENCES work_unit_repository_integration(id, task_id, repo_id) ON DELETE RESTRICT,
    CHECK (state NOT IN ('open', 'merged') OR pr_metadata_id IS NOT NULL)
);
CREATE INDEX idx_task_repository_merge_batch_repo_state
    ON task_repository_merge_batch_repo(batch_id, state, position);

-- Keep a Task already in Merging attached to its exact durable reason_ref.
-- The historical batch id equals the running TaskMerge operation id.
CREATE TEMP TABLE _v183_running_merge_scope_guard (
    unresolved INTEGER NOT NULL CHECK (unresolved = 0)
);
INSERT INTO _v183_running_merge_scope_guard
SELECT COUNT(*) FROM task_integration_operation op
WHERE op.kind = 'task_merge' AND op.status = 'running'
  AND (op.gate_evaluation_id IS NULL OR op.repo_id IS NULL
       OR NOT EXISTS (
           SELECT 1 FROM task_repository_set_revision revision
           WHERE revision.task_id = op.task_id AND revision.state = 'current'
       ));
DROP TABLE _v183_running_merge_scope_guard;

INSERT INTO task_repository_merge_batch(
    id, task_id, gate_evaluation_id, repository_set_revision, repository_set_digest,
    state, version, created_at, updated_at
)
SELECT op.id, op.task_id, op.gate_evaluation_id,
       COALESCE((SELECT revision.revision FROM task_repository_set_revision revision
                 WHERE revision.task_id = op.task_id AND revision.state = 'current'), 1),
       NULL,
       CASE WHEN admission.state = 'open' THEN 'open'
            WHEN admission.state = 'closed' THEN 'blocked'
            WHEN op.remote_waiting = 1 THEN 'publishing'
            ELSE 'integrating' END,
       1, op.created_at, op.updated_at
FROM task_integration_operation op
LEFT JOIN remote_pr_admission admission
  ON admission.task_merge_operation_id = op.id
WHERE op.kind = 'task_merge' AND op.status = 'running';

INSERT INTO task_repository_merge_batch_repo(
    batch_id, task_id, repository_set_revision, repo_id, position, state,
    task_merge_operation_id, pr_metadata_id, before_sha, after_sha,
    created_at, updated_at
)
SELECT batch.id, batch.task_id, batch.repository_set_revision, op.repo_id, 0,
       CASE WHEN admission.state = 'open' THEN 'open'
            WHEN admission.state = 'merged' THEN 'merged'
            WHEN admission.state = 'closed' THEN 'closed'
            WHEN op.remote_waiting = 1 THEN 'publishing'
            ELSE 'integrating' END,
       op.id, metadata.id, NULL, NULL, op.created_at, op.updated_at
FROM task_repository_merge_batch batch
JOIN task_integration_operation op ON op.id = batch.id
LEFT JOIN remote_pr_admission admission ON admission.task_merge_operation_id = op.id
LEFT JOIN pr_metadata metadata ON metadata.task_merge_operation_id = op.id
WHERE op.kind = 'task_merge' AND op.status = 'running';


CREATE TABLE pr_metadata_v183 (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id),
    repo_id                 TEXT REFERENCES repo(id) ON DELETE RESTRICT,
    provider_type           TEXT NOT NULL,
    provider_pr_id          TEXT,
    pr_url                  TEXT,
    source_branch           TEXT NOT NULL,
    target_branch           TEXT NOT NULL,
    pr_state                TEXT NOT NULL DEFAULT 'draft',
    merge_status            TEXT NOT NULL DEFAULT 'pending',
    last_synced_at          TEXT,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    task_merge_operation_id TEXT
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    publish_operation_id    TEXT
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    admission_status        TEXT NOT NULL DEFAULT 'legacy_unadmitted'
        CHECK (admission_status IN (
            'admitted', 'reconciliation_required', 'open', 'merged', 'closed',
            'failed', 'legacy_unadmitted'
        )),
    UNIQUE (task_id, repo_id)
);

INSERT INTO pr_metadata_v183(
    id, task_id, repo_id, provider_type, provider_pr_id, pr_url,
    source_branch, target_branch, pr_state, merge_status, last_synced_at,
    created_at, updated_at, task_merge_operation_id, publish_operation_id,
    admission_status
)
SELECT m.id, m.task_id,
       COALESCE(merge_op.repo_id, admission.repo_id, t.repo_id),
       m.provider_type, m.provider_pr_id, m.pr_url,
       m.source_branch, m.target_branch, m.pr_state, m.merge_status,
       m.last_synced_at, m.created_at, m.updated_at,
       m.task_merge_operation_id, m.publish_operation_id, m.admission_status
FROM pr_metadata AS m
LEFT JOIN task AS t ON t.id = m.task_id
LEFT JOIN task_integration_operation AS merge_op
  ON merge_op.id = m.task_merge_operation_id
LEFT JOIN remote_pr_admission AS admission
  ON admission.metadata_id = m.id;

-- Preserve cross-table trigger SQL and inbound FK target names. With
-- foreign_keys=OFF and legacy_alter_table=ON, this rename does not rewrite
-- references in other tables' triggers or foreign keys. Attached triggers and
-- autoindexes move with the legacy table and disappear when it is dropped.
ALTER TABLE pr_metadata RENAME TO pr_metadata_legacy;
ALTER TABLE pr_metadata_v183 RENAME TO pr_metadata;
DROP TABLE pr_metadata_legacy;

-- The composite UNIQUE index is the task_id/repo_id lookup index. This index
-- supports repo-oriented sync/reconciliation without duplicating that prefix.
CREATE INDEX idx_pr_metadata_repo_task
    ON pr_metadata(repo_id, task_id) WHERE repo_id IS NOT NULL;

CREATE TRIGGER pr_metadata_merge_admission_guard_insert
BEFORE INSERT ON pr_metadata
WHEN NEW.task_merge_operation_id IS NULL
  OR NEW.publish_operation_id IS NULL
  OR NOT EXISTS (
      SELECT 1
      FROM task_integration_operation AS merge_op
      JOIN task ON task.id = merge_op.task_id
      JOIN task_integration_operation AS publish_op
        ON publish_op.id = NEW.publish_operation_id
       AND publish_op.parent_operation_id = merge_op.id
       AND publish_op.task_id = merge_op.task_id
      WHERE merge_op.id = NEW.task_merge_operation_id
        AND merge_op.task_id = NEW.task_id
        AND COALESCE(merge_op.repo_id,
                     task.repo_id) = COALESCE(NEW.repo_id, task.repo_id)
        AND (NEW.repo_id IS NOT NULL OR (
            merge_op.repository_merge_batch_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM task_requires_repository_merge_batch r
                            WHERE r.task_id = merge_op.task_id)
        ))
        AND merge_op.kind = 'task_merge'
        AND merge_op.status = 'running'
        AND merge_op.remote_waiting = 1
        AND publish_op.kind = 'publish_pr'
        AND COALESCE(publish_op.repo_id, task.repo_id) = COALESCE(NEW.repo_id, task.repo_id)
        AND publish_op.repository_merge_batch_id IS merge_op.repository_merge_batch_id
        AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
  )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata requires its exact repo-scoped TaskMerge and PublishPr');
END;

CREATE TRIGGER pr_metadata_merge_admission_guard_update
BEFORE UPDATE OF task_merge_operation_id, publish_operation_id ON pr_metadata
WHEN (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
      OR NEW.publish_operation_id IS NOT OLD.publish_operation_id)
 AND (
    NEW.task_merge_operation_id IS NULL
    OR NEW.publish_operation_id IS NULL
    OR (OLD.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM remote_pr_admission AS paused_admission
        JOIN pr_rework_supersession AS paused
          ON paused.id = paused_admission.supersession_id
        WHERE paused_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND paused_admission.publish_operation_id = OLD.publish_operation_id
          AND paused_admission.metadata_id = OLD.id
          AND paused_admission.task_id = OLD.task_id
          AND paused_admission.repo_id IS OLD.repo_id
          AND paused_admission.rework_outcome = 'superseded_for_rework'
          AND paused.task_id = OLD.task_id
          AND paused.state IN ('open', 'remote_closed')
    ) AND NOT EXISTS (
        SELECT 1
        FROM remote_pr_admission AS old_admission
        JOIN remote_pr_history AS history
          ON history.task_merge_operation_id = old_admission.task_merge_operation_id
         AND history.task_id = old_admission.task_id
         AND history.original_metadata_id = old_admission.metadata_id
        WHERE old_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND old_admission.publish_operation_id = OLD.publish_operation_id
          AND old_admission.metadata_id = OLD.id
          AND old_admission.task_id = OLD.task_id
          AND old_admission.repo_id IS OLD.repo_id
          AND old_admission.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
          AND history.provider_type = OLD.provider_type
          AND history.provider_pr_id IS OLD.provider_pr_id
          AND history.pr_url IS OLD.pr_url
          AND history.source_branch = OLD.source_branch
          AND history.target_branch = OLD.target_branch
          AND history.pr_state = OLD.pr_state
          AND history.merge_status = OLD.merge_status
          AND history.admission_status = OLD.admission_status
    ))
    OR NOT EXISTS (
        SELECT 1
        FROM task_integration_operation AS merge_op
        JOIN task_integration_operation AS publish_op
          ON publish_op.parent_operation_id = merge_op.id
         AND publish_op.task_id = merge_op.task_id
        JOIN task ON task.id = merge_op.task_id
        WHERE merge_op.id = NEW.task_merge_operation_id
          AND merge_op.task_id = NEW.task_id
          AND COALESCE(merge_op.repo_id,
                       task.repo_id) = COALESCE(NEW.repo_id, task.repo_id)
          AND (NEW.repo_id IS NOT NULL OR (
              merge_op.repository_merge_batch_id IS NULL
              AND NOT EXISTS (SELECT 1 FROM task_requires_repository_merge_batch r
                              WHERE r.task_id = merge_op.task_id)
          ))
          AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.remote_waiting = 1
          AND publish_op.id = NEW.publish_operation_id
          AND publish_op.kind = 'publish_pr'
          AND publish_op.status = 'running'
          AND COALESCE(publish_op.repo_id, task.repo_id) = COALESCE(NEW.repo_id, task.repo_id)
          AND publish_op.repository_merge_batch_id IS merge_op.repository_merge_batch_id
          AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata rebind requires exact repo-scoped history and admission');
END;

CREATE TRIGGER pr_metadata_admission_status_guard
BEFORE UPDATE OF admission_status ON pr_metadata
WHEN NEW.admission_status IS NOT OLD.admission_status
 AND NEW.admission_status != 'legacy_unadmitted'
 AND NOT EXISTS (
     SELECT 1 FROM remote_pr_admission AS admission
     WHERE admission.metadata_id = NEW.id
       AND admission.task_id = NEW.task_id
       AND admission.repo_id IS NEW.repo_id
       AND admission.task_merge_operation_id = NEW.task_merge_operation_id
       AND admission.publish_operation_id = NEW.publish_operation_id
       AND (
           (NEW.admission_status = 'admitted' AND admission.state = 'admitted')
           OR (NEW.admission_status = 'reconciliation_required'
               AND admission.state = 'reconciliation_required')
           OR (NEW.admission_status = 'open' AND admission.state = 'open')
           OR (NEW.admission_status = 'merged' AND admission.state = 'merged')
           OR (NEW.admission_status = 'closed' AND admission.state = 'closed')
           OR (NEW.admission_status = 'failed'
               AND admission.state IN ('head_mismatch', 'publication_failed'))
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR admission provenance must match its exact repo-scoped remote admission');
END;

CREATE TRIGGER pr_metadata_repo_insert_guard
BEFORE INSERT ON pr_metadata
WHEN NOT EXISTS (
    SELECT 1
    FROM task AS t
    JOIN repo AS r ON r.id = COALESCE(NEW.repo_id, t.repo_id) AND r.project_id = t.project_id
    JOIN task_integration_operation AS merge_op
      ON merge_op.id = NEW.task_merge_operation_id
     AND merge_op.task_id = t.id
    WHERE t.id = NEW.task_id
      AND COALESCE(merge_op.repo_id, t.repo_id) = COALESCE(NEW.repo_id, t.repo_id)
      AND (
          (merge_op.repository_merge_batch_id IS NOT NULL AND NEW.repo_id IS NOT NULL AND EXISTS (
              SELECT 1 FROM task_repository_merge_batch_repo batch_repo
              WHERE batch_repo.batch_id = merge_op.repository_merge_batch_id
                AND batch_repo.task_id = t.id AND batch_repo.repo_id = NEW.repo_id
          ))
          OR (merge_op.repository_merge_batch_id IS NULL
              AND (NEW.repo_id IS NULL OR NEW.repo_id = t.repo_id)
              AND NOT EXISTS (SELECT 1 FROM task_requires_repository_merge_batch r
                              WHERE r.task_id = t.id))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'new PR metadata requires its exact writable repository authority');
END;

CREATE TRIGGER pr_metadata_repo_immutable
BEFORE UPDATE OF repo_id ON pr_metadata
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    SELECT RAISE(ABORT, 'PR metadata repository identity is immutable');
END;

-- V183 repo-set, integration, batch, and retained-history guards follow.

DROP TRIGGER work_unit_execution_admission_guard;
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
              AND EXISTS (
                  SELECT 1 FROM work_unit_review_subject_repository subject_scope
                  WHERE subject_scope.review_work_unit_id = w.id
                    AND subject_scope.task_id = w.task_id
              )
              AND (
                  NEW.workspace_id IS NOT NULL
                  OR NOT EXISTS (
                      SELECT 1 FROM workspace_group g
                      WHERE g.task_id = w.task_id AND g.work_unit_id = w.id
                  )
                  OR NOT EXISTS (
                      SELECT 1 FROM work_unit_review_subject_repository subject
                      WHERE subject.review_work_unit_id = w.id
                        AND subject.task_id = w.task_id
                        AND subject.result_execution_id IS NOT NULL
                  )
                  OR EXISTS (
                      SELECT 1 FROM work_unit_review_subject_repository subject
                      WHERE subject.review_work_unit_id = w.id
                        AND subject.task_id = w.task_id
                        AND (subject.result_execution_id IS NULL OR NOT EXISTS (
                            SELECT 1 FROM workspace_scope ws
                            JOIN workspace bound_workspace
                              ON bound_workspace.id = ws.workspace_id
                             AND bound_workspace.task_id = ws.task_id
                            JOIN workspace_group g
                              ON g.id = ws.workspace_group_id
                             AND g.task_id = ws.task_id
                             AND g.work_unit_id = w.id
                            WHERE ws.work_unit_id = w.id AND ws.task_id = w.task_id
                              AND ws.scope_kind = 'work_unit'
                              AND bound_workspace.repo_id = subject.repo_id
                              AND bound_workspace.status = 'ready'
                              AND bound_workspace.before_sha IS subject.result_commit_sha
                        ))
                  )
                  OR EXISTS (
                      SELECT 1 FROM workspace_scope ws
                      JOIN workspace bound_workspace
                        ON bound_workspace.id = ws.workspace_id
                       AND bound_workspace.task_id = ws.task_id
                      WHERE ws.work_unit_id = w.id AND ws.task_id = w.task_id
                        AND ws.scope_kind = 'work_unit'
                        AND NOT EXISTS (
                            SELECT 1 FROM work_unit_review_subject_repository subject
                            WHERE subject.review_work_unit_id = w.id
                              AND subject.task_id = w.task_id
                              AND subject.repo_id = bound_workspace.repo_id
                              AND subject.result_execution_id IS NOT NULL
                              AND subject.result_commit_sha IS bound_workspace.before_sha
                        )
                  )
                  OR EXISTS (
                      SELECT 1 FROM workspace_group g
                      JOIN workspace_scope ws
                        ON ws.workspace_group_id = g.id AND ws.task_id = g.task_id
                       AND ws.work_unit_id = g.work_unit_id
                      JOIN workspace bound_workspace
                        ON bound_workspace.id = ws.workspace_id
                       AND bound_workspace.task_id = ws.task_id
                      WHERE g.task_id = w.task_id AND g.work_unit_id = w.id
                        AND bound_workspace.status != 'ready'
                  )
              )
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_REVIEW_GROUP_UNAVAILABLE')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.requires_integration = 1
              AND lower(trim(w.role)) != 'reviewer'
              AND (
                  NOT EXISTS (
                      SELECT 1 FROM workspace_group g
                      WHERE g.task_id = w.task_id AND g.work_unit_id = w.id
                  )
                  OR NEW.workspace_id IS NOT (
                      SELECT ws.workspace_id
                      FROM workspace_scope ws
                      JOIN workspace primary_workspace
                        ON primary_workspace.id = ws.workspace_id
                       AND primary_workspace.task_id = ws.task_id
                      JOIN work_unit_repository_set_revision primary_pin
                        ON primary_pin.work_unit_id = w.id AND primary_pin.task_id = w.task_id
                      JOIN task_repository_set_revision_repo primary_repository
                        ON primary_repository.task_id = primary_pin.task_id
                       AND primary_repository.revision = primary_pin.revision
                       AND primary_repository.role = 'primary'
                      WHERE ws.work_unit_id = w.id AND ws.task_id = w.task_id
                        AND ws.scope_kind = 'work_unit'
                        AND primary_workspace.repo_id = primary_repository.repo_id
                  )
                  OR EXISTS (
                      SELECT 1 FROM work_unit_repository_set_revision repository_pin
                      JOIN task_repository_set_revision_repo tr
                        ON tr.task_id = repository_pin.task_id
                       AND tr.revision = repository_pin.revision
                      WHERE repository_pin.work_unit_id = w.id
                        AND repository_pin.task_id = w.task_id
                        AND tr.task_id = w.task_id
                        AND (tr.role = 'context' OR (tr.role IN ('primary', 'affected')
                             AND (NOT EXISTS (
                                 SELECT 1 FROM work_unit_repository selected
                                 WHERE selected.work_unit_id = w.id
                             ) OR EXISTS (
                                 SELECT 1 FROM work_unit_repository selected
                                 WHERE selected.work_unit_id = w.id
                                   AND selected.repo_id = tr.repo_id
                             ))))
                        AND NOT EXISTS (
                            SELECT 1 FROM workspace_scope ws
                            JOIN workspace bound_workspace
                              ON bound_workspace.id = ws.workspace_id
                             AND bound_workspace.task_id = ws.task_id
                            JOIN workspace_group g
                              ON g.id = ws.workspace_group_id
                             AND g.task_id = ws.task_id
                             AND g.work_unit_id = w.id
                            WHERE ws.work_unit_id = w.id AND ws.task_id = w.task_id
                              AND ws.scope_kind = 'work_unit'
                              AND bound_workspace.repo_id = tr.repo_id
                              AND bound_workspace.status = 'ready'
                              AND (tr.role != 'context' OR EXISTS (
                                  SELECT 1 FROM task_repository_context_snapshot pin
                                  WHERE pin.task_id = tr.task_id AND pin.repo_id = tr.repo_id
                                    AND pin.commit_sha IS bound_workspace.before_sha
                              ))
                        )
                  )
                  OR EXISTS (
                      SELECT 1 FROM workspace_group g
                      JOIN workspace_scope ws
                        ON ws.workspace_group_id = g.id AND ws.task_id = g.task_id
                       AND ws.work_unit_id = g.work_unit_id
                      JOIN workspace bound_workspace
                        ON bound_workspace.id = ws.workspace_id
                       AND bound_workspace.task_id = ws.task_id
                      LEFT JOIN work_unit_repository_set_revision repository_pin
                        ON repository_pin.work_unit_id = w.id AND repository_pin.task_id = w.task_id
                      LEFT JOIN task_repository_set_revision_repo tr
                        ON tr.task_id = repository_pin.task_id
                       AND tr.revision = repository_pin.revision
                       AND tr.repo_id = bound_workspace.repo_id
                      WHERE g.task_id = w.task_id AND g.work_unit_id = w.id
                        AND ws.scope_kind = 'work_unit'
                        AND (bound_workspace.status != 'ready' OR tr.repo_id IS NULL
                             OR (tr.role = 'context' AND NOT EXISTS (
                                 SELECT 1 FROM task_repository_context_snapshot pin
                                 WHERE pin.task_id = tr.task_id AND pin.repo_id = tr.repo_id
                                   AND pin.commit_sha IS bound_workspace.before_sha
                             ))
                             OR (tr.role NOT IN ('primary', 'affected', 'context'))
                             OR (tr.role IN ('primary', 'affected') AND EXISTS (
                                 SELECT 1 FROM work_unit_repository selected
                                 WHERE selected.work_unit_id = w.id
                                   AND selected.repo_id != tr.repo_id
                             ) AND NOT EXISTS (
                                 SELECT 1 FROM work_unit_repository selected
                                 WHERE selected.work_unit_id = w.id
                                   AND selected.repo_id = tr.repo_id
                             )))
                  )
              )
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORKSPACE_UNAVAILABLE')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.requires_integration = 0
              AND lower(trim(w.role)) != 'reviewer'
        ) AND NEW.workspace_id IS NOT NULL
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

DROP TRIGGER work_unit_completion_readiness_guard;
CREATE TRIGGER work_unit_completion_readiness_guard
BEFORE UPDATE OF status ON work_unit
WHEN NEW.status = 'completed'
 AND (
     EXISTS (
         SELECT 1 FROM work_unit_dependency d
         JOIN work_unit prerequisite ON prerequisite.id = d.depends_on_work_unit_id
                                    AND prerequisite.task_id = d.task_id
         WHERE d.work_unit_id = OLD.id
           AND (prerequisite.status != 'completed'
                OR (prerequisite.requires_integration = 1 AND NOT EXISTS (
                    SELECT 1 FROM work_unit_integration i
                    WHERE i.work_unit_id = prerequisite.id
                      AND i.task_id = prerequisite.task_id
                      AND i.outcome = 'success'
                )))
     )
     OR (NEW.requires_integration = 1 AND NOT EXISTS (
         SELECT 1 FROM execution e
         WHERE e.work_unit_id = OLD.id AND e.task_id = OLD.task_id
           AND e.work_unit_version = OLD.version AND e.status = 'completed'
           AND NOT EXISTS (
               SELECT 1 FROM work_unit_repository_set_revision repository_pin
               JOIN task_repository_set_revision_repo tr
                 ON tr.task_id = repository_pin.task_id
                AND tr.revision = repository_pin.revision
               WHERE repository_pin.work_unit_id = OLD.id
                 AND repository_pin.task_id = OLD.task_id
                 AND tr.task_id = OLD.task_id AND tr.role IN ('primary', 'affected')
                 AND (NOT EXISTS (
                     SELECT 1 FROM work_unit_repository selected
                     WHERE selected.work_unit_id = OLD.id
                 ) OR EXISTS (
                     SELECT 1 FROM work_unit_repository selected
                     WHERE selected.work_unit_id = OLD.id AND selected.repo_id = tr.repo_id
                 ))
                 AND NOT EXISTS (
                     SELECT 1 FROM execution_repository_result result
                     WHERE result.execution_id = e.id AND result.task_id = e.task_id
                       AND result.repo_id = tr.repo_id AND result.after_sha IS NOT NULL
                       AND EXISTS (
                           SELECT 1 FROM workspace_scope scope
                           JOIN workspace workspace
                             ON workspace.id = scope.workspace_id
                            AND workspace.task_id = scope.task_id
                           WHERE scope.work_unit_id = OLD.id AND scope.task_id = OLD.task_id
                             AND scope.scope_kind = 'work_unit'
                             AND workspace.repo_id = tr.repo_id
                             AND result.workspace_id = workspace.id
                       )
                 )
           )
     ))
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit completion requires satisfied dependencies and exact repository results');
END;

DROP TRIGGER work_unit_review_subject_repository_insert_guard;
CREATE TRIGGER work_unit_review_subject_repository_insert_guard
BEFORE INSERT ON work_unit_review_subject_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit review_wu
    JOIN work_unit reviewed_wu
      ON reviewed_wu.id = NEW.reviewed_work_unit_id
     AND reviewed_wu.task_id = NEW.task_id
    JOIN work_unit_review_subject scalar_subject
      ON scalar_subject.review_work_unit_id = NEW.review_work_unit_id
     AND scalar_subject.task_id = NEW.task_id
     AND scalar_subject.reviewed_work_unit_id = NEW.reviewed_work_unit_id
     AND scalar_subject.reviewed_work_unit_version IS NEW.reviewed_work_unit_version
     AND scalar_subject.result_execution_id IS NEW.result_execution_id
     AND scalar_subject.result_artifact_id IS NEW.result_artifact_id
    JOIN work_unit_repository_set_revision repository_pin
      ON repository_pin.work_unit_id = reviewed_wu.id
     AND repository_pin.task_id = reviewed_wu.task_id
    JOIN task_repository_set_revision_repo tr
      ON tr.task_id = repository_pin.task_id AND tr.revision = repository_pin.revision
     AND tr.repo_id = NEW.repo_id AND tr.role IN ('primary', 'affected')
    WHERE review_wu.id = NEW.review_work_unit_id
      AND review_wu.task_id = NEW.task_id
      AND (NOT EXISTS (
          SELECT 1 FROM work_unit_repository scope
          WHERE scope.work_unit_id = NEW.reviewed_work_unit_id
      ) OR EXISTS (
          SELECT 1 FROM work_unit_repository scope
          WHERE scope.work_unit_id = NEW.reviewed_work_unit_id
            AND scope.repo_id = NEW.repo_id
      ))
      AND (NEW.result_execution_id IS NULL OR EXISTS (
          SELECT 1 FROM execution e
          WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
            AND e.work_unit_id = NEW.reviewed_work_unit_id
            AND e.status = 'completed'
            AND e.work_unit_version = NEW.reviewed_work_unit_version
            AND EXISTS (
                SELECT 1 FROM execution_repository_result rr
                WHERE rr.execution_id = e.id AND rr.task_id = e.task_id
                  AND rr.repo_id = NEW.repo_id
                  AND rr.workspace_id = NEW.result_workspace_id
                  AND rr.after_sha IS NEW.result_commit_sha
                  AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
            )
      ))
      AND (NEW.result_workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
            AND w.repo_id = NEW.repo_id
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'review subject repo must bind exact same-Task WorkUnits and result');
END;

DROP TRIGGER work_unit_result_requirement_repository_insert_guard;
CREATE TRIGGER work_unit_result_requirement_repository_insert_guard
BEFORE INSERT ON work_unit_result_requirement_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit wu
    JOIN work_unit source_wu
      ON source_wu.id = NEW.source_work_unit_id AND source_wu.task_id = NEW.task_id
    JOIN work_unit_repository_set_revision repository_pin
      ON repository_pin.work_unit_id = source_wu.id
     AND repository_pin.task_id = source_wu.task_id
    JOIN task_repository_set_revision_repo tr
      ON tr.task_id = repository_pin.task_id AND tr.revision = repository_pin.revision
     AND tr.repo_id = NEW.repo_id AND tr.role IN ('primary', 'affected')
    JOIN execution e
      ON e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
     AND e.work_unit_id = NEW.source_work_unit_id
     AND e.work_unit_version = NEW.source_work_unit_version
     AND e.status = 'completed'
    JOIN execution_repository_result rr
      ON rr.execution_id = e.id AND rr.task_id = e.task_id
     AND rr.repo_id = NEW.repo_id
    WHERE wu.id = NEW.work_unit_id AND wu.task_id = NEW.task_id
      AND rr.workspace_id = NEW.result_workspace_id
      AND rr.after_sha IS NEW.result_commit_sha
      AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
      AND EXISTS (
          SELECT 1 FROM work_unit_result_requirement scalar_requirement
          WHERE scalar_requirement.work_unit_id = NEW.work_unit_id
            AND scalar_requirement.task_id = NEW.task_id
            AND scalar_requirement.source_work_unit_id = NEW.source_work_unit_id
            AND scalar_requirement.source_work_unit_version = NEW.source_work_unit_version
            AND scalar_requirement.result_execution_id = NEW.result_execution_id
            AND scalar_requirement.result_artifact_id IS NEW.result_artifact_id
      )
      AND (NEW.result_workspace_id IS NULL OR EXISTS (
          SELECT 1 FROM workspace w
          WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
            AND w.repo_id = NEW.repo_id
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'result requirement repo must bind exact same-Task WorkUnits and result');
END;

DROP TRIGGER work_unit_repository_insert_guard;
CREATE TRIGGER work_unit_repository_insert_guard
BEFORE INSERT ON work_unit_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit w
    JOIN work_unit_repository_set_revision pin
      ON pin.work_unit_id = w.id AND pin.task_id = w.task_id
    JOIN task_repository_set_revision_repo repository
      ON repository.task_id = pin.task_id AND repository.revision = pin.revision
     AND repository.repo_id = NEW.repo_id
     AND repository.role IN ('primary', 'affected')
    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
) OR EXISTS (
    SELECT 1 FROM execution e WHERE e.work_unit_id = NEW.work_unit_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository scope must be writable in its pinned revision and fixed before its first Execution');
END;

DROP TRIGGER task_repository_context_snapshot_insert_guard;
CREATE TRIGGER task_repository_context_snapshot_insert_guard
BEFORE INSERT ON task_repository_context_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository_set_revision revision
    JOIN task_repository_set_revision_repo repository
      ON repository.task_id = revision.task_id AND repository.revision = revision.revision
    WHERE revision.task_id = NEW.task_id AND revision.state = 'current'
      AND repository.repo_id = NEW.repo_id AND repository.role = 'context'
)
BEGIN
    SELECT RAISE(ABORT, 'context snapshot must bind a context repository in the current Task revision');
END;

DROP TRIGGER execution_repository_result_insert_guard;
CREATE TRIGGER execution_repository_result_insert_guard
BEFORE INSERT ON execution_repository_result
WHEN NOT EXISTS (
    SELECT 1
    FROM execution e
    JOIN workspace w ON w.id = NEW.workspace_id AND w.task_id = NEW.task_id
    JOIN task_repository_set_revision revision
      ON revision.task_id = NEW.task_id
     AND revision.revision = COALESCE(
         (SELECT pin.revision FROM work_unit_repository_set_revision pin
          WHERE pin.work_unit_id = e.work_unit_id AND pin.task_id = e.task_id),
         (SELECT current_revision.revision FROM task_repository_set_revision current_revision
          WHERE current_revision.task_id = e.task_id AND current_revision.state = 'current')
     )
    JOIN task_repository_set_revision_repo repository
      ON repository.task_id = revision.task_id AND repository.revision = revision.revision
     AND repository.repo_id = NEW.repo_id
     AND repository.role IN ('primary', 'affected')
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND w.repo_id = NEW.repo_id
      AND (e.work_unit_id IS NULL
           OR NOT EXISTS (SELECT 1 FROM work_unit_repository selected
                          WHERE selected.work_unit_id = e.work_unit_id)
           OR EXISTS (SELECT 1 FROM work_unit_repository selected
                      WHERE selected.work_unit_id = e.work_unit_id
                        AND selected.repo_id = NEW.repo_id))
      AND ((e.work_unit_id IS NULL AND EXISTS (
                SELECT 1 FROM workspace_scope ws
                WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                  AND ws.repo_id = NEW.repo_id AND ws.scope_kind = 'integration'
           )) OR (e.work_unit_id IS NOT NULL AND EXISTS (
                SELECT 1 FROM workspace_scope ws
                WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                  AND ws.repo_id = NEW.repo_id AND ws.scope_kind = 'work_unit'
                  AND ws.work_unit_id = e.work_unit_id
                  AND (ws.workspace_group_id IS NULL OR EXISTS (
                      SELECT 1 FROM workspace_group g
                      WHERE g.id = ws.workspace_group_id
                        AND g.task_id = e.task_id AND g.work_unit_id = e.work_unit_id
                  ))
           )))
)
BEGIN
    SELECT RAISE(ABORT, 'repository result must bind the exact pinned Execution, writable repo, and workspace');
END;

DROP TRIGGER execution_repository_result_legacy_scalar_after_update;
CREATE TRIGGER execution_repository_result_legacy_scalar_after_update
AFTER UPDATE OF status, work_unit_id, work_unit_version, after_sha ON execution
WHEN NEW.status = 'completed'
 AND NEW.work_unit_id IS NOT NULL
 AND NEW.workspace_id IS NOT NULL
 AND NEW.after_sha IS NOT NULL
BEGIN
    INSERT INTO execution_repository_result(
        execution_id, task_id, repo_id, workspace_id,
        before_sha, after_sha, snapshot_digest, created_at
    )
    SELECT NEW.id, NEW.task_id, workspace.repo_id, workspace.id,
           workspace.before_sha, NEW.after_sha, NULL, NEW.updated_at
    FROM workspace
    JOIN work_unit_repository_set_revision pin
      ON pin.work_unit_id = NEW.work_unit_id AND pin.task_id = NEW.task_id
    JOIN task_repository_set_revision_repo repository
      ON repository.task_id = pin.task_id AND repository.revision = pin.revision
     AND repository.repo_id = workspace.repo_id
     AND repository.role IN ('primary', 'affected')
    WHERE workspace.id = NEW.workspace_id
      AND workspace.task_id = NEW.task_id
      AND (NOT EXISTS (
          SELECT 1 FROM work_unit_repository selected
          WHERE selected.work_unit_id = NEW.work_unit_id
      ) OR EXISTS (
          SELECT 1 FROM work_unit_repository selected
          WHERE selected.work_unit_id = NEW.work_unit_id
            AND selected.repo_id = workspace.repo_id
      ))
      AND EXISTS (
          SELECT 1 FROM workspace_scope scope
          WHERE scope.workspace_id = workspace.id
            AND scope.task_id = NEW.task_id AND scope.repo_id = workspace.repo_id
            AND scope.scope_kind = 'work_unit' AND scope.work_unit_id = NEW.work_unit_id
      )
      AND NOT EXISTS (
          SELECT 1 FROM execution_repository_result existing
          WHERE existing.execution_id = NEW.id AND existing.repo_id = workspace.repo_id
      );
END;

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

CREATE TRIGGER task_repository_merge_batch_insert_guard
BEFORE INSERT ON task_repository_merge_batch
WHEN NEW.repository_set_digest IS NULL OR length(NEW.repository_set_digest) != 64
  OR NEW.version != 1 OR NEW.state != 'integrating'
  OR NOT EXISTS (
      SELECT 1 FROM gate_evaluation evaluation
      JOIN task_repository_set_revision revision
        ON revision.task_id = evaluation.task_id
       AND revision.revision = NEW.repository_set_revision
       AND revision.state = 'current'
      WHERE evaluation.id = NEW.gate_evaluation_id
        AND evaluation.task_id = NEW.task_id
        AND evaluation.outcome = 'satisfied'
        AND (revision.digest IS NULL OR revision.digest = NEW.repository_set_digest)
  )
BEGIN
    SELECT RAISE(ABORT, 'new repository merge batch requires its exact satisfied Gate and current repository revision');
END;

CREATE TRIGGER task_repository_merge_batch_identity_immutable
BEFORE UPDATE OF id, task_id, gate_evaluation_id, repository_set_revision,
                 repository_set_digest, created_at ON task_repository_merge_batch
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.gate_evaluation_id IS NOT OLD.gate_evaluation_id
  OR NEW.repository_set_revision IS NOT OLD.repository_set_revision
  OR NEW.repository_set_digest IS NOT OLD.repository_set_digest
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'repository merge batch identity and authority are immutable');
END;

CREATE TRIGGER task_repository_merge_batch_state_monotonic
BEFORE UPDATE OF state, version ON task_repository_merge_batch
WHEN NEW.version != OLD.version + 1
  OR NOT (
      OLD.state = NEW.state
      OR (OLD.state = 'integrating' AND NEW.state IN ('publishing', 'open', 'completed', 'blocked', 'failed'))
      OR (OLD.state = 'publishing' AND NEW.state IN ('open', 'completed', 'blocked', 'failed'))
      OR (OLD.state = 'open' AND NEW.state IN ('completed', 'blocked', 'failed'))
      OR (OLD.state = 'blocked' AND NEW.state IN ('publishing', 'open', 'completed', 'failed'))
  )
  OR (NEW.state = 'open' AND EXISTS (
      SELECT 1 FROM task_repository_merge_batch_repo repo
      WHERE repo.batch_id = NEW.id
        AND repo.state NOT IN ('open', 'merged', 'integrated')
  ))
  OR (NEW.state = 'completed' AND EXISTS (
      SELECT 1 FROM task_repository_merge_batch_repo repo
      WHERE repo.batch_id = NEW.id AND repo.state NOT IN ('merged', 'integrated')
  ))
BEGIN
    SELECT RAISE(ABORT, 'repository merge batch state/version transition is not monotonic or complete');
END;

CREATE TRIGGER task_repository_merge_batch_delete_guard
BEFORE DELETE ON task_repository_merge_batch
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'repository merge batch is immutable outside Project teardown');
END;

CREATE TRIGGER task_repository_merge_batch_repo_insert_guard
BEFORE INSERT ON task_repository_merge_batch_repo
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository_merge_batch batch
    JOIN task_repository_set_revision_repo repository
      ON repository.task_id = batch.task_id
     AND repository.revision = batch.repository_set_revision
     AND repository.repo_id = NEW.repo_id
     AND repository.role IN ('primary', 'affected')
    WHERE batch.id = NEW.batch_id AND batch.task_id = NEW.task_id
      AND batch.repository_set_revision = NEW.repository_set_revision
      AND batch.state = 'integrating'
)
 OR (NEW.work_unit_repository_integration_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM work_unit_repository_integration integration
     WHERE integration.id = NEW.work_unit_repository_integration_id
       AND integration.task_id = NEW.task_id AND integration.repo_id = NEW.repo_id
       AND integration.repository_set_revision = NEW.repository_set_revision
       AND integration.outcome = 'success'
 ))
 OR (NEW.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM task_integration_operation operation
     WHERE operation.id = NEW.task_merge_operation_id
       AND operation.task_id = NEW.task_id AND operation.repo_id = NEW.repo_id
       AND operation.repository_merge_batch_id = NEW.batch_id
       AND operation.kind = 'task_merge'
 ))
 OR (NEW.pr_metadata_id IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM pr_metadata metadata
     WHERE metadata.id = NEW.pr_metadata_id
       AND metadata.task_id = NEW.task_id AND metadata.repo_id = NEW.repo_id
 ))
BEGIN
    SELECT RAISE(ABORT, 'merge batch repository row must bind exact revision, integration, operation, and PR identities');
END;

CREATE TRIGGER task_repository_merge_batch_repo_identity_immutable
BEFORE UPDATE OF batch_id, task_id, repository_set_revision, repo_id, position, created_at
ON task_repository_merge_batch_repo
WHEN NEW.batch_id IS NOT OLD.batch_id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.repository_set_revision IS NOT OLD.repository_set_revision
  OR NEW.repo_id IS NOT OLD.repo_id OR NEW.position IS NOT OLD.position
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'merge batch repository identity and order are immutable');
END;

CREATE TRIGGER task_repository_merge_batch_repo_state_monotonic
BEFORE UPDATE OF state ON task_repository_merge_batch_repo
WHEN NOT (
      (OLD.state = 'pending' AND NEW.state IN ('integrating', 'integrated', 'publishing', 'failed'))
      OR (OLD.state = 'integrating' AND NEW.state IN ('integrated', 'conflict', 'failed'))
      OR (OLD.state = 'conflict' AND NEW.state IN ('integrating', 'failed'))
      OR (OLD.state = 'integrated' AND NEW.state IN ('publishing', 'open', 'merged', 'failed'))
      OR (OLD.state = 'publishing' AND NEW.state IN ('open', 'conflict', 'failed'))
      OR (OLD.state = 'open' AND NEW.state IN ('merged', 'closed', 'failed'))
  )
  OR (NEW.state IN ('open', 'merged') AND NEW.pr_metadata_id IS NULL)
  OR (NEW.work_unit_repository_integration_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM work_unit_repository_integration integration
      WHERE integration.id = NEW.work_unit_repository_integration_id
        AND integration.task_id = NEW.task_id AND integration.repo_id = NEW.repo_id
        AND integration.repository_set_revision = NEW.repository_set_revision
        AND integration.outcome = 'success'
  ))
  OR (NEW.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM task_integration_operation operation
      WHERE operation.id = NEW.task_merge_operation_id
        AND operation.task_id = NEW.task_id AND operation.repo_id = NEW.repo_id
        AND operation.repository_merge_batch_id = NEW.batch_id
  ))
  OR (NEW.pr_metadata_id IS NOT NULL AND NOT EXISTS (
      SELECT 1 FROM pr_metadata metadata
      WHERE metadata.id = NEW.pr_metadata_id
        AND metadata.task_id = NEW.task_id AND metadata.repo_id = NEW.repo_id
  ))
BEGIN
    SELECT RAISE(ABORT, 'merge batch repository transition must advance and preserve exact repo authority');
END;

CREATE TRIGGER task_repository_merge_batch_repo_delete_guard
BEFORE DELETE ON task_repository_merge_batch_repo
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'merge batch repository history is immutable outside Project teardown');
END;

CREATE TRIGGER work_unit_repository_integration_insert_guard
BEFORE INSERT ON work_unit_repository_integration
WHEN NEW.outcome != 'running' OR NEW.version != 1
  OR NEW.finished_at IS NOT NULL OR NEW.target_after_sha IS NOT NULL
  OR NOT EXISTS (
      SELECT 1 FROM work_unit_integration parent
      JOIN work_unit wu ON wu.id = NEW.work_unit_id AND wu.task_id = NEW.task_id
      JOIN work_unit_repository_set_revision pin
        ON pin.work_unit_id = wu.id AND pin.task_id = wu.task_id
       AND pin.revision = NEW.repository_set_revision
      JOIN task_repository_set_revision_repo repository
        ON repository.task_id = pin.task_id AND repository.revision = pin.revision
       AND repository.repo_id = NEW.repo_id
       AND repository.role IN ('primary', 'affected')
      JOIN execution e ON e.id = NEW.execution_id AND e.task_id = NEW.task_id
       AND e.work_unit_id = NEW.work_unit_id AND e.status = 'completed'
      JOIN execution_repository_result result
        ON result.execution_id = e.id AND result.task_id = e.task_id
       AND result.repo_id = NEW.repo_id AND result.workspace_id = NEW.source_workspace_id
       AND result.after_sha = NEW.source_sha
      JOIN workspace source_workspace
        ON source_workspace.id = NEW.source_workspace_id
       AND source_workspace.task_id = NEW.task_id AND source_workspace.repo_id = NEW.repo_id
      JOIN workspace target_workspace
        ON target_workspace.id = NEW.target_workspace_id
       AND target_workspace.task_id = NEW.task_id AND target_workspace.repo_id = NEW.repo_id
      JOIN workspace_scope target_scope
        ON target_scope.workspace_id = target_workspace.id
       AND target_scope.task_id = target_workspace.task_id
       AND target_scope.repo_id = NEW.repo_id AND target_scope.scope_kind = 'integration'
      WHERE parent.id = NEW.work_unit_integration_id
        AND parent.task_id = NEW.task_id AND parent.work_unit_id = NEW.work_unit_id
        AND parent.execution_id = NEW.execution_id
        AND parent.outcome IN ('running', 'success', 'conflict', 'failed', 'rejected')
        AND source_workspace.branch = NEW.source_branch
        AND target_workspace.branch = NEW.target_branch
        AND target_workspace.before_sha IS NEW.target_before_sha
  )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository integration must bind its exact pinned result and target Workspace');
END;

CREATE TRIGGER work_unit_repository_integration_identity_immutable
BEFORE UPDATE OF id, work_unit_integration_id, task_id, work_unit_id, execution_id,
                 repository_set_revision, repo_id, source_workspace_id, source_branch,
                 source_sha, target_workspace_id, target_branch, target_before_sha,
                 started_at, created_at ON work_unit_repository_integration
WHEN NEW.id IS NOT OLD.id
  OR NEW.work_unit_integration_id IS NOT OLD.work_unit_integration_id
  OR NEW.task_id IS NOT OLD.task_id OR NEW.work_unit_id IS NOT OLD.work_unit_id
  OR NEW.execution_id IS NOT OLD.execution_id
  OR NEW.repository_set_revision IS NOT OLD.repository_set_revision
  OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.source_workspace_id IS NOT OLD.source_workspace_id
  OR NEW.source_branch IS NOT OLD.source_branch OR NEW.source_sha IS NOT OLD.source_sha
  OR NEW.target_workspace_id IS NOT OLD.target_workspace_id
  OR NEW.target_branch IS NOT OLD.target_branch
  OR NEW.target_before_sha IS NOT OLD.target_before_sha
  OR NEW.started_at IS NOT OLD.started_at OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository integration subject is immutable');
END;

CREATE TRIGGER work_unit_repository_integration_transition_guard
BEFORE UPDATE ON work_unit_repository_integration
WHEN NEW.version != OLD.version + 1
  OR NOT (
      (OLD.outcome = 'running' AND NEW.outcome IN ('success', 'unchanged', 'conflict', 'failed', 'rejected'))
      OR (OLD.outcome = 'conflict' AND NEW.outcome = 'running')
  )
  OR ((NEW.outcome = 'running') != (NEW.finished_at IS NULL))
  OR ((NEW.outcome IN ('success', 'unchanged')) != (NEW.target_after_sha IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository integration transition is invalid or non-monotonic');
END;

CREATE TRIGGER work_unit_repository_integration_delete_guard
BEFORE DELETE ON work_unit_repository_integration
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit repository integration history is immutable outside Project teardown');
END;

DROP TRIGGER task_integration_operation_remote_admission_guard;
CREATE TRIGGER task_integration_operation_remote_admission_guard
BEFORE INSERT ON task_integration_operation
WHEN (NEW.kind = 'task_merge' AND (
        (NEW.repository_merge_batch_id IS NOT NULL AND (
            NEW.repo_id IS NULL OR NOT EXISTS (
            SELECT 1 FROM task_repository_merge_batch_repo row
            WHERE row.batch_id = NEW.repository_merge_batch_id
              AND row.task_id = NEW.task_id AND row.repo_id = NEW.repo_id
            )
        ))
        OR (NEW.repository_merge_batch_id IS NULL AND (
            EXISTS (
                SELECT 1 FROM task_repository first_repo
                JOIN task_repository second_repo
                  ON second_repo.task_id = first_repo.task_id
                 AND second_repo.repo_id != first_repo.repo_id
                WHERE first_repo.task_id = NEW.task_id
                  AND first_repo.role IN ('primary', 'affected')
                  AND second_repo.role IN ('primary', 'affected')
            )
            OR EXISTS (
                SELECT 1 FROM task_repository_set_revision revision
                JOIN task_repository_set_revision_repo first_repo
                  ON first_repo.task_id = revision.task_id
                 AND first_repo.revision = revision.revision
                JOIN task_repository_set_revision_repo second_repo
                  ON second_repo.task_id = first_repo.task_id
                 AND second_repo.revision = first_repo.revision
                 AND second_repo.repo_id != first_repo.repo_id
                WHERE revision.task_id = NEW.task_id AND revision.state = 'current'
                  AND first_repo.role IN ('primary', 'affected')
                  AND second_repo.role IN ('primary', 'affected')
            )
            OR NOT EXISTS (
                SELECT 1 FROM task t
                WHERE t.id = NEW.task_id AND t.repo_id IS NOT NULL
                  AND (NEW.repo_id IS NULL OR NEW.repo_id = t.repo_id)
            )
        ))
    ))
  OR (NEW.remote_waiting = 1 AND (
        NEW.kind != 'task_merge'
        OR NEW.parent_operation_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1 FROM task t JOIN repo r
              ON r.id = COALESCE(NEW.repo_id, t.repo_id) AND r.project_id = t.project_id
            WHERE t.id = NEW.task_id AND r.work_mode = 'pull_request'
        )
     ))
  OR (NEW.kind = 'task_merge' AND NEW.remote_waiting = 0 AND EXISTS (
        SELECT 1 FROM task t JOIN repo r
          ON r.id = COALESCE(NEW.repo_id, t.repo_id) AND r.project_id = t.project_id
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
              AND ((merge_op.repository_merge_batch_id IS NULL
                    AND lifecycle.reason_ref IS merge_op.id)
                   OR (merge_op.repository_merge_batch_id IS NOT NULL
                       AND lifecycle.reason_ref IS merge_op.repository_merge_batch_id))
              AND merge_op.repo_id IS NEW.repo_id
              AND merge_op.repository_merge_batch_id IS NEW.repository_merge_batch_id
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
          AND ((merge_op.repository_merge_batch_id IS NULL
                AND lifecycle.reason_ref IS merge_op.id)
               OR (merge_op.repository_merge_batch_id IS NOT NULL
                   AND lifecycle.reason_ref IS merge_op.repository_merge_batch_id))
          AND merge_op.repo_id IS NEW.repo_id
          AND merge_op.repository_merge_batch_id IS NEW.repository_merge_batch_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'PR publication requires its exact durable TaskMerge admission');
END;

CREATE TRIGGER task_integration_operation_repo_batch_insert_guard
BEFORE INSERT ON task_integration_operation
WHEN (NEW.kind = 'task_merge' AND (
        (NEW.repository_merge_batch_id IS NULL AND (
            EXISTS (
                SELECT 1 FROM task_repository first_repo
                JOIN task_repository second_repo
                  ON second_repo.task_id = first_repo.task_id
                 AND second_repo.repo_id != first_repo.repo_id
                WHERE first_repo.task_id = NEW.task_id
                  AND first_repo.role IN ('primary', 'affected')
                  AND second_repo.role IN ('primary', 'affected')
            )
            OR EXISTS (
                SELECT 1 FROM task_repository_set_revision revision
                JOIN task_repository_set_revision_repo first_repo
                  ON first_repo.task_id = revision.task_id
                 AND first_repo.revision = revision.revision
                JOIN task_repository_set_revision_repo second_repo
                  ON second_repo.task_id = first_repo.task_id
                 AND second_repo.revision = first_repo.revision
                 AND second_repo.repo_id != first_repo.repo_id
                WHERE revision.task_id = NEW.task_id AND revision.state = 'current'
                  AND first_repo.role IN ('primary', 'affected')
                  AND second_repo.role IN ('primary', 'affected')
            )
            OR NOT EXISTS (
                SELECT 1 FROM task t
                WHERE t.id = NEW.task_id AND t.repo_id IS NOT NULL
                  AND (NEW.repo_id IS NULL OR NEW.repo_id = t.repo_id)
            )
        ))
        OR (NEW.repository_merge_batch_id IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM task_repository_merge_batch batch
            JOIN task_repository_merge_batch_repo row
              ON row.batch_id = batch.id AND row.task_id = batch.task_id
            JOIN task_repository_set_revision_repo repository
              ON repository.task_id = batch.task_id
             AND repository.revision = batch.repository_set_revision
             AND repository.repo_id = row.repo_id
             AND repository.role IN ('primary', 'affected')
            WHERE batch.id = NEW.repository_merge_batch_id
              AND batch.task_id = NEW.task_id
              AND row.repo_id = NEW.repo_id
              AND (row.task_merge_operation_id IS NULL OR row.task_merge_operation_id = NEW.id)
        ))
     ))
  OR (NEW.kind = 'publish_pr' AND NOT EXISTS (
        SELECT 1 FROM task_integration_operation merge_op
        WHERE merge_op.id = NEW.parent_operation_id
          AND merge_op.task_id = NEW.task_id AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.repo_id IS NEW.repo_id
          AND merge_op.repository_merge_batch_id IS NEW.repository_merge_batch_id
          AND (
              (NEW.repository_merge_batch_id IS NULL AND NOT EXISTS (
                  SELECT 1 FROM task_repository first_repo
                  JOIN task_repository second_repo
                    ON second_repo.task_id = first_repo.task_id
                   AND second_repo.repo_id != first_repo.repo_id
                  WHERE first_repo.task_id = NEW.task_id
                    AND first_repo.role IN ('primary', 'affected')
                    AND second_repo.role IN ('primary', 'affected')
              ) AND NOT EXISTS (
                  SELECT 1 FROM task_repository_set_revision revision
                  JOIN task_repository_set_revision_repo first_repo
                    ON first_repo.task_id = revision.task_id
                   AND first_repo.revision = revision.revision
                  JOIN task_repository_set_revision_repo second_repo
                    ON second_repo.task_id = first_repo.task_id
                   AND second_repo.revision = first_repo.revision
                   AND second_repo.repo_id != first_repo.repo_id
                  WHERE revision.task_id = NEW.task_id AND revision.state = 'current'
                    AND first_repo.role IN ('primary', 'affected')
                    AND second_repo.role IN ('primary', 'affected')
              ))
              OR (NEW.repository_merge_batch_id IS NOT NULL AND EXISTS (
                  SELECT 1 FROM task_repository_merge_batch_repo row
                  WHERE row.batch_id = NEW.repository_merge_batch_id
                    AND row.task_id = NEW.task_id AND row.repo_id = NEW.repo_id
                    AND row.task_merge_operation_id = merge_op.id
              ))
          )
     ))
BEGIN
    SELECT RAISE(ABORT, 'TaskMerge and PublishPr require their exact repository merge batch row');
END;

CREATE TRIGGER task_integration_operation_repository_immutable
BEFORE UPDATE OF repo_id, repository_merge_batch_id ON task_integration_operation
WHEN NEW.repo_id IS NOT OLD.repo_id
  OR NEW.repository_merge_batch_id IS NOT OLD.repository_merge_batch_id
BEGIN
    SELECT RAISE(ABORT, 'integration operation repository and batch are immutable');
END;

DROP TRIGGER remote_pr_admission_insert_guard;
CREATE TRIGGER remote_pr_admission_insert_guard
BEFORE INSERT ON remote_pr_admission
WHEN NOT EXISTS (
    SELECT 1 FROM task_integration_operation merge_op
    JOIN task_integration_operation publish_op
      ON publish_op.id = NEW.publish_operation_id
     AND publish_op.parent_operation_id = merge_op.id
     AND publish_op.task_id = merge_op.task_id
    JOIN pr_metadata metadata
      ON metadata.id = NEW.metadata_id AND metadata.task_id = merge_op.task_id
    JOIN task t ON t.id = merge_op.task_id
    JOIN repo r ON r.id = COALESCE(merge_op.repo_id, t.repo_id)
                AND r.project_id = t.project_id
    JOIN pr_provider_config config
      ON config.id = NEW.provider_config_id AND config.repo_id = r.id
    LEFT JOIN task_repository_merge_batch batch
      ON batch.id = merge_op.repository_merge_batch_id AND batch.task_id = merge_op.task_id
    LEFT JOIN task_repository_merge_batch_repo batch_repo
      ON batch_repo.batch_id = batch.id AND batch_repo.task_id = batch.task_id
     AND batch_repo.repo_id = NEW.repo_id
    LEFT JOIN task_repository_set_revision_repo revision_repo
      ON revision_repo.task_id = batch.task_id
     AND revision_repo.revision = batch.repository_set_revision
     AND revision_repo.repo_id = COALESCE(NEW.repo_id, t.repo_id)
     AND revision_repo.role IN ('primary', 'affected')
    JOIN task_lifecycle lifecycle ON lifecycle.task_id = merge_op.task_id
    WHERE merge_op.id = NEW.task_merge_operation_id
      AND merge_op.task_id = NEW.task_id
      AND COALESCE(merge_op.repo_id, t.repo_id) = COALESCE(NEW.repo_id, t.repo_id)
      AND merge_op.repository_merge_batch_id IS batch.id
      AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
      AND merge_op.remote_waiting = 1
      AND publish_op.kind = 'publish_pr' AND publish_op.status = 'running'
      AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
      AND COALESCE(publish_op.repo_id, t.repo_id) = COALESCE(NEW.repo_id, t.repo_id)
      AND publish_op.repository_merge_batch_id IS merge_op.repository_merge_batch_id
      AND COALESCE(metadata.repo_id, t.repo_id) = COALESCE(NEW.repo_id, t.repo_id)
      AND metadata.task_merge_operation_id = merge_op.id
      AND metadata.publish_operation_id = publish_op.id
      AND metadata.provider_type = NEW.provider_type
      AND metadata.source_branch = NEW.source_branch
      AND metadata.target_branch = NEW.target_branch
      AND r.remote_url = NEW.remote_repo_identity
      AND config.provider_type = NEW.provider_type
      AND config.updated_at = NEW.provider_config_revision
      AND config.base_url IS NEW.provider_base_url
      AND config.token_secret_ref IS NEW.token_secret_ref
      AND config.credential_handle_id IS NEW.credential_handle_id
      AND config.credential_owner_user_id IS NEW.credential_owner_user_id
      AND config.feedback_policy_json IS NEW.feedback_policy_json
      AND (NEW.feedback_policy_json IS NULL) = (NEW.feedback_policy_digest IS NULL)
      AND lifecycle.state = 'merging'
      AND ((merge_op.repository_merge_batch_id IS NULL
            AND lifecycle.reason_ref IS merge_op.id)
           OR (merge_op.repository_merge_batch_id IS NOT NULL
               AND lifecycle.reason_ref IS merge_op.repository_merge_batch_id))
      AND (
          (merge_op.repository_merge_batch_id IS NULL
           AND publish_op.repository_merge_batch_id IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM task_requires_repository_merge_batch required
               WHERE required.task_id = merge_op.task_id
           ))
          OR (merge_op.repository_merge_batch_id IS NOT NULL
              AND batch.id = merge_op.repository_merge_batch_id
              AND batch_repo.batch_id = batch.id
              AND revision_repo.repo_id = NEW.repo_id)
      )
      -- V175: a remotely merged paused PR is not re-admitted until abandoned;
      -- an open pause must be re-admitted exactly (same PR identity); no open
      -- pause means a fresh admission.
      AND NOT EXISTS (
          SELECT 1 FROM pr_rework_supersession merged_pause
          WHERE merged_pause.task_id = NEW.task_id AND merged_pause.state = 'remote_merged')
      AND (
          (NEW.readmits_supersession_id IS NULL AND NOT EXISTS (
              SELECT 1 FROM pr_rework_supersession open_pause
              WHERE open_pause.task_id = NEW.task_id AND open_pause.state = 'open'))
          OR (NEW.readmits_supersession_id IS NOT NULL AND EXISTS (
              SELECT 1 FROM pr_rework_supersession open_pause
              JOIN remote_pr_admission paused
                ON paused.task_merge_operation_id = open_pause.task_merge_operation_id
              WHERE open_pause.id = NEW.readmits_supersession_id
                AND open_pause.task_id = NEW.task_id AND open_pause.state = 'open'
                AND paused.source_branch = NEW.source_branch
                AND paused.target_branch = NEW.target_branch
                AND paused.remote_repo_identity = NEW.remote_repo_identity))
      )
)
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission is missing its exact TaskMerge, PublishPr, or metadata authority');
END;

CREATE TRIGGER remote_pr_admission_repo_immutable
BEFORE UPDATE OF repo_id ON remote_pr_admission
WHEN NEW.repo_id IS NOT OLD.repo_id
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission repository identity is immutable');
END;

DROP TRIGGER pr_rework_supersession_insert_guard;
CREATE TRIGGER pr_rework_supersession_insert_guard
BEFORE INSERT ON pr_rework_supersession
WHEN NEW.version != 1 OR NEW.state != 'open'
  OR NEW.readmission_merge_operation_id IS NOT NULL
  OR NEW.convergence_decision_id IS NOT NULL
  OR NOT EXISTS (
      SELECT 1 FROM task_coordination_request r
      JOIN remote_pr_admission a ON a.task_merge_operation_id = NEW.task_merge_operation_id
      JOIN task_integration_operation merge_op ON merge_op.id = a.task_merge_operation_id
      JOIN task_lifecycle l ON l.task_id = NEW.task_id
      JOIN pr_metadata m ON m.id = a.metadata_id
      JOIN domain_event e ON e.id = NEW.result_event_id
      WHERE r.id = NEW.request_id AND r.task_id = NEW.task_id
        AND r.kind = 'external_feedback' AND r.state = 'triaged'
        AND r.source_subject_kind = 'external_feedback_set'
        AND r.source_subject_id = a.task_merge_operation_id
        AND r.source_subject_digest = NEW.triage_set_digest
        AND EXISTS (
            SELECT 1 FROM task_external_feedback_disposition d
            WHERE d.request_id = r.id AND d.task_id = r.task_id
              AND d.disposition = 'extend_task')
        AND a.task_id = NEW.task_id AND a.state = 'open' AND a.rework_outcome IS NULL
        AND a.publish_operation_id = NEW.publish_operation_id
        AND a.metadata_id = NEW.metadata_id
        AND a.admitted_source_sha = NEW.superseded_head_sha
        AND m.task_id = NEW.task_id AND m.provider_pr_id = NEW.provider_pr_id
        AND m.task_merge_operation_id = a.task_merge_operation_id
        AND merge_op.task_id = NEW.task_id AND merge_op.kind = 'task_merge'
        AND merge_op.status = 'running' AND merge_op.remote_waiting = 1
        AND l.state = 'merging'
        AND ((merge_op.repository_merge_batch_id IS NULL AND l.reason_ref IS merge_op.id)
             OR (merge_op.repository_merge_batch_id IS NOT NULL
                 AND l.reason_ref IS merge_op.repository_merge_batch_id))
        AND COALESCE(merge_op.repo_id,
                     (SELECT task.repo_id FROM task WHERE task.id = merge_op.task_id))
            = COALESCE(a.repo_id,
                       (SELECT task.repo_id FROM task WHERE task.id = a.task_id))
        AND (
            (merge_op.repository_merge_batch_id IS NULL
             AND NOT EXISTS (
                 SELECT 1 FROM task_requires_repository_merge_batch required
                 WHERE required.task_id = merge_op.task_id
             ))
            OR (merge_op.repository_merge_batch_id IS NOT NULL AND EXISTS (
                SELECT 1 FROM task_repository_merge_batch_repo batch_repo
                WHERE batch_repo.batch_id = merge_op.repository_merge_batch_id
                  AND batch_repo.task_id = merge_op.task_id
                  AND batch_repo.repo_id = COALESCE(merge_op.repo_id,
                      (SELECT task.repo_id FROM task WHERE task.id = merge_op.task_id))
                  AND batch_repo.task_merge_operation_id = merge_op.id
            ))
        )
        AND m.repo_id IS a.repo_id
        AND e.event_type = 'pr.admission_superseded' AND e.entity_type = 'pr_metadata'
        AND e.entity_id = m.id AND e.scope_type = 'task' AND e.scope_id = NEW.task_id
        AND json_valid(e.payload_json)
        AND json_extract(e.payload_json, '$.task_id') = NEW.task_id
        AND json_extract(e.payload_json, '$.repo_id') IS m.repo_id
        AND json_extract(e.payload_json, '$.pr_metadata_id') = m.id
        AND json_extract(e.payload_json, '$.task_merge_operation_id') = merge_op.id
        AND json_extract(e.payload_json, '$.publish_operation_id') = NEW.publish_operation_id
        AND json_extract(e.payload_json, '$.supersession_id') = NEW.id
        AND json_extract(e.payload_json, '$.provider_pr_id') = NEW.provider_pr_id
        AND json_extract(e.payload_json, '$.superseded_head_sha') = NEW.superseded_head_sha
  )
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_REWORK_NOT_AUTHORIZED');
END;

DROP TRIGGER remote_pr_history_insert_guard;
CREATE TRIGGER remote_pr_history_insert_guard
BEFORE INSERT ON remote_pr_history
WHEN NEW.history_origin NOT IN ('v116_backfill', 'terminal_snapshot')
  OR NOT EXISTS (
      SELECT 1 FROM remote_pr_admission admission
      JOIN task_integration_operation merge_op
        ON merge_op.id = admission.task_merge_operation_id
       AND merge_op.task_id = admission.task_id
      JOIN task_integration_operation publish_op
        ON publish_op.id = admission.publish_operation_id
       AND publish_op.task_id = admission.task_id
       AND publish_op.parent_operation_id = merge_op.id
      JOIN task ON task.id = merge_op.task_id
      JOIN domain_event event ON event.id = admission.result_event_id
      WHERE admission.task_merge_operation_id = NEW.task_merge_operation_id
        AND admission.publish_operation_id = NEW.publish_operation_id
        AND admission.metadata_id = NEW.original_metadata_id
        AND admission.task_id = NEW.task_id
        AND admission.provider_config_id = NEW.provider_config_id
        AND admission.provider_type = NEW.provider_type
        AND admission.provider_config_revision = NEW.provider_config_revision
        AND admission.provider_config_digest = NEW.provider_config_digest
        AND admission.provider_base_url IS NEW.provider_base_url
        AND admission.token_secret_ref IS NEW.token_secret_ref
        AND admission.remote_repo_identity = NEW.remote_repo_identity
        AND admission.source_branch = NEW.source_branch
        AND admission.target_branch = NEW.target_branch
        AND admission.admitted_source_sha = NEW.admitted_source_sha
        AND admission.state = NEW.admission_state
        AND admission.provider_status = NEW.provider_status
        AND admission.result_classification IS NEW.result_classification
        AND admission.provider_event_id IS NEW.provider_event_id
        AND admission.observed_head_sha IS NEW.observed_head_sha
        AND admission.merged_commit_sha IS NEW.merged_commit_sha
        AND admission.result_event_id = NEW.result_event_id
        AND admission.created_at = NEW.admission_created_at
        AND event.created_at = NEW.result_created_at
        AND admission.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
        AND merge_op.kind = 'task_merge'
        AND COALESCE(merge_op.repo_id, task.repo_id) =
            COALESCE(admission.repo_id, task.repo_id)
        AND merge_op.remote_waiting = 1
        AND merge_op.result_event_id = event.id
        AND publish_op.kind = 'publish_pr'
        AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
        AND (
            (admission.state = 'merged'
             AND merge_op.status = 'succeeded'
             AND publish_op.status = 'succeeded')
            OR (admission.state IN ('closed', 'head_mismatch')
                AND merge_op.status = 'failed'
                AND publish_op.status = 'succeeded')
            OR (admission.state = 'publication_failed'
                AND merge_op.status = 'failed'
                AND publish_op.status = 'failed'
                AND publish_op.result_event_id = event.id)
        )
        AND event.event_type = 'pr.status_changed'
        AND event.entity_type = 'pr_metadata'
        AND event.entity_id = admission.metadata_id
        AND event.scope_type = 'task'
        AND event.scope_id = admission.task_id
        AND json_valid(event.payload_json)
        AND json_extract(event.payload_json, '$.task_id') = admission.task_id
        AND json_extract(event.payload_json, '$.repo_id') IS admission.repo_id
        AND json_extract(event.payload_json, '$.pr_metadata_id') = admission.metadata_id
        AND json_extract(event.payload_json, '$.task_merge_operation_id') = admission.task_merge_operation_id
        AND json_extract(event.payload_json, '$.publish_operation_id') = admission.publish_operation_id
        AND json_extract(event.payload_json, '$.provider_type') = admission.provider_type
        AND json_extract(event.payload_json, '$.provider_config_id') = admission.provider_config_id
        AND json_extract(event.payload_json, '$.provider_config_revision') = admission.provider_config_revision
        AND json_extract(event.payload_json, '$.provider_config_digest') = admission.provider_config_digest
        AND json_extract(event.payload_json, '$.remote_repo_identity') = admission.remote_repo_identity
        AND json_extract(event.payload_json, '$.source_branch') = admission.source_branch
        AND json_extract(event.payload_json, '$.target_branch') = admission.target_branch
        AND json_extract(event.payload_json, '$.admitted_source_sha') = admission.admitted_source_sha
        AND json_extract(event.payload_json, '$.provider_event_id') IS admission.provider_event_id
        AND json_extract(event.payload_json, '$.provider_pr_id') IS NEW.provider_pr_id
        AND json_extract(event.payload_json, '$.pr_url') IS NEW.pr_url
        AND json_extract(event.payload_json, '$.head_sha') IS admission.observed_head_sha
        AND json_extract(event.payload_json, '$.merged_commit_sha') IS admission.merged_commit_sha
        AND (
            (admission.state = 'merged'
             AND admission.provider_status = 'merged'
             AND admission.result_classification IS NULL
             AND json_extract(event.payload_json, '$.status') = 'merged'
             AND json_extract(event.payload_json, '$.result_classification') IS NULL)
            OR (admission.state = 'closed'
                AND admission.provider_status = 'closed'
                AND admission.result_classification IS NULL
                AND json_extract(event.payload_json, '$.status') = 'closed'
                AND json_extract(event.payload_json, '$.result_classification') IS NULL)
            OR (admission.state = 'head_mismatch'
                AND admission.provider_status = 'merged'
                AND admission.result_classification = 'head_mismatch'
                AND ((json_extract(event.payload_json, '$.status') = 'head_mismatch'
                      AND json_extract(event.payload_json, '$.result_classification') IS NULL)
                     OR (json_extract(event.payload_json, '$.status') = 'merged'
                         AND json_extract(event.payload_json, '$.result_classification') = 'head_mismatch')))
            OR (admission.state = 'publication_failed'
                AND admission.provider_status = 'publication_failed'
                AND admission.result_classification IS NULL
                AND json_extract(event.payload_json, '$.status') = 'publication_failed'
                AND json_extract(event.payload_json, '$.result_classification') IS NULL)
        )
        AND NEW.pr_state = CASE json_extract(event.payload_json, '$.status')
            WHEN 'publication_failed' THEN 'failed'
            ELSE json_extract(event.payload_json, '$.status')
        END
        AND NEW.merge_status = CASE admission.provider_status
            WHEN 'merged' THEN 'merged'
            WHEN 'closed' THEN 'closed_without_merge'
            WHEN 'publication_failed' THEN 'publication_failed'
        END
        AND NEW.admission_status = CASE admission.state
            WHEN 'merged' THEN 'merged'
            WHEN 'closed' THEN 'closed'
            ELSE 'failed'
        END
        AND (
            NEW.history_origin = 'v116_backfill'
            OR EXISTS (
                SELECT 1 FROM pr_metadata metadata
                WHERE metadata.id = admission.metadata_id
                  AND metadata.task_id = admission.task_id
                  AND metadata.repo_id IS admission.repo_id
                  AND metadata.task_merge_operation_id = admission.task_merge_operation_id
                  AND metadata.publish_operation_id = admission.publish_operation_id
                  AND metadata.provider_type = admission.provider_type
                  AND metadata.provider_pr_id IS NEW.provider_pr_id
                  AND metadata.pr_url IS NEW.pr_url
                  AND metadata.source_branch = admission.source_branch
                  AND metadata.target_branch = admission.target_branch
                  AND metadata.pr_state = NEW.pr_state
                  AND metadata.merge_status = NEW.merge_status
                  AND metadata.admission_status = NEW.admission_status
            )
        )
  )
BEGIN
    SELECT RAISE(ABORT, 'remote PR history requires an exact terminal admission and provider result');
END;

DROP TRIGGER task_integration_operation_provider_result_guard;
CREATE TRIGGER task_integration_operation_provider_result_guard
BEFORE UPDATE OF status, result_event_id ON task_integration_operation
WHEN (
    (OLD.kind = 'task_merge' AND OLD.remote_waiting = 1)
    OR (OLD.kind = 'publish_pr' AND NEW.status = 'failed')
)
 AND (NEW.status IS NOT OLD.status OR NEW.result_event_id IS NOT OLD.result_event_id)
 AND (
    OLD.result_event_id IS NOT NULL
    OR (NOT EXISTS (
        SELECT 1 FROM remote_pr_admission admission
        JOIN domain_event event ON event.id = NEW.result_event_id
        JOIN pr_metadata metadata ON metadata.id = admission.metadata_id
        WHERE admission.task_merge_operation_id =
              CASE WHEN NEW.kind = 'task_merge' THEN NEW.id ELSE NEW.parent_operation_id END
          AND admission.publish_operation_id =
              CASE WHEN NEW.kind = 'publish_pr' THEN NEW.id ELSE metadata.publish_operation_id END
          AND admission.task_id = NEW.task_id
          AND admission.repo_id IS NEW.repo_id
          AND metadata.task_id = NEW.task_id
          AND metadata.repo_id IS admission.repo_id
          AND metadata.task_merge_operation_id = admission.task_merge_operation_id
          AND metadata.publish_operation_id = admission.publish_operation_id
          AND event.event_type = 'pr.status_changed'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task'
          AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
          AND json_extract(event.payload_json, '$.repo_id') IS metadata.repo_id
          AND json_extract(event.payload_json, '$.pr_metadata_id') = metadata.id
          AND json_extract(event.payload_json, '$.task_merge_operation_id') =
              admission.task_merge_operation_id
          AND json_extract(event.payload_json, '$.publish_operation_id') =
              admission.publish_operation_id
          AND json_extract(event.payload_json, '$.provider_event_id') IS
              admission.provider_event_id
          AND json_extract(event.payload_json, '$.provider_pr_id') IS
              metadata.provider_pr_id
          AND json_extract(event.payload_json, '$.head_sha') IS
              admission.observed_head_sha
          AND json_extract(event.payload_json, '$.merged_commit_sha') IS
              admission.merged_commit_sha
          AND (
              (NEW.kind = 'task_merge'
               AND NEW.remote_waiting = 1
               AND OLD.status = 'running'
               AND NEW.status = 'succeeded'
               AND admission.state = 'merged'
               AND admission.provider_status = 'merged'
               AND admission.result_classification IS NULL
               AND event.id = admission.result_event_id
               AND json_extract(event.payload_json, '$.status') = 'merged'
               AND json_extract(event.payload_json, '$.result_classification') IS NULL
               AND json_extract(event.payload_json, '$.admitted_source_sha') =
                   admission.admitted_source_sha
               AND json_extract(event.payload_json, '$.head_sha') =
                   admission.admitted_source_sha)
              OR (NEW.kind = 'task_merge'
                  AND NEW.remote_waiting = 1
                  AND OLD.status = 'running'
                  AND NEW.status = 'failed'
                  AND admission.state = 'closed'
                  AND admission.provider_status = 'closed'
                  AND admission.result_classification IS NULL
                  AND event.id = admission.result_event_id
                  AND json_extract(event.payload_json, '$.status') = 'closed'
                  AND json_extract(event.payload_json, '$.result_classification') IS NULL)
              OR (NEW.kind = 'task_merge'
                  AND NEW.remote_waiting = 1
                  AND OLD.status = 'running'
                  AND NEW.status = 'failed'
                  AND admission.state = 'head_mismatch'
                  AND admission.provider_status = 'merged'
                  AND admission.result_classification = 'head_mismatch'
                  AND event.id = admission.result_event_id
                  AND json_extract(event.payload_json, '$.status') = 'merged'
                  AND json_extract(event.payload_json, '$.result_classification') = 'head_mismatch'
                  AND json_extract(event.payload_json, '$.admitted_source_sha') =
                      admission.admitted_source_sha
                  AND json_extract(event.payload_json, '$.head_sha') IS NOT
                      admission.admitted_source_sha)
              OR (NEW.kind = 'task_merge'
                  AND NEW.remote_waiting = 1
                  AND OLD.status = 'running'
                  AND NEW.status = 'failed'
                  AND admission.state = 'publication_failed'
                  AND admission.provider_status = 'publication_failed'
                  AND admission.result_classification IS NULL
                  AND event.id = admission.result_event_id
                  AND json_extract(event.payload_json, '$.status') = 'publication_failed'
                  AND json_extract(event.payload_json, '$.result_classification') IS NULL)
              OR (NEW.kind = 'publish_pr'
                  AND NEW.status = 'failed'
                  AND OLD.status = 'running'
                  AND admission.state = 'publication_failed'
                  AND admission.provider_status = 'publication_failed'
                  AND admission.result_classification IS NULL
                  AND admission.publish_operation_id = NEW.id
                  AND admission.task_merge_operation_id = NEW.parent_operation_id
                  AND event.id = admission.result_event_id
                  AND json_extract(event.payload_json, '$.status') = 'publication_failed'
                  AND json_extract(event.payload_json, '$.result_classification') IS NULL)
          )
    ) AND NOT EXISTS (
        -- V175: non-destructive pause of an open PR for same-PR rework.
        SELECT 1 FROM remote_pr_admission admission
        JOIN pr_rework_supersession s ON s.id = admission.supersession_id
        JOIN pr_metadata metadata ON metadata.id = admission.metadata_id
        JOIN domain_event event ON event.id = NEW.result_event_id
        WHERE NEW.kind = 'task_merge' AND NEW.remote_waiting = 1
          AND OLD.status = 'running' AND NEW.status = 'abandoned'
          AND admission.task_merge_operation_id = NEW.id
          AND admission.task_id = NEW.task_id
          AND admission.repo_id IS NEW.repo_id
          AND admission.state = 'open'
          AND admission.rework_outcome = 'superseded_for_rework'
          AND s.task_id = NEW.task_id
          AND s.task_merge_operation_id = NEW.id
          AND s.publish_operation_id = admission.publish_operation_id
          AND s.metadata_id = admission.metadata_id
          AND s.state = 'open'
          AND metadata.task_id = NEW.task_id
          AND metadata.repo_id IS admission.repo_id
          AND metadata.task_merge_operation_id = NEW.id
          AND event.id = s.result_event_id
          AND event.event_type = 'pr.admission_superseded'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task'
          AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
          AND json_extract(event.payload_json, '$.repo_id') IS metadata.repo_id
          AND json_extract(event.payload_json, '$.pr_metadata_id') = metadata.id
          AND json_extract(event.payload_json, '$.task_merge_operation_id') = NEW.id
          AND json_extract(event.payload_json, '$.publish_operation_id') =
              admission.publish_operation_id
          AND json_extract(event.payload_json, '$.supersession_id') = s.id
          AND json_extract(event.payload_json, '$.provider_pr_id') = s.provider_pr_id
          AND json_extract(event.payload_json, '$.superseded_head_sha') =
              s.superseded_head_sha
    ))
 )
BEGIN
    SELECT RAISE(ABORT, 'remote TaskMerge terminal result requires exact provider outcome authority');
END;

COMMIT;
PRAGMA foreign_keys = ON;
PRAGMA legacy_alter_table = OFF;
