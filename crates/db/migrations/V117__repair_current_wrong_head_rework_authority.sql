-- V115 conservatively left some exact wrong-head reworks unrepaired when an
-- unrelated Human Decision or retry event followed the provider result. Those
-- events do not supersede lifecycle authority unless they produced a later
-- task_lifecycle_transition. Recheck the exact causal chain and repair only
-- while its Blocked -> Active transition is still the current lifecycle fact.
CREATE TEMP TABLE v117_remote_pr_integrity_repair AS
SELECT task.id AS task_id,
       merge.id AS task_merge_operation_id,
       admission.publish_operation_id,
       admission.metadata_id,
       admission.provider_config_id,
       admission.provider_config_digest,
       admission.remote_repo_identity,
       admission.admitted_source_sha,
       result.id AS provider_result_event_id,
       receipt.id AS retry_receipt_id,
       request.id AS wrong_rework_event_id,
       prior_transition.id AS prior_lifecycle_transition_id,
       prior_transition.domain_event_id AS prior_transition_event_id,
       lifecycle.version AS from_lifecycle_version,
       task.version AS expected_task_version,
       task.updated_at AS prior_task_updated_at,
       receipt.policy_version,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now') AS repaired_at,
       lower(hex(randomblob(16))) AS repair_event_seed,
       lower(hex(randomblob(16))) AS transition_event_seed,
       lower(hex(randomblob(16))) AS transition_seed
FROM task
JOIN task_lifecycle lifecycle ON lifecycle.task_id = task.id
JOIN remote_pr_admission admission ON admission.task_id = task.id
JOIN task_integration_operation merge
  ON merge.id = admission.task_merge_operation_id
 AND merge.task_id = admission.task_id
JOIN task_integration_operation publish
  ON publish.id = admission.publish_operation_id
 AND publish.task_id = admission.task_id
 AND publish.parent_operation_id = merge.id
JOIN domain_event result ON result.id = admission.result_event_id
JOIN task_lifecycle_transition terminal_transition
  ON terminal_transition.task_id = task.id
 AND terminal_transition.cause_kind = 'merge_operation'
 AND terminal_transition.cause_ref = merge.id
 AND terminal_transition.domain_event_id = (
     SELECT event.id FROM domain_event event
     WHERE event.event_type = 'task.lifecycle_changed'
       AND event.entity_type = 'task' AND event.entity_id = task.id
       AND event.scope_type = 'task' AND event.scope_id = task.id
       AND event.causation_id = merge.id
       AND json_valid(event.payload_json)
       AND json_extract(event.payload_json, '$.cause_kind') = 'merge_operation'
       AND json_extract(event.payload_json, '$.cause_ref') = merge.id
       AND json_extract(event.payload_json, '$.task_merge_status') = 'failed'
       AND (
           (json_extract(event.payload_json, '$.provider_status') = 'head_mismatch'
            AND json_extract(event.payload_json, '$.result_classification') IS NULL)
           OR (json_extract(event.payload_json, '$.provider_status') = 'merged'
               AND json_extract(event.payload_json, '$.result_classification') = 'head_mismatch')
       )
       AND json_extract(event.payload_json, '$.provider_result_event_id') = result.id
     ORDER BY event.sequence DESC LIMIT 1
 )
JOIN task_failure_retry_receipt receipt
  ON receipt.task_id = task.id
 AND receipt.failure_kind = 'task_merge_failed'
 AND receipt.failure_ref = merge.id
 AND receipt.source_event_id = terminal_transition.domain_event_id
 AND receipt.disposition = 'rework'
JOIN domain_event source ON source.id = receipt.source_event_id
JOIN domain_event request ON request.id = receipt.receipt_event_id
JOIN task_lifecycle_transition prior_transition
  ON prior_transition.task_id = task.id
 AND prior_transition.from_state = 'blocked'
 AND prior_transition.to_state = 'active'
 AND prior_transition.cause_kind = 'domain_event'
 AND prior_transition.cause_ref = request.id
 AND prior_transition.reason_kind = 'merge_failure_rework'
 AND prior_transition.reason_ref = merge.id
JOIN domain_event prior_transition_event
  ON prior_transition_event.id = prior_transition.domain_event_id
