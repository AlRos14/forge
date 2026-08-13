CREATE TABLE domain_event (
    sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
    id                TEXT NOT NULL UNIQUE,
    event_type        TEXT NOT NULL,
    entity_type       TEXT NOT NULL,
    entity_id         TEXT NOT NULL,
    actor_type        TEXT NOT NULL,
    actor_id          TEXT,
    scope_type        TEXT NOT NULL
                          CHECK (scope_type IN ('account', 'project', 'room', 'task', 'system')),
    scope_id          TEXT NOT NULL,
    correlation_id    TEXT NOT NULL,
    causation_id      TEXT,
    causation_depth   INTEGER NOT NULL DEFAULT 0
                          CHECK (causation_depth BETWEEN 0 AND 16),
    dedupe_key        TEXT,
    payload_json      TEXT NOT NULL DEFAULT '{}',
    created_at        TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_domain_event_dedupe
    ON domain_event(dedupe_key)
    WHERE dedupe_key IS NOT NULL;
CREATE INDEX idx_domain_event_scope_sequence
    ON domain_event(scope_type, scope_id, sequence);
CREATE INDEX idx_domain_event_entity_sequence
    ON domain_event(entity_type, entity_id, sequence);
CREATE INDEX idx_domain_event_type_sequence
    ON domain_event(event_type, sequence);

CREATE TABLE event_consumer_cursor (
    consumer_name     TEXT PRIMARY KEY,
    last_sequence     INTEGER NOT NULL DEFAULT 0,
    version           INTEGER NOT NULL DEFAULT 1,
    updated_at        TEXT NOT NULL
);

CREATE TABLE event_projection_receipt (
    consumer_name     TEXT NOT NULL,
    event_id          TEXT NOT NULL REFERENCES domain_event(id) ON DELETE CASCADE,
    dedupe_key        TEXT NOT NULL,
    processed_at      TEXT NOT NULL,
    PRIMARY KEY (consumer_name, event_id),
    UNIQUE (consumer_name, dedupe_key)
);

CREATE TABLE event_processing_lease (
    consumer_name     TEXT NOT NULL,
    event_sequence    INTEGER NOT NULL REFERENCES domain_event(sequence) ON DELETE CASCADE,
    lease_owner       TEXT NOT NULL,
    leased_until      TEXT NOT NULL,
    attempts          INTEGER NOT NULL DEFAULT 1,
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (consumer_name, event_sequence)
);

CREATE TABLE agent_wake_lease (
    identity_id       TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type        TEXT NOT NULL
                          CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id          TEXT NOT NULL,
    incident_key      TEXT NOT NULL,
    lease_owner       TEXT NOT NULL,
    leased_until      TEXT NOT NULL,
    reaction_depth    INTEGER NOT NULL DEFAULT 0 CHECK (reaction_depth BETWEEN 0 AND 8),
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (identity_id, scope_type, scope_id, incident_key)
);

CREATE TABLE attention_projection (
    id                TEXT PRIMARY KEY,
    attention_type    TEXT NOT NULL,
    scope_type        TEXT NOT NULL,
    scope_id          TEXT NOT NULL,
    identity_id       TEXT REFERENCES agent_identity(id) ON DELETE CASCADE,
    source_event_id   TEXT NOT NULL REFERENCES domain_event(id) ON DELETE CASCADE,
    priority          INTEGER NOT NULL,
    status            TEXT NOT NULL DEFAULT 'open'
                          CHECK (status IN ('open', 'acknowledged', 'resolved')),
    summary           TEXT NOT NULL,
    details_json      TEXT NOT NULL DEFAULT '{}',
    dedupe_key        TEXT NOT NULL UNIQUE,
    occurred_at       TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE INDEX idx_attention_projection_status_priority
    ON attention_projection(status, priority DESC, occurred_at ASC);
CREATE INDEX idx_attention_projection_scope
    ON attention_projection(scope_type, scope_id, status);

-- Existing Task transitions are the only durable pre-ledger event history.
-- Preserve their identifiers and chronology as replayable domain events.
INSERT INTO domain_event (
    id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT
    lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
    'task.transitioned',
    'task',
    transition_log.task_id,
    'legacy',
    transition_log.triggered_by,
    'task',
    transition_log.task_id,
    transition_log.id,
    NULL,
    0,
    'migration:transition:' || transition_log.id,
    json_object(
        'transition_log_id', transition_log.id,
        'from_state', transition_log.from_state,
        'to_state', transition_log.to_state,
        'trigger_reason', transition_log.trigger_reason,
        'trigger_name', transition_log.trigger_name,
        'rejection', transition_log.rejection
    ),
    transition_log.created_at
FROM transition_log
ORDER BY transition_log.created_at ASC, transition_log.id ASC;
