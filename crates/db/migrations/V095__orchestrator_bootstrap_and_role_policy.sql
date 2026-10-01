-- Plan PR6 keeps source-event wakes separate from current-state bootstrap.
-- A bootstrap event records this migration's reconciliation decision; it does
-- not rewrite or replay an older Task event.

ALTER TABLE orchestrator_wake
    ADD COLUMN task_role_version INTEGER NOT NULL DEFAULT 1
        CHECK (task_role_version >= 1);
ALTER TABLE orchestrator_wake
    ADD COLUMN task_role_policy_json TEXT NOT NULL DEFAULT '{}'
        CHECK (json_valid(task_role_policy_json)
               AND json_type(task_role_policy_json) = 'object');

-- Existing wake obligations were admitted before PR6 read TaskRole policy.
-- Snapshot the current role configuration at this explicit cutover boundary;
-- the dispatch path then requires this exact version and JSON to remain live.
UPDATE orchestrator_wake
SET task_role_version = (
        SELECT role.version FROM task_role AS role
        WHERE role.id = orchestrator_wake.task_role_id
    ),
    task_role_policy_json = (
        SELECT role.policy_json FROM task_role AS role
        WHERE role.id = orchestrator_wake.task_role_id
    );

DROP TRIGGER orchestrator_wake_authority_guard_insert;
CREATE TRIGGER orchestrator_wake_authority_guard_insert
BEFORE INSERT ON orchestrator_wake
WHEN NOT EXISTS (
        SELECT 1 FROM domain_event AS event
        WHERE event.id = NEW.event_id AND event.sequence = NEW.event_sequence
          AND event.scope_type = 'task' AND event.scope_id = NEW.task_id
    )
  OR NOT EXISTS (
        SELECT 1 FROM task_role AS role
        WHERE role.id = NEW.task_role_id AND role.task_id = NEW.task_id
          AND role.role = 'orchestrator'
          AND role.coordination_mode IS NEW.coordination_mode
          AND role.version = NEW.task_role_version
          AND role.policy_json = NEW.task_role_policy_json
    )
  OR NOT EXISTS (
        SELECT 1 FROM role_membership AS member
        WHERE member.task_role_id = NEW.task_role_id
          AND member.actor_kind = NEW.actor_kind AND member.actor_id = NEW.actor_id
          AND member.status = 'active'
    )
  OR (NEW.work_unit_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM work_unit AS unit
        WHERE unit.id = NEW.work_unit_id AND unit.task_id = NEW.task_id
    ))
BEGIN
    SELECT RAISE(ABORT, 'orchestrator wake requires exact durable cause, current TaskRole policy, active Actor, and same-Task WorkUnit');
END;

DROP TRIGGER orchestrator_wake_identity_immutable;
CREATE TRIGGER orchestrator_wake_identity_immutable
BEFORE UPDATE OF event_id, event_sequence, task_id, task_role_id,
                 coordination_mode, actor_kind, actor_id, work_unit_id,
                 correlation_id, causation_id, causation_depth,
                 policy_ref, policy_version, policy_digest,
                 task_role_version, task_role_policy_json
ON orchestrator_wake
BEGIN
    SELECT RAISE(ABORT, 'orchestrator wake provenance and authority are immutable');
END;

