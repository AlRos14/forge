CREATE TABLE workspace_group (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    work_unit_id TEXT NOT NULL,
    root_path    TEXT NOT NULL CHECK (length(trim(root_path)) > 0),
    created_at   TEXT NOT NULL,
    UNIQUE (work_unit_id),
    UNIQUE (id, task_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE
);
CREATE TRIGGER workspace_group_immutable_update
BEFORE UPDATE ON workspace_group
BEGIN
    SELECT RAISE(ABORT, 'WorkspaceGroup identity and root are immutable');
END;
CREATE TRIGGER workspace_group_immutable_delete
BEFORE DELETE ON workspace_group
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkspaceGroup is immutable outside Project teardown');
END;
ALTER TABLE workspace_scope
    ADD COLUMN workspace_group_id TEXT
        REFERENCES workspace_group(id) ON DELETE RESTRICT;
DROP INDEX IF EXISTS idx_workspace_scope_work_unit;
CREATE INDEX idx_workspace_scope_work_unit
    ON workspace_scope(work_unit_id, workspace_id)
    WHERE scope_kind = 'work_unit';
CREATE INDEX idx_workspace_scope_group
    ON workspace_scope(workspace_group_id, workspace_id)
    WHERE workspace_group_id IS NOT NULL;

-- V181 has no Git SHA for a context repository. Pin on first group preparation;
-- all later WorkUnit groups for the Task reuse it, even across a repo-set
-- replan. The insert guard checks the current context association.
CREATE TABLE task_repository_context_snapshot (
    task_id      TEXT NOT NULL,
    repo_id      TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    commit_sha   TEXT NOT NULL CHECK (length(trim(commit_sha)) > 0),
    captured_at  TEXT NOT NULL,
    PRIMARY KEY (task_id, repo_id),
    FOREIGN KEY (task_id) REFERENCES task(id) ON DELETE CASCADE
);
CREATE TRIGGER task_repository_context_snapshot_insert_guard
BEFORE INSERT ON task_repository_context_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM task_repository tr
    WHERE tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
      AND tr.role = 'context'
)
BEGIN
    SELECT RAISE(ABORT, 'context snapshot must bind a context repository of its Task');
END;
CREATE TRIGGER task_repository_context_snapshot_immutable_update
BEFORE UPDATE ON task_repository_context_snapshot
BEGIN
    SELECT RAISE(ABORT, 'context repository snapshot is immutable');
END;
CREATE TRIGGER task_repository_context_snapshot_immutable_delete
BEFORE DELETE ON task_repository_context_snapshot
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'context repository snapshot is immutable outside Project teardown');
END;

-- One active lease per WorkUnit/repository Workspace; retain the legacy
-- Task-wide lease uniqueness for non-WorkUnit records.
DROP INDEX IF EXISTS idx_workspace_lease_active_work_unit;
CREATE UNIQUE INDEX idx_workspace_lease_active_work_unit
    ON workspace_lease(work_unit_id, workspace_id)
    WHERE status = 'active' AND work_unit_id IS NOT NULL;

CREATE TABLE execution_repository_result (
    execution_id    TEXT NOT NULL REFERENCES execution(id) ON DELETE CASCADE,
    task_id         TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    repo_id         TEXT NOT NULL REFERENCES repo(id) ON DELETE RESTRICT,
    workspace_id    TEXT NOT NULL REFERENCES workspace(id) ON DELETE RESTRICT,
    before_sha      TEXT,
    after_sha       TEXT,
    snapshot_digest TEXT CHECK (
        snapshot_digest IS NULL OR length(snapshot_digest) = 64
    ),
    created_at      TEXT NOT NULL,
    PRIMARY KEY (execution_id, repo_id),
    UNIQUE (execution_id, workspace_id)
);
CREATE INDEX idx_execution_repository_result_task
    ON execution_repository_result(task_id, execution_id, repo_id);
