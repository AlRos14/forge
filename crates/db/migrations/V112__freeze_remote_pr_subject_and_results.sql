-- Freeze the remote PR identity and admitted merge subject beside the exact
-- TaskMerge/PublishPr admission. Provider observations then advance this one
-- durable record and the existing operation/lifecycle authority atomically.
CREATE TABLE remote_pr_admission (
    task_merge_operation_id TEXT PRIMARY KEY
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    publish_operation_id TEXT NOT NULL UNIQUE
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    metadata_id TEXT NOT NULL
        REFERENCES pr_metadata(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    provider_config_id TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    provider_config_revision TEXT NOT NULL,
    provider_config_digest TEXT NOT NULL,
    provider_base_url TEXT,
    token_secret_ref TEXT,
    remote_repo_identity TEXT NOT NULL,
    source_branch TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    admitted_source_sha TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'admitted' CHECK (state IN (
        'admitted', 'reconciliation_required', 'open', 'merged', 'closed',
        'head_mismatch', 'publication_failed'
    )),
    reconciliation_reason TEXT,
    provider_event_id TEXT,
    observed_head_sha TEXT,
    merged_commit_sha TEXT,
    result_event_id TEXT REFERENCES domain_event(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (length(trim(provider_config_digest)) > 0),
    CHECK (length(trim(remote_repo_identity)) > 0),
    CHECK (length(trim(admitted_source_sha)) > 0)
);

CREATE INDEX idx_remote_pr_admission_reconcile
    ON remote_pr_admission(state, updated_at, task_merge_operation_id);

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
      AND lifecycle.state = 'merging' AND lifecycle.reason_ref = merge_op.id
)
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission is missing its exact TaskMerge, PublishPr, or metadata authority');
END;

CREATE TRIGGER remote_pr_admission_identity_immutable
BEFORE UPDATE ON remote_pr_admission
WHEN NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
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
  OR NEW.created_at IS NOT OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission subject and provider identity are immutable');
END;

CREATE TRIGGER remote_pr_admission_terminal_result_immutable
BEFORE UPDATE OF state, reconciliation_reason, provider_event_id,
                 observed_head_sha, merged_commit_sha, result_event_id
ON remote_pr_admission
WHEN OLD.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
 AND (NEW.state IS NOT OLD.state
   OR NEW.reconciliation_reason IS NOT OLD.reconciliation_reason
   OR NEW.provider_event_id IS NOT OLD.provider_event_id
   OR NEW.observed_head_sha IS NOT OLD.observed_head_sha
   OR NEW.merged_commit_sha IS NOT OLD.merged_commit_sha
   OR NEW.result_event_id IS NOT OLD.result_event_id)
BEGIN
    SELECT RAISE(ABORT, 'terminal remote PR result is immutable');
END;

CREATE TRIGGER remote_pr_admission_delete_guard
BEFORE DELETE ON remote_pr_admission
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'remote PR admission history is removed only during guarded Project teardown');
END;

DROP TRIGGER task_integration_operation_provider_result_guard;
CREATE TRIGGER task_integration_operation_provider_result_guard
BEFORE UPDATE OF status, result_event_id ON task_integration_operation
WHEN (NEW.result_event_id IS NOT OLD.result_event_id
      OR (NEW.status IS NOT OLD.status AND (
          (NEW.kind = 'task_merge' AND NEW.remote_waiting = 1
           AND NEW.status IN ('succeeded', 'failed'))
          OR (NEW.kind = 'publish_pr' AND NEW.status = 'failed')
      )))
 AND (
    OLD.result_event_id IS NOT NULL
    OR NEW.status NOT IN ('succeeded', 'failed')
    OR NOT EXISTS (
        SELECT 1 FROM domain_event event
        JOIN remote_pr_admission admission
          ON admission.task_id = NEW.task_id
        JOIN pr_metadata metadata ON metadata.id = admission.metadata_id
        WHERE event.id = NEW.result_event_id
          AND admission.task_merge_operation_id =
              CASE WHEN NEW.kind = 'task_merge' THEN NEW.id ELSE NEW.parent_operation_id END
          AND admission.publish_operation_id =
              CASE WHEN NEW.kind = 'publish_pr' THEN NEW.id ELSE metadata.publish_operation_id END
          AND event.event_type = 'pr.status_changed'
          AND event.entity_type = 'pr_metadata'
          AND event.entity_id = metadata.id
          AND event.scope_type = 'task' AND event.scope_id = NEW.task_id
          AND json_valid(event.payload_json)
          AND json_extract(event.payload_json, '$.task_id') = NEW.task_id
          AND json_extract(event.payload_json, '$.pr_metadata_id') = metadata.id
          AND json_extract(event.payload_json, '$.task_merge_operation_id') = admission.task_merge_operation_id
          AND json_extract(event.payload_json, '$.publish_operation_id') = admission.publish_operation_id
          AND (
              (NEW.kind = 'task_merge' AND NEW.status = 'succeeded'
               AND admission.state = 'merged'
               AND json_extract(event.payload_json, '$.status') = 'merged'
               AND json_extract(event.payload_json, '$.head_sha') = admission.admitted_source_sha)
              OR (NEW.kind = 'task_merge' AND NEW.status = 'failed'
                  AND admission.state IN ('closed', 'head_mismatch', 'publication_failed')
                  AND json_extract(event.payload_json, '$.status') IN ('closed', 'head_mismatch', 'publication_failed'))
              OR (NEW.kind = 'publish_pr' AND NEW.status = 'failed'
                  AND admission.state = 'publication_failed'
                  AND json_extract(event.payload_json, '$.status') = 'publication_failed')
          )
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'TaskMerge result lacks exact frozen remote PR provenance');
END;
