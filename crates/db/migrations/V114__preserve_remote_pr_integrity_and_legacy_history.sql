-- Keep provider outcome separate from Forge's integrity classification. Older
-- rows encode a wrong merged head as state=head_mismatch and event status
-- head_mismatch; that state was only produced after a provider Merged result.
ALTER TABLE remote_pr_admission
    ADD COLUMN provider_status TEXT
        CHECK (provider_status IS NULL OR provider_status IN (
            'open', 'merged', 'closed', 'publication_failed'
        ));

ALTER TABLE remote_pr_admission
    ADD COLUMN result_classification TEXT
        CHECK (result_classification IS NULL OR (
            result_classification = 'head_mismatch'
            AND provider_status = 'merged'
        ));

UPDATE remote_pr_admission
SET provider_status = CASE state
        WHEN 'open' THEN 'open'
        WHEN 'merged' THEN 'merged'
        WHEN 'closed' THEN 'closed'
        WHEN 'head_mismatch' THEN 'merged'
        WHEN 'publication_failed' THEN 'publication_failed'
        ELSE (
            SELECT CASE json_extract(event.payload_json, '$.status')
                WHEN 'open' THEN 'open'
                WHEN 'merged' THEN 'merged'
                WHEN 'closed' THEN 'closed'
                WHEN 'publication_failed' THEN 'publication_failed'
                WHEN 'head_mismatch' THEN 'merged'
            END
            FROM domain_event event
            WHERE event.id = remote_pr_admission.result_event_id
              AND json_valid(event.payload_json)
        )
    END,
    result_classification = CASE
        WHEN state = 'head_mismatch' THEN 'head_mismatch'
        ELSE NULL
    END;

-- V109/V113 can leave a recognized legacy failed provider outcome displayed
-- as pending. Restore only the standard failed state; custom values remain
-- unchanged and fail closed.
UPDATE pr_metadata
SET merge_status = 'publication_failed'
WHERE admission_status = 'legacy_unadmitted'
  AND merge_status = 'pending'
  AND lower(pr_state) = 'failed';

CREATE TABLE legacy_pr_history (
    history_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    legacy_metadata_id TEXT NOT NULL UNIQUE,
    provider_type TEXT NOT NULL,
    provider_pr_id TEXT,
    pr_url TEXT,
    source_branch TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    pr_state TEXT NOT NULL,
    merge_status TEXT NOT NULL,
    admission_status TEXT NOT NULL CHECK (admission_status = 'legacy_unadmitted'),
    task_merge_operation_id TEXT,
    publish_operation_id TEXT,
    last_synced_at TEXT,
    metadata_created_at TEXT NOT NULL,
    metadata_updated_at TEXT NOT NULL,
    archived_at TEXT NOT NULL
);

CREATE INDEX idx_legacy_pr_history_task
    ON legacy_pr_history(task_id, archived_at, legacy_metadata_id);

CREATE TRIGGER legacy_pr_history_insert_guard
BEFORE INSERT ON legacy_pr_history
WHEN NOT EXISTS (
    SELECT 1 FROM pr_metadata metadata
    WHERE metadata.id = NEW.legacy_metadata_id
      AND metadata.task_id = NEW.task_id
      AND metadata.provider_type = NEW.provider_type
      AND metadata.provider_pr_id IS NEW.provider_pr_id
      AND metadata.pr_url IS NEW.pr_url
      AND metadata.source_branch = NEW.source_branch
      AND metadata.target_branch = NEW.target_branch
      AND metadata.pr_state = NEW.pr_state
      AND metadata.merge_status = NEW.merge_status
      AND metadata.admission_status = 'legacy_unadmitted'
      AND metadata.task_merge_operation_id IS NEW.task_merge_operation_id
      AND metadata.publish_operation_id IS NEW.publish_operation_id
      AND metadata.last_synced_at IS NEW.last_synced_at
      AND metadata.created_at = NEW.metadata_created_at
      AND metadata.updated_at = NEW.metadata_updated_at
      AND metadata.merge_status IN (
          'merged', 'closed_without_merge', 'publication_failed'
      )
      AND NOT EXISTS (
          SELECT 1 FROM remote_pr_admission admission
          WHERE admission.metadata_id = metadata.id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'legacy PR history requires an exact terminal unadmitted PR snapshot');
END;

CREATE TRIGGER legacy_pr_history_immutable_update
BEFORE UPDATE ON legacy_pr_history
BEGIN
    SELECT RAISE(ABORT, 'legacy PR history is immutable');
END;

CREATE TRIGGER legacy_pr_history_delete_guard
BEFORE DELETE ON legacy_pr_history
WHEN NOT EXISTS (
    SELECT 1 FROM task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'legacy PR history is removed only during guarded Project teardown');
END;

DROP TRIGGER remote_pr_admission_terminal_result_immutable;
CREATE TRIGGER remote_pr_admission_terminal_result_immutable
BEFORE UPDATE OF state, provider_status, result_classification,
                 reconciliation_reason, provider_event_id, observed_head_sha,
                 merged_commit_sha, result_event_id
ON remote_pr_admission
WHEN OLD.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
 AND (NEW.state IS NOT OLD.state
   OR NEW.provider_status IS NOT OLD.provider_status
   OR NEW.result_classification IS NOT OLD.result_classification
   OR NEW.reconciliation_reason IS NOT OLD.reconciliation_reason
   OR NEW.provider_event_id IS NOT OLD.provider_event_id
   OR NEW.observed_head_sha IS NOT OLD.observed_head_sha
   OR NEW.merged_commit_sha IS NOT OLD.merged_commit_sha
   OR NEW.result_event_id IS NOT OLD.result_event_id)
BEGIN
    SELECT RAISE(ABORT, 'terminal remote PR result is immutable');
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
    OR NOT EXISTS (
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
    )
 )
BEGIN
    SELECT RAISE(ABORT, 'remote TaskMerge terminal result requires exact provider outcome authority');
END;
