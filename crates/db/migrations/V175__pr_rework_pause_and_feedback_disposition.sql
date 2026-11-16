-- Phase 2 (node PX, D3'): non-destructive pause of a remote PR admission so
-- external-feedback rework runs on the SAME Task and the SAME open PR.
-- Additive only: remote_pr_admission, remote_pr_history, pr_metadata,
-- task_integration_operation and task_external_feedback_triage are NOT rebuilt.
-- The pause terminal is the TaskMerge status 'abandoned' plus the additive
-- remote_pr_admission.rework_outcome marker and the immutable
-- pr_rework_supersession receipt (the admission state stays 'open': it is the
-- last provider-observed state and the PR is never closed or merged by Forge).
-- Also adds the real per-item disposition (extend_task|propose_task|human|dismiss)
-- next to the V173 triage row, which keeps its legacy mapping.

CREATE TABLE pr_rework_supersession (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    task_merge_operation_id TEXT NOT NULL UNIQUE
        REFERENCES remote_pr_admission(task_merge_operation_id) ON DELETE RESTRICT,
    publish_operation_id TEXT NOT NULL,
    metadata_id TEXT NOT NULL,
    request_id TEXT NOT NULL REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    triage_set_digest TEXT NOT NULL CHECK (length(triage_set_digest) = 64),
    provider_pr_id TEXT NOT NULL CHECK (length(trim(provider_pr_id)) > 0),
    superseded_head_sha TEXT NOT NULL
        CHECK (length(trim(superseded_head_sha)) BETWEEN 7 AND 128),
    result_event_id TEXT NOT NULL REFERENCES domain_event(id) ON DELETE RESTRICT,
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN
        ('open', 'readmitted', 'remote_merged', 'remote_closed', 'abandoned')),
    readmission_merge_operation_id TEXT,
    convergence_decision_id TEXT,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(id, task_id),
    CHECK ((state = 'readmitted') = (readmission_merge_operation_id IS NOT NULL))
);
CREATE UNIQUE INDEX idx_pr_rework_supersession_open
    ON pr_rework_supersession(task_id) WHERE state = 'open';

ALTER TABLE remote_pr_admission ADD COLUMN rework_outcome TEXT
    CHECK (rework_outcome IS NULL OR rework_outcome = 'superseded_for_rework');
ALTER TABLE remote_pr_admission ADD COLUMN supersession_id TEXT;
ALTER TABLE remote_pr_admission ADD COLUMN readmits_supersession_id TEXT;

CREATE TABLE task_external_feedback_disposition (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    triage_id TEXT NOT NULL UNIQUE
        REFERENCES task_external_feedback_triage(id) ON DELETE RESTRICT,
    request_id TEXT NOT NULL,
    feedback_item_id TEXT NOT NULL,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    disposition TEXT NOT NULL
        CHECK (disposition IN ('extend_task', 'propose_task', 'human', 'dismiss')),
    legacy_disposition TEXT NOT NULL
        CHECK (legacy_disposition IN ('correct', 'human', 'dismiss')),
    scope_classification TEXT
        CHECK (scope_classification IN ('within_task_scope', 'scope_change')),
    classification_facts_json TEXT
        CHECK (classification_facts_json IS NULL OR json_valid(classification_facts_json)),
    dismiss_reason TEXT CHECK (dismiss_reason IN
        ('not_actionable', 'disagree', 'duplicate', 'already_addressed', 'out_of_date')),
    proposal_json TEXT CHECK (proposal_json IS NULL OR json_valid(proposal_json)),
    human_decision_id TEXT,
    followup_task_id TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(request_id, feedback_item_id),
    FOREIGN KEY (feedback_item_id, task_id)
        REFERENCES external_feedback_item(id, task_id) ON DELETE RESTRICT,
    CHECK ((disposition = 'extend_task' AND legacy_disposition = 'correct'
            AND scope_classification = 'within_task_scope')
        OR (disposition = 'propose_task' AND legacy_disposition = 'human'
            AND scope_classification IS NOT NULL AND proposal_json IS NOT NULL)
        OR (disposition = 'human' AND legacy_disposition = 'human')
        OR (disposition = 'dismiss' AND legacy_disposition = 'dismiss'
            AND dismiss_reason IS NOT NULL))
);