CREATE TRIGGER execution_repository_result_insert_guard
BEFORE INSERT ON execution_repository_result
WHEN NOT EXISTS (
    SELECT 1
    FROM execution e
    JOIN workspace w
      ON w.id = NEW.workspace_id AND w.task_id = NEW.task_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
    WHERE e.id = NEW.execution_id AND e.task_id = NEW.task_id
      AND w.repo_id = NEW.repo_id
      AND (e.work_unit_id IS NULL
           OR NOT EXISTS (SELECT 1 FROM work_unit_repository s
                          WHERE s.work_unit_id = e.work_unit_id)
           OR EXISTS (SELECT 1 FROM work_unit_repository s
                      WHERE s.work_unit_id = e.work_unit_id
                        AND s.repo_id = NEW.repo_id))
      AND (e.work_unit_id IS NULL AND EXISTS (
               SELECT 1 FROM workspace_scope ws
               WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                 AND ws.scope_kind = 'integration'
           ) OR e.work_unit_id IS NOT NULL AND EXISTS (
               SELECT 1 FROM workspace_scope ws
               WHERE ws.workspace_id = w.id AND ws.task_id = e.task_id
                 AND ws.scope_kind = 'work_unit'
                 AND ws.work_unit_id = e.work_unit_id
                 AND (ws.workspace_group_id IS NULL OR EXISTS (
                     SELECT 1 FROM workspace_group g
                     WHERE g.id = ws.workspace_group_id
                       AND g.task_id = e.task_id AND g.work_unit_id = e.work_unit_id
                 ))
           ))
)
BEGIN
    SELECT RAISE(ABORT, 'repository result must bind the exact Execution, writable repo, and workspace');
END;
CREATE TRIGGER execution_repository_result_immutable_update
BEFORE UPDATE ON execution_repository_result
BEGIN
    SELECT RAISE(ABORT, 'Execution repository results are immutable');
END;
CREATE TRIGGER execution_repository_result_immutable_delete
BEFORE DELETE ON execution_repository_result
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Execution repository results are immutable outside Project teardown');
END;

-- Legacy single-repository producers still update the scalar Execution result.
-- Materialize that exact Workspace result in the same transaction, after the
-- completed Execution is bound to its WorkUnit. Multi-repository producers
-- write every result explicitly before terminalization; this fills only a
-- missing scalar repository row and leaves those exact rows untouched.
CREATE TRIGGER execution_repository_result_legacy_scalar_after_update
AFTER UPDATE OF status, work_unit_id, work_unit_version ON execution
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
    JOIN task_repository repository
      ON repository.task_id = NEW.task_id
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
            AND scope.task_id = NEW.task_id
            AND scope.scope_kind = 'work_unit'
            AND scope.work_unit_id = NEW.work_unit_id
      )
      AND NOT EXISTS (
          SELECT 1 FROM execution_repository_result existing
          WHERE existing.execution_id = NEW.id
            AND existing.repo_id = workspace.repo_id
      );
END;

