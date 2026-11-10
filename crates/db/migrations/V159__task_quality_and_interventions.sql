-- Structured quality requests and the single durable Task intervention
-- circuit. A quality request records routing facts; it is not an executable
-- WorkUnit. An intervention records the request and its applied effect so a
-- Human or Project Copilot retry cannot create a second effect.

CREATE TABLE task_quality_request (
    id                      TEXT PRIMARY KEY,
    task_id                 TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    source_kind             TEXT NOT NULL
                                CHECK (source_kind IN ('review_report', 'human',
                                                       'coordination_plan', 'intervention')),
    source_id               TEXT NOT NULL CHECK (length(trim(source_id)) > 0),
    category                TEXT NOT NULL
                                CHECK (category IN ('delivery_correction', 'plan_revision',
                                                    'specialist', 'human_decision')),
    target_kind             TEXT NOT NULL
                                CHECK (target_kind IN ('implementation', 'orchestrator',
                                                       'specialist', 'human')),
    target_task_role_id     TEXT REFERENCES task_role(id) ON DELETE RESTRICT,
    target_actor_kind       TEXT CHECK (target_actor_kind IN ('human', 'agent')),
    target_actor_id         TEXT CHECK (
                                target_actor_id IS NULL
                                OR length(trim(target_actor_id)) > 0
                            ),
    summary                 TEXT NOT NULL CHECK (length(trim(summary)) > 0),
    findings_json           TEXT NOT NULL DEFAULT '[]'
                                CHECK (json_valid(findings_json)
                                       AND json_type(findings_json) = 'array'),
    resolution_condition    TEXT NOT NULL CHECK (length(trim(resolution_condition)) > 0),
    state                   TEXT NOT NULL
                                CHECK (state IN ('requested', 'notified', 'work_created',
                                                 'running', 'resolved', 'requires_human',
                                                 'cancelled')),
    handoff_id              TEXT REFERENCES handoff(id) ON DELETE RESTRICT,
    work_unit_id            TEXT REFERENCES work_unit(id) ON DELETE RESTRICT,
    budget_round            INTEGER NOT NULL CHECK (budget_round >= 1),
    source_digest           TEXT NOT NULL CHECK (length(source_digest) = 64),
    request_digest          TEXT NOT NULL CHECK (length(request_digest) = 64),
    idempotency_key         TEXT NOT NULL
                                CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    created_by_kind         TEXT NOT NULL CHECK (created_by_kind IN ('human', 'agent')),
    created_by_id           TEXT NOT NULL CHECK (length(trim(created_by_id)) > 0),
    version                 INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    resolved_at             TEXT,
    UNIQUE(task_id, idempotency_key),
    CHECK ((target_actor_kind IS NULL) = (target_actor_id IS NULL)),
    CHECK (target_kind != 'human' OR target_actor_kind = 'human'),
    CHECK (category != 'human_decision' OR target_kind = 'human')
);

CREATE INDEX idx_task_quality_request_task_created
    ON task_quality_request(task_id, created_at DESC, id DESC);
CREATE INDEX idx_task_quality_request_state
    ON task_quality_request(task_id, state, created_at);
CREATE INDEX idx_task_quality_request_source
    ON task_quality_request(task_id, source_kind, source_id);

CREATE TABLE task_intervention (
    id                          TEXT PRIMARY KEY,
    task_id                     TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    kind                        TEXT NOT NULL
                                    CHECK (kind IN ('instruction', 'decision', 'pause', 'resume',
                                                    'cancel', 'revoke_authorization',
                                                    'request_replan')),
    state                       TEXT NOT NULL
                                    CHECK (state IN ('recorded', 'applied', 'requires_human',
                                                     'rejected')),
    source_kind                 TEXT NOT NULL CHECK (source_kind IN ('human', 'project_copilot')),
    source_id                   TEXT NOT NULL CHECK (length(trim(source_id)) > 0),
    payload_json                TEXT NOT NULL
                                    CHECK (json_valid(payload_json)
                                           AND json_type(payload_json) = 'object'),
    request_digest              TEXT NOT NULL CHECK (length(request_digest) = 64),
    idempotency_key             TEXT NOT NULL
                                    CHECK (length(trim(idempotency_key)) BETWEEN 1 AND 256),
    expected_task_version       INTEGER CHECK (expected_task_version IS NULL
                                               OR expected_task_version >= 1),
    expected_coordination_version INTEGER CHECK (expected_coordination_version IS NULL
                                               OR expected_coordination_version >= 1),
    proposal_id                 TEXT REFERENCES proposal(id) ON DELETE RESTRICT,
    decision_id                 TEXT REFERENCES decision(id) ON DELETE RESTRICT,
    coordination_request_id     TEXT REFERENCES task_coordination_request(id) ON DELETE RESTRICT,
    failure_code                TEXT,
    failure_message             TEXT,
    version                     INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL,
    UNIQUE(task_id, idempotency_key)
);

CREATE INDEX idx_task_intervention_task_created
    ON task_intervention(task_id, created_at DESC, id DESC);
CREATE INDEX idx_task_intervention_state
    ON task_intervention(task_id, state, kind, created_at);
