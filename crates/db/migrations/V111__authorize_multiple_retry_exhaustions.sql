-- A Task may have more than one exact exhausted retry receipt. Each receipt
-- may be superseded independently, so retry_epoch is sequence evidence rather
-- than a uniqueness key for a single failure kind.
DROP TRIGGER task_retry_override_exact_authority_guard;
DROP TRIGGER task_retry_override_immutable_update;
DROP TRIGGER task_retry_override_immutable_delete;
DROP INDEX idx_task_retry_override_task_kind_epoch;

ALTER TABLE task_retry_override RENAME TO task_retry_override_v110;

CREATE TABLE task_retry_override (
    id                              TEXT PRIMARY KEY,
    task_id                         TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    failure_kind                    TEXT NOT NULL,
    exhaustion_receipt_id           TEXT NOT NULL UNIQUE
                                        REFERENCES task_failure_retry_receipt(id) ON DELETE RESTRICT,
    decision_id                     TEXT NOT NULL UNIQUE,
    authorization_event_id          TEXT NOT NULL UNIQUE REFERENCES domain_event(id) ON DELETE RESTRICT,
    retry_epoch                     INTEGER NOT NULL CHECK (retry_epoch >= 1),
    expected_task_version           INTEGER NOT NULL CHECK (expected_task_version >= 1),
    retry_policy_ref                TEXT NOT NULL,
    retry_policy_version            INTEGER NOT NULL CHECK (retry_policy_version = 3),
    retry_policy_digest             TEXT NOT NULL CHECK (
                                        retry_policy_digest = 'e5e28d200405a47a232d679c07d26d917fe954b08ec664236f42e0f65c4d4903'
                                    ),
    authorization_policy_ref        TEXT NOT NULL,
    authorization_policy_version    INTEGER NOT NULL CHECK (authorization_policy_version >= 1),
    authorization_policy_digest     TEXT NOT NULL CHECK (
                                        authorization_policy_digest = '8d1a1fc8856797674914ce8bb46d1d514c17c6f1cdc1976048322c24cb3c61ec'
                                    ),
    created_at                      TEXT NOT NULL,
    FOREIGN KEY (decision_id, task_id) REFERENCES decision(id, task_id) ON DELETE RESTRICT
);

INSERT INTO task_retry_override (
    id, task_id, failure_kind, exhaustion_receipt_id, decision_id,
    authorization_event_id, retry_epoch, expected_task_version,
    retry_policy_ref, retry_policy_version, retry_policy_digest,
    authorization_policy_ref, authorization_policy_version,
    authorization_policy_digest, created_at
)
SELECT id, task_id, failure_kind, exhaustion_receipt_id, decision_id,
       authorization_event_id, retry_epoch, expected_task_version,
       retry_policy_ref, retry_policy_version, retry_policy_digest,
       authorization_policy_ref, authorization_policy_version,
       authorization_policy_digest, created_at
FROM task_retry_override_v110;

DROP TABLE task_retry_override_v110;

CREATE INDEX idx_task_retry_override_task_kind_epoch
    ON task_retry_override(task_id, failure_kind, retry_epoch);