-- ---------------------------------------------------------------------------
-- Disposition triggers.
-- ---------------------------------------------------------------------------
CREATE TRIGGER task_external_feedback_disposition_insert_guard
BEFORE INSERT ON task_external_feedback_disposition
WHEN NEW.human_decision_id IS NOT NULL OR NEW.followup_task_id IS NOT NULL
  OR NOT EXISTS (
      SELECT 1 FROM task_external_feedback_triage t
      WHERE t.id = NEW.triage_id AND t.task_id = NEW.task_id
        AND t.request_id = NEW.request_id
        AND t.feedback_item_id = NEW.feedback_item_id
        AND t.body_digest = NEW.body_digest
        AND t.disposition = NEW.legacy_disposition
  )
BEGIN
    SELECT RAISE(ABORT, 'external feedback disposition requires its exact triage row and legacy mapping');
END;

CREATE TRIGGER task_external_feedback_disposition_update_guard
BEFORE UPDATE ON task_external_feedback_disposition
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.triage_id IS NOT OLD.triage_id OR NEW.request_id IS NOT OLD.request_id
  OR NEW.feedback_item_id IS NOT OLD.feedback_item_id
  OR NEW.body_digest IS NOT OLD.body_digest
  OR NEW.disposition IS NOT OLD.disposition
  OR NEW.legacy_disposition IS NOT OLD.legacy_disposition
  OR NEW.scope_classification IS NOT OLD.scope_classification
  OR NEW.classification_facts_json IS NOT OLD.classification_facts_json
  OR NEW.dismiss_reason IS NOT OLD.dismiss_reason
  OR NEW.proposal_json IS NOT OLD.proposal_json
  OR NEW.created_at IS NOT OLD.created_at
  OR (OLD.human_decision_id IS NOT NULL AND NEW.human_decision_id IS NOT OLD.human_decision_id)
  OR (OLD.followup_task_id IS NOT NULL AND NEW.followup_task_id IS NOT OLD.followup_task_id)
  OR (NEW.human_decision_id IS NOT OLD.human_decision_id AND (
        NEW.disposition NOT IN ('propose_task', 'human')
        OR NOT EXISTS (
            SELECT 1 FROM human_decision_request h
            WHERE h.id = NEW.human_decision_id AND h.task_id = NEW.task_id)))
  OR (NEW.followup_task_id IS NOT OLD.followup_task_id AND (
        NEW.disposition != 'propose_task'
        OR NOT EXISTS (
            SELECT 1 FROM human_decision_request h
            WHERE h.id = NEW.human_decision_id AND h.task_id = NEW.task_id
              AND h.state = 'answered' AND h.answered_option_id = 'approve')))
BEGIN
    SELECT RAISE(ABORT, 'external feedback disposition is immutable except its set-once decision and follow-up links');
END;

CREATE TRIGGER task_external_feedback_disposition_immutable_delete
BEFORE DELETE ON task_external_feedback_disposition
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'external feedback disposition is preserved outside Project teardown');
END;

-- ---------------------------------------------------------------------------
-- Supersession receipt and admission-marker triggers.
-- ---------------------------------------------------------------------------
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
        AND l.state = 'merging' AND l.reason_ref = merge_op.id
        AND e.event_type = 'pr.admission_superseded' AND e.entity_type = 'pr_metadata'
        AND e.entity_id = m.id AND e.scope_type = 'task' AND e.scope_id = NEW.task_id
        AND json_valid(e.payload_json)
        AND json_extract(e.payload_json, '$.task_id') = NEW.task_id
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

CREATE TRIGGER pr_rework_supersession_update_guard
BEFORE UPDATE ON pr_rework_supersession
WHEN NEW.id IS NOT OLD.id OR NEW.task_id IS NOT OLD.task_id
  OR NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
  OR NEW.publish_operation_id IS NOT OLD.publish_operation_id
  OR NEW.metadata_id IS NOT OLD.metadata_id
  OR NEW.request_id IS NOT OLD.request_id
  OR NEW.triage_set_digest IS NOT OLD.triage_set_digest
  OR NEW.provider_pr_id IS NOT OLD.provider_pr_id
  OR NEW.superseded_head_sha IS NOT OLD.superseded_head_sha
  OR NEW.result_event_id IS NOT OLD.result_event_id
  OR NEW.created_at IS NOT OLD.created_at
  OR NEW.version != OLD.version + 1
  OR NOT ((OLD.state = 'open'
           AND NEW.state IN ('readmitted', 'remote_merged', 'remote_closed', 'abandoned'))
          OR (OLD.state = 'remote_merged' AND NEW.state = 'abandoned'))
  OR (NEW.readmission_merge_operation_id IS NOT OLD.readmission_merge_operation_id
      AND NEW.state != 'readmitted')
  OR (NEW.convergence_decision_id IS NOT OLD.convergence_decision_id
      AND (OLD.convergence_decision_id IS NOT NULL
           OR NEW.state NOT IN ('remote_merged', 'abandoned')))
  OR (NEW.state = 'readmitted' AND NOT EXISTS (
        SELECT 1 FROM remote_pr_admission a
        WHERE a.task_merge_operation_id = NEW.readmission_merge_operation_id
          AND a.task_id = NEW.task_id AND a.readmits_supersession_id = NEW.id))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_REWORK_NOT_AUTHORIZED');