CREATE TABLE work_unit_review_subject_repository (
    review_work_unit_id        TEXT NOT NULL,
    task_id                    TEXT NOT NULL,
    repo_id                    TEXT NOT NULL,
    reviewed_work_unit_id      TEXT NOT NULL,
    reviewed_work_unit_version INTEGER CHECK (
        reviewed_work_unit_version IS NULL OR reviewed_work_unit_version >= 1
    ),
    result_execution_id        TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id         TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha          TEXT,
    result_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest  TEXT CHECK (
        workspace_snapshot_digest IS NULL OR length(workspace_snapshot_digest) = 64
    ),
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (review_work_unit_id, repo_id),
    FOREIGN KEY (review_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (reviewed_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT,
    CHECK (review_work_unit_id != reviewed_work_unit_id),
    CHECK (result_execution_id IS NOT NULL OR (
        result_artifact_id IS NULL AND result_commit_sha IS NULL
        AND result_workspace_id IS NULL AND workspace_snapshot_digest IS NULL
    ))
);
CREATE INDEX idx_work_unit_review_subject_repository_reviewed
    ON work_unit_review_subject_repository(
        task_id, reviewed_work_unit_id, reviewed_work_unit_version, repo_id
    );
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
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
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
CREATE TRIGGER work_unit_review_subject_repository_freeze_update
BEFORE UPDATE ON work_unit_review_subject_repository
WHEN OLD.reviewed_work_unit_version IS NOT NULL OR OLD.result_execution_id IS NOT NULL
  OR NEW.review_work_unit_id IS NOT OLD.review_work_unit_id
  OR NEW.task_id IS NOT OLD.task_id OR NEW.repo_id IS NOT OLD.repo_id
  OR NEW.reviewed_work_unit_id IS NOT OLD.reviewed_work_unit_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.reviewed_work_unit_version IS NULL OR NEW.result_execution_id IS NULL
  OR NOT EXISTS (
      SELECT 1 FROM work_unit_review_subject scalar_subject
      WHERE scalar_subject.review_work_unit_id = NEW.review_work_unit_id
        AND scalar_subject.task_id = NEW.task_id
        AND scalar_subject.reviewed_work_unit_id = NEW.reviewed_work_unit_id
        AND scalar_subject.reviewed_work_unit_version IS NEW.reviewed_work_unit_version
        AND scalar_subject.result_execution_id IS NEW.result_execution_id
        AND scalar_subject.result_artifact_id IS NEW.result_artifact_id
  )
  OR NOT EXISTS (
      SELECT 1 FROM execution e
      JOIN execution_repository_result rr
        ON rr.execution_id = e.id AND rr.task_id = e.task_id AND rr.repo_id = NEW.repo_id
      WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
        AND e.work_unit_id = NEW.reviewed_work_unit_id AND e.status = 'completed'
        AND e.work_unit_version = NEW.reviewed_work_unit_version
        AND rr.workspace_id = NEW.result_workspace_id
        AND rr.after_sha IS NEW.result_commit_sha
        AND rr.snapshot_digest IS NEW.workspace_snapshot_digest
  )
BEGIN
    SELECT RAISE(ABORT, 'review subject repo may only freeze its exact completed repository result');
END;
CREATE TRIGGER work_unit_review_subject_repository_immutable_delete
BEFORE DELETE ON work_unit_review_subject_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Review subject repository is immutable outside Project teardown');
END;

CREATE TABLE work_unit_result_requirement_repository (
    work_unit_id               TEXT NOT NULL,
    task_id                    TEXT NOT NULL,
    repo_id                    TEXT NOT NULL,
    source_work_unit_id        TEXT NOT NULL,
    source_work_unit_version   INTEGER NOT NULL CHECK (source_work_unit_version >= 1),
    result_execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id         TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha          TEXT,
    result_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest  TEXT CHECK (
        workspace_snapshot_digest IS NULL OR length(workspace_snapshot_digest) = 64
    ),
    created_at                 TEXT NOT NULL,
    PRIMARY KEY (work_unit_id, repo_id),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (source_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (task_id, repo_id)
        REFERENCES task_repository(task_id, repo_id) ON DELETE RESTRICT,
    CHECK (work_unit_id != source_work_unit_id)
);
CREATE INDEX idx_work_unit_result_requirement_repository_source
    ON work_unit_result_requirement_repository(task_id, source_work_unit_id, repo_id);
CREATE TRIGGER work_unit_result_requirement_repository_insert_guard
BEFORE INSERT ON work_unit_result_requirement_repository
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit wu
    JOIN work_unit source_wu
      ON source_wu.id = NEW.source_work_unit_id AND source_wu.task_id = NEW.task_id
    JOIN task_repository tr
      ON tr.task_id = NEW.task_id AND tr.repo_id = NEW.repo_id
     AND tr.role IN ('primary', 'affected')
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
CREATE TRIGGER work_unit_result_requirement_repository_immutable_update
BEFORE UPDATE ON work_unit_result_requirement_repository
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirement repositories are immutable');
END;
CREATE TRIGGER work_unit_result_requirement_repository_immutable_delete
BEFORE DELETE ON work_unit_result_requirement_repository
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit result requirement repositories are immutable outside Project teardown');
END;

-- Backfill only completed exact Executions with a Workspace. Unknown SHA and
-- snapshot values remain NULL; no commit or digest is inferred.
INSERT INTO execution_repository_result(
    execution_id, task_id, repo_id, workspace_id,
    before_sha, after_sha, snapshot_digest, created_at
)
SELECT e.id, e.task_id, w.repo_id, w.id,
       e.before_sha, e.after_sha, NULL, e.updated_at
FROM execution e
JOIN workspace w ON w.id = e.workspace_id AND w.task_id = e.task_id
JOIN task_repository tr
  ON tr.task_id = e.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected')
WHERE e.status = 'completed';

INSERT INTO work_unit_review_subject_repository(
    review_work_unit_id, task_id, repo_id, reviewed_work_unit_id,
    reviewed_work_unit_version, result_execution_id, result_artifact_id,
    result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
)
SELECT s.review_work_unit_id, s.task_id, w.repo_id, s.reviewed_work_unit_id,
       s.reviewed_work_unit_version, s.result_execution_id, s.result_artifact_id,
       rr.after_sha, rr.workspace_id, rr.snapshot_digest,
       s.created_at
FROM work_unit_review_subject s
JOIN workspace w ON w.id = s.result_workspace_id AND w.task_id = s.task_id
JOIN execution_repository_result rr
  ON rr.execution_id = s.result_execution_id AND rr.task_id = s.task_id
 AND rr.repo_id = w.repo_id AND rr.workspace_id = s.result_workspace_id
JOIN task_repository tr
  ON tr.task_id = s.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected');

INSERT INTO work_unit_result_requirement_repository(
    work_unit_id, task_id, repo_id, source_work_unit_id, source_work_unit_version,
    result_execution_id, result_artifact_id, result_commit_sha, result_workspace_id,
    workspace_snapshot_digest, created_at
)
SELECT r.work_unit_id, r.task_id, w.repo_id, r.source_work_unit_id,
       r.source_work_unit_version, r.result_execution_id, r.result_artifact_id,
       rr.after_sha, rr.workspace_id, rr.snapshot_digest,
       r.created_at
FROM work_unit_result_requirement r
JOIN workspace w ON w.id = r.result_workspace_id AND w.task_id = r.task_id
JOIN execution_repository_result rr
  ON rr.execution_id = r.result_execution_id AND rr.task_id = r.task_id
 AND rr.repo_id = w.repo_id AND rr.workspace_id = r.result_workspace_id
JOIN task_repository tr
  ON tr.task_id = r.task_id AND tr.repo_id = w.repo_id
 AND tr.role IN ('primary', 'affected');

-- A WorkUnit has one scope row per repository. Context Workspaces are pinned
-- read-only views, while reviewer Workspaces are restricted to their frozen
-- result subject. New WorkUnit scopes always belong to their immutable group.
DROP TRIGGER workspace_scope_insert_guard;
CREATE TRIGGER workspace_scope_insert_guard
BEFORE INSERT ON workspace_scope
WHEN (NEW.scope_kind = 'integration' AND EXISTS (
          SELECT 1 FROM workspace_scope s
          WHERE s.task_id = NEW.task_id AND s.scope_kind = 'integration'
      ))
  OR NOT EXISTS (
      SELECT 1
      FROM workspace ws
      JOIN task t ON t.id = ws.task_id
      JOIN repo r ON r.id = ws.repo_id AND r.project_id = t.project_id
      JOIN task_repository tr ON tr.task_id = t.id AND tr.repo_id = ws.repo_id
      WHERE ws.id = NEW.workspace_id AND ws.task_id = NEW.task_id
        AND (
            (NEW.scope_kind = 'integration'
             AND NEW.work_unit_id IS NULL
             AND NEW.workspace_group_id IS NULL
             AND tr.role = 'primary')
            OR (NEW.scope_kind = 'work_unit'
                AND NEW.work_unit_id IS NOT NULL
                AND NEW.workspace_group_id IS NOT NULL
                AND EXISTS (
                    SELECT 1 FROM work_unit w
                    JOIN workspace_group g
                      ON g.id = NEW.workspace_group_id
                     AND g.task_id = w.task_id AND g.work_unit_id = w.id
                    WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
                      AND (
                          (tr.role = 'context' AND w.requires_integration = 1
                           AND EXISTS (
                               SELECT 1 FROM task_repository_context_snapshot pin
                               WHERE pin.task_id = t.id AND pin.repo_id = ws.repo_id
                                 AND pin.commit_sha IS ws.before_sha
                           ))
                          OR (tr.role IN ('primary', 'affected')
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
                                    AND subject.task_id = t.id
                                    AND subject.repo_id = ws.repo_id
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
        AND current_workspace.repo_id = (
            SELECT candidate.repo_id FROM workspace candidate
            WHERE candidate.id = NEW.workspace_id AND candidate.task_id = NEW.task_id
        )
  ))
BEGIN
    SELECT RAISE(ABORT, 'Workspace scope is duplicate, cross-Task, cross-repository, or outside its exact group');
END;

-- A completed repository WorkUnit is backed by one exact result for every
-- writable repository in its immutable scope; the scalar Execution may omit
-- the primary when the WorkUnit selected only other repositories.
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
               SELECT 1 FROM task_repository tr
               WHERE tr.task_id = OLD.task_id AND tr.role IN ('primary', 'affected')
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

-- Preserve all current WorkUnit admission checks while requiring the complete
-- ready group and using the primary Workspace only as its nullable scalar mirror.
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
                      JOIN task t ON t.id = ws.task_id
                      WHERE ws.work_unit_id = w.id AND ws.task_id = w.task_id
                        AND ws.scope_kind = 'work_unit'
                        AND primary_workspace.repo_id = t.repo_id
                  )
                  OR EXISTS (
                      SELECT 1 FROM task_repository tr
                      WHERE tr.task_id = w.task_id
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
                      LEFT JOIN task_repository tr
                        ON tr.task_id = ws.task_id AND tr.repo_id = bound_workspace.repo_id
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

-- Preserve the current Task/Actor/RoleMembership/TaskRole and capability
-- checks. WorkUnit leases resolve their repository through the exact Workspace.
DROP TRIGGER workspace_lease_scope_guard_insert;
CREATE TRIGGER workspace_lease_scope_guard_insert
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.issuing_principal_type != 'system'
          OR NEW.issuing_principal_id != 'task-service-scheduler'
        THEN RAISE(ABORT, 'Workspace lease may only be issued by the scheduler')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version
              AND t.deleted_at IS NULL
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  WHEN 'interactive' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND NEW.role = CASE trim(e.role)
                  WHEN 'reviewer' THEN 'reviewer'
                  ELSE 'worker'
              END
              AND EXISTS (
                  SELECT 1 FROM agent_current a
                  WHERE a.id = e.actor_id
                    AND (a.visibility = 'global' OR (
                         a.visibility = 'account' AND a.owner_id IS NOT NULL
                         AND (a.owner_id = p.owner_id OR EXISTS (
                             SELECT 1 FROM project_member pm
                             WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                         ))))
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') = 'repository_write'
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
              AND (
                  (NEW.work_unit_id IS NULL
                   AND NEW.workspace_id IS NULL
                   AND t.repo_id = NEW.repository_binding_id)
                  OR (NEW.work_unit_id IS NOT NULL AND EXISTS (
                      SELECT 1 FROM workspace bound_workspace
                      JOIN workspace_scope ws
                        ON ws.workspace_id = bound_workspace.id
                       AND ws.task_id = bound_workspace.task_id
                      JOIN workspace_group g
                        ON g.id = ws.workspace_group_id
                       AND g.task_id = ws.task_id
                       AND g.work_unit_id = NEW.work_unit_id
                      JOIN work_unit w ON w.id = NEW.work_unit_id AND w.task_id = t.id
                      JOIN task_repository repository
                        ON repository.task_id = t.id
                       AND repository.repo_id = bound_workspace.repo_id
                       AND repository.role IN ('primary', 'affected')
                      WHERE bound_workspace.id = NEW.workspace_id
                        AND bound_workspace.task_id = t.id
                        AND bound_workspace.repo_id = NEW.repository_binding_id
                        AND bound_workspace.status = 'ready'
                        AND ws.scope_kind = 'work_unit'
                        AND ws.work_unit_id = w.id
                        AND w.requires_integration = 1
                        AND lower(trim(w.role)) != 'reviewer'
                        AND (NOT EXISTS (
                            SELECT 1 FROM work_unit_repository selected
                            WHERE selected.work_unit_id = w.id
                        ) OR EXISTS (
                            SELECT 1 FROM work_unit_repository selected
                            WHERE selected.work_unit_id = w.id
                              AND selected.repo_id = bound_workspace.repo_id
                        ))
                        AND EXISTS (
                            SELECT 1 FROM execution current_execution
                            WHERE current_execution.id = NEW.execution_id
                              AND current_execution.work_unit_id = w.id
                              AND current_execution.workspace_id IS (
                                  SELECT primary_scope.workspace_id
                                  FROM workspace_scope primary_scope
                                  JOIN workspace primary_workspace
                                    ON primary_workspace.id = primary_scope.workspace_id
                                   AND primary_workspace.task_id = primary_scope.task_id
                                  WHERE primary_scope.work_unit_id = w.id
                                    AND primary_scope.task_id = t.id
                                    AND primary_scope.scope_kind = 'work_unit'
                                    AND primary_scope.workspace_group_id = g.id
                                    AND primary_workspace.repo_id = t.repo_id
                              )
                        )
                        AND NEW.base_ref = COALESCE(bound_workspace.before_sha,
                            (SELECT default_branch FROM repo WHERE id = bound_workspace.repo_id))
                  ))
              )
        ) THEN RAISE(ABORT, 'Workspace lease Task is stale or lacks current TaskRole/Execution authority')
    END;
END;

DROP TRIGGER workspace_lease_active_renewal_guard;
CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.deleted_at IS NULL
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  WHEN 'interactive' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND NEW.role = CASE trim(e.role)
                  WHEN 'reviewer' THEN 'reviewer'
                  ELSE 'worker'
              END
              AND EXISTS (
                  SELECT 1 FROM agent_current a
                  WHERE a.id = e.actor_id
                    AND (a.visibility = 'global' OR (
                         a.visibility = 'account' AND a.owner_id IS NOT NULL
                         AND (a.owner_id = p.owner_id OR EXISTS (
                             SELECT 1 FROM project_member pm
                             WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                         ))))
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') = 'repository_write'
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
              AND (
                  (NEW.work_unit_id IS NULL
                   AND NEW.workspace_id IS NULL
                   AND t.repo_id = NEW.repository_binding_id)
                  OR (NEW.work_unit_id IS NOT NULL AND EXISTS (
                      SELECT 1 FROM workspace bound_workspace
                      JOIN workspace_scope ws
                        ON ws.workspace_id = bound_workspace.id
                       AND ws.task_id = bound_workspace.task_id
                      JOIN workspace_group g
                        ON g.id = ws.workspace_group_id
                       AND g.task_id = ws.task_id
                       AND g.work_unit_id = NEW.work_unit_id
                      JOIN work_unit w ON w.id = NEW.work_unit_id AND w.task_id = t.id
                      JOIN task_repository repository
                        ON repository.task_id = t.id
                       AND repository.repo_id = bound_workspace.repo_id
                       AND repository.role IN ('primary', 'affected')
                      WHERE bound_workspace.id = NEW.workspace_id
                        AND bound_workspace.task_id = t.id
                        AND bound_workspace.repo_id = NEW.repository_binding_id
                        AND bound_workspace.status = 'ready'
                        AND ws.scope_kind = 'work_unit'
                        AND ws.work_unit_id = w.id
                        AND w.requires_integration = 1
                        AND lower(trim(w.role)) != 'reviewer'
                        AND (NOT EXISTS (
                            SELECT 1 FROM work_unit_repository selected
                            WHERE selected.work_unit_id = w.id
                        ) OR EXISTS (
                            SELECT 1 FROM work_unit_repository selected
                            WHERE selected.work_unit_id = w.id
                              AND selected.repo_id = bound_workspace.repo_id
                        ))
                        AND EXISTS (
                            SELECT 1 FROM execution current_execution
                            WHERE current_execution.id = NEW.execution_id
                              AND current_execution.work_unit_id = w.id
                              AND current_execution.workspace_id IS (
                                  SELECT primary_scope.workspace_id
                                  FROM workspace_scope primary_scope
                                  JOIN workspace primary_workspace
                                    ON primary_workspace.id = primary_scope.workspace_id
                                   AND primary_workspace.task_id = primary_scope.task_id
                                  WHERE primary_scope.work_unit_id = w.id
                                    AND primary_scope.task_id = t.id
                                    AND primary_scope.scope_kind = 'work_unit'
                                    AND primary_scope.workspace_group_id = g.id
                                    AND primary_workspace.repo_id = t.repo_id
                              )
                        )
                        AND NEW.base_ref = COALESCE(bound_workspace.before_sha,
                            (SELECT default_branch FROM repo WHERE id = bound_workspace.repo_id))
                  ))
              )
        ) THEN RAISE(ABORT, 'Workspace lease renewal lacks current TaskRole/Execution authority')
    END;
END;

DROP TRIGGER workspace_lease_work_unit_binding_guard;
CREATE TRIGGER workspace_lease_work_unit_binding_guard
BEFORE INSERT ON workspace_lease
WHEN NEW.work_unit_id IS NOT NULL
 AND NOT EXISTS (
      SELECT 1 FROM work_unit w
      JOIN workspace_scope ws ON ws.work_unit_id = w.id AND ws.task_id = w.task_id
      JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
      JOIN workspace_group g ON g.id = ws.workspace_group_id
       AND g.task_id = ws.task_id AND g.work_unit_id = w.id
      JOIN execution e ON e.work_unit_id = w.id AND e.task_id = w.task_id
      JOIN task t ON t.id = w.task_id
      JOIN task_repository tr ON tr.task_id = t.id
       AND tr.repo_id = bound_workspace.repo_id
       AND tr.role IN ('primary', 'affected')
      WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        AND ws.workspace_id = NEW.workspace_id AND ws.scope_kind = 'work_unit'
        AND bound_workspace.repo_id = NEW.repository_binding_id
        AND bound_workspace.status = 'ready'
        AND w.requires_integration = 1 AND lower(trim(w.role)) != 'reviewer'
        AND (NOT EXISTS (
            SELECT 1 FROM work_unit_repository selected
            WHERE selected.work_unit_id = w.id
        ) OR EXISTS (
            SELECT 1 FROM work_unit_repository selected
            WHERE selected.work_unit_id = w.id AND selected.repo_id = bound_workspace.repo_id
        ))
        AND e.id = NEW.execution_id AND e.status = 'running'
        AND e.workspace_id IS (
            SELECT primary_scope.workspace_id
            FROM workspace_scope primary_scope
            JOIN workspace primary_workspace
              ON primary_workspace.id = primary_scope.workspace_id
             AND primary_workspace.task_id = primary_scope.task_id
            WHERE primary_scope.work_unit_id = w.id AND primary_scope.task_id = t.id
              AND primary_scope.scope_kind = 'work_unit'
              AND primary_scope.workspace_group_id = g.id
              AND primary_workspace.repo_id = t.repo_id
        )
        AND e.actor_kind = NEW.assigned_principal_type
        AND e.actor_id = NEW.assigned_principal_id AND e.role = w.role
        AND NEW.role = CASE WHEN lower(trim(w.role)) = 'reviewer' THEN 'reviewer' ELSE 'worker' END
        AND NEW.project_id = t.project_id
        AND NEW.base_ref = COALESCE(bound_workspace.before_sha,
            (SELECT default_branch FROM repo WHERE id = bound_workspace.repo_id))
        AND NEW.status = 'active'
        AND NEW.issuing_principal_type = 'system'
        AND NEW.issuing_principal_id = 'task-service-scheduler'
        AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
  )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease binding is invalid');
END;

DROP TRIGGER workspace_lease_work_unit_active_renewal_guard;
CREATE TRIGGER workspace_lease_work_unit_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.work_unit_id IS NOT NULL AND OLD.status = 'active' AND NEW.status = 'active'
 AND NOT EXISTS (
      SELECT 1 FROM work_unit w
      JOIN workspace_scope ws ON ws.work_unit_id = w.id AND ws.task_id = w.task_id
      JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
      JOIN workspace_group g ON g.id = ws.workspace_group_id
       AND g.task_id = ws.task_id AND g.work_unit_id = w.id
      JOIN execution e ON e.work_unit_id = w.id AND e.task_id = w.task_id
      JOIN task t ON t.id = w.task_id
      JOIN task_repository tr ON tr.task_id = t.id
       AND tr.repo_id = bound_workspace.repo_id
       AND tr.role IN ('primary', 'affected')
      WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        AND ws.workspace_id = NEW.workspace_id AND ws.scope_kind = 'work_unit'
        AND bound_workspace.repo_id = NEW.repository_binding_id
        AND bound_workspace.status = 'ready'
        AND w.requires_integration = 1 AND lower(trim(w.role)) != 'reviewer'
        AND (NOT EXISTS (
            SELECT 1 FROM work_unit_repository selected
            WHERE selected.work_unit_id = w.id
        ) OR EXISTS (
            SELECT 1 FROM work_unit_repository selected
            WHERE selected.work_unit_id = w.id AND selected.repo_id = bound_workspace.repo_id
        ))
        AND e.id = NEW.execution_id AND e.status = 'running'
        AND e.workspace_id IS (
            SELECT primary_scope.workspace_id
            FROM workspace_scope primary_scope
            JOIN workspace primary_workspace
              ON primary_workspace.id = primary_scope.workspace_id
             AND primary_workspace.task_id = primary_scope.task_id
            WHERE primary_scope.work_unit_id = w.id AND primary_scope.task_id = t.id
              AND primary_scope.scope_kind = 'work_unit'
              AND primary_scope.workspace_group_id = g.id
              AND primary_workspace.repo_id = t.repo_id
        )
        AND e.actor_kind = NEW.assigned_principal_type
        AND e.actor_id = NEW.assigned_principal_id AND e.role = w.role
        AND NEW.project_id = t.project_id
        AND NEW.repository_binding_id = bound_workspace.repo_id
        AND NEW.role = CASE WHEN lower(trim(w.role)) = 'reviewer' THEN 'reviewer' ELSE 'worker' END
        AND EXISTS (
            SELECT 1 FROM task_role role JOIN role_membership membership
              ON membership.task_role_id = role.id
            WHERE role.task_id = w.task_id AND role.role = w.role
              AND membership.actor_kind = e.actor_kind
              AND membership.actor_id = e.actor_id
              AND membership.status = 'active'
        )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease renewal authority is stale');
END;

