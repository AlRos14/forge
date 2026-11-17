-- Review independence for Agents belongs to isolated role sessions, not model identities.
-- Human contributors remain ineligible to review their own result. Historical rows are preserved.

DROP TRIGGER work_unit_review_subject_same_task_insert;
DROP TRIGGER work_unit_review_subject_reviewer_role_immutable;
DROP TRIGGER work_unit_review_independent_allocation;
DROP TRIGGER work_unit_review_independent_execution;

CREATE TRIGGER work_unit_review_subject_same_task_insert
BEFORE INSERT ON work_unit_review_subject
WHEN NOT EXISTS (
    SELECT 1
    FROM work_unit review
    JOIN task_role role ON role.task_id = review.task_id AND role.role = review.role
    JOIN task_role_capability_configuration configuration
      ON configuration.task_role_id = role.id
    WHERE review.id = NEW.review_work_unit_id AND review.task_id = NEW.task_id
      AND json_valid(configuration.configuration_json)
      AND json_extract(configuration.configuration_json, '$.schema_version') = 1
      AND EXISTS (
          SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
          WHERE type = 'text' AND value = 'formal_review'
      )
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
         AND review.assigned_actor_kind = 'human'
         AND review.assigned_actor_kind = reviewed.assigned_actor_kind
         AND review.assigned_actor_id = reviewed.assigned_actor_id
   )
   OR EXISTS (
       SELECT 1 FROM work_unit review
       JOIN execution contributor ON contributor.work_unit_id = NEW.reviewed_work_unit_id
       WHERE review.id = NEW.review_work_unit_id
         AND review.assigned_actor_kind = 'human'
         AND review.assigned_actor_kind = contributor.actor_kind
         AND review.assigned_actor_id = contributor.actor_id
   )
BEGIN
    SELECT RAISE(ABORT, 'review subject must name an exact same-Task result and independent formal reviewer');
END;

CREATE TRIGGER work_unit_review_subject_reviewer_role_immutable
BEFORE UPDATE OF role ON work_unit
WHEN EXISTS (
    SELECT 1 FROM work_unit_review_subject s WHERE s.review_work_unit_id = NEW.id
)
 AND NOT EXISTS (
    SELECT 1 FROM task_role role
    JOIN task_role_capability_configuration configuration
      ON configuration.task_role_id = role.id
    WHERE role.task_id = NEW.task_id AND role.role = NEW.role
      AND json_valid(configuration.configuration_json)
      AND json_extract(configuration.configuration_json, '$.schema_version') = 1
      AND EXISTS (
          SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
          WHERE type = 'text' AND value = 'formal_review'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'review WorkUnit requires a TaskRole with formal_review capability');
END;

CREATE TRIGGER work_unit_review_independent_allocation
BEFORE UPDATE OF role, assigned_actor_kind, assigned_actor_id ON work_unit
WHEN NEW.assigned_actor_kind IS NOT NULL
 AND EXISTS (
     SELECT 1 FROM work_unit_review_subject s
     WHERE s.review_work_unit_id = NEW.id
 )
 AND (
     EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit reviewed ON reviewed.id = s.reviewed_work_unit_id
         JOIN task_role role ON role.task_id = NEW.task_id AND role.role = NEW.role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE s.review_work_unit_id = NEW.id
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
           AND NEW.assigned_actor_kind = 'human'
           AND NEW.assigned_actor_kind = reviewed.assigned_actor_kind
           AND NEW.assigned_actor_id = reviewed.assigned_actor_id
     )
     OR NOT EXISTS (
         SELECT 1 FROM task_role role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE role.task_id = NEW.task_id AND role.role = NEW.role
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN execution contributor ON contributor.work_unit_id = s.reviewed_work_unit_id
         JOIN task_role role ON role.task_id = NEW.task_id AND role.role = NEW.role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE s.review_work_unit_id = NEW.id
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
           AND NEW.assigned_actor_kind = 'human'
           AND NEW.assigned_actor_kind = contributor.actor_kind
           AND NEW.assigned_actor_id = contributor.actor_id
     )
     OR EXISTS (
         SELECT 1 FROM work_unit_review_subject s
         JOIN work_unit review ON review.id = s.review_work_unit_id
         JOIN task_role role ON role.task_id = review.task_id AND role.role = review.role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         WHERE s.reviewed_work_unit_id = NEW.id
           AND EXISTS (
               SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities')
               WHERE type = 'text' AND value = 'formal_review'
           )
           AND NEW.assigned_actor_kind = 'human'
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

CREATE TRIGGER execution_role_session_isolation_insert
BEFORE INSERT ON execution
WHEN NEW.actor_kind = 'agent' AND NEW.harness_session_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.harness_session_id
        UNION
        SELECT s.predecessor_session_id FROM harness_session s JOIN ancestors a ON a.id=s.id
        WHERE s.predecessor_session_id IS NOT NULL
    )
    SELECT 1 FROM execution other JOIN ancestors a ON a.id=other.harness_session_id
    WHERE other.id != NEW.id AND (other.task_id != NEW.task_id OR other.role != NEW.role)
)
BEGIN
    SELECT RAISE(ABORT, 'Agent sessions and their context ancestry cannot cross Task roles');
END;

CREATE TRIGGER execution_role_session_isolation_update
BEFORE UPDATE OF harness_session_id, role ON execution
WHEN (NEW.harness_session_id IS NOT OLD.harness_session_id OR NEW.role IS NOT OLD.role) AND NEW.actor_kind = 'agent' AND NEW.harness_session_id IS NOT NULL AND EXISTS (
    WITH RECURSIVE ancestors(id) AS (
        SELECT NEW.harness_session_id
        UNION
        SELECT s.predecessor_session_id FROM harness_session s JOIN ancestors a ON a.id=s.id
        WHERE s.predecessor_session_id IS NOT NULL
    )
    SELECT 1 FROM execution other JOIN ancestors a ON a.id=other.harness_session_id
    WHERE other.id != NEW.id AND (other.task_id != NEW.task_id OR other.role != NEW.role)
)
BEGIN
    SELECT RAISE(ABORT, 'Agent sessions and their context ancestry cannot cross Task roles');
END;

CREATE TRIGGER harness_session_role_ancestry_immutable
BEFORE UPDATE OF predecessor_session_id ON harness_session
WHEN NEW.predecessor_session_id IS NOT OLD.predecessor_session_id
 AND EXISTS (SELECT 1 FROM execution WHERE harness_session_id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'A bound role session cannot change its context ancestry');
END;

CREATE TRIGGER same_agent_review_requires_isolated_session
BEFORE INSERT ON execution
WHEN NEW.actor_kind='agent' AND NEW.harness_session_id IS NULL
 AND EXISTS(SELECT 1 FROM work_unit_review_subject s JOIN execution contributor ON contributor.work_unit_id=s.reviewed_work_unit_id
            WHERE s.review_work_unit_id=NEW.work_unit_id AND contributor.actor_kind=NEW.actor_kind AND contributor.actor_id=NEW.actor_id)
BEGIN
    SELECT RAISE(ABORT, 'Review by the same Agent profile requires a fresh explicit HarnessSession');
END;
