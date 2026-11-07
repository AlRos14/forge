-- Task coordination authorization, turns, inspectable plans, and atomic
-- materialization. TaskLifecycle remains the only aggregate Task state
-- authority; these records describe preparation and coordination only.
-- Existing Tasks become manual; no turn is triggered retrospectively.

CREATE TABLE task_coordination (
    task_id                     TEXT PRIMARY KEY REFERENCES task(id) ON DELETE CASCADE,
    mode                        TEXT NOT NULL DEFAULT 'manual'
                                    CHECK (mode IN ('manual', 'proposal', 'prepare_and_execute')),
    coordinator_task_role_id    TEXT REFERENCES task_role(id) ON DELETE RESTRICT,
    coordinator_actor_kind      TEXT CHECK (coordinator_actor_kind IN ('human', 'agent')),
    coordinator_actor_id        TEXT CHECK (
                                    coordinator_actor_id IS NULL
                                    OR length(trim(coordinator_actor_id)) > 0
                                ),
    authorization_version       INTEGER NOT NULL DEFAULT 1
                                    CHECK (authorization_version >= 1),
    team_snapshot_json          TEXT NOT NULL
                                    CHECK (json_valid(team_snapshot_json)
                                           AND json_type(team_snapshot_json) = 'object'),
    team_digest                 TEXT NOT NULL CHECK (length(team_digest) = 64),
    envelope_json               TEXT NOT NULL
                                    CHECK (json_valid(envelope_json)
                                           AND json_type(envelope_json) = 'object'),
    envelope_digest             TEXT NOT NULL CHECK (length(envelope_digest) = 64),
    requested_by                TEXT REFERENCES user(id) ON DELETE RESTRICT,
    turn_count                  INTEGER NOT NULL DEFAULT 0 CHECK (turn_count >= 0),
    quality_request_count       INTEGER NOT NULL DEFAULT 0
                                    CHECK (quality_request_count >= 0),
    max_turns                   INTEGER NOT NULL DEFAULT 8 CHECK (max_turns >= 1),
    max_quality_requests        INTEGER NOT NULL DEFAULT 8
                                    CHECK (max_quality_requests >= 1),
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL,
    revoked_at                  TEXT,
    revoke_reason               TEXT,
    CHECK ((coordinator_actor_kind IS NULL) = (coordinator_actor_id IS NULL)),
    CHECK (mode = 'manual' OR (
        coordinator_task_role_id IS NOT NULL
        AND coordinator_actor_kind IS NOT NULL
        AND coordinator_actor_id IS NOT NULL
    )),
    CHECK (mode != 'manual' OR coordinator_actor_kind IS NULL OR coordinator_task_role_id IS NOT NULL)
);

CREATE TABLE task_coordination_receipt (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    operation               TEXT NOT NULL CHECK (operation IN ('authorize', 'revoke')),
    idempotency_key         TEXT NOT NULL
                                CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    request_digest          TEXT NOT NULL CHECK (length(request_digest) = 64),
    authorization_version   INTEGER NOT NULL CHECK (authorization_version >= 1),
    created_at              TEXT NOT NULL,
    UNIQUE(task_id, idempotency_key)
);

CREATE INDEX idx_task_coordination_receipt_task_created
    ON task_coordination_receipt(task_id, created_at DESC, id DESC);

CREATE TRIGGER task_coordination_receipt_immutable_update
BEFORE UPDATE ON task_coordination_receipt
BEGIN
    SELECT RAISE(ABORT, 'Task coordination receipts are immutable');
END;

CREATE TRIGGER task_coordination_receipt_immutable_delete
BEFORE DELETE ON task_coordination_receipt
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task coordination receipts are immutable outside Project teardown');
END;

CREATE TABLE task_coordination_request (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    authorization_version   INTEGER NOT NULL CHECK (authorization_version >= 1),
    mode                    TEXT NOT NULL CHECK (mode IN ('proposal', 'prepare_and_execute')),
    brief_json              TEXT NOT NULL
                                CHECK (json_valid(brief_json)
                                       AND json_type(brief_json) = 'object'),
    objective_digest        TEXT NOT NULL CHECK (length(objective_digest) = 64),
    request_digest          TEXT NOT NULL CHECK (length(request_digest) = 64),
    idempotency_key         TEXT NOT NULL
                                CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    state                   TEXT NOT NULL
                                CHECK (state IN ('requested', 'running', 'plan_ready',
                                                 'awaiting_approval', 'materialized',
                                                 'failed', 'cancelled', 'uncertain')),
    wake_id                 TEXT REFERENCES orchestrator_wake(id) ON DELETE RESTRICT,
    execution_id            TEXT REFERENCES execution(id) ON DELETE RESTRICT,
    plan_artifact_id        TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    failure_code            TEXT,
    failure_message         TEXT,
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    completed_at            TEXT,
    UNIQUE(task_id, idempotency_key)
);

CREATE INDEX idx_task_coordination_request_task_created
    ON task_coordination_request(task_id, created_at DESC, id DESC);
CREATE INDEX idx_task_coordination_request_wake
    ON task_coordination_request(wake_id);
CREATE INDEX idx_task_coordination_request_execution
    ON task_coordination_request(execution_id);
CREATE INDEX idx_task_coordination_request_state
    ON task_coordination_request(task_id, state, created_at);