END;

CREATE TRIGGER pr_rework_supersession_immutable_delete
BEFORE DELETE ON pr_rework_supersession
WHEN NOT EXISTS (SELECT 1 FROM task t JOIN project_deletion_guard g
                 ON g.project_id = t.project_id WHERE t.id = OLD.task_id)
BEGIN
    SELECT RAISE(ABORT, 'PR rework supersession history is preserved outside Project teardown');
END;

CREATE TRIGGER remote_pr_admission_rework_guard
BEFORE UPDATE OF rework_outcome, supersession_id, readmits_supersession_id
ON remote_pr_admission
WHEN (NEW.rework_outcome IS NOT OLD.rework_outcome
      OR NEW.supersession_id IS NOT OLD.supersession_id
      OR NEW.readmits_supersession_id IS NOT OLD.readmits_supersession_id)
 AND NOT (
      OLD.rework_outcome IS NULL AND OLD.supersession_id IS NULL
  AND NEW.rework_outcome = 'superseded_for_rework'
  AND OLD.state = 'open' AND NEW.state = 'open'
  AND NEW.readmits_supersession_id IS OLD.readmits_supersession_id
  AND EXISTS (
      SELECT 1 FROM pr_rework_supersession s
      WHERE s.id = NEW.supersession_id AND s.task_id = OLD.task_id
        AND s.task_merge_operation_id = OLD.task_merge_operation_id
        AND s.publish_operation_id = OLD.publish_operation_id
        AND s.metadata_id = OLD.metadata_id AND s.state = 'open')
 )
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_REWORK_NOT_AUTHORIZED');
END;

CREATE TRIGGER remote_pr_admission_superseded_immutable
BEFORE UPDATE ON remote_pr_admission
WHEN OLD.rework_outcome IS NOT NULL
 AND (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
   OR NEW.publish_operation_id IS NOT OLD.publish_operation_id
   OR NEW.metadata_id IS NOT OLD.metadata_id
   OR NEW.task_id IS NOT OLD.task_id
   OR NEW.provider_config_id IS NOT OLD.provider_config_id
   OR NEW.provider_type IS NOT OLD.provider_type
   OR NEW.provider_config_revision IS NOT OLD.provider_config_revision
   OR NEW.provider_config_digest IS NOT OLD.provider_config_digest
   OR NEW.provider_base_url IS NOT OLD.provider_base_url
   OR NEW.token_secret_ref IS NOT OLD.token_secret_ref
   OR NEW.remote_repo_identity IS NOT OLD.remote_repo_identity
   OR NEW.source_branch IS NOT OLD.source_branch
   OR NEW.target_branch IS NOT OLD.target_branch
   OR NEW.admitted_source_sha IS NOT OLD.admitted_source_sha
   OR NEW.state IS NOT OLD.state
   OR NEW.reconciliation_reason IS NOT OLD.reconciliation_reason
   OR NEW.provider_event_id IS NOT OLD.provider_event_id
   OR NEW.observed_head_sha IS NOT OLD.observed_head_sha
   OR NEW.merged_commit_sha IS NOT OLD.merged_commit_sha
   OR NEW.result_event_id IS NOT OLD.result_event_id
   OR NEW.created_at IS NOT OLD.created_at
   OR NEW.updated_at IS NOT OLD.updated_at
   OR NEW.provider_status IS NOT OLD.provider_status
   OR NEW.result_classification IS NOT OLD.result_classification
   OR NEW.credential_handle_id IS NOT OLD.credential_handle_id
   OR NEW.credential_owner_user_id IS NOT OLD.credential_owner_user_id
   OR NEW.feedback_policy_json IS NOT OLD.feedback_policy_json
   OR NEW.feedback_policy_digest IS NOT OLD.feedback_policy_digest
   OR NEW.rework_outcome IS NOT OLD.rework_outcome
   OR NEW.supersession_id IS NOT OLD.supersession_id
   OR NEW.readmits_supersession_id IS NOT OLD.readmits_supersession_id)
