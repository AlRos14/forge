-- Forge-owned persistence for Agent Runtime's lossless context memory (LCM).
--
-- LCM rows are scoped by stable identity and canonical scope.  The runtime
-- adapter performs the typed invariant checks; these tables provide the
-- durable revision, immutable source rows, DAG rows, and idempotency ledger
-- needed to make those checks restart-safe.

CREATE TABLE agent_lcm_timeline (
    id                    TEXT PRIMARY KEY,
    identity_id           TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN ('account', 'project', 'room', 'task')),
    scope_id              TEXT NOT NULL,
    authorization_revision TEXT NOT NULL,
    revision              INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE(identity_id, scope_type, scope_id)
);

CREATE INDEX idx_agent_lcm_timeline_scope
    ON agent_lcm_timeline(scope_type, scope_id, identity_id);

CREATE TABLE agent_lcm_entry (
    timeline_id        TEXT NOT NULL REFERENCES agent_lcm_timeline(id) ON DELETE CASCADE,
    entry_id           TEXT NOT NULL,
    sequence           INTEGER NOT NULL CHECK (sequence >= 0),
    content_json       TEXT NOT NULL,
    content_fingerprint TEXT NOT NULL,
    source_json        TEXT NOT NULL,
    created_at         TEXT NOT NULL,
    PRIMARY KEY (timeline_id, entry_id),
    UNIQUE (timeline_id, sequence)
);

CREATE INDEX idx_agent_lcm_entry_sequence
    ON agent_lcm_entry(timeline_id, sequence);

-- Source entries are append-only.  Deletes would invalidate checkpoint and
-- summary-DAG provenance, so fail closed rather than silently compacting the
-- durable timeline.
CREATE TRIGGER agent_lcm_entry_immutable_update
BEFORE UPDATE ON agent_lcm_entry
BEGIN
    SELECT RAISE(ABORT, 'LCM entries are immutable');
END;

CREATE TRIGGER agent_lcm_entry_immutable_delete
BEFORE DELETE ON agent_lcm_entry
BEGIN
    SELECT RAISE(ABORT, 'LCM entries are immutable');
END;

CREATE TABLE agent_lcm_node (
    timeline_id          TEXT NOT NULL REFERENCES agent_lcm_timeline(id) ON DELETE CASCADE,
    node_id              TEXT NOT NULL,
    kind                 TEXT NOT NULL CHECK (kind IN ('leaf', 'condensed')),
    range_start          INTEGER NOT NULL CHECK (range_start >= 0),
    range_end            INTEGER NOT NULL CHECK (range_end >= range_start),
    edges_json           TEXT NOT NULL,
    source_fingerprint   TEXT NOT NULL,
    summary_revision     TEXT NOT NULL,
    summary              TEXT NOT NULL,
    policy_revision      TEXT NOT NULL,
    algorithm_revision   TEXT NOT NULL,
    sizer_revision       TEXT NOT NULL,
    provenance_json      TEXT NOT NULL,
    token_count          INTEGER NOT NULL CHECK (token_count >= 0),
    source_token_count   INTEGER NOT NULL CHECK (source_token_count > token_count),
    classification_json  TEXT NOT NULL,
    revision             INTEGER NOT NULL CHECK (revision >= 0),
    superseded_by        TEXT,
    operation_id         TEXT NOT NULL,
    operation_fingerprint TEXT NOT NULL,
    created_at           TEXT NOT NULL,
    PRIMARY KEY (timeline_id, node_id),
    UNIQUE (timeline_id, operation_id),
    UNIQUE (timeline_id, operation_fingerprint)
);

CREATE INDEX idx_agent_lcm_node_active
    ON agent_lcm_node(timeline_id, superseded_by, range_start, range_end, node_id);

-- Summary bodies and metadata are immutable.  The only legal lifecycle
-- update is setting superseded_by during an atomic condensation commit.
CREATE TRIGGER agent_lcm_node_immutable_update
BEFORE UPDATE OF node_id, kind, range_start, range_end, edges_json,
    source_fingerprint, summary_revision, summary, policy_revision,
    algorithm_revision, sizer_revision, provenance_json, token_count,
    source_token_count, classification_json, revision, operation_id,
    operation_fingerprint, created_at ON agent_lcm_node
BEGIN
    SELECT RAISE(ABORT, 'LCM nodes are immutable');
END;

CREATE TRIGGER agent_lcm_node_no_delete
BEFORE DELETE ON agent_lcm_node
BEGIN
    SELECT RAISE(ABORT, 'LCM nodes are immutable');
END;

CREATE TABLE agent_lcm_operation (
    timeline_id          TEXT NOT NULL REFERENCES agent_lcm_timeline(id) ON DELETE CASCADE,
    operation_id         TEXT NOT NULL,
    operation_kind       TEXT NOT NULL CHECK (operation_kind IN ('append', 'leaf', 'condensation')),
    operation_fingerprint TEXT NOT NULL,
    result_revision      INTEGER NOT NULL CHECK (result_revision >= 0),
    result_entries       INTEGER NOT NULL DEFAULT 0 CHECK (result_entries >= 0),
    result_node_id       TEXT,
    created_at            TEXT NOT NULL,
    PRIMARY KEY (timeline_id, operation_id),
    UNIQUE (timeline_id, operation_fingerprint)
);

CREATE INDEX idx_agent_lcm_operation_fingerprint
    ON agent_lcm_operation(timeline_id, operation_fingerprint);