-- Reconcile already-existing Tasks once at cutover. Every bootstrap row names
-- one exact active Actor; partitioned Roles additionally require that Actor's
-- exact orchestrator WorkUnit allocation. A live wake for the same target
-- suppresses the bootstrap, and the domain-event dedupe key makes migration
-- retry idempotent. These are new bootstrap events at migration time, not
-- reconstructed historical Task events.
WITH bootstrap_target AS (
    SELECT role.id AS task_role_id, role.task_id, role.coordination_mode,
           member.actor_kind, member.actor_id, NULL AS work_unit_id
    FROM task_role AS role
    JOIN task AS task ON task.id = role.task_id AND task.deleted_at IS NULL
                       AND task.status NOT IN ('done', 'cancelled')
    JOIN role_membership AS member
      ON member.task_role_id = role.id AND member.status = 'active'
    WHERE role.role = 'orchestrator'
      AND role.coordination_mode = 'collaborative'

    UNION ALL

    SELECT role.id, role.task_id, role.coordination_mode,
           member.actor_kind, member.actor_id, NULL
    FROM task_role AS role
    JOIN task AS task ON task.id = role.task_id AND task.deleted_at IS NULL
                       AND task.status NOT IN ('done', 'cancelled')
    JOIN role_membership AS member
      ON member.task_role_id = role.id AND member.status = 'active'
    WHERE role.role = 'orchestrator'
      AND role.coordination_mode = 'independent'
      AND 1 = (
          SELECT COUNT(*) FROM role_membership AS active_member
          WHERE active_member.task_role_id = role.id
            AND active_member.status = 'active'
      )

    UNION ALL

    SELECT role.id, role.task_id, role.coordination_mode,
           member.actor_kind, member.actor_id, NULL
    FROM task_role AS role
    JOIN task AS task ON task.id = role.task_id AND task.deleted_at IS NULL
                       AND task.status NOT IN ('done', 'cancelled')
    JOIN role_membership AS member
      ON member.task_role_id = role.id AND member.status = 'active'
    WHERE role.role = 'orchestrator'
      AND role.coordination_mode IS NULL
      AND 1 = (
          SELECT COUNT(*) FROM role_membership AS active_member
          WHERE active_member.task_role_id = role.id
            AND active_member.status = 'active'
      )

    UNION ALL

    SELECT role.id, role.task_id, role.coordination_mode,
           member.actor_kind, member.actor_id, unit.id
    FROM task_role AS role
    JOIN task AS task ON task.id = role.task_id AND task.deleted_at IS NULL
                       AND task.status NOT IN ('done', 'cancelled')
    JOIN role_membership AS member
      ON member.task_role_id = role.id AND member.status = 'active'
    JOIN work_unit AS unit
      ON unit.task_id = role.task_id AND unit.role = 'orchestrator'
     AND unit.assigned_actor_kind = member.actor_kind
     AND unit.assigned_actor_id = member.actor_id
    WHERE role.role = 'orchestrator'
      AND role.coordination_mode = 'partitioned'
), candidates AS (
    SELECT bootstrap_target.*,
           'pr6-orchestrator-bootstrap:V095:' || task_role_id || ':' ||
               actor_kind || ':' || actor_id || ':' ||
               COALESCE(work_unit_id, 'task') AS dedupe_key
    FROM bootstrap_target
    WHERE NOT EXISTS (
        SELECT 1 FROM orchestrator_wake AS wake
        WHERE wake.task_id = bootstrap_target.task_id
          AND wake.task_role_id = bootstrap_target.task_role_id
          AND wake.actor_kind = bootstrap_target.actor_kind
          AND wake.actor_id = bootstrap_target.actor_id
          AND wake.work_unit_id IS bootstrap_target.work_unit_id
          AND wake.state IN ('pending', 'leased', 'running', 'awaiting_human', 'uncertain')
    )
)
INSERT OR IGNORE INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    'orchestrator.bootstrap_reconciled', 'task_role', task_role_id,
    'system', NULL, 'task', task_id, task_role_id, NULL, 0, dedupe_key,
    json_object(
        'task_role_id', task_role_id,
        'coordination_mode', coordination_mode,
        'actor_kind', actor_kind,
        'actor_id', actor_id,
        'work_unit_id', work_unit_id,
        'bootstrap_version', 1
    ),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM candidates;

-- Task inserts gain a durable source record in the same transaction. A Task is
-- not wake-eligible until its canonical RoleMembership is active; those later
-- activations have their own event below.
CREATE TRIGGER pr6_task_created_event
AFTER INSERT ON task
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
        'task.created', 'task', NEW.id, 'system', NULL,
        'task', NEW.id, NEW.id, NULL, 0,
        'pr6-task-created:' || NEW.id,
        json_object('task_version', NEW.version),
        NEW.created_at
    );
END;