WHERE task.deleted_at IS NULL
  AND task.status = 'in_progress'
  AND lifecycle.state = 'active'
  AND lifecycle.version = prior_transition.to_version
  AND lifecycle.reason_kind = prior_transition.reason_kind
  AND lifecycle.reason_ref = prior_transition.reason_ref
  AND admission.state = 'head_mismatch'
  AND admission.provider_status = 'merged'
  AND admission.result_classification = 'head_mismatch'
  AND admission.result_event_id = merge.result_event_id
  AND merge.kind = 'task_merge'
  AND merge.remote_waiting = 1
  AND merge.status = 'failed'
  AND publish.kind = 'publish_pr'
  AND publish.status = 'succeeded'
  AND result.event_type = 'pr.status_changed'
  AND result.entity_type = 'pr_metadata'
  AND result.entity_id = admission.metadata_id
  AND result.scope_type = 'task'
  AND result.scope_id = task.id
  AND result.causation_id = admission.publish_operation_id
  AND json_valid(result.payload_json)
  AND json_extract(result.payload_json, '$.task_id') = task.id
  AND json_extract(result.payload_json, '$.pr_metadata_id') = admission.metadata_id
  AND json_extract(result.payload_json, '$.task_merge_operation_id') = merge.id
  AND json_extract(result.payload_json, '$.publish_operation_id') = publish.id
  AND json_extract(result.payload_json, '$.provider_type') = admission.provider_type
  AND json_extract(result.payload_json, '$.provider_config_id') = admission.provider_config_id
  AND json_extract(result.payload_json, '$.provider_config_revision') = admission.provider_config_revision
  AND json_extract(result.payload_json, '$.provider_config_digest') = admission.provider_config_digest
  AND json_extract(result.payload_json, '$.remote_repo_identity') = admission.remote_repo_identity
  AND json_extract(result.payload_json, '$.source_branch') = admission.source_branch
  AND json_extract(result.payload_json, '$.target_branch') = admission.target_branch
  AND json_extract(result.payload_json, '$.admitted_source_sha') = admission.admitted_source_sha
  AND json_extract(result.payload_json, '$.merged_commit_sha') IS admission.merged_commit_sha
  AND (
      (json_extract(result.payload_json, '$.status') = 'head_mismatch'
       AND json_extract(result.payload_json, '$.result_classification') IS NULL)
      OR (json_extract(result.payload_json, '$.status') = 'merged'
          AND json_extract(result.payload_json, '$.result_classification') = 'head_mismatch')
  )
  AND json_extract(result.payload_json, '$.head_sha') IS NOT admission.admitted_source_sha
  AND json_extract(result.payload_json, '$.provider_event_id') IS admission.provider_event_id
  AND terminal_transition.from_state = 'merging'
  AND terminal_transition.to_state = 'blocked'
  AND (
      (terminal_transition.reason_kind = 'task_merge_failed'
       AND terminal_transition.reason_ref = merge.id)
      OR (terminal_transition.reason_kind = 'remote_pr_head_mismatch'
          AND terminal_transition.reason_ref = result.id)
  )
  AND source.event_type = 'task.lifecycle_changed'
  AND source.entity_type = 'task'
  AND source.entity_id = task.id
  AND source.scope_type = 'task'
  AND source.scope_id = task.id
  AND json_valid(source.payload_json)
  AND json_extract(source.payload_json, '$.from_state') = 'merging'
  AND json_extract(source.payload_json, '$.to_state') = 'blocked'
  AND json_extract(source.payload_json, '$.cause_kind') = 'merge_operation'
  AND json_extract(source.payload_json, '$.cause_ref') = merge.id
  AND json_extract(source.payload_json, '$.task_merge_status') = 'failed'
  AND json_extract(source.payload_json, '$.provider_result_event_id') = result.id
  AND (
      (json_extract(source.payload_json, '$.provider_status') = 'head_mismatch'
       AND json_extract(source.payload_json, '$.result_classification') IS NULL)
      OR (json_extract(source.payload_json, '$.provider_status') = 'merged'
          AND json_extract(source.payload_json, '$.result_classification') = 'head_mismatch')
  )
  AND request.event_type = 'task.rework_requested'
  AND request.entity_type = 'task'
  AND request.entity_id = task.id
  AND request.scope_type = 'task'
  AND request.scope_id = task.id
  AND request.causation_id = source.id
  AND json_valid(request.payload_json)
  AND json_extract(request.payload_json, '$.task_id') = task.id
  AND json_extract(request.payload_json, '$.failure_kind') = receipt.failure_kind
  AND json_extract(request.payload_json, '$.failure_ref') = receipt.failure_ref
  AND json_extract(request.payload_json, '$.source_event_id') = source.id
  AND json_extract(request.payload_json, '$.attempt_number') = receipt.attempt_number
  AND json_extract(request.payload_json, '$.retry_budget') = receipt.retry_budget
  AND (
      json_extract(request.payload_json, '$.retry_epoch') = receipt.retry_epoch
      OR (receipt.policy_version IN (1, 2) AND receipt.retry_epoch = 0
          AND json_type(request.payload_json, '$.retry_epoch') IS NULL)
  )
  AND json_extract(request.payload_json, '$.disposition') = receipt.disposition
  AND json_extract(request.payload_json, '$.policy_ref') = receipt.policy_ref
  AND json_extract(request.payload_json, '$.policy_version') = receipt.policy_version
  AND json_extract(request.payload_json, '$.policy_digest') = receipt.policy_digest
  AND prior_transition_event.event_type = 'task.lifecycle_changed'
  AND prior_transition_event.entity_type = 'task'
  AND prior_transition_event.entity_id = task.id
  AND prior_transition_event.scope_type = 'task'
  AND prior_transition_event.scope_id = task.id
  AND prior_transition_event.causation_id = request.id
  AND json_valid(prior_transition_event.payload_json)
  AND json_extract(prior_transition_event.payload_json, '$.from_state') = 'blocked'
  AND json_extract(prior_transition_event.payload_json, '$.to_state') = 'active'
  AND json_extract(prior_transition_event.payload_json, '$.cause_kind') = 'domain_event'
  AND json_extract(prior_transition_event.payload_json, '$.cause_ref') = request.id
  AND json_extract(prior_transition_event.payload_json, '$.reason_kind') = 'merge_failure_rework'
  AND json_extract(prior_transition_event.payload_json, '$.reason_ref') = merge.id
  AND NOT EXISTS (
      SELECT 1 FROM task_lifecycle_transition later
      WHERE later.task_id = task.id
        AND later.to_version > prior_transition.to_version
  );

