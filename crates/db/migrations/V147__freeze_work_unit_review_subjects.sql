-- Review subjects can be prepared before their target has a result. Freeze the
-- exact WorkUnit revision and result only when that revision is accepted.

DROP TRIGGER work_unit_review_subject_same_task_insert;
DROP TRIGGER work_unit_review_subject_immutable_update;
DROP TRIGGER work_unit_review_subject_immutable_delete;
DROP INDEX idx_work_unit_review_subject_reviewed;

ALTER TABLE work_unit_review_subject RENAME TO work_unit_review_subject_v143;

CREATE TABLE work_unit_review_subject (
    review_work_unit_id        TEXT PRIMARY KEY REFERENCES work_unit(id) ON DELETE CASCADE,
    task_id                    TEXT NOT NULL,
    reviewed_work_unit_id      TEXT NOT NULL,
    reviewed_work_unit_version INTEGER CHECK (
        reviewed_work_unit_version IS NULL OR reviewed_work_unit_version >= 1
    ),
    result_execution_id        TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    result_artifact_id         TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    result_commit_sha          TEXT,
    result_workspace_id        TEXT REFERENCES workspace(id) ON DELETE RESTRICT,
    workspace_snapshot_digest  TEXT,
    created_at                 TEXT NOT NULL,
    CHECK (review_work_unit_id != reviewed_work_unit_id),
    CHECK (result_execution_id IS NULL OR reviewed_work_unit_version IS NOT NULL),
    CHECK (result_execution_id IS NOT NULL OR (
        result_artifact_id IS NULL AND result_commit_sha IS NULL
        AND result_workspace_id IS NULL AND workspace_snapshot_digest IS NULL
    ))
);

INSERT INTO work_unit_review_subject (
    review_work_unit_id, task_id, reviewed_work_unit_id,
    reviewed_work_unit_version, result_execution_id, result_artifact_id,
    result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
)
SELECT review_work_unit_id, task_id, reviewed_work_unit_id,
       reviewed_work_unit_version, result_execution_id, result_artifact_id,
       result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
FROM work_unit_review_subject_v143;

-- V143 could store a planned version before admission. Such a version was not
-- authoritative: allocation or a failed attempt could advance it before the
-- result was accepted. Keep immutable completed subjects; let pending subjects
-- bind to the revision that eventually completes.
UPDATE work_unit_review_subject
   SET reviewed_work_unit_version = NULL,
       result_artifact_id = NULL,
       result_commit_sha = NULL,
       result_workspace_id = NULL,
       workspace_snapshot_digest = NULL
 WHERE result_execution_id IS NULL;

DROP TABLE work_unit_review_subject_v143;

CREATE INDEX idx_work_unit_review_subject_reviewed
    ON work_unit_review_subject(task_id, reviewed_work_unit_id, reviewed_work_unit_version);