-- Empty TaskRole creation alone is not eligibility. Active membership writes
-- below are the activation source; updates to existing orchestrator policy
-- or coordination become a source event only when an active Actor exists.
CREATE TRIGGER pr6_orchestrator_task_role_changed_event
AFTER UPDATE OF coordination_mode, policy_json ON task_role
WHEN OLD.role = 'orchestrator'
 AND (OLD.coordination_mode IS NOT NEW.coordination_mode
      OR OLD.policy_json IS NOT NEW.policy_json)
 AND EXISTS (
      SELECT 1 FROM role_membership AS member
      WHERE member.task_role_id = NEW.id AND member.status = 'active'
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', NEW.id, 'system', NULL,
        'task', NEW.task_id, NEW.id,
        NULL, 0, 'pr6-task-role:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', NEW.id,
            'task_role_version', NEW.version,
            'coordination_mode', NEW.coordination_mode,
            'policy_changed', OLD.policy_json IS NOT NEW.policy_json,
            'actor_kind', NULL,
            'actor_id', NULL,
            'work_unit_id', NULL
        ),
        NEW.updated_at
    WHERE NEW.coordination_mode IS NOT 'partitioned';

    -- A partitioned role-level configuration change is not a generic fanout.
    -- Emit one exact Actor + assigned orchestrator WorkUnit signal instead.
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.task_role_changed', 'task_role', NEW.id, 'system', NULL,
        'task', NEW.task_id, NEW.id,
        NULL, 0,
        'pr6-task-role:' || NEW.id || ':' || NEW.version || ':' ||
            member.actor_kind || ':' || member.actor_id || ':' || unit.id,
        json_object(
            'task_role_id', NEW.id,
            'task_role_version', NEW.version,
            'coordination_mode', NEW.coordination_mode,
            'policy_changed', OLD.policy_json IS NOT NEW.policy_json,
            'actor_kind', member.actor_kind,
            'actor_id', member.actor_id,
            'work_unit_id', unit.id
        ),
        NEW.updated_at
    FROM role_membership AS member
    JOIN work_unit AS unit
      ON unit.task_id = NEW.task_id AND unit.role = 'orchestrator'
     AND unit.assigned_actor_kind = member.actor_kind
     AND unit.assigned_actor_id = member.actor_id
    WHERE NEW.coordination_mode = 'partitioned'
      AND member.task_role_id = NEW.id AND member.status = 'active';
END;

CREATE TRIGGER pr6_orchestrator_membership_inserted_event
AFTER INSERT ON role_membership
WHEN NEW.status = 'active'
 AND EXISTS (
      SELECT 1 FROM task_role AS role
      WHERE role.id = NEW.task_role_id AND role.role = 'orchestrator'
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.membership_changed', 'role_membership', NEW.id, 'system', NULL,
        'task', role.task_id, NEW.id, NULL, 0,
        'pr6-membership:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', role.id,
            'actor_kind', NEW.actor_kind,
            'actor_id', NEW.actor_id,
            'status', NEW.status,
            'membership_version', NEW.version
        ),
        NEW.created_at
    FROM task_role AS role
    WHERE role.id = NEW.task_role_id;
END;

CREATE TRIGGER pr6_orchestrator_membership_updated_event
AFTER UPDATE OF status, actor_kind, actor_id, task_role_id ON role_membership
WHEN (OLD.status IS NOT NEW.status
      OR OLD.actor_kind IS NOT NEW.actor_kind
      OR OLD.actor_id IS NOT NEW.actor_id
      OR OLD.task_role_id IS NOT NEW.task_role_id)
 AND EXISTS (
      SELECT 1 FROM task_role AS role
      WHERE role.id = NEW.task_role_id AND role.role = 'orchestrator'
 )
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'orchestrator.membership_changed', 'role_membership', NEW.id, 'system', NULL,
        'task', role.task_id, NEW.id, NULL, 0,
        'pr6-membership:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_role_id', role.id,
            'actor_kind', NEW.actor_kind,
            'actor_id', NEW.actor_id,
            'status', NEW.status,
            'membership_version', NEW.version
        ),
        NEW.updated_at
    FROM task_role AS role
    WHERE role.id = NEW.task_role_id;
END;

-- Task blocking/failure metadata may be written without changing the legacy
-- Task status column. Preserve every actual metadata change durably when no
-- task.status_changed event already represents a simultaneous status change.
CREATE TRIGGER pr6_task_block_state_event
AFTER UPDATE OF blocked_json, failed_json ON task
WHEN NEW.deleted_at IS NULL
 AND OLD.status IS NEW.status
 AND (OLD.blocked_json IS NOT NEW.blocked_json
      OR OLD.failed_json IS NOT NEW.failed_json)
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
        CASE WHEN NEW.failed_json IS NOT NULL THEN 'task.failed'
             WHEN NEW.blocked_json IS NOT NULL THEN 'task.blocked'
             ELSE 'task.unblocked' END,
        'task', NEW.id, 'system', NULL, 'task', NEW.id,
        NEW.id,
        NULL, 0, 'pr6-task-block-state:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_version', NEW.version,
            'blocked', NEW.blocked_json IS NOT NULL,
            'failed', NEW.failed_json IS NOT NULL
        ),
        NEW.updated_at
    );