-- One Task can have many historical admissions but only one lifecycle
-- transition can own its current version. Abort on any ambiguous candidate.
CREATE UNIQUE INDEX idx_v117_remote_pr_integrity_repair_task
    ON v117_remote_pr_integrity_repair(task_id);

-- Keep event identifiers UUID-shaped while assigning them inside this one
-- migration transaction. The three seeds are retained only in this TEMP table.
UPDATE v117_remote_pr_integrity_repair
SET repair_event_seed = lower(
        substr(repair_event_seed, 1, 8) || '-' ||
        substr(repair_event_seed, 9, 4) || '-4' ||
        substr(repair_event_seed, 14, 3) || '-' ||
        substr('89ab', (abs(random()) % 4) + 1, 1) ||
        substr(repair_event_seed, 18, 3) || '-' ||
        substr(repair_event_seed, 21, 12)
    ),
    transition_event_seed = lower(
        substr(transition_event_seed, 1, 8) || '-' ||
        substr(transition_event_seed, 9, 4) || '-4' ||
        substr(transition_event_seed, 14, 3) || '-' ||
        substr('89ab', (abs(random()) % 4) + 1, 1) ||
        substr(transition_event_seed, 18, 3) || '-' ||
        substr(transition_event_seed, 21, 12)
    ),
    transition_seed = lower(
        substr(transition_seed, 1, 8) || '-' ||
        substr(transition_seed, 9, 4) || '-4' ||
        substr(transition_seed, 14, 3) || '-' ||
        substr('89ab', (abs(random()) % 4) + 1, 1) ||
        substr(transition_seed, 18, 3) || '-' ||
        substr(transition_seed, 21, 12)
    );