BEGIN
    SELECT RAISE(ABORT, 'a superseded remote PR admission is immutable');
END;

CREATE TRIGGER task_lifecycle_pr_feedback_rework_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.reason_kind = 'pr_feedback_rework'
 AND NOT (
     (NEW.from_state = 'merging' AND NEW.to_state = 'blocked'
      AND NEW.cause_kind = 'merge_operation'
      AND EXISTS (
          SELECT 1 FROM pr_rework_supersession s
          JOIN task_integration_operation op ON op.id = s.task_merge_operation_id
          WHERE s.id = NEW.reason_ref AND s.task_id = NEW.task_id AND s.state = 'open'
            AND op.id = NEW.cause_ref AND op.task_id = NEW.task_id
            AND op.kind = 'task_merge' AND op.status = 'abandoned'))
  OR (NEW.from_state = 'blocked' AND NEW.to_state = 'active'
      AND NEW.cause_kind = 'domain_event'
      AND EXISTS (
          SELECT 1 FROM pr_rework_supersession s
          JOIN task_lifecycle l ON l.task_id = s.task_id
          JOIN task_coordination_request r ON r.id = s.request_id AND r.task_id = s.task_id
          WHERE s.id = NEW.reason_ref AND s.task_id = NEW.task_id AND s.state = 'open'
            AND s.result_event_id = NEW.cause_ref
            AND l.state = 'blocked' AND l.reason_kind = 'pr_feedback_rework'
            AND l.reason_ref = s.id
            AND r.kind = 'external_feedback' AND r.state = 'triaged'
            AND EXISTS (
                SELECT 1 FROM task_external_feedback_disposition d
                WHERE d.request_id = r.id AND d.task_id = r.task_id
                  AND d.disposition = 'extend_task')))
 )
BEGIN
    SELECT RAISE(ABORT, 'pr_feedback_rework lifecycle receipt lacks its exact supersession authority');
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
          AND metadata.task_id = NEW.task_id
          AND metadata.task_merge_operation_id = admission.task_merge_operation_id
          AND metadata.publish_operation_id = admission.publish_operation_id
          AND event.event_type = 'pr.status_changed'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task'
          AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
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
          AND admission.state = 'open'
          AND admission.rework_outcome = 'superseded_for_rework'
          AND s.task_id = NEW.task_id
          AND s.task_merge_operation_id = NEW.id
          AND s.publish_operation_id = admission.publish_operation_id
          AND s.metadata_id = admission.metadata_id
          AND s.state = 'open'
          AND metadata.task_id = NEW.task_id
          AND metadata.task_merge_operation_id = NEW.id
          AND event.id = s.result_event_id
          AND event.event_type = 'pr.admission_superseded'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task'
          AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
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

