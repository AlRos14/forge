-- Admission provenance is independent from the historical/provider PR
-- outcome. V109 overwrote this outcome for rows lacking PR9 operation IDs;
-- restore terminal facts from the preserved provider state and leave active
-- rows explicitly legacy-unadmitted.
ALTER TABLE pr_metadata
    ADD COLUMN admission_status TEXT NOT NULL DEFAULT 'legacy_unadmitted'
        CHECK (admission_status IN (
            'admitted', 'reconciliation_required', 'open', 'merged', 'closed',
            'failed', 'legacy_unadmitted'
        ));

WITH exact_legacy_result AS (
    SELECT metadata.id AS metadata_id,
           CASE json_extract(event.payload_json, '$.status')
               WHEN 'merged' THEN 'merged'
               WHEN 'closed' THEN 'closed_without_merge'
               WHEN 'publication_failed' THEN 'publication_failed'
           END AS merge_status,
           CASE json_extract(event.payload_json, '$.status')
               WHEN 'merged' THEN 'merged'
               WHEN 'closed' THEN 'closed'
               WHEN 'publication_failed' THEN 'failed'
           END AS pr_state
    FROM pr_metadata metadata
    JOIN task_integration_operation merge_op
      ON merge_op.id = metadata.task_merge_operation_id
     AND merge_op.task_id = metadata.task_id
     AND merge_op.kind = 'task_merge'
     AND merge_op.remote_waiting = 1
     AND merge_op.result_event_id IS NOT NULL
    JOIN task_integration_operation publish_op
      ON publish_op.id = metadata.publish_operation_id
     AND publish_op.task_id = metadata.task_id
     AND publish_op.kind = 'publish_pr'
     AND publish_op.parent_operation_id = merge_op.id
     AND publish_op.gate_evaluation_id IS merge_op.gate_evaluation_id
    JOIN domain_event event
      ON event.id = merge_op.result_event_id
     AND event.event_type = 'pr.status_changed'
     AND event.entity_type = 'pr_metadata'
     AND event.entity_id = metadata.id
     AND event.scope_type = 'task'
     AND event.scope_id = metadata.task_id
     AND json_valid(event.payload_json)
     AND json_extract(event.payload_json, '$.task_id') = metadata.task_id
     AND json_extract(event.payload_json, '$.pr_metadata_id') = metadata.id
     AND json_extract(event.payload_json, '$.task_merge_operation_id') = merge_op.id
     AND json_extract(event.payload_json, '$.publish_operation_id') = publish_op.id
    WHERE (merge_op.status = 'succeeded'
           AND publish_op.status = 'succeeded'
           AND json_extract(event.payload_json, '$.status') = 'merged')
       OR (merge_op.status = 'failed'
           AND publish_op.status = 'succeeded'
           AND json_extract(event.payload_json, '$.status') = 'closed')
       OR (merge_op.status = 'failed'
           AND publish_op.status = 'failed'
           AND json_extract(event.payload_json, '$.status') = 'publication_failed')
)
UPDATE pr_metadata
SET merge_status = COALESCE(
        (SELECT result.merge_status FROM exact_legacy_result result
         WHERE result.metadata_id = pr_metadata.id),
        CASE WHEN merge_status = 'legacy_unadmitted' THEN
            CASE
                WHEN lower(pr_state) = 'merged' THEN 'merged'
                WHEN lower(pr_state) = 'closed' THEN 'closed_without_merge'
                ELSE 'pending'
            END
        ELSE merge_status END
    ),
    pr_state = COALESCE(
        (SELECT result.pr_state FROM exact_legacy_result result
         WHERE result.metadata_id = pr_metadata.id),
        pr_state
    ),
    admission_status = 'legacy_unadmitted'
WHERE NOT EXISTS (
    SELECT 1 FROM remote_pr_admission admission
    WHERE admission.metadata_id = pr_metadata.id
);

CREATE TRIGGER pr_metadata_admission_status_guard
BEFORE UPDATE OF admission_status ON pr_metadata
WHEN NEW.admission_status IS NOT OLD.admission_status
 AND NEW.admission_status != 'legacy_unadmitted'
 AND NOT EXISTS (
     SELECT 1 FROM remote_pr_admission admission
     WHERE admission.metadata_id = NEW.id
       AND admission.task_id = NEW.task_id
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
    SELECT RAISE(ABORT, 'PR admission provenance must match its exact remote admission');
END;
