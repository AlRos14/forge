-- Keep automatic intent identity while allowing it to acknowledge one exact
-- manual receipt for the same ReviewReport. The receipt link is an audit fact;
-- all source, report, reviewed revision, pair, and round checks remain exact.

ALTER TABLE task_rework_automation_intent
    ADD COLUMN reconciled_manual_receipt_id TEXT
        REFERENCES task_rework_receipt(id) ON DELETE RESTRICT;

DROP TRIGGER task_rework_automation_intent_identity_immutable;
DROP TRIGGER task_rework_automation_intent_pair_guard;

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
  )
BEGIN
    SELECT RAISE(ABORT, 'automatic rework intent identity is immutable and state transitions are terminal');
END;

CREATE TRIGGER task_rework_automation_intent_manual_reconciliation_insert_guard
BEFORE INSERT ON task_rework_automation_intent
WHEN NEW.reconciled_manual_receipt_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'manual receipt reconciliation is only recorded when completing a pending intent');
END;

CREATE TRIGGER task_rework_automation_intent_pair_guard
BEFORE UPDATE OF state, correction_work_unit_id, review_work_unit_id,
                 rework_round, reconciled_manual_receipt_id
ON task_rework_automation_intent
WHEN NEW.state = 'completed'
 AND NOT EXISTS (
    SELECT 1 FROM task_rework_receipt receipt
    WHERE receipt.task_id = NEW.task_id
      AND receipt.review_report_artifact_id = NEW.review_report_artifact_id
      AND receipt.reviewed_work_unit_id = NEW.reviewed_work_unit_id
      AND receipt.reviewed_work_unit_version = NEW.reviewed_work_unit_version
      AND receipt.correction_work_unit_id = NEW.correction_work_unit_id
      AND receipt.review_work_unit_id = NEW.review_work_unit_id
      AND receipt.round = NEW.rework_round
      AND (
          (NEW.reconciled_manual_receipt_id IS NULL
           AND receipt.idempotency_key = NEW.idempotency_key)
          OR
          (NEW.reconciled_manual_receipt_id IS NOT NULL
           AND receipt.id = NEW.reconciled_manual_receipt_id
           AND receipt.idempotency_key <> NEW.idempotency_key)
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'completed automatic rework intent must reference its exact atomic rework receipt');
END;