END;

DROP TRIGGER orchestrator_execution_authority_guard;
CREATE TRIGGER orchestrator_execution_authority_guard
BEFORE INSERT ON execution
WHEN NEW.role = 'orchestrator'
  AND (
        NEW.purpose IS NOT 'orchestrate'
        OR NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake AS wake
            JOIN orchestrator_wake_execution AS attempt
              ON attempt.wake_id = wake.id AND attempt.execution_id = NEW.id
            JOIN task_role AS role ON role.id = wake.task_role_id
            JOIN role_membership AS member
              ON member.task_role_id = wake.task_role_id
             AND member.actor_kind = wake.actor_kind
             AND member.actor_id = wake.actor_id
             AND member.status = 'active'
            WHERE wake.task_id = NEW.task_id
              AND wake.actor_kind = NEW.actor_kind
              AND wake.actor_id = NEW.actor_id
              AND role.task_id = NEW.task_id
              AND role.role = 'orchestrator'
              AND role.version = wake.task_role_version
              AND role.policy_json = wake.task_role_policy_json
              AND role.coordination_mode IS wake.coordination_mode
              AND wake.state = 'leased'
              AND attempt.state = 'reserved'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrate Execution requires its exact leased wake, current TaskRole policy, active Actor, and reserved attempt');
END;

DROP TRIGGER orchestrator_execution_authority_guard_update;
CREATE TRIGGER orchestrator_execution_authority_guard_update
BEFORE UPDATE OF task_id, actor_kind, actor_id, role, purpose, workspace_id ON execution
WHEN NEW.role = 'orchestrator'
  AND (
        NEW.purpose IS NOT 'orchestrate'
        OR NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake AS wake
            JOIN orchestrator_wake_execution AS attempt
              ON attempt.wake_id = wake.id AND attempt.execution_id = NEW.id
            JOIN task_role AS role ON role.id = wake.task_role_id
            JOIN role_membership AS member
              ON member.task_role_id = wake.task_role_id
             AND member.actor_kind = wake.actor_kind
             AND member.actor_id = wake.actor_id
             AND member.status = 'active'
            WHERE wake.task_id = NEW.task_id
              AND wake.actor_kind = NEW.actor_kind
              AND wake.actor_id = NEW.actor_id
              AND role.task_id = NEW.task_id
              AND role.role = 'orchestrator'
              AND role.version = wake.task_role_version
              AND role.policy_json = wake.task_role_policy_json
              AND role.coordination_mode IS wake.coordination_mode
              AND wake.state IN ('leased', 'running', 'uncertain')
              AND attempt.state IN ('reserved', 'start_requested', 'running', 'uncertain')
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrate Execution updates require their exact wake and current TaskRole policy');
END;

DROP TRIGGER orchestrator_action_execution_guard;
CREATE TRIGGER orchestrator_action_execution_guard
BEFORE INSERT ON orchestrator_action
WHEN NOT EXISTS (
    SELECT 1 FROM execution AS execution
    JOIN orchestrator_wake_execution AS attempt
      ON attempt.execution_id = execution.id
    JOIN orchestrator_wake AS wake ON wake.id = attempt.wake_id
    JOIN task_role AS role ON role.id = wake.task_role_id
    JOIN role_membership AS member
      ON member.task_role_id = wake.task_role_id
     AND member.actor_kind = wake.actor_kind
     AND member.actor_id = wake.actor_id
     AND member.status = 'active'
    WHERE execution.id = NEW.execution_id
      AND execution.role = 'orchestrator'
      AND execution.purpose = 'orchestrate'
      AND execution.status = 'completed'
      AND role.task_id = wake.task_id
      AND role.role = 'orchestrator'
      AND role.version = wake.task_role_version
      AND role.policy_json = wake.task_role_policy_json
      AND role.coordination_mode IS wake.coordination_mode
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action requires completed Execution and unchanged current TaskRole policy');
END;
