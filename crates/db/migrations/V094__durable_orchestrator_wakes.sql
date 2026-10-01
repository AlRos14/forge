-- Plan PR6 adds exact, retryable event -> TaskRole -> Actor wake obligations.
-- The event consumer starts at this upgrade high-water mark so historical
-- events are not reinterpreted as new work. Future events remain replayable.

CREATE TABLE orchestrator_wake (
    id                  TEXT PRIMARY KEY,
    event_id            TEXT NOT NULL REFERENCES domain_event(id) ON DELETE RESTRICT,
    event_sequence      INTEGER NOT NULL CHECK (event_sequence >= 1),
    task_id             TEXT NOT NULL REFERENCES task(id) ON DELETE RESTRICT,
    task_role_id        TEXT NOT NULL REFERENCES task_role(id) ON DELETE RESTRICT,
    coordination_mode   TEXT CHECK (
                            coordination_mode IS NULL OR
                            coordination_mode IN ('partitioned', 'collaborative', 'independent')
                        ),
    actor_kind          TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id            TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    work_unit_id        TEXT,
    correlation_id      TEXT NOT NULL,
    causation_id        TEXT,
    causation_depth     INTEGER NOT NULL CHECK (causation_depth BETWEEN 0 AND 16),
    policy_ref          TEXT NOT NULL,
    policy_version      INTEGER NOT NULL CHECK (policy_version >= 1),
    policy_digest       TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (
                            state IN ('pending', 'leased', 'running', 'awaiting_human',
                                      'completed', 'failed', 'uncertain')
                        ),
    available_at        TEXT NOT NULL,
    lease_owner         TEXT,
    lease_until         TEXT,
    attempt_count       INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    current_attempt     INTEGER,
    last_error          TEXT,
    version             INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE(event_id, task_id, actor_kind, actor_id),
    FOREIGN KEY(work_unit_id, task_id)
        REFERENCES work_unit(id, task_id) ON DELETE RESTRICT,
    CHECK ((lease_owner IS NULL) = (lease_until IS NULL)),
    CHECK ((state = 'leased') = (lease_owner IS NOT NULL)),
    CHECK (current_attempt IS NULL OR current_attempt >= 1)
);

CREATE INDEX idx_orchestrator_wake_pending
    ON orchestrator_wake(state, available_at, event_sequence, id)
    WHERE state IN ('pending', 'leased');
CREATE INDEX idx_orchestrator_wake_task
    ON orchestrator_wake(task_id, created_at, id);

CREATE TRIGGER orchestrator_wake_authority_guard_insert
BEFORE INSERT ON orchestrator_wake
WHEN NOT EXISTS (
        SELECT 1 FROM domain_event e
        WHERE e.id = NEW.event_id AND e.sequence = NEW.event_sequence
          AND e.scope_type = 'task' AND e.scope_id = NEW.task_id
    )
  OR NOT EXISTS (
        SELECT 1 FROM task_role tr
        WHERE tr.id = NEW.task_role_id AND tr.task_id = NEW.task_id
          AND tr.role = 'orchestrator'
          AND tr.coordination_mode IS NEW.coordination_mode
    )
  OR NOT EXISTS (
        SELECT 1 FROM role_membership rm
        WHERE rm.task_role_id = NEW.task_role_id
          AND rm.actor_kind = NEW.actor_kind AND rm.actor_id = NEW.actor_id
          AND rm.status = 'active'
    )
  OR (NEW.work_unit_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM work_unit wu
        WHERE wu.id = NEW.work_unit_id AND wu.task_id = NEW.task_id
    ))
BEGIN
    SELECT RAISE(ABORT, 'orchestrator wake must bind one durable same-Task event, canonical TaskRole, active Actor, and WorkUnit');
END;

CREATE TRIGGER orchestrator_wake_identity_immutable
BEFORE UPDATE OF event_id, event_sequence, task_id, task_role_id, coordination_mode, actor_kind,
                 actor_id, work_unit_id, correlation_id, causation_id,
                 causation_depth, policy_ref, policy_version, policy_digest
ON orchestrator_wake
BEGIN
    SELECT RAISE(ABORT, 'orchestrator wake provenance and authority are immutable');
END;

CREATE TRIGGER orchestrator_wake_delete_guard
BEFORE DELETE ON orchestrator_wake
WHEN NOT EXISTS (
    SELECT 1 FROM task t
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'durable orchestrator wake history can only be removed by guarded Project teardown');
END;

CREATE TABLE orchestrator_wake_execution (
    wake_id             TEXT NOT NULL REFERENCES orchestrator_wake(id) ON DELETE RESTRICT,
    attempt_number      INTEGER NOT NULL CHECK (attempt_number >= 1),
    execution_id        TEXT NOT NULL UNIQUE,
    state               TEXT NOT NULL CHECK (
                            state IN ('reserved', 'start_requested', 'running',
                                      'completed', 'failed', 'uncertain')
                        ),
    last_error          TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY(wake_id, attempt_number)
);
CREATE INDEX idx_orchestrator_wake_execution_execution
    ON orchestrator_wake_execution(execution_id);