DROP TRIGGER pr_metadata_merge_admission_guard_update;
CREATE TRIGGER pr_metadata_merge_admission_guard_update
BEFORE UPDATE OF task_merge_operation_id, publish_operation_id ON pr_metadata
WHEN (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
      OR NEW.publish_operation_id IS NOT OLD.publish_operation_id)
 AND (
    NEW.task_merge_operation_id IS NULL
    OR NEW.publish_operation_id IS NULL
    OR (OLD.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
        -- V175: a paused (superseded) admission has no remote result and no history;
        -- its PR stays open (rework) or was closed remotely (new PR).
        SELECT 1 FROM remote_pr_admission paused_admission
        JOIN pr_rework_supersession paused ON paused.id = paused_admission.supersession_id
        WHERE paused_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND paused_admission.publish_operation_id = OLD.publish_operation_id
          AND paused_admission.metadata_id = OLD.id
          AND paused_admission.task_id = OLD.task_id
          AND paused_admission.rework_outcome = 'superseded_for_rework'
          AND paused.task_id = OLD.task_id
          AND paused.state IN ('open', 'remote_closed')
    ) AND NOT EXISTS (
        SELECT 1 FROM remote_pr_admission old_admission
        JOIN remote_pr_history history
          ON history.task_merge_operation_id = old_admission.task_merge_operation_id
         AND history.task_id = old_admission.task_id
         AND history.original_metadata_id = old_admission.metadata_id
        WHERE old_admission.task_merge_operation_id = OLD.task_merge_operation_id
          AND old_admission.publish_operation_id = OLD.publish_operation_id
          AND old_admission.metadata_id = OLD.id
          AND old_admission.task_id = OLD.task_id
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
        SELECT 1 FROM task_integration_operation merge_op
        JOIN task_integration_operation publish_op
          ON publish_op.parent_operation_id = merge_op.id
         AND publish_op.task_id = merge_op.task_id
        WHERE merge_op.id = NEW.task_merge_operation_id
          AND merge_op.task_id = NEW.task_id
          AND merge_op.kind = 'task_merge'
          AND merge_op.status = 'running'
          AND merge_op.remote_waiting = 1
          AND publish_op.id = NEW.publish_operation_id
          AND publish_op.kind = 'publish_pr'
          AND publish_op.status = 'running'
          AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'PR metadata can be rebound only after exact terminal history is durable');
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
    JOIN repo r ON r.id = t.repo_id
    JOIN pr_provider_config config
      ON config.id = NEW.provider_config_id AND config.repo_id = r.id
    JOIN task_lifecycle lifecycle ON lifecycle.task_id = merge_op.task_id
    WHERE merge_op.id = NEW.task_merge_operation_id
      AND merge_op.task_id = NEW.task_id
      AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
      AND merge_op.remote_waiting = 1
      AND publish_op.kind = 'publish_pr' AND publish_op.status = 'running'
      AND publish_op.gate_evaluation_id = merge_op.gate_evaluation_id
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
      AND lifecycle.state = 'merging' AND lifecycle.reason_ref = merge_op.id
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

DROP TRIGGER external_feedback_item_insert_guard;
CREATE TRIGGER external_feedback_item_insert_guard
BEFORE INSERT ON external_feedback_item
WHEN NOT EXISTS (
    SELECT 1 FROM remote_pr_admission a
    WHERE a.task_merge_operation_id = NEW.admission_id
      AND a.task_id = NEW.task_id
      AND a.state = 'open' AND a.rework_outcome IS NULL
      AND a.feedback_policy_json IS NOT NULL
      AND json_extract(a.feedback_policy_json, '$.ingest') = 1
)
BEGIN
    SELECT RAISE(ABORT, 'external feedback requires the Task''s open admission with ingestion enabled in its frozen policy');
END;

DROP TRIGGER task_coordination_request_external_feedback_source_guard;
CREATE TRIGGER task_coordination_request_external_feedback_source_guard
BEFORE INSERT ON task_coordination_request
WHEN NEW.kind = 'external_feedback' AND NOT EXISTS (
    SELECT 1 FROM remote_pr_admission a
    WHERE a.task_merge_operation_id = NEW.source_subject_id
      AND a.task_id = NEW.task_id
      AND a.state = 'open' AND a.rework_outcome IS NULL
      AND a.feedback_policy_json IS NOT NULL
      AND json_extract(a.feedback_policy_json, '$.ingest') = 1
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE');
END;

DROP TRIGGER task_external_feedback_set_item_guard;
CREATE TRIGGER task_external_feedback_set_item_guard
BEFORE INSERT ON task_external_feedback_set_item
WHEN NOT EXISTS (
    SELECT 1 FROM task_coordination_request r
    JOIN external_feedback_item i ON i.id = NEW.feedback_item_id AND i.task_id = r.task_id
    JOIN remote_pr_admission a ON a.task_merge_operation_id = i.admission_id
     AND a.rework_outcome IS NULL
    WHERE r.id = NEW.request_id AND r.task_id = NEW.task_id
      AND r.kind = 'external_feedback' AND r.state = 'requested'
      AND r.source_subject_kind = 'external_feedback_set'
      AND r.source_subject_id = i.admission_id
      AND i.trust_class = 'trusted' AND i.head_state = 'current'
      AND i.body_digest = NEW.body_digest
)
BEGIN
    SELECT RAISE(ABORT, 'FORGE_COORDINATION_EXTERNAL_FEEDBACK_STALE');
END;

DROP TRIGGER pr_external_action_insert_guard;
CREATE TRIGGER pr_external_action_insert_guard
BEFORE INSERT ON pr_external_action
WHEN NEW.version != 1 OR NEW.finished_at IS NOT NULL
  OR NEW.proposal_id IS NOT NULL OR NEW.decision_id IS NOT NULL
  OR NEW.state NOT IN ('proposed', 'approved')
  OR (NEW.state = 'approved' AND NEW.authorization_mode != 'automatic')
  OR NOT EXISTS (
      SELECT 1 FROM remote_pr_admission a
      WHERE a.task_merge_operation_id = NEW.admission_id
        AND a.task_id = NEW.task_id
        AND (
            (NEW.kind = 'push_update' AND a.rework_outcome IS NULL
             AND (a.state = 'open'
                  OR (a.state = 'admitted' AND a.readmits_supersession_id IS NOT NULL
                      AND EXISTS (SELECT 1 FROM pr_rework_supersession s
                                  WHERE s.id = a.readmits_supersession_id
                                    AND s.task_id = a.task_id AND s.state = 'open'))))
            OR (NEW.kind = 'post_comment' AND a.state = 'open'))
        AND a.feedback_policy_json IS NOT NULL
        AND (NEW.authorization_mode != 'automatic'
             OR json_extract(a.feedback_policy_json,
                  CASE NEW.kind WHEN 'push_update' THEN '$.push_corrections'
                                ELSE '$.post_summary' END) = 'automatic')
        AND (NEW.kind != 'post_comment'
             OR json_extract(a.feedback_policy_json, '$.post_summary') != 'off'))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

DROP TRIGGER pr_external_action_scope_guard;
CREATE TRIGGER pr_external_action_scope_guard
BEFORE INSERT ON pr_external_action
WHEN (NEW.kind = 'push_update' AND EXISTS (
        SELECT 1 FROM task_integration_operation op
        WHERE op.task_id = NEW.task_id AND op.status = 'running'
          AND op.id != NEW.admission_id
          -- V175: the re-admission's own PublishPr is the admission itself
          AND NOT (op.kind = 'publish_pr' AND op.parent_operation_id = NEW.admission_id
                   AND EXISTS (SELECT 1 FROM remote_pr_admission readmitted
                               WHERE readmitted.task_merge_operation_id = NEW.admission_id
                                 AND readmitted.readmits_supersession_id IS NOT NULL))))
  OR (NEW.kind = 'push_update' AND NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate_evaluation_currentness currentness ON currentness.evaluation_id = e.id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied' AND currentness.inputs_current = 1))
  OR (NEW.kind = 'post_comment' AND NOT EXISTS (
        SELECT 1 FROM artifact a
        WHERE a.id = NEW.summary_artifact_id AND a.task_id = NEW.task_id))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;

DROP TRIGGER pr_external_action_start_guard;
CREATE TRIGGER pr_external_action_start_guard
BEFORE UPDATE OF state ON pr_external_action
WHEN NEW.state = 'running' AND OLD.state != 'running'
 AND (NOT EXISTS (
        SELECT 1 FROM remote_pr_admission a
        WHERE a.task_merge_operation_id = NEW.admission_id
          AND a.task_id = NEW.task_id
          AND (
              (NEW.kind = 'push_update' AND a.rework_outcome IS NULL
               AND (a.state = 'open'
                    OR (a.state = 'admitted' AND a.readmits_supersession_id IS NOT NULL
                        AND EXISTS (SELECT 1 FROM pr_rework_supersession s
                                    WHERE s.id = a.readmits_supersession_id
                                      AND s.task_id = a.task_id AND s.state = 'open'))))
              OR (NEW.kind = 'post_comment' AND a.state = 'open')))
   OR (NEW.kind = 'push_update' AND EXISTS (
        SELECT 1 FROM task_integration_operation op
        WHERE op.task_id = NEW.task_id AND op.status = 'running'
          AND op.id != NEW.admission_id
          AND NOT (op.kind = 'publish_pr' AND op.parent_operation_id = NEW.admission_id
                   AND EXISTS (SELECT 1 FROM remote_pr_admission readmitted
                               WHERE readmitted.task_merge_operation_id = NEW.admission_id
                                 AND readmitted.readmits_supersession_id IS NOT NULL))))
   OR (NEW.kind = 'push_update' AND NOT EXISTS (
        SELECT 1 FROM gate_evaluation e
        JOIN gate_evaluation_currentness currentness ON currentness.evaluation_id = e.id
        WHERE e.id = NEW.gate_evaluation_id AND e.task_id = NEW.task_id
          AND e.outcome = 'satisfied' AND currentness.inputs_current = 1)))
BEGIN
    SELECT RAISE(ABORT, 'FORGE_PR_ACTION_NOT_AUTHORIZED');
END;
