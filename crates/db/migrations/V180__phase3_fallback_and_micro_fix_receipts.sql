-- Phase 3 receipts (the frozen V176 block, renumbered because V177-V179 exist).
-- task_coordinator_replan is already present in V172; this migration only adds
-- fallback and micro-fix evidence.

CREATE TABLE work_unit_fallback_receipt (
    id                          TEXT PRIMARY KEY,
    task_id                     TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    work_unit_id                TEXT NOT NULL,
    work_unit_version_before    INTEGER NOT NULL CHECK (work_unit_version_before >= 1),
    work_unit_version_after     INTEGER NOT NULL CHECK (work_unit_version_after = work_unit_version_before + 1),
    origin_execution_id         TEXT NOT NULL,
    failure_class               TEXT NOT NULL CHECK (failure_class = 'executor_unavailable'),
    route_attempts_digest       TEXT NOT NULL CHECK (length(route_attempts_digest) = 64),
    task_role_id                TEXT NOT NULL,
    from_actor_kind             TEXT NOT NULL CHECK (from_actor_kind = 'agent'),
    from_actor_id               TEXT NOT NULL CHECK (length(trim(from_actor_id)) > 0),
    to_actor_kind               TEXT NOT NULL CHECK (to_actor_kind = 'agent'),
    to_actor_id                 TEXT NOT NULL CHECK (length(trim(to_actor_id)) > 0),
    chain_index                 INTEGER NOT NULL CHECK (chain_index BETWEEN 0 AND 2),
    cost_class                  TEXT NOT NULL CHECK (cost_class IN ('economy', 'standard', 'premium')),
    authorization_version       INTEGER NOT NULL CHECK (authorization_version >= 1),
    envelope_digest             TEXT NOT NULL CHECK (length(envelope_digest) = 64),
    new_execution_id            TEXT,
    created_at                  TEXT NOT NULL,
    UNIQUE (origin_execution_id),
    UNIQUE (work_unit_id, chain_index),
    FOREIGN KEY (work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    FOREIGN KEY (origin_execution_id, task_id)
        REFERENCES execution(id, task_id) ON DELETE RESTRICT
);

CREATE INDEX idx_work_unit_fallback_receipt_task
    ON work_unit_fallback_receipt(task_id, created_at, id);

-- Bind every receipt to the exact failed WorkUnit attempt, unchanged TaskRole,
-- currently authorized envelope candidate, and active same-account Agent.
-- Route exhaustion and the start_uncertain exclusion are rechecked by the
-- dispatcher from the provider/routing receipts because neither is an
-- Execution column in the current schema.
CREATE TRIGGER work_unit_fallback_receipt_insert_guard
BEFORE INSERT ON work_unit_fallback_receipt
WHEN NOT EXISTS (
    SELECT 1
    FROM task_coordination coordination
    JOIN task task_row ON task_row.id = coordination.task_id
    JOIN project project_row ON project_row.id = task_row.project_id
    JOIN work_unit work_unit_row
      ON work_unit_row.id = NEW.work_unit_id
     AND work_unit_row.task_id = NEW.task_id
    JOIN task_role role_row
      ON role_row.id = NEW.task_role_id
     AND role_row.task_id = NEW.task_id
     AND role_row.role = work_unit_row.role
    JOIN execution origin
      ON origin.id = NEW.origin_execution_id
     AND origin.task_id = NEW.task_id
     AND origin.work_unit_id = NEW.work_unit_id
     AND origin.work_unit_version = NEW.work_unit_version_before
     AND origin.actor_kind = NEW.from_actor_kind
     AND origin.actor_id = NEW.from_actor_id
     AND origin.role = role_row.role
     AND origin.status = 'failed'
    JOIN execution_task_role_authority origin_authority
      ON origin_authority.execution_id = origin.id
     AND origin_authority.task_role_id = role_row.id
     AND origin_authority.actor_kind = origin.actor_kind
     AND origin_authority.actor_id = origin.actor_id
    JOIN role_membership target_membership
      ON target_membership.task_role_id = role_row.id
     AND target_membership.actor_kind = NEW.to_actor_kind
     AND target_membership.actor_id = NEW.to_actor_id
     AND target_membership.status = 'active'
    JOIN agent_identity target_agent ON target_agent.id = NEW.to_actor_id
    JOIN agent_identity origin_agent ON origin_agent.id = NEW.from_actor_id
    WHERE coordination.task_id = NEW.task_id
      AND coordination.mode != 'manual'
      AND coordination.revoked_at IS NULL
      AND coordination.team_stale = 0
      AND coordination.authorization_version = NEW.authorization_version
      AND coordination.envelope_digest = NEW.envelope_digest
      AND work_unit_row.status = 'open'
      AND work_unit_row.version = NEW.work_unit_version_after
      AND work_unit_row.assigned_actor_kind = NEW.to_actor_kind
      AND work_unit_row.assigned_actor_id = NEW.to_actor_id
      AND project_row.owner_id IS NOT NULL
      AND target_agent.owner_id = project_row.owner_id
      AND origin_agent.owner_id = project_row.owner_id
      AND NOT (coordination.coordinator_actor_kind = 'agent'
               AND coordination.coordinator_actor_id = NEW.to_actor_id)
      AND json_type(coordination.envelope_json, '$.fallback_agents') = 'array'
      AND EXISTS (
          SELECT 1
          FROM json_each(coordination.envelope_json, '$.fallback_agents') candidate
          WHERE json_extract(candidate.value, '$.task_role_id') = NEW.task_role_id
            AND json_extract(candidate.value, '$.agent_id') = NEW.to_actor_id
            AND json_extract(candidate.value, '$.cost_class') = NEW.cost_class
            AND (
                SELECT COUNT(*)
                FROM json_each(coordination.envelope_json, '$.fallback_agents') earlier
                WHERE json_extract(earlier.value, '$.task_role_id') = NEW.task_role_id
                  AND CAST(earlier.key AS INTEGER) < CAST(candidate.key AS INTEGER)
            ) = NEW.chain_index
      )
      AND (
          NEW.new_execution_id IS NULL
          OR EXISTS (
              SELECT 1 FROM execution replacement
              WHERE replacement.id = NEW.new_execution_id
                AND replacement.task_id = NEW.task_id
                AND replacement.work_unit_id = NEW.work_unit_id
                AND replacement.work_unit_version = NEW.work_unit_version_after
                AND replacement.role = role_row.role
                AND replacement.actor_kind = NEW.to_actor_kind
                AND replacement.actor_id = NEW.to_actor_id
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_FALLBACK_NOT_AUTHORIZED');
END;

CREATE TRIGGER work_unit_fallback_receipt_immutable_update
BEFORE UPDATE ON work_unit_fallback_receipt
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit fallback receipts are immutable');
END;

CREATE TRIGGER work_unit_fallback_receipt_immutable_delete
BEFORE DELETE ON work_unit_fallback_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task task_row
    JOIN project_deletion_guard guard ON guard.project_id = task_row.project_id
    WHERE task_row.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit fallback receipts are preserved outside Project teardown');
END;

CREATE TABLE micro_fix_receipt (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    request_id              TEXT NOT NULL REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    source_kind             TEXT NOT NULL CHECK (source_kind IN ('review_findings', 'validation_findings', 'external_feedback')),
    finding_refs_json       TEXT NOT NULL CHECK (json_valid(finding_refs_json) AND json_type(finding_refs_json) = 'array'),
    declared_paths_json     TEXT NOT NULL CHECK (json_valid(declared_paths_json) AND json_type(declared_paths_json) = 'array'
        AND json_array_length(declared_paths_json) <= 3),
    fix_work_unit_id        TEXT NOT NULL,
    reviewed_commit_sha     TEXT NOT NULL CHECK (length(trim(reviewed_commit_sha)) BETWEEN 7 AND 128),
    result_commit_sha       TEXT,
    changed_files           INTEGER CHECK (changed_files IS NULL OR changed_files >= 0),
    changed_lines           INTEGER CHECK (changed_lines IS NULL OR changed_lines >= 0),
    bounds_outcome          TEXT NOT NULL CHECK (bounds_outcome IN ('pending', 'within_bounds', 'bounds_exceeded')),
    bounds_violation        TEXT CHECK (bounds_violation IN ('paths_outside_declared', 'forbidden_glob',
        'too_many_files', 'too_many_lines', 'new_dependency')),
    reviewer_cost_class     TEXT CHECK (reviewer_cost_class IN ('economy', 'standard', 'premium')),
    reviewer_actor_kind     TEXT CHECK (reviewer_actor_kind IS NULL OR reviewer_actor_kind = 'agent'),
    reviewer_actor_id       TEXT CHECK (reviewer_actor_id IS NULL OR length(trim(reviewer_actor_id)) > 0),
    envelope_digest         TEXT NOT NULL CHECK (length(envelope_digest) = 64),
    ordinal                 INTEGER NOT NULL CHECK (ordinal BETWEEN 1 AND 3),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE (task_id, ordinal),
    UNIQUE (request_id, fix_work_unit_id),
    FOREIGN KEY (fix_work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    CHECK ((reviewer_actor_kind IS NULL) = (reviewer_actor_id IS NULL)),
    CHECK ((reviewer_actor_kind IS NULL) = (reviewer_cost_class IS NULL)),
    CHECK ((bounds_outcome = 'pending' AND result_commit_sha IS NULL
            AND changed_files IS NULL AND changed_lines IS NULL AND bounds_violation IS NULL)
        OR (bounds_outcome = 'within_bounds' AND result_commit_sha IS NOT NULL
            AND changed_files IS NOT NULL AND changed_lines IS NOT NULL AND bounds_violation IS NULL)
        OR (bounds_outcome = 'bounds_exceeded' AND result_commit_sha IS NOT NULL
            AND changed_files IS NOT NULL AND changed_lines IS NOT NULL AND bounds_violation IS NOT NULL))
);

CREATE INDEX idx_micro_fix_receipt_task
    ON micro_fix_receipt(task_id, ordinal);

CREATE TRIGGER micro_fix_receipt_insert_guard
BEFORE INSERT ON micro_fix_receipt
WHEN NOT EXISTS (
    SELECT 1
    FROM task_coordination_request request
    JOIN task_coordination coordination ON coordination.task_id = request.task_id
    JOIN work_unit fix_work_unit
      ON fix_work_unit.id = NEW.fix_work_unit_id
     AND fix_work_unit.task_id = NEW.task_id
    WHERE request.id = NEW.request_id
      AND request.task_id = NEW.task_id
      AND request.kind = NEW.source_kind
      AND request.state = 'triaged'
      AND request.execution_id IS NOT NULL
      AND coordination.mode != 'manual'
      AND coordination.revoked_at IS NULL
      AND coordination.team_stale = 0
      AND coordination.authorization_version = request.authorization_version
      AND coordination.envelope_digest = NEW.envelope_digest
      AND json_extract(coordination.envelope_json, '$.micro_fix.enabled') = 1
      AND NEW.ordinal = COALESCE((
          SELECT MAX(existing.ordinal) + 1
          FROM micro_fix_receipt existing
          WHERE existing.task_id = NEW.task_id
      ), 1)
      AND NEW.ordinal <= json_extract(coordination.envelope_json, '$.micro_fix.max_per_task')
      AND fix_work_unit.status = 'open'
      AND (
          (NEW.source_kind = 'review_findings' AND EXISTS (
              SELECT 1 FROM task_review_triage triage
              WHERE triage.task_id = NEW.task_id
                AND triage.review_report_artifact_id = request.review_report_artifact_id
                AND triage.report_digest = request.review_report_digest
                AND triage.execution_id = request.execution_id
                AND triage.authorization_version = request.authorization_version
                AND triage.disposition = 'correct'
          ))
          OR (NEW.source_kind = 'validation_findings' AND EXISTS (
              SELECT 1 FROM task_validation_triage triage
              WHERE triage.request_id = request.id
                AND triage.task_id = NEW.task_id
                AND triage.execution_id = request.execution_id
                AND triage.authorization_version = request.authorization_version
                AND triage.disposition = 'correct'
          ))
          OR (NEW.source_kind = 'external_feedback' AND EXISTS (
              SELECT 1 FROM task_external_feedback_triage triage
              JOIN task_external_feedback_disposition disposition
                ON disposition.triage_id = triage.id
               AND disposition.request_id = triage.request_id
               AND disposition.feedback_item_id = triage.feedback_item_id
              WHERE triage.request_id = request.id
                AND triage.task_id = NEW.task_id
                AND triage.execution_id = request.execution_id
                AND triage.disposition = 'correct'
                AND disposition.disposition = 'extend_task'
          ))
      )
      AND (
          NEW.reviewer_actor_id IS NULL
          OR EXISTS (
              SELECT 1
              FROM task_role reviewer_role
              JOIN role_membership reviewer_membership
                ON reviewer_membership.task_role_id = reviewer_role.id
               AND reviewer_membership.actor_kind = 'agent'
               AND reviewer_membership.actor_id = NEW.reviewer_actor_id
               AND reviewer_membership.status = 'active'
              JOIN agent_identity reviewer_agent ON reviewer_agent.id = NEW.reviewer_actor_id
              JOIN task reviewer_task ON reviewer_task.id = NEW.task_id
              JOIN project reviewer_project ON reviewer_project.id = reviewer_task.project_id
              JOIN task_role_capability_configuration reviewer_configuration
                ON reviewer_configuration.task_role_id = reviewer_role.id
              WHERE json_type(coordination.envelope_json, '$.micro_fix_reviewer') = 'object'
                AND reviewer_role.id = json_extract(coordination.envelope_json,
                                                     '$.micro_fix_reviewer.task_role_id')
                AND reviewer_role.task_id = NEW.task_id
                AND json_extract(coordination.envelope_json,
                                 '$.micro_fix_reviewer.agent_id') = NEW.reviewer_actor_id
                AND json_extract(coordination.envelope_json,
                                 '$.micro_fix_reviewer.cost_class') = NEW.reviewer_cost_class
                AND reviewer_agent.owner_id = reviewer_project.owner_id
                AND reviewer_role.role IN (
                    SELECT allowed.value
                    FROM json_each(coordination.envelope_json, '$.allowed_review_roles') allowed
                )
                AND EXISTS (
                    SELECT 1
                    FROM json_each(reviewer_configuration.configuration_json, '$.capabilities') capability
                    WHERE capability.value = 'formal_review'
                )
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_MICRO_FIX_NOT_AUTHORIZED');
END;

CREATE TRIGGER micro_fix_receipt_bounds_advance_guard
BEFORE UPDATE ON micro_fix_receipt
WHEN OLD.bounds_outcome != 'pending'
  OR NEW.bounds_outcome NOT IN ('within_bounds', 'bounds_exceeded')
  OR NEW.id IS NOT OLD.id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.request_id IS NOT OLD.request_id
  OR NEW.source_kind IS NOT OLD.source_kind
  OR NEW.finding_refs_json IS NOT OLD.finding_refs_json
  OR NEW.declared_paths_json IS NOT OLD.declared_paths_json
  OR NEW.fix_work_unit_id IS NOT OLD.fix_work_unit_id
  OR NEW.reviewed_commit_sha IS NOT OLD.reviewed_commit_sha
  OR NEW.reviewer_cost_class IS NOT OLD.reviewer_cost_class
  OR NEW.reviewer_actor_kind IS NOT OLD.reviewer_actor_kind
  OR NEW.reviewer_actor_id IS NOT OLD.reviewer_actor_id
  OR NEW.envelope_digest IS NOT OLD.envelope_digest
  OR NEW.ordinal IS NOT OLD.ordinal
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.result_commit_sha IS NULL
  OR NEW.changed_files IS NULL
  OR NEW.changed_lines IS NULL
  OR (NEW.bounds_outcome = 'within_bounds' AND NEW.bounds_violation IS NOT NULL)
  OR (NEW.bounds_outcome = 'bounds_exceeded' AND NEW.bounds_violation IS NULL)
  OR NEW.updated_at IS OLD.updated_at
BEGIN
    SELECT RAISE(ABORT, 'FORGE_MICRO_FIX_BOUNDS_ALREADY_ADVANCED');
END;

CREATE TRIGGER micro_fix_receipt_immutable_delete
BEFORE DELETE ON micro_fix_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task task_row
    JOIN project_deletion_guard guard ON guard.project_id = task_row.project_id
    WHERE task_row.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Micro-fix receipts are preserved outside Project teardown');
END;