CREATE TRIGGER orchestrator_execution_authority_guard
BEFORE INSERT ON execution
WHEN NEW.role = 'orchestrator'
  AND (
        NEW.purpose IS NOT 'orchestrate'
        OR NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake w
            JOIN orchestrator_wake_execution x
              ON x.wake_id = w.id AND x.execution_id = NEW.id
            JOIN role_membership rm
              ON rm.task_role_id = w.task_role_id
             AND rm.actor_kind = w.actor_kind AND rm.actor_id = w.actor_id
             AND rm.status = 'active'
            WHERE w.task_id = NEW.task_id
              AND w.actor_kind = NEW.actor_kind AND w.actor_id = NEW.actor_id
              AND w.task_role_id = (
                  SELECT id FROM task_role
                  WHERE task_id = NEW.task_id AND role = 'orchestrator'
              )
              AND w.state = 'leased'
              AND x.state = 'reserved'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrate Execution requires its exact leased durable wake and reserved attempt');
END;

-- A stable, app-generated UUIDv4 result id makes each typed action replayable
-- after a crash between the collaboration transaction and wake completion.
CREATE TRIGGER orchestrator_execution_authority_guard_update
BEFORE UPDATE OF task_id, actor_kind, actor_id, role, purpose, workspace_id ON execution
WHEN NEW.role = 'orchestrator'
  AND (
        NEW.purpose IS NOT 'orchestrate'
        OR NEW.workspace_id IS NOT NULL
        OR NOT EXISTS (
            SELECT 1
            FROM orchestrator_wake w
            JOIN orchestrator_wake_execution x
              ON x.wake_id = w.id AND x.execution_id = NEW.id
            JOIN role_membership rm
              ON rm.task_role_id = w.task_role_id
             AND rm.actor_kind = w.actor_kind AND rm.actor_id = w.actor_id
             AND rm.status = 'active'
            WHERE w.task_id = NEW.task_id
              AND w.actor_kind = NEW.actor_kind AND w.actor_id = NEW.actor_id
              AND w.task_role_id = (
                  SELECT id FROM task_role
                  WHERE task_id = NEW.task_id AND role = 'orchestrator'
              )
              AND w.state IN ('leased', 'running', 'uncertain')
              AND x.state IN ('reserved', 'start_requested', 'running', 'uncertain')
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'orchestrate Execution updates require their exact durable wake and attempt');
END;

CREATE TRIGGER orchestrator_wake_execution_identity_immutable
BEFORE UPDATE OF wake_id, attempt_number, execution_id, created_at
ON orchestrator_wake_execution
BEGIN
    SELECT RAISE(ABORT, 'orchestrator Execution attempt identity is immutable');
END;

CREATE TRIGGER orchestrator_wake_execution_delete_guard
BEFORE DELETE ON orchestrator_wake_execution
WHEN NOT EXISTS (
    SELECT 1 FROM orchestrator_wake w
    JOIN task t ON t.id = w.task_id
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE w.id = OLD.wake_id
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator Execution attempt history can only be removed by guarded Project teardown');
END;

CREATE TRIGGER orchestrator_wake_current_attempt_guard
BEFORE UPDATE OF current_attempt, attempt_count ON orchestrator_wake
WHEN NEW.current_attempt IS NOT NULL
 AND (
      NEW.current_attempt > NEW.attempt_count
      OR NOT EXISTS (
          SELECT 1 FROM orchestrator_wake_execution x
          WHERE x.wake_id = NEW.id
            AND x.attempt_number = NEW.current_attempt
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'current orchestrator wake attempt must reference its durable attempt row');
END;

CREATE TABLE orchestrator_action (
    execution_id    TEXT NOT NULL REFERENCES execution(id) ON DELETE RESTRICT,
    action_index    INTEGER NOT NULL CHECK (action_index >= 0),
    action_type     TEXT NOT NULL CHECK (action_type IN ('message', 'handoff', 'work_unit', 'proposal')),
    action_digest   TEXT NOT NULL,
    result_id       TEXT NOT NULL UNIQUE,
    state           TEXT NOT NULL CHECK (state IN ('reserved', 'completed')),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY(execution_id, action_index)
);

CREATE TRIGGER orchestrator_action_execution_guard
BEFORE INSERT ON orchestrator_action
WHEN NOT EXISTS (
    SELECT 1 FROM execution e
    WHERE e.id = NEW.execution_id AND e.role = 'orchestrator'
      AND e.purpose = 'orchestrate' AND e.status = 'completed'
)
  OR NOT EXISTS (
    SELECT 1 FROM orchestrator_wake_execution x
    WHERE x.execution_id = NEW.execution_id
  )
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action requires a completed orchestrate Execution');
END;

CREATE TRIGGER orchestrator_action_identity_immutable
BEFORE UPDATE OF execution_id, action_index, action_type, action_digest, result_id, created_at
ON orchestrator_action
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action replay identity is immutable');
END;

CREATE TRIGGER orchestrator_action_completion_guard
BEFORE UPDATE OF state ON orchestrator_action
WHEN OLD.state = 'completed' AND NEW.state != 'completed'
BEGIN
    SELECT RAISE(ABORT, 'completed orchestrator action cannot be reopened');
END;

CREATE TRIGGER orchestrator_action_delete_guard
BEFORE DELETE ON orchestrator_action
WHEN NOT EXISTS (
    SELECT 1 FROM execution e
    JOIN task t ON t.id = e.task_id
    JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE e.id = OLD.execution_id
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator action history can only be removed by guarded Project teardown');
END;

CREATE TRIGGER orchestrator_wake_execution_attempt_guard
BEFORE INSERT ON orchestrator_wake_execution
WHEN NOT EXISTS (
    SELECT 1 FROM orchestrator_wake w
    WHERE w.id = NEW.wake_id AND w.state = 'leased'
)
BEGIN
    SELECT RAISE(ABORT, 'orchestrator Execution attempt requires a leased wake');
END;

-- Existing domain-event claim/lease/receipt tables remain the source-event
-- authority. Seed only this new consumer; no old consumer cursor is changed.
INSERT INTO event_consumer_cursor(consumer_name, last_sequence, version, updated_at)
SELECT 'task-orchestrator-wakes', COALESCE(MAX(sequence), 0), 1,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM domain_event
WHERE 1 = 1
ON CONFLICT(consumer_name) DO NOTHING;