CREATE TRIGGER work_unit_review_subject_same_task_insert
BEFORE INSERT ON work_unit_review_subject
WHEN NOT EXISTS (
    SELECT 1 FROM work_unit review
    WHERE review.id = NEW.review_work_unit_id AND review.task_id = NEW.task_id
      AND review.role = 'reviewer'
)
   OR NOT EXISTS (
    SELECT 1 FROM work_unit reviewed
    WHERE reviewed.id = NEW.reviewed_work_unit_id AND reviewed.task_id = NEW.task_id
)
   OR EXISTS (
       SELECT 1 FROM work_unit reviewed
       WHERE reviewed.id = NEW.reviewed_work_unit_id
         AND reviewed.status != 'open'
         AND NOT (
             reviewed.status = 'completed'
             AND NEW.reviewed_work_unit_version = reviewed.version - 1
             AND NEW.result_execution_id IS NOT NULL
         )
   )
   OR EXISTS (
       SELECT 1 FROM work_unit reviewed
       WHERE reviewed.id = NEW.reviewed_work_unit_id AND reviewed.status = 'open'
         AND (NEW.reviewed_work_unit_version IS NOT NULL OR NEW.result_execution_id IS NOT NULL)
   )
   OR EXISTS (
       SELECT 1 FROM work_unit reviewed
       WHERE reviewed.id = NEW.reviewed_work_unit_id AND reviewed.status = 'completed'
         AND NEW.result_execution_id IS NULL
   )
   OR (NEW.reviewed_work_unit_version IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM execution e
       WHERE e.task_id = NEW.task_id
         AND e.work_unit_id = NEW.reviewed_work_unit_id
         AND e.work_unit_version = NEW.reviewed_work_unit_version
   ))
   OR (NEW.result_execution_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM execution e
       WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
         AND e.work_unit_id = NEW.reviewed_work_unit_id
         AND e.work_unit_version = NEW.reviewed_work_unit_version
         AND e.status = 'completed'
         AND e.after_sha IS NEW.result_commit_sha
         AND e.workspace_id IS NEW.result_workspace_id
   ))
   OR (NEW.result_artifact_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM artifact a
       WHERE a.id = NEW.result_artifact_id AND a.task_id = NEW.task_id
         AND EXISTS (
             SELECT 1 FROM artifact_execution_producer p
             WHERE p.artifact_id = a.id AND p.task_id = a.task_id
               AND p.execution_id = NEW.result_execution_id
         )
   ))
   OR (NEW.workspace_snapshot_digest IS NOT NULL
       AND length(NEW.workspace_snapshot_digest) != 64)
   OR (NEW.result_workspace_id IS NOT NULL
       AND NEW.workspace_snapshot_digest IS NULL)
   OR (NEW.result_workspace_id IS NOT NULL AND NOT EXISTS (
       SELECT 1 FROM workspace w
       WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
   ))
   OR EXISTS (
       SELECT 1 FROM work_unit review
       JOIN work_unit reviewed ON reviewed.id = NEW.reviewed_work_unit_id
       WHERE review.id = NEW.review_work_unit_id
         AND review.assigned_actor_kind IS NOT NULL
         AND review.assigned_actor_kind = reviewed.assigned_actor_kind
         AND review.assigned_actor_id = reviewed.assigned_actor_id
   )
   OR EXISTS (
       SELECT 1 FROM work_unit review
       JOIN execution contributor ON contributor.work_unit_id = NEW.reviewed_work_unit_id
       WHERE review.id = NEW.review_work_unit_id
         AND review.assigned_actor_kind = contributor.actor_kind
         AND review.assigned_actor_id = contributor.actor_id
   )
BEGIN
    SELECT RAISE(ABORT, 'review subject must name an exact same-Task result and independent reviewer');
END;

