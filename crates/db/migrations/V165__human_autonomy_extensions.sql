-- Human grants preserve the original exhausted receipt and exact source.
CREATE TABLE task_autonomy_retry_grant (
    intent_id TEXT NOT NULL REFERENCES task_rework_automation_intent(id) ON DELETE CASCADE,
    limits_version INTEGER NOT NULL,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    PRIMARY KEY(intent_id, limits_version)
);
CREATE TABLE task_quality_retry_grant (
    request_id TEXT NOT NULL REFERENCES task_quality_request(id) ON DELETE CASCADE,
    limits_version INTEGER NOT NULL,
    user_id TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    PRIMARY KEY(request_id, limits_version)
);
DROP TRIGGER task_rework_automation_intent_identity_immutable;
CREATE TRIGGER task_rework_automation_intent_identity_immutable
BEFORE UPDATE ON task_rework_automation_intent
WHEN NEW.id IS NOT OLD.id
  OR NEW.task_id IS NOT OLD.task_id
  OR NEW.source_execution_id IS NOT OLD.source_execution_id
  OR NEW.source_task_role_id IS NOT OLD.source_task_role_id
  OR NEW.source_task_role_version IS NOT OLD.source_task_role_version
  OR NEW.source_configuration_json IS NOT OLD.source_configuration_json
  OR NEW.policy_digest IS NOT OLD.policy_digest
  OR NEW.review_report_artifact_id IS NOT OLD.review_report_artifact_id
  OR NEW.reviewed_work_unit_id IS NOT OLD.reviewed_work_unit_id
  OR NEW.reviewed_work_unit_version IS NOT OLD.reviewed_work_unit_version
  OR NEW.idempotency_key IS NOT OLD.idempotency_key
  OR NEW.created_at IS NOT OLD.created_at
  OR (NEW.reconciled_manual_receipt_id IS NOT OLD.reconciled_manual_receipt_id
      AND NOT (OLD.state = 'pending' AND NEW.state = 'completed'
               AND OLD.reconciled_manual_receipt_id IS NULL
               AND NEW.reconciled_manual_receipt_id IS NOT NULL))
  OR NOT (
      (OLD.state = 'pending' AND NEW.state IN ('completed', 'blocked', 'cancelled'))
      OR (OLD.state = NEW.state AND OLD.state IN ('completed', 'blocked', 'cancelled'))
      OR (OLD.state = 'blocked' AND NEW.state = 'pending' AND NEW.block_reason IS NULL
          AND OLD.block_reason LIKE '%rework budget exhausted%'
          AND EXISTS(SELECT 1 FROM task_autonomy_retry_grant grant_record JOIN task_autonomy_limits limits ON limits.task_id=OLD.task_id
                     WHERE grant_record.intent_id=OLD.id AND grant_record.limits_version=limits.version))
  )
BEGIN
    SELECT RAISE(ABORT, 'automatic rework intent identity is immutable and state transitions are terminal');
END;
