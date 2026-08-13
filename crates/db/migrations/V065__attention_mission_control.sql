-- Attention is a rebuildable projection, but its lifecycle mutations are
-- still durable and optimistic.  V060 created the first projection shape;
-- this migration adds the fields needed by Mission Control without touching
-- the authoritative Task/Agent tables.
ALTER TABLE attention_projection ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE attention_projection ADD COLUMN acknowledged_at TEXT;
ALTER TABLE attention_projection ADD COLUMN snoozed_until TEXT;
ALTER TABLE attention_projection ADD COLUMN resolved_at TEXT;
ALTER TABLE attention_projection ADD COLUMN updated_by_user_id TEXT REFERENCES user(id) ON DELETE SET NULL;
ALTER TABLE attention_projection ADD COLUMN recommended_action TEXT NOT NULL DEFAULT 'inspect';
ALTER TABLE attention_projection ADD COLUMN source_sequence INTEGER;

CREATE INDEX idx_attention_projection_visible
    ON attention_projection(scope_type, scope_id, status, snoozed_until,
                           priority DESC, occurred_at ASC, id ASC);
CREATE INDEX idx_attention_projection_source_sequence
    ON attention_projection(source_sequence);

-- A cursor alone cannot explain whether a projection is healthy.  This small
-- operational projection records bounded, non-sensitive consumer diagnostics
-- so a stale Mission Control warning does not masquerade as current work.
CREATE TABLE attention_consumer_health (
    consumer_name      TEXT PRIMARY KEY,
    last_sequence      INTEGER NOT NULL DEFAULT 0,
    last_started_at    TEXT,
    last_success_at    TEXT,
    last_error_at      TEXT,
    last_error_code    TEXT,
    last_error_message TEXT,
    lease_owner        TEXT,
    lease_until        TEXT,
    processed_events   INTEGER NOT NULL DEFAULT 0,
    version            INTEGER NOT NULL DEFAULT 1,
    updated_at         TEXT NOT NULL
);

INSERT INTO attention_consumer_health (
    consumer_name, last_sequence, processed_events, version, updated_at
)
SELECT 'attention_projection', last_sequence, 0, 1, updated_at
FROM event_consumer_cursor
WHERE consumer_name = 'attention_projection';

CREATE INDEX idx_attention_consumer_health_stale
    ON attention_consumer_health(last_success_at, lease_until);

-- Wake admission is deliberately separate from Attention lifecycle.  The
-- lease prevents two workers from analysing one incident at once; the
-- cooldown and budget window prevent a replay storm from becoming model work.
ALTER TABLE agent_wake_lease ADD COLUMN cooldown_until TEXT;
ALTER TABLE agent_wake_lease ADD COLUMN last_admitted_at TEXT;
ALTER TABLE agent_wake_lease ADD COLUMN admission_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_wake_lease ADD COLUMN correlation_id TEXT;
ALTER TABLE agent_wake_lease ADD COLUMN causation_id TEXT;

CREATE TABLE agent_wake_budget_window (
    identity_id       TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type        TEXT NOT NULL CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id          TEXT NOT NULL,
    window_started_at TEXT NOT NULL,
    window_seconds    INTEGER NOT NULL DEFAULT 3600 CHECK (window_seconds > 0),
    admitted_count    INTEGER NOT NULL DEFAULT 0 CHECK (admitted_count >= 0),
    version           INTEGER NOT NULL DEFAULT 1,
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (identity_id, scope_type, scope_id)
);

CREATE INDEX idx_agent_wake_budget_window_updated
    ON agent_wake_budget_window(updated_at);