CREATE TRIGGER work_unit_review_subject_completion_event_guard
BEFORE INSERT ON domain_event
WHEN NEW.event_type = 'work_unit.completed'
 AND NEW.entity_type = 'work_unit'
 AND NEW.scope_type = 'task'
 AND EXISTS (
     SELECT 1 FROM work_unit_review_subject s
     WHERE s.task_id = NEW.scope_id AND s.reviewed_work_unit_id = NEW.entity_id
       AND s.result_execution_id IS NULL
 )
 AND (
     json_valid(NEW.payload_json) = 0
     OR NOT EXISTS (
         SELECT 1 FROM work_unit w
         WHERE w.id = NEW.entity_id AND w.task_id = NEW.scope_id
           AND w.status = 'completed'
           AND w.version = json_extract(NEW.payload_json, '$.version')
     )
     OR json_type(NEW.payload_json, '$.reviewed_result.work_unit_version') != 'integer'
     OR json_extract(NEW.payload_json, '$.reviewed_result.work_unit_version') != (
         SELECT version - 1 FROM work_unit WHERE id = NEW.entity_id AND task_id = NEW.scope_id
     )
     OR json_extract(NEW.payload_json, '$.reviewed_result.work_unit_version') !=
        json_extract(NEW.payload_json, '$.version') - 1
     OR json_type(NEW.payload_json, '$.reviewed_result.execution_id') != 'text'
     OR NOT EXISTS (
         SELECT 1 FROM execution e
         WHERE e.id = json_extract(NEW.payload_json, '$.reviewed_result.execution_id')
           AND e.task_id = NEW.scope_id AND e.work_unit_id = NEW.entity_id
           AND e.work_unit_version = json_extract(NEW.payload_json, '$.reviewed_result.work_unit_version')
           AND e.status = 'completed'
           AND e.after_sha IS json_extract(NEW.payload_json, '$.reviewed_result.commit_sha')
           AND e.workspace_id IS json_extract(NEW.payload_json, '$.reviewed_result.workspace_id')
           AND (
               (e.workspace_id IS NULL
                AND json_type(NEW.payload_json, '$.reviewed_result.workspace_snapshot_digest') = 'null')
               OR
               (e.workspace_id IS NOT NULL
                AND json_type(NEW.payload_json, '$.reviewed_result.workspace_snapshot_digest') = 'text'
                AND length(json_extract(NEW.payload_json, '$.reviewed_result.workspace_snapshot_digest')) = 64)
           )
           AND (
               json_extract(NEW.payload_json, '$.reviewed_result.artifact_id') IS NULL
               OR EXISTS (
                   SELECT 1 FROM artifact_execution_producer p
                   WHERE p.execution_id = e.id AND p.task_id = e.task_id
                     AND p.artifact_id = json_extract(NEW.payload_json, '$.reviewed_result.artifact_id')
               )
           )
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'completed reviewed WorkUnit requires an exact frozen result');
END;

CREATE TRIGGER work_unit_review_subject_freeze_result_on_completion_event
AFTER INSERT ON domain_event
WHEN NEW.event_type = 'work_unit.completed'
 AND NEW.entity_type = 'work_unit'
 AND NEW.scope_type = 'task'
 AND EXISTS (
     SELECT 1 FROM work_unit_review_subject s
     WHERE s.task_id = NEW.scope_id AND s.reviewed_work_unit_id = NEW.entity_id
       AND s.result_execution_id IS NULL
 )
BEGIN
    UPDATE work_unit_review_subject
       SET reviewed_work_unit_version = json_extract(NEW.payload_json, '$.reviewed_result.work_unit_version'),
           result_execution_id = json_extract(NEW.payload_json, '$.reviewed_result.execution_id'),
           result_artifact_id = json_extract(NEW.payload_json, '$.reviewed_result.artifact_id'),
           result_commit_sha = json_extract(NEW.payload_json, '$.reviewed_result.commit_sha'),
           result_workspace_id = json_extract(NEW.payload_json, '$.reviewed_result.workspace_id'),
           workspace_snapshot_digest = json_extract(NEW.payload_json, '$.reviewed_result.workspace_snapshot_digest')
     WHERE task_id = NEW.scope_id AND reviewed_work_unit_id = NEW.entity_id
       AND result_execution_id IS NULL;
END;

CREATE TRIGGER work_unit_review_subject_immutable_update
BEFORE UPDATE ON work_unit_review_subject
WHEN NEW.review_work_unit_id IS NOT OLD.review_work_unit_id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.reviewed_work_unit_id IS NOT OLD.reviewed_work_unit_id
  OR NEW.created_at IS NOT OLD.created_at
  OR (NEW.reviewed_work_unit_version IS NOT OLD.reviewed_work_unit_version
      AND NOT (
          OLD.reviewed_work_unit_version IS NULL
          AND NEW.reviewed_work_unit_version IS NOT NULL
          AND EXISTS (
              SELECT 1 FROM domain_event d
              WHERE d.event_type = 'work_unit.completed'
                AND d.entity_type = 'work_unit'
                AND d.entity_id = NEW.reviewed_work_unit_id
                AND d.scope_type = 'task' AND d.scope_id = NEW.task_id
                AND json_extract(d.payload_json, '$.reviewed_result.work_unit_version')
                    = NEW.reviewed_work_unit_version
          )
      ))
  OR ((NEW.result_execution_id IS NOT OLD.result_execution_id
       OR NEW.result_artifact_id IS NOT OLD.result_artifact_id
       OR NEW.result_commit_sha IS NOT OLD.result_commit_sha
       OR NEW.result_workspace_id IS NOT OLD.result_workspace_id
       OR NEW.workspace_snapshot_digest IS NOT OLD.workspace_snapshot_digest)
      AND NOT (
          (
              OLD.result_execution_id IS NULL
              AND NEW.result_execution_id IS NOT NULL
              AND NEW.reviewed_work_unit_version IS NOT NULL
              AND EXISTS (
                  SELECT 1 FROM domain_event d
                  WHERE d.event_type = 'work_unit.completed'
                    AND d.entity_type = 'work_unit'
                    AND d.entity_id = NEW.reviewed_work_unit_id
                    AND d.scope_type = 'task' AND d.scope_id = NEW.task_id
                    AND json_extract(d.payload_json, '$.reviewed_result.work_unit_version')
                        = NEW.reviewed_work_unit_version
                    AND json_extract(d.payload_json, '$.reviewed_result.execution_id')
                        = NEW.result_execution_id
                    AND json_extract(d.payload_json, '$.reviewed_result.artifact_id') IS NEW.result_artifact_id
                    AND json_extract(d.payload_json, '$.reviewed_result.commit_sha') IS NEW.result_commit_sha
                    AND json_extract(d.payload_json, '$.reviewed_result.workspace_id') IS NEW.result_workspace_id
                    AND json_extract(d.payload_json, '$.reviewed_result.workspace_snapshot_digest')
                        IS NEW.workspace_snapshot_digest
              )
              AND EXISTS (
                  SELECT 1 FROM execution e
                  WHERE e.id = NEW.result_execution_id AND e.task_id = NEW.task_id
                    AND e.work_unit_id = NEW.reviewed_work_unit_id
                    AND e.work_unit_version = NEW.reviewed_work_unit_version
                    AND e.status = 'completed'
                    AND e.after_sha IS NEW.result_commit_sha
                    AND e.workspace_id IS NEW.result_workspace_id
              )
              AND (NEW.result_artifact_id IS NULL OR EXISTS (
                  SELECT 1 FROM artifact a
                  JOIN artifact_execution_producer p ON p.artifact_id = a.id
                  WHERE a.id = NEW.result_artifact_id AND a.task_id = NEW.task_id
                    AND p.execution_id = NEW.result_execution_id AND p.task_id = NEW.task_id
              ))
              AND (NEW.result_workspace_id IS NULL OR EXISTS (
                  SELECT 1 FROM workspace w
                  WHERE w.id = NEW.result_workspace_id AND w.task_id = NEW.task_id
              ))
          )
      ))
BEGIN
    SELECT RAISE(ABORT, 'review subject identity and frozen result are immutable');
END;

CREATE TRIGGER work_unit_review_subject_immutable_delete
BEFORE DELETE ON work_unit_review_subject
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'review subjects are immutable outside Project teardown');
END;

