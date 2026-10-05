-- A Task keeps one mutable pr_metadata projection, but every modern remote
-- admission is an immutable historical identity. Pin terminal PR metadata to
-- its exact admission and result event before a later admission reuses that
-- projection. Existing terminal admissions can be reconstructed only from
-- their frozen admission plus exact durable provider result event.
CREATE TABLE remote_pr_history (
    history_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    original_metadata_id TEXT NOT NULL,
    history_origin TEXT NOT NULL CHECK (history_origin IN (
        'v116_backfill', 'terminal_snapshot'
    )),
    task_merge_operation_id TEXT NOT NULL UNIQUE
        REFERENCES remote_pr_admission(task_merge_operation_id) ON DELETE RESTRICT,
    publish_operation_id TEXT NOT NULL
        REFERENCES task_integration_operation(id) ON DELETE RESTRICT,
    provider_config_id TEXT NOT NULL,
    provider_type TEXT NOT NULL,
    provider_config_revision TEXT NOT NULL,
    provider_config_digest TEXT NOT NULL,
    provider_base_url TEXT,
    token_secret_ref TEXT,
    provider_pr_id TEXT,
    pr_url TEXT,
    remote_repo_identity TEXT NOT NULL,
    source_branch TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    admitted_source_sha TEXT NOT NULL,
    admission_state TEXT NOT NULL CHECK (admission_state IN (
        'merged', 'closed', 'head_mismatch', 'publication_failed'
    )),
    provider_status TEXT NOT NULL CHECK (provider_status IN (
        'merged', 'closed', 'publication_failed'
    )),
    result_classification TEXT CHECK (
        result_classification IS NULL
        OR result_classification = 'head_mismatch'
    ),
    observed_head_sha TEXT,
    merged_commit_sha TEXT,
    pr_state TEXT NOT NULL CHECK (pr_state IN (
        'merged', 'closed', 'head_mismatch', 'failed'
    )),
    merge_status TEXT NOT NULL CHECK (merge_status IN (
        'merged', 'closed_without_merge', 'head_mismatch', 'publication_failed'
    )),
    admission_status TEXT NOT NULL CHECK (admission_status IN (
        'merged', 'closed', 'failed'
    )),
    provider_event_id TEXT,
    result_event_id TEXT NOT NULL REFERENCES domain_event(id) ON DELETE RESTRICT,
    admission_created_at TEXT NOT NULL,
    result_created_at TEXT NOT NULL,
    archived_at TEXT NOT NULL,
    CHECK ((admission_state = 'head_mismatch'
            AND result_classification = 'head_mismatch')
        OR (admission_state != 'head_mismatch'
            AND result_classification IS NULL)),
    CHECK ((provider_status = 'merged'
            AND admission_state IN ('merged', 'head_mismatch'))
        OR (provider_status = 'closed' AND admission_state = 'closed')
        OR (provider_status = 'publication_failed'
            AND admission_state = 'publication_failed'))
);

CREATE INDEX idx_remote_pr_history_task
    ON remote_pr_history(task_id, admission_created_at, task_merge_operation_id);

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

CREATE TRIGGER remote_pr_history_immutable_update
BEFORE UPDATE ON remote_pr_history
BEGIN
    SELECT RAISE(ABORT, 'terminal remote PR history is immutable');
END;

CREATE TRIGGER remote_pr_history_delete_guard
BEFORE DELETE ON remote_pr_history
WHEN NOT EXISTS (
    SELECT 1 FROM task
    JOIN project_deletion_guard guard ON guard.project_id = task.project_id
    WHERE task.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'remote PR history is removed only during guarded Project teardown');
END;

INSERT INTO remote_pr_history (
    history_id, task_id, original_metadata_id, history_origin,
    task_merge_operation_id, publish_operation_id,
    provider_config_id, provider_type, provider_config_revision,
    provider_config_digest, provider_base_url, token_secret_ref,
    provider_pr_id, pr_url, remote_repo_identity, source_branch,
    target_branch, admitted_source_sha, admission_state, provider_status,
    result_classification, observed_head_sha, merged_commit_sha,
    pr_state, merge_status, admission_status, provider_event_id,
    result_event_id, admission_created_at, result_created_at, archived_at
)
SELECT lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) ||
       '-4' || substr(lower(hex(randomblob(2))), 2, 3) || '-' ||
       substr('89ab', (abs(random()) % 4) + 1, 1) ||
       substr(lower(hex(randomblob(2))), 2, 3) || '-' || lower(hex(randomblob(6))),
       admission.task_id, admission.metadata_id, 'v116_backfill',
       admission.task_merge_operation_id, admission.publish_operation_id,
       admission.provider_config_id, admission.provider_type,
       admission.provider_config_revision, admission.provider_config_digest,
       admission.provider_base_url, admission.token_secret_ref,
       json_extract(event.payload_json, '$.provider_pr_id'),
       json_extract(event.payload_json, '$.pr_url'),
       admission.remote_repo_identity, admission.source_branch,
       admission.target_branch, admission.admitted_source_sha,
       admission.state, admission.provider_status, admission.result_classification,
       admission.observed_head_sha, admission.merged_commit_sha,
       CASE json_extract(event.payload_json, '$.status')
           WHEN 'publication_failed' THEN 'failed'
           ELSE json_extract(event.payload_json, '$.status')
       END,
       CASE admission.provider_status
           WHEN 'merged' THEN 'merged'
           WHEN 'closed' THEN 'closed_without_merge'
           WHEN 'publication_failed' THEN 'publication_failed'
       END,
       CASE admission.state
           WHEN 'merged' THEN 'merged'
           WHEN 'closed' THEN 'closed'
           ELSE 'failed'
       END,
       admission.provider_event_id, event.id, admission.created_at,
       event.created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM remote_pr_admission admission
JOIN task_integration_operation merge_op
  ON merge_op.id = admission.task_merge_operation_id
 AND merge_op.task_id = admission.task_id
JOIN task_integration_operation publish_op
  ON publish_op.id = admission.publish_operation_id
 AND publish_op.task_id = admission.task_id
 AND publish_op.parent_operation_id = merge_op.id
JOIN domain_event event ON event.id = admission.result_event_id
WHERE admission.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
  AND merge_op.result_event_id = event.id
  AND (
      (admission.state = 'merged'
       AND merge_op.status = 'succeeded' AND publish_op.status = 'succeeded')
      OR (admission.state IN ('closed', 'head_mismatch')
          AND merge_op.status = 'failed' AND publish_op.status = 'succeeded')
      OR (admission.state = 'publication_failed'
          AND merge_op.status = 'failed' AND publish_op.status = 'failed'
          AND publish_op.result_event_id = event.id)
  )
  AND NOT EXISTS (
      SELECT 1 FROM remote_pr_history existing
      WHERE existing.task_merge_operation_id = admission.task_merge_operation_id
  );

DROP TRIGGER pr_metadata_merge_admission_guard_update;
CREATE TRIGGER pr_metadata_merge_admission_guard_update
BEFORE UPDATE OF task_merge_operation_id, publish_operation_id ON pr_metadata
WHEN (NEW.task_merge_operation_id IS NOT OLD.task_merge_operation_id
      OR NEW.publish_operation_id IS NOT OLD.publish_operation_id)
 AND (
    NEW.task_merge_operation_id IS NULL
    OR NEW.publish_operation_id IS NULL
    OR (OLD.task_merge_operation_id IS NOT NULL AND NOT EXISTS (
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