CREATE TABLE task_coordination_plan (
    artifact_id             TEXT PRIMARY KEY REFERENCES artifact(id) ON DELETE RESTRICT,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    request_id              TEXT NOT NULL
                                REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    authorization_version   INTEGER NOT NULL CHECK (authorization_version >= 1),
    plan_digest             TEXT NOT NULL CHECK (length(plan_digest) = 64),
    state                   TEXT NOT NULL
                                CHECK (state IN ('proposed', 'approved', 'rejected',
                                                 'superseded', 'materialized')),
    proposal_id             TEXT REFERENCES proposal(id) ON DELETE RESTRICT,
    decision_id             TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    UNIQUE(artifact_id, task_id),
    UNIQUE(request_id, artifact_id)
);

CREATE INDEX idx_task_coordination_plan_request
    ON task_coordination_plan(request_id, created_at, artifact_id);
CREATE INDEX idx_task_coordination_plan_task_state
    ON task_coordination_plan(task_id, state, created_at);

CREATE TABLE task_coordination_materialization (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    request_id              TEXT NOT NULL
                                REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    plan_artifact_id        TEXT NOT NULL
                                REFERENCES task_coordination_plan(artifact_id) ON DELETE RESTRICT,
    plan_digest             TEXT NOT NULL CHECK (length(plan_digest) = 64),
    authorization_version   INTEGER NOT NULL CHECK (authorization_version >= 1),
    task_version_before     INTEGER NOT NULL CHECK (task_version_before >= 1),
    task_version_after      INTEGER NOT NULL CHECK (task_version_after >= 1),
    work_unit_ids_json      TEXT NOT NULL
                                CHECK (json_valid(work_unit_ids_json)
                                       AND json_type(work_unit_ids_json) = 'array'),
    receipt_digest          TEXT NOT NULL CHECK (length(receipt_digest) = 64),
    created_by              TEXT NOT NULL REFERENCES user(id) ON DELETE RESTRICT,
    created_at              TEXT NOT NULL,
    UNIQUE(plan_artifact_id)
);

CREATE INDEX idx_task_coordination_materialization_task
    ON task_coordination_materialization(task_id, created_at DESC, id DESC);

CREATE TRIGGER task_coordination_materialization_immutable_update
BEFORE UPDATE ON task_coordination_materialization
BEGIN
    SELECT RAISE(ABORT, 'Task coordination materialization receipts are immutable');
END;

CREATE TRIGGER task_coordination_materialization_immutable_delete
BEFORE DELETE ON task_coordination_materialization
WHEN NOT EXISTS (
    SELECT 1 FROM task t JOIN project_deletion_guard g ON g.project_id = t.project_id
    WHERE t.id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Task coordination materialization receipts are immutable outside Project teardown');
END;

INSERT INTO task_coordination (
    task_id, mode, coordinator_task_role_id, coordinator_actor_kind,
    coordinator_actor_id, authorization_version, team_snapshot_json,
    team_digest, envelope_json, envelope_digest, requested_by, turn_count,
    quality_request_count, max_turns, max_quality_requests, created_at,
    updated_at, revoked_at, revoke_reason
)
SELECT
    id, 'manual', NULL, NULL, NULL, 1,
    '{"memberships":[],"schema_version":1,"task_roles":[]}',
    '5e5cc8d4fce6282d417627249b208e767f209c3960a93a3beb0d3a438be53585',
    '{"allow_capability_changes":false,"allow_external_actions":false,"allow_integration":true,"allow_membership_changes":false,"allow_merge":false,"allow_new_task_roles":false,"allow_repository_write":true,"allowed_review_roles":["reviewer"],"max_work_units":4,"require_independent_review":true,"schema_version":1}',
    'd1cfc78bf8b23ae5418ed45700c087109de0ed8647bd99b4823ddde7b1f15051',
    NULL, 0, 0, 8, 8, created_at, updated_at, NULL, NULL
FROM task;

CREATE TRIGGER task_coordination_insert_for_new_task
AFTER INSERT ON task
BEGIN
    INSERT INTO task_coordination (
        task_id, mode, coordinator_task_role_id, coordinator_actor_kind,
        coordinator_actor_id, authorization_version, team_snapshot_json,
        team_digest, envelope_json, envelope_digest, requested_by, turn_count,
        quality_request_count, max_turns, max_quality_requests, created_at,
        updated_at, revoked_at, revoke_reason
    ) VALUES (
        NEW.id, 'manual', NULL, NULL, NULL, 1,
        '{"memberships":[],"schema_version":1,"task_roles":[]}',
        '5e5cc8d4fce6282d417627249b208e767f209c3960a93a3beb0d3a438be53585',
        '{"allow_capability_changes":false,"allow_external_actions":false,"allow_integration":true,"allow_membership_changes":false,"allow_merge":false,"allow_new_task_roles":false,"allow_repository_write":true,"allowed_review_roles":["reviewer"],"max_work_units":4,"require_independent_review":true,"schema_version":1}',
        'd1cfc78bf8b23ae5418ed45700c087109de0ed8647bd99b4823ddde7b1f15051',
        NULL, 0, 0, 8, 8, NEW.created_at, NEW.updated_at, NULL, NULL
    );
END;
