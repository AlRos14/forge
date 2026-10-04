-- TaskRole policy/coordination revisions affect frozen reviewer authority.
-- Membership events are emitted by V100; this event covers the remaining
-- TaskRole revision writes so every active Gate can re-evaluate durably.
CREATE TRIGGER pr9_task_role_revision_version_guard
BEFORE UPDATE OF role, coordination_mode, policy_json ON task_role
WHEN (OLD.role IS NOT NEW.role
   OR OLD.coordination_mode IS NOT NEW.coordination_mode
   OR OLD.policy_json IS NOT NEW.policy_json)
 AND NEW.version != OLD.version + 1
BEGIN
    SELECT RAISE(ABORT, 'TaskRole policy revisions must advance their version fence');
END;

CREATE TRIGGER pr9_gate_task_role_revision_event
AFTER UPDATE OF role, coordination_mode, policy_json ON task_role
WHEN OLD.role IS NOT NEW.role
  OR OLD.coordination_mode IS NOT NEW.coordination_mode
  OR OLD.policy_json IS NOT NEW.policy_json
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    ) VALUES (
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'gate.task_role_changed', 'task_role', NEW.id, 'system', NULL,
        'task', NEW.task_id, NEW.id, NULL, 0,
        'pr9-gate-task-role:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', NEW.id,
            'old_version', OLD.version,
            'version', NEW.version,
            'role', NEW.role,
            'coordination_mode', NEW.coordination_mode
        ),
        NEW.updated_at
    );
END;

-- Retry receipts remain immutable during normal operation. The guarded
-- Project teardown is the only operation allowed to remove their owner rows.
DROP TRIGGER task_failure_retry_receipt_immutable_delete;
CREATE TRIGGER task_failure_retry_receipt_immutable_delete
BEFORE DELETE ON task_failure_retry_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard guard ON guard.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task retry receipts are immutable outside Project teardown');
END;