INSERT INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT repair_event_seed,
       'task.remote_pr_integrity_repaired', 'task', task_id, 'system', NULL,
       'task', task_id,
       'remote-pr-integrity-repair:V117:' || task_merge_operation_id,
       prior_transition_event_id, 2,
       'remote-pr-integrity-repair:V117:' || task_merge_operation_id,
       json_object(
           'task_id', task_id,
           'task_merge_operation_id', task_merge_operation_id,
           'remote_pr_admission', json_object(
               'metadata_id', metadata_id,
               'publish_operation_id', publish_operation_id,
               'provider_config_id', provider_config_id,
               'provider_config_digest', provider_config_digest,
               'remote_repo_identity', remote_repo_identity,
               'admitted_source_sha', admitted_source_sha
           ),
           'provider_result_event_id', provider_result_event_id,
           'wrong_retry_receipt_id', retry_receipt_id,
           'wrong_rework_event_id', wrong_rework_event_id,
           'prior_lifecycle_transition_id', prior_lifecycle_transition_id,
           'migration_version', 'V117',
           'policy_version', 'remote-pr-head-mismatch-current-authority-v1',
           'reason', 'The exact erroneous wrong-head rework transition remains current; later non-lifecycle events do not supersede it.'
       ),
       repaired_at
FROM v117_remote_pr_integrity_repair;

INSERT INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT transition_event_seed,
       'task.lifecycle_changed', 'task', task_id, 'system', NULL,
       'task', task_id,
       'remote-pr-integrity-repair:V117:' || task_merge_operation_id,
       repair_event_seed, 3,
       'task-lifecycle:remote-pr-integrity-repair:V117:' || task_merge_operation_id,
       json_object(
           'task_id', task_id,
           'from_state', 'active',
           'to_state', 'blocked',
           'from_version', from_lifecycle_version,
           'to_version', from_lifecycle_version + 1,
           'cause_kind', 'domain_event',
           'cause_ref', repair_event_seed,
           'gate_evaluation_id', NULL,
           'reason_kind', 'remote_pr_head_mismatch',
           'reason_ref', provider_result_event_id
       ),
       repaired_at
FROM v117_remote_pr_integrity_repair;

INSERT INTO task_lifecycle_transition (
    id, task_id, idempotency_key, request_digest, expected_task_version,
    from_state, to_state, from_version, to_version, cause_kind, cause_ref,
    gate_evaluation_id, reason_kind, reason_ref, domain_event_id, created_at
)
SELECT transition_seed,
       task_id,
       'remote-pr-integrity-repair:V117:' || task_merge_operation_id,
       lower(hex(randomblob(32))),
       expected_task_version,
       'active', 'blocked', from_lifecycle_version,
       from_lifecycle_version + 1,
       'domain_event', repair_event_seed, NULL,
       'remote_pr_head_mismatch', provider_result_event_id,
       transition_event_seed, repaired_at
FROM v117_remote_pr_integrity_repair;

UPDATE task_lifecycle
SET state = 'blocked', version = version + 1,
    reason_kind = 'remote_pr_head_mismatch',
    reason_ref = (
        SELECT repair.provider_result_event_id
        FROM v117_remote_pr_integrity_repair repair
        WHERE repair.task_id = task_lifecycle.task_id
    ),
    updated_at = (
        SELECT repair.repaired_at
        FROM v117_remote_pr_integrity_repair repair
        WHERE repair.task_id = task_lifecycle.task_id
    )
WHERE task_id IN (SELECT task_id FROM v117_remote_pr_integrity_repair)
  AND state = 'active'
  AND version = (
      SELECT repair.from_lifecycle_version
      FROM v117_remote_pr_integrity_repair repair
      WHERE repair.task_id = task_lifecycle.task_id
  )
  AND reason_kind = 'merge_failure_rework'
  AND reason_ref = (
      SELECT repair.task_merge_operation_id
      FROM v117_remote_pr_integrity_repair repair
      WHERE repair.task_id = task_lifecycle.task_id
  );

UPDATE task
SET status = 'blocked', version = version + 1,
    updated_at = (
        SELECT repair.repaired_at
        FROM v117_remote_pr_integrity_repair repair
        WHERE repair.task_id = task.id
    )
WHERE id IN (SELECT task_id FROM v117_remote_pr_integrity_repair)
  AND status = 'in_progress';

DROP TABLE v117_remote_pr_integrity_repair;
