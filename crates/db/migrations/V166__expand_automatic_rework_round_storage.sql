-- Widen the historical eight-round storage bound while preserving all rows,
-- exact receipt links, foreign keys and current automatic rework guards.
PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;
CREATE TEMP TABLE _v166_check (ok INTEGER NOT NULL CHECK(ok=1));
CREATE TABLE task_rework_automation_intent_v166 (
    id                         TEXT PRIMARY KEY,
    task_id                    TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    source_execution_id        TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    source_task_role_id        TEXT NOT NULL REFERENCES task_role(id) ON DELETE RESTRICT,
    source_task_role_version   INTEGER NOT NULL CHECK (source_task_role_version >= 1),
    source_configuration_json  TEXT NOT NULL CHECK (
        json_valid(source_configuration_json)
        AND json_type(source_configuration_json) = 'object'
    ),
    policy_digest              TEXT NOT NULL CHECK (length(policy_digest) = 64),
    review_report_artifact_id  TEXT NOT NULL REFERENCES artifact(id) ON DELETE RESTRICT,
    reviewed_work_unit_id      TEXT NOT NULL REFERENCES work_unit(id) ON DELETE RESTRICT,
    reviewed_work_unit_version INTEGER NOT NULL CHECK (reviewed_work_unit_version >= 1),
    idempotency_key            TEXT NOT NULL CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    state                      TEXT NOT NULL CHECK (state IN ('pending', 'completed', 'blocked', 'cancelled')),
    block_reason               TEXT,
    correction_work_unit_id    TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    review_work_unit_id        TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    rework_round               INTEGER CHECK (rework_round IS NULL OR rework_round BETWEEN 1 AND 1000),
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL,
    completed_at               TEXT,
    reconciled_manual_receipt_id TEXT REFERENCES task_rework_receipt(id) ON DELETE RESTRICT,
    UNIQUE(task_id, idempotency_key),
    UNIQUE(review_report_artifact_id),
    CHECK (
        (state = 'completed' AND correction_work_unit_id IS NOT NULL
         AND review_work_unit_id IS NOT NULL AND rework_round IS NOT NULL
         AND completed_at IS NOT NULL AND block_reason IS NULL)
        OR (state = 'blocked' AND block_reason IS NOT NULL
            AND correction_work_unit_id IS NULL AND review_work_unit_id IS NULL
            AND rework_round IS NULL AND completed_at IS NULL)
        OR (state IN ('pending', 'cancelled')
            AND correction_work_unit_id IS NULL AND review_work_unit_id IS NULL
            AND rework_round IS NULL AND completed_at IS NULL)
    )
);
INSERT INTO task_rework_automation_intent_v166 SELECT * FROM task_rework_automation_intent;
INSERT INTO _v166_check SELECT NOT EXISTS(SELECT * FROM task_rework_automation_intent EXCEPT SELECT * FROM task_rework_automation_intent_v166);
DROP TRIGGER task_rework_automation_intent_insert_guard;
DROP TRIGGER task_rework_automation_intent_identity_immutable;
DROP TRIGGER task_rework_automation_intent_pair_guard;
DROP TRIGGER task_rework_automation_intent_immutable_delete;
DROP TRIGGER task_rework_automation_intent_manual_reconciliation_insert_guard;
DROP INDEX idx_task_rework_automation_pending;
DROP INDEX idx_task_rework_automation_task;
ALTER TABLE task_rework_automation_intent RENAME TO task_rework_automation_intent_v153;
ALTER TABLE task_rework_automation_intent_v166 RENAME TO task_rework_automation_intent;
DROP TABLE task_rework_automation_intent_v153;
CREATE INDEX idx_task_rework_automation_pending ON task_rework_automation_intent(state,created_at,id);
CREATE INDEX idx_task_rework_automation_task ON task_rework_automation_intent(task_id,created_at DESC,id DESC);

CREATE TRIGGER task_rework_automation_intent_insert_guard
BEFORE INSERT ON task_rework_automation_intent
WHEN NOT EXISTS (
    SELECT 1
    FROM execution execution
    JOIN execution_task_role_authority authority
      ON authority.execution_id = execution.id
    JOIN task_role role
      ON role.id = authority.task_role_id AND role.task_id = execution.task_id
    JOIN task_role_capability_configuration configuration
      ON configuration.task_role_id = role.id
    JOIN role_membership membership
      ON membership.id = authority.membership_id
     AND membership.task_role_id = role.id
     AND membership.actor_kind = execution.actor_kind
     AND membership.actor_id = execution.actor_id
     AND membership.status = 'active'
    JOIN artifact_execution_producer producer
      ON producer.execution_id = execution.id
     AND producer.task_id = execution.task_id
    JOIN artifact report
      ON report.id = producer.artifact_id
     AND report.task_id = execution.task_id
     AND report.kind = 'review_report'
    JOIN work_unit_review_subject subject
      ON subject.review_work_unit_id = execution.work_unit_id
     AND subject.task_id = execution.task_id
    WHERE execution.id = NEW.source_execution_id
      AND execution.task_id = NEW.task_id
      AND execution.work_unit_id IS NOT NULL
      AND role.id = NEW.source_task_role_id
      AND role.version = NEW.source_task_role_version
      AND authority.task_role_version = NEW.source_task_role_version
      AND authority.configuration_json = NEW.source_configuration_json
      AND configuration.configuration_json = NEW.source_configuration_json
      AND authority.source = 'admission'
      AND authority.capabilities_json = json_extract(configuration.configuration_json, '$.capabilities')
      AND EXISTS (
          SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities') capability
          WHERE capability.type = 'text'
            AND capability.value = 'formal_review'
      )
      AND EXISTS (
          SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities') capability
          WHERE capability.type = 'text'
            AND capability.value = 'request_rework'
      )
      AND json_extract(configuration.configuration_json, '$.rework.mode') = 'automatic'
      AND report.id = NEW.review_report_artifact_id
      AND subject.reviewed_work_unit_id = NEW.reviewed_work_unit_id
      AND subject.reviewed_work_unit_version = NEW.reviewed_work_unit_version
      AND json_extract(report.content, '$.verdict') = 'request_changes'
      AND json_extract(report.content, '$.subject.reviewed_work_unit_id') = NEW.reviewed_work_unit_id
      AND json_extract(report.content, '$.subject.reviewed_work_unit_version') = NEW.reviewed_work_unit_version
      AND json_extract(report.content, '$.subject.reviewed_result.execution_id') = subject.result_execution_id
)
BEGIN
    SELECT RAISE(ABORT, 'automatic rework intent must bind its exact current review authority and subject');
END;

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

CREATE TRIGGER task_rework_automation_intent_immutable_delete
BEFORE DELETE ON task_rework_automation_intent
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'automatic rework intents are immutable outside Project teardown');
END;

CREATE TRIGGER task_rework_automation_intent_manual_reconciliation_insert_guard
BEFORE INSERT ON task_rework_automation_intent
WHEN NEW.reconciled_manual_receipt_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'manual receipt reconciliation is only recorded when completing a pending intent');
END;

INSERT INTO _v166_check SELECT NOT EXISTS(SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _v166_check SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1)='ok';
DROP TABLE _v166_check;
INSERT INTO _migration(version,name,applied_at) VALUES(166,'expand_automatic_rework_round_storage',strftime('%Y-%m-%dT%H:%M:%fZ','now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
