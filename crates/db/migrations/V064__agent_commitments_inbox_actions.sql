-- Durable coordination state belongs to the stable AgentIdentity.  None of
-- these rows are owned by a runtime profile or backend session, so replacing a
-- profile/session cannot orphan an obligation or its audit history.

CREATE TABLE agent_commitment (
    id                    TEXT PRIMARY KEY,
    owner_identity_id     TEXT NOT NULL REFERENCES agent_identity(id),
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN ('account', 'project', 'room', 'task', 'agent')),
    scope_id              TEXT NOT NULL,
    title                 TEXT NOT NULL,
    description           TEXT,
    status                TEXT NOT NULL DEFAULT 'open'
                              CHECK (status IN (
                                  'proposed', 'open', 'accepted', 'in_progress',
                                  'blocked', 'completed', 'cancelled'
                              )),
    due_at                TEXT,
    correlation_id        TEXT NOT NULL,
    originating_action_id TEXT,
    originating_task_id   TEXT REFERENCES task(id) ON DELETE SET NULL,
    evidence_required     INTEGER NOT NULL DEFAULT 1 CHECK (evidence_required IN (0, 1)),
    cancellation_reason   TEXT,
    blocked_reason        TEXT,
    completed_at          TEXT,
    cancelled_at          TEXT,
    version               INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE INDEX idx_agent_commitment_owner_status
    ON agent_commitment(owner_identity_id, status, due_at, updated_at DESC);
CREATE INDEX idx_agent_commitment_scope_status
    ON agent_commitment(scope_type, scope_id, status, updated_at DESC);
CREATE INDEX idx_agent_commitment_originating_task
    ON agent_commitment(originating_task_id);

CREATE TABLE agent_commitment_evidence (
    id                    TEXT PRIMARY KEY,
    commitment_id         TEXT NOT NULL REFERENCES agent_commitment(id) ON DELETE CASCADE,
    evidence_type         TEXT NOT NULL,
    evidence_id           TEXT NOT NULL,
    scope_type            TEXT NOT NULL,
    scope_id              TEXT NOT NULL,
    description           TEXT,
    metadata_json         TEXT NOT NULL DEFAULT '{}',
    authorized_by_type    TEXT NOT NULL,
    authorized_by_id      TEXT NOT NULL,
    dedupe_key            TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    UNIQUE(commitment_id, dedupe_key),
    UNIQUE(commitment_id, evidence_type, evidence_id)
);

CREATE INDEX idx_agent_commitment_evidence_commitment
    ON agent_commitment_evidence(commitment_id, created_at ASC, id ASC);

CREATE TABLE agent_commitment_transfer (
    id                    TEXT PRIMARY KEY,
    commitment_id         TEXT NOT NULL REFERENCES agent_commitment(id) ON DELETE CASCADE,
    from_identity_id      TEXT NOT NULL REFERENCES agent_identity(id),
    to_identity_id        TEXT NOT NULL REFERENCES agent_identity(id),
    reason                TEXT NOT NULL,
    actor_type            TEXT NOT NULL,
    actor_id              TEXT NOT NULL,
    dedupe_key            TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    UNIQUE(commitment_id, dedupe_key)
);

CREATE INDEX idx_agent_commitment_transfer_commitment
    ON agent_commitment_transfer(commitment_id, created_at ASC, id ASC);

CREATE TABLE agent_commitment_lifecycle (
    id                    TEXT PRIMARY KEY,
    commitment_id         TEXT NOT NULL REFERENCES agent_commitment(id) ON DELETE CASCADE,
    from_status           TEXT,
    to_status             TEXT NOT NULL,
    actor_type            TEXT NOT NULL,
    actor_id              TEXT NOT NULL,
    reason                TEXT,
    evidence_id           TEXT,
    dedupe_key            TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    UNIQUE(commitment_id, dedupe_key)
);

CREATE INDEX idx_agent_commitment_lifecycle_commitment
    ON agent_commitment_lifecycle(commitment_id, created_at ASC, id ASC);

CREATE TABLE agent_inbox_item (
    id                    TEXT PRIMARY KEY,
    recipient_identity_id TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN ('account', 'project', 'room', 'task', 'agent')),
    scope_id              TEXT NOT NULL,
    kind                  TEXT NOT NULL
                              CHECK (kind IN (
                                  'message', 'question', 'commitment', 'task_outcome',
                                  'action_result', 'review_request', 'system'
                              )),
    status                TEXT NOT NULL DEFAULT 'unread'
                              CHECK (status IN ('unread', 'read', 'acknowledged', 'dismissed')),
    title                 TEXT NOT NULL,
    body                  TEXT NOT NULL,
    payload_json          TEXT NOT NULL DEFAULT '{}',
    source_type           TEXT,
    source_id             TEXT,
    correlation_id        TEXT NOT NULL,
    causation_id          TEXT,
    dedupe_key            TEXT NOT NULL,
    read_at               TEXT,
    acknowledged_at       TEXT,
    version               INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE(recipient_identity_id, dedupe_key)
);