CREATE TRIGGER work_unit_review_subject_reviewer_role_immutable
BEFORE UPDATE OF role ON work_unit
WHEN NEW.role != 'reviewer'
 AND EXISTS (
     SELECT 1 FROM work_unit_review_subject s
     WHERE s.review_work_unit_id = NEW.id
 )
BEGIN
    SELECT RAISE(ABORT, 'review WorkUnit role is fixed by its immutable subject');
END;

-- A review subject is a readiness edge from the reviewed result to its review
-- WorkUnit. Keep the combined dependency/readiness graph acyclic.
CREATE TRIGGER work_unit_dependency_review_cycle_guard_insert
BEFORE INSERT ON work_unit_dependency
WHEN EXISTS (
    WITH RECURSIVE edges(src, dst) AS (
        SELECT depends_on_work_unit_id, work_unit_id FROM work_unit_dependency
        UNION ALL
        SELECT reviewed_work_unit_id, review_work_unit_id FROM work_unit_review_subject
    ), reachable(id) AS (
        SELECT NEW.work_unit_id
        UNION
        SELECT edges.dst FROM edges JOIN reachable ON edges.src = reachable.id
    )
    SELECT 1 FROM reachable WHERE id = NEW.depends_on_work_unit_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit dependency would create a combined review cycle');
END;

CREATE TRIGGER work_unit_review_independent_allocation
BEFORE UPDATE OF role, assigned_actor_kind, assigned_actor_id ON work_unit
WHEN NEW.assigned_actor_kind IS NOT NULL
 AND (
     EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit reviewed ON reviewed.id = s.reviewed_work_unit_id
         WHERE s.review_work_unit_id = NEW.id
           AND NEW.role = 'reviewer'
           AND NEW.assigned_actor_kind = reviewed.assigned_actor_kind
           AND NEW.assigned_actor_id = reviewed.assigned_actor_id
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN execution contributor ON contributor.work_unit_id = s.reviewed_work_unit_id
         WHERE s.review_work_unit_id = NEW.id
           AND NEW.role = 'reviewer'
           AND NEW.assigned_actor_kind = contributor.actor_kind
           AND NEW.assigned_actor_id = contributor.actor_id
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit review ON review.id = s.review_work_unit_id
         WHERE s.reviewed_work_unit_id = NEW.id
           AND review.assigned_actor_kind = NEW.assigned_actor_kind
           AND review.assigned_actor_id = NEW.assigned_actor_id
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit allocation violates exact review independence');
END;

CREATE TRIGGER work_unit_review_independent_execution
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND (
     (NEW.role = 'reviewer' AND (
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
               AND NEW.actor_kind = reviewed.assigned_actor_kind
               AND NEW.actor_id = reviewed.assigned_actor_id
         )
         OR EXISTS (
             SELECT 1 FROM work_unit_review_subject s
             JOIN execution contributor ON contributor.work_unit_id = s.reviewed_work_unit_id
             WHERE s.review_work_unit_id = NEW.work_unit_id
               AND NEW.actor_kind = contributor.actor_kind
               AND NEW.actor_id = contributor.actor_id
         )
     ))
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit review ON review.id = s.review_work_unit_id
         WHERE s.reviewed_work_unit_id = NEW.work_unit_id
           AND review.assigned_actor_kind = NEW.actor_kind
           AND review.assigned_actor_id = NEW.actor_id
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Execution violates exact review independence or readiness');
END;