CREATE TRIGGER task_retry_override_exact_authority_guard
BEFORE INSERT ON task_retry_override
WHEN NOT EXISTS (
    SELECT 1
    FROM task_failure_retry_receipt receipt
    JOIN task t ON t.id = receipt.task_id
    JOIN task_lifecycle lifecycle ON lifecycle.task_id = receipt.task_id
    JOIN decision d ON d.id = NEW.decision_id AND d.task_id = receipt.task_id
    JOIN proposal p ON p.id = d.proposal_id AND p.task_id = d.task_id
    JOIN decision_actor actor
      ON actor.decision_id = d.id AND actor.task_id = d.task_id AND actor.actor_kind = 'human'
    JOIN domain_event decision_event
      ON decision_event.event_type = 'decision.recorded'
     AND decision_event.entity_type = 'decision'
     AND decision_event.entity_id = d.id
     AND decision_event.scope_type = 'task'
     AND decision_event.scope_id = d.task_id
    JOIN domain_event authorization ON authorization.id = NEW.authorization_event_id
    WHERE receipt.id = NEW.exhaustion_receipt_id
      AND receipt.task_id = NEW.task_id
      AND receipt.failure_kind = NEW.failure_kind
      AND receipt.disposition = 'exhausted'
      AND lifecycle.state = 'blocked'
      AND lifecycle.reason_kind = 'retry_budget_exhausted'
      AND NOT EXISTS (
          SELECT 1 FROM task_retry_override existing
          WHERE existing.exhaustion_receipt_id = receipt.id
      )
      AND t.version = NEW.expected_task_version
      AND d.outcome = 'approve'
      AND decision_event.sequence > (
          SELECT exhaustion_event.sequence
          FROM domain_event exhaustion_event
          WHERE exhaustion_event.id = receipt.receipt_event_id
      )
      AND json_valid(decision_event.payload_json)
      AND json_extract(decision_event.payload_json, '$.decision_id') = d.id
      AND json_extract(decision_event.payload_json, '$.task_id') = d.task_id
      AND json_extract(decision_event.payload_json, '$.proposal_id') = d.proposal_id
      AND json_extract(decision_event.payload_json, '$.outcome') = d.outcome
      AND d.proposal_version = p.content_version
      AND p.status = 'resolved'
      AND p.action = 'retry_exhaustion_override:' || receipt.failure_kind || ':' || receipt.id
      AND p.target_kind = 'task'
      AND p.target_id = receipt.task_id
      AND d.policy_ref = NEW.authorization_policy_ref
      AND d.policy_version = NEW.authorization_policy_version
      AND d.policy_digest = NEW.authorization_policy_digest
      AND p.required_policy_ref = NEW.authorization_policy_ref
      AND p.required_policy_version = NEW.authorization_policy_version
      AND p.required_policy_digest = NEW.authorization_policy_digest
      AND NEW.retry_policy_ref = 'forge.task_failure_retry'
      AND NEW.retry_policy_version = 3
      AND NEW.retry_policy_digest = 'e5e28d200405a47a232d679c07d26d917fe954b08ec664236f42e0f65c4d4903'
      AND NEW.authorization_policy_ref = 'forge.task_retry_override'
      AND NEW.authorization_policy_version = 1
      AND NEW.authorization_policy_digest = '8d1a1fc8856797674914ce8bb46d1d514c17c6f1cdc1976048322c24cb3c61ec'
      AND NEW.retry_epoch = COALESCE((
          SELECT MAX(previous.retry_epoch)
          FROM task_retry_override previous
          WHERE previous.task_id = NEW.task_id
            AND previous.failure_kind = NEW.failure_kind
      ), 0) + 1
      AND authorization.event_type = 'task.retry_override_authorized'
      AND authorization.entity_type = 'task'
      AND authorization.entity_id = receipt.task_id
      AND authorization.scope_type = 'task'
      AND authorization.scope_id = receipt.task_id
      AND authorization.causation_id = d.id
      AND json_valid(authorization.payload_json)
      AND json_extract(authorization.payload_json, '$.task_id') = receipt.task_id
      AND json_extract(authorization.payload_json, '$.exhaustion_receipt_id') = receipt.id
      AND json_extract(authorization.payload_json, '$.failure_kind') = receipt.failure_kind
      AND json_extract(authorization.payload_json, '$.failure_ref') = receipt.failure_ref
      AND json_extract(authorization.payload_json, '$.decision_id') = d.id
      AND json_extract(authorization.payload_json, '$.retry_epoch') = NEW.retry_epoch
      AND json_extract(authorization.payload_json, '$.expected_task_version') = NEW.expected_task_version
      AND json_extract(authorization.payload_json, '$.retry_policy_ref') = NEW.retry_policy_ref
      AND json_extract(authorization.payload_json, '$.retry_policy_version') = NEW.retry_policy_version
      AND json_extract(authorization.payload_json, '$.retry_policy_digest') = NEW.retry_policy_digest
      AND json_extract(authorization.payload_json, '$.authorization_policy_ref') = NEW.authorization_policy_ref
      AND json_extract(authorization.payload_json, '$.authorization_policy_version') = NEW.authorization_policy_version
      AND json_extract(authorization.payload_json, '$.authorization_policy_digest') = NEW.authorization_policy_digest
)
BEGIN
    SELECT RAISE(ABORT, 'retry override requires the exact approved same-Task Human Decision and exhaustion receipt');
END;

CREATE TRIGGER task_retry_override_immutable_update
BEFORE UPDATE ON task_retry_override
BEGIN
    SELECT RAISE(ABORT, 'Task retry overrides are immutable');
END;

CREATE TRIGGER task_retry_override_immutable_delete
BEFORE DELETE ON task_retry_override
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task retry overrides are immutable outside Project teardown');
END;

-- All unoverridden exhausted receipts remain independent fences. When none
-- remain, only the latest exact Human override may move an exhaustion-blocked
-- Task to ready; delayed earlier Decisions cannot borrow that authority.
DROP TRIGGER task_lifecycle_retry_exhaustion_guard;
CREATE TRIGGER task_lifecycle_retry_exhaustion_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.to_state IN ('ready', 'active')
 AND (
     EXISTS (
         SELECT 1 FROM task_failure_retry_receipt receipt
         WHERE receipt.task_id = NEW.task_id
           AND receipt.disposition = 'exhausted'
           AND NOT EXISTS (
               SELECT 1 FROM task_retry_override override
               WHERE override.exhaustion_receipt_id = receipt.id
                 AND override.task_id = receipt.task_id
                 AND override.failure_kind = receipt.failure_kind
           )
     )
     OR EXISTS (
         SELECT 1 FROM task_lifecycle current
         WHERE current.task_id = NEW.task_id
           AND current.state = 'blocked'
           AND current.reason_kind = 'retry_budget_exhausted'
           AND NOT (
               NEW.from_state = 'blocked'
               AND NEW.to_state = 'ready'
               AND NEW.cause_kind = 'domain_event'
               AND NEW.reason_kind = 'retry_exhaustion_superseded'
               AND EXISTS (
                   SELECT 1
                   FROM task_retry_override override
                   JOIN task_failure_retry_receipt receipt
                     ON receipt.id = override.exhaustion_receipt_id
                    AND receipt.task_id = override.task_id
                    AND receipt.failure_kind = override.failure_kind
                   JOIN domain_event authorization
                     ON authorization.id = override.authorization_event_id
                   WHERE override.task_id = NEW.task_id
                     AND override.authorization_event_id = NEW.cause_ref
                     AND NEW.reason_ref = receipt.id
                     AND NOT EXISTS (
                         SELECT 1 FROM task_failure_retry_receipt pending
                         WHERE pending.task_id = NEW.task_id
                           AND pending.disposition = 'exhausted'
                           AND NOT EXISTS (
                               SELECT 1 FROM task_retry_override pending_override
                               WHERE pending_override.exhaustion_receipt_id = pending.id
                                 AND pending_override.task_id = pending.task_id
                                 AND pending_override.failure_kind = pending.failure_kind
                           )
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM task_retry_override later_override
                         JOIN domain_event later_authorization
                           ON later_authorization.id = later_override.authorization_event_id
                         WHERE later_override.task_id = override.task_id
                           AND later_authorization.sequence > authorization.sequence
                     )
               )
           )
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'Task retry exhaustion requires its exact final external Decision override');
END;