CREATE INDEX idx_agent_inbox_recipient_status
    ON agent_inbox_item(recipient_identity_id, status, created_at DESC, id DESC);
CREATE INDEX idx_agent_inbox_scope
    ON agent_inbox_item(scope_type, scope_id, created_at DESC, id DESC);

CREATE TABLE agent_question (
    id                    TEXT PRIMARY KEY,
    recipient_identity_id TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN ('account', 'project', 'room', 'task', 'agent')),
    scope_id              TEXT NOT NULL,
    status                TEXT NOT NULL DEFAULT 'open'
                              CHECK (status IN ('open', 'answered', 'dismissed', 'expired')),
    question              TEXT NOT NULL,
    context_json          TEXT NOT NULL DEFAULT '{}',
    answer                TEXT,
    asked_by_type         TEXT NOT NULL,
    asked_by_id           TEXT NOT NULL,
    answered_by_type      TEXT,
    answered_by_id        TEXT,
    inbox_item_id         TEXT REFERENCES agent_inbox_item(id) ON DELETE SET NULL,
    due_at                TEXT,
    correlation_id        TEXT NOT NULL,
    version               INTEGER NOT NULL DEFAULT 1,
    answered_at           TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

CREATE INDEX idx_agent_question_recipient_status
    ON agent_question(recipient_identity_id, status, due_at, created_at DESC);
CREATE INDEX idx_agent_question_scope_status
    ON agent_question(scope_type, scope_id, status, created_at DESC);
CREATE UNIQUE INDEX idx_agent_question_inbox_item
    ON agent_question(inbox_item_id)
    WHERE inbox_item_id IS NOT NULL;

CREATE TABLE agent_action (
    id                    TEXT PRIMARY KEY,
    actor_identity_id     TEXT NOT NULL REFERENCES agent_identity(id),
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN ('account', 'project', 'room', 'task', 'agent')),
    scope_id              TEXT NOT NULL,
    operation             TEXT NOT NULL,
    payload_json          TEXT NOT NULL DEFAULT '{}',
    payload_hash          TEXT NOT NULL,
    dedupe_key            TEXT NOT NULL,
    correlation_id        TEXT NOT NULL,
    causation_id          TEXT,
    causation_depth       INTEGER NOT NULL DEFAULT 0 CHECK (causation_depth >= 0),
    requested_permission  TEXT NOT NULL,
    policy_result         TEXT NOT NULL
                              CHECK (policy_result IN ('allowed', 'approval_required', 'denied')),
    policy_reason         TEXT,
    status                TEXT NOT NULL DEFAULT 'proposed'
                              CHECK (status IN (
                                  'proposed', 'pending_approval', 'approved', 'denied',
                                  'executing', 'executed', 'failed', 'cancelled'
                              )),
    target_type           TEXT,
    target_id             TEXT,
    outcome_json          TEXT,
    version               INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE(actor_identity_id, scope_type, scope_id, dedupe_key)
);

CREATE INDEX idx_agent_action_scope_status
    ON agent_action(scope_type, scope_id, status, created_at DESC);
CREATE INDEX idx_agent_action_actor_status
    ON agent_action(actor_identity_id, status, created_at DESC);

CREATE TABLE agent_action_approval (
    id                    TEXT PRIMARY KEY,
    action_id             TEXT NOT NULL REFERENCES agent_action(id) ON DELETE CASCADE,
    approver_identity_id  TEXT NOT NULL REFERENCES agent_identity(id),
    decision              TEXT NOT NULL CHECK (decision IN ('approved', 'denied')),
    reason                TEXT,
    created_at            TEXT NOT NULL,
    UNIQUE(action_id, approver_identity_id)
);

CREATE INDEX idx_agent_action_approval_action
    ON agent_action_approval(action_id, created_at ASC, id ASC);

CREATE TABLE agent_action_execution (
    id                    TEXT PRIMARY KEY,
    action_id             TEXT NOT NULL REFERENCES agent_action(id) ON DELETE CASCADE,
    attempt               INTEGER NOT NULL CHECK (attempt > 0),
    status                TEXT NOT NULL CHECK (status IN ('started', 'succeeded', 'failed')),
    result_json           TEXT,
    error                 TEXT,
    executed_by_type      TEXT NOT NULL,
    executed_by_id        TEXT NOT NULL,
    idempotency_key       TEXT NOT NULL,
    created_at            TEXT NOT NULL,
    completed_at          TEXT,
    UNIQUE(action_id, attempt),
    UNIQUE(action_id, idempotency_key)
);

CREATE INDEX idx_agent_action_execution_action
    ON agent_action_execution(action_id, attempt ASC);
