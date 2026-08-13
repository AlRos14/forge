-- Agent Chat is a first-class canonical scope.  V060-V070 predate that
-- scope and encode their accepted vocabulary in SQLite CHECK constraints.
-- SQLite cannot alter a CHECK in place, so rebuild only the affected tables,
-- copying every row and restoring their indexes/triggers/FKs unchanged.

PRAGMA foreign_keys = OFF;

-- Durable event history and wake leases are consumed by the Agent Chat
-- workers as well as the legacy Account/Project/Room/Task workers.
CREATE TABLE domain_event_new (
    sequence          INTEGER PRIMARY KEY AUTOINCREMENT,
    id                TEXT NOT NULL UNIQUE,
    event_type        TEXT NOT NULL,
    entity_type       TEXT NOT NULL,
    entity_id         TEXT NOT NULL,
    actor_type        TEXT NOT NULL,
    actor_id          TEXT,
    scope_type        TEXT NOT NULL
                          CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'system', 'agent_chat'
                          )),
    scope_id          TEXT NOT NULL,
    correlation_id    TEXT NOT NULL,
    causation_id      TEXT,
    causation_depth   INTEGER NOT NULL DEFAULT 0
                          CHECK (causation_depth BETWEEN 0 AND 16),
    dedupe_key        TEXT,
    payload_json      TEXT NOT NULL DEFAULT '{}',
    created_at        TEXT NOT NULL
);

INSERT INTO domain_event_new (
    sequence, id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
)
SELECT
    sequence, id, event_type, entity_type, entity_id, actor_type, actor_id,
    scope_type, scope_id, correlation_id, causation_id, causation_depth,
    dedupe_key, payload_json, created_at
FROM domain_event;

DROP TABLE domain_event;
ALTER TABLE domain_event_new RENAME TO domain_event;

CREATE UNIQUE INDEX idx_domain_event_dedupe
    ON domain_event(dedupe_key)
    WHERE dedupe_key IS NOT NULL;
CREATE INDEX idx_domain_event_scope_sequence
    ON domain_event(scope_type, scope_id, sequence);
CREATE INDEX idx_domain_event_entity_sequence
    ON domain_event(entity_type, entity_id, sequence);
CREATE INDEX idx_domain_event_type_sequence
    ON domain_event(event_type, sequence);

CREATE TABLE agent_wake_lease_new (
    identity_id       TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type        TEXT NOT NULL
                          CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent_chat'
                          )),
    scope_id          TEXT NOT NULL,
    incident_key      TEXT NOT NULL,
    lease_owner       TEXT NOT NULL,
    leased_until      TEXT NOT NULL,
    reaction_depth    INTEGER NOT NULL DEFAULT 0 CHECK (reaction_depth BETWEEN 0 AND 8),
    updated_at        TEXT NOT NULL,
    cooldown_until    TEXT,
    last_admitted_at  TEXT,
    admission_count   INTEGER NOT NULL DEFAULT 0,
    correlation_id    TEXT,
    causation_id      TEXT,
    PRIMARY KEY (identity_id, scope_type, scope_id, incident_key)
);

INSERT INTO agent_wake_lease_new (
    identity_id, scope_type, scope_id, incident_key, lease_owner, leased_until,
    reaction_depth, updated_at, cooldown_until, last_admitted_at,
    admission_count, correlation_id, causation_id
)
SELECT
    identity_id, scope_type, scope_id, incident_key, lease_owner, leased_until,
    reaction_depth, updated_at, cooldown_until, last_admitted_at,
    admission_count, correlation_id, causation_id
FROM agent_wake_lease;

DROP TABLE agent_wake_lease;
ALTER TABLE agent_wake_lease_new RENAME TO agent_wake_lease;

-- Context scopes are the admission authority for sessions, LCM, memory, and
-- manifests.  An Agent Chat may carry its Project id as a linkage, but its
-- canonical scope id is the chat id rather than the Project id.
DROP TRIGGER agent_context_scope_identity_profile_guard;
DROP TRIGGER agent_context_scope_identity_profile_guard_update;

CREATE TABLE agent_context_scope_new (
    id                  TEXT PRIMARY KEY,
    identity_id         TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type          TEXT NOT NULL
                            CHECK (scope_type IN (
                                'account', 'project', 'room', 'task', 'agent_chat'
                            )),
    scope_id            TEXT NOT NULL,
    project_id          TEXT REFERENCES project(id) ON DELETE CASCADE,
    room_id             TEXT REFERENCES room(id) ON DELETE CASCADE,
    task_id             TEXT REFERENCES task(id) ON DELETE CASCADE,
    task_role           TEXT,
    workspace_access    TEXT NOT NULL DEFAULT 'deny'
                            CHECK (workspace_access IN ('deny', 'task_read', 'task_write')),
    authority_json      TEXT NOT NULL DEFAULT '{}',
    version             INTEGER NOT NULL DEFAULT 1,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE(identity_id, scope_type, scope_id),
    CHECK (
        (scope_type = 'account' AND project_id IS NULL AND room_id IS NULL
            AND task_id IS NULL AND workspace_access = 'deny')
        OR (scope_type = 'project' AND project_id = scope_id AND room_id IS NULL
            AND task_id IS NULL AND workspace_access = 'deny')
        OR (scope_type = 'room' AND room_id = scope_id AND task_id IS NULL
            AND workspace_access = 'deny')
        OR (scope_type = 'task' AND task_id = scope_id AND project_id IS NOT NULL
            AND task_role IS NOT NULL AND workspace_access IN ('task_read', 'task_write'))
        OR (scope_type = 'agent_chat' AND room_id IS NULL AND task_id IS NULL
            AND workspace_access = 'deny')
    )
);

INSERT INTO agent_context_scope_new (
    id, identity_id, scope_type, scope_id, project_id, room_id, task_id,
    task_role, workspace_access, authority_json, version, created_at, updated_at
)
SELECT
    id, identity_id, scope_type, scope_id, project_id, room_id, task_id,
    task_role, workspace_access, authority_json, version, created_at, updated_at
FROM agent_context_scope;

DROP TABLE agent_context_scope;
ALTER TABLE agent_context_scope_new RENAME TO agent_context_scope;

CREATE INDEX idx_agent_context_scope_scope
    ON agent_context_scope(scope_type, scope_id, identity_id);

CREATE TRIGGER agent_chat_context_scope_guard_insert
BEFORE INSERT ON agent_context_scope
WHEN NEW.scope_type = 'agent_chat'
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_chat AS chat
            WHERE chat.id = NEW.scope_id
              AND (
                  (chat.project_id IS NULL AND NEW.project_id IS NULL)
                  OR chat.project_id = NEW.project_id
              )
        ) THEN RAISE(ABORT, 'Agent Chat context scope must reference its chat')
    END;
END;

CREATE TRIGGER agent_chat_context_scope_guard_update
BEFORE UPDATE OF scope_type, scope_id, project_id ON agent_context_scope
WHEN NEW.scope_type = 'agent_chat'
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_chat AS chat
            WHERE chat.id = NEW.scope_id
              AND (
                  (chat.project_id IS NULL AND NEW.project_id IS NULL)
                  OR chat.project_id = NEW.project_id
              )
        ) THEN RAISE(ABORT, 'Agent Chat context scope must reference its chat')
    END;
END;

CREATE TRIGGER agent_context_scope_identity_profile_guard
BEFORE INSERT ON agent_session
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_context_scope AS scope
            JOIN agent_profile AS profile ON profile.id = NEW.profile_id
            WHERE scope.id = NEW.context_scope_id
              AND scope.identity_id = NEW.identity_id
              AND profile.identity_id = NEW.identity_id
              AND profile.backend_kind = NEW.backend_kind
        )
        THEN RAISE(ABORT, 'session identity, profile, and scope must match')
    END;
END;

CREATE TRIGGER agent_context_scope_identity_profile_guard_update
BEFORE UPDATE OF identity_id, profile_id, context_scope_id ON agent_session
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_context_scope AS scope
            JOIN agent_profile AS profile ON profile.id = NEW.profile_id
            WHERE scope.id = NEW.context_scope_id
              AND scope.identity_id = NEW.identity_id
              AND profile.identity_id = NEW.identity_id
              AND profile.backend_kind = NEW.backend_kind
        )
        THEN RAISE(ABORT, 'session identity, profile, and scope must match')
    END;
END;

CREATE TABLE agent_lcm_timeline_new (
    id                    TEXT PRIMARY KEY,
    identity_id           TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL
                              CHECK (scope_type IN (
                                  'account', 'project', 'room', 'task', 'agent_chat'
                              )),
    scope_id              TEXT NOT NULL,
    authorization_revision TEXT NOT NULL,
    revision              INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    UNIQUE(identity_id, scope_type, scope_id)
);

INSERT INTO agent_lcm_timeline_new (
    id, identity_id, scope_type, scope_id, authorization_revision,
    revision, created_at, updated_at
)
SELECT
    id, identity_id, scope_type, scope_id, authorization_revision,
    revision, created_at, updated_at
FROM agent_lcm_timeline;

DROP TABLE agent_lcm_timeline;
ALTER TABLE agent_lcm_timeline_new RENAME TO agent_lcm_timeline;

CREATE INDEX idx_agent_lcm_timeline_scope
    ON agent_lcm_timeline(scope_type, scope_id, identity_id);

-- Semantic memory is an external-content FTS table.  Recreate its companion
-- table/triggers as part of the rebuild so existing searchable rows and their
-- integer row ids remain intact.
DROP TRIGGER memory_item_ai;
DROP TRIGGER memory_item_ad;
DROP TRIGGER memory_item_immutable_update;
DROP TABLE memory_item_fts;
DROP INDEX idx_memory_item_project;
DROP INDEX idx_memory_item_task;
DROP INDEX idx_memory_item_room;
DROP INDEX idx_memory_item_scope;
DROP INDEX idx_memory_item_owner;
DROP INDEX idx_memory_item_authority;
DROP INDEX idx_memory_item_source_scope;
DROP INDEX idx_memory_item_created_at;

CREATE TABLE memory_item_new (
    row_id                 INTEGER PRIMARY KEY,
    id                     TEXT NOT NULL UNIQUE,
    project_id             TEXT REFERENCES project(id) ON DELETE CASCADE,
    task_id               TEXT REFERENCES task(id) ON DELETE SET NULL,
    execution_id           TEXT REFERENCES execution(id) ON DELETE SET NULL,
    room_id                TEXT REFERENCES room(id) ON DELETE SET NULL,
    scope_type             TEXT NOT NULL
                                CHECK (scope_type IN (
                                    'account', 'project', 'room', 'task', 'agent_chat'
                                )),
    scope_id               TEXT NOT NULL,
    visibility             TEXT NOT NULL DEFAULT 'project'
                                CHECK (visibility IN (
                                    'private', 'participants', 'project', 'account'
                                )),
    owner_identity_id      TEXT REFERENCES agent_identity(id) ON DELETE SET NULL,
    authority              TEXT NOT NULL DEFAULT 'observation'
                                CHECK (authority IN (
                                    'observation', 'hypothesis', 'proposal', 'decision',
                                    'verified_fact', 'procedure'
                                )),
    sensitivity            TEXT NOT NULL DEFAULT 'internal'
                                CHECK (sensitivity IN ('public', 'internal', 'restricted', 'secret')),
    retention_priority     INTEGER NOT NULL DEFAULT 0,
    provenance_json        TEXT NOT NULL DEFAULT '{}',
    publication_source_id  TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    supersedes_id          TEXT REFERENCES memory_item(id) ON DELETE SET NULL,
    valid_from             TEXT,
    valid_until            TEXT,
    source_event_id        TEXT REFERENCES domain_event(id) ON DELETE SET NULL,
    source_scope_type      TEXT,
    source_scope_id        TEXT,
    source_revision        TEXT,
    source_room_sequence   INTEGER,
    source_type            TEXT NOT NULL,
    kind                   TEXT NOT NULL,
    title                  TEXT NOT NULL,
    summary                TEXT,
    body                   TEXT NOT NULL,
    metadata_json          TEXT NOT NULL DEFAULT '{}',
    confidence             TEXT,
    quality_score          INTEGER,
    created_by_type        TEXT,
    created_by_id          TEXT,
    created_at             TEXT NOT NULL
);

INSERT INTO memory_item_new (
    row_id, id, project_id, task_id, execution_id, room_id,
    scope_type, scope_id, visibility, owner_identity_id, authority,
    sensitivity, retention_priority, provenance_json, publication_source_id,
    supersedes_id, valid_from, valid_until, source_event_id,
    source_scope_type, source_scope_id, source_revision, source_room_sequence,
    source_type, kind, title, summary, body, metadata_json, confidence,
    quality_score, created_by_type, created_by_id, created_at
)
SELECT
    row_id, id, project_id, task_id, execution_id, room_id,
    scope_type, scope_id, visibility, owner_identity_id, authority,
    sensitivity, retention_priority, provenance_json, publication_source_id,
    supersedes_id, valid_from, valid_until, source_event_id,
    source_scope_type, source_scope_id, source_revision, source_room_sequence,
    source_type, kind, title, summary, body, metadata_json, confidence,
    quality_score, created_by_type, created_by_id, created_at
FROM memory_item;

DROP TABLE memory_item;
ALTER TABLE memory_item_new RENAME TO memory_item;

CREATE VIRTUAL TABLE memory_item_fts USING fts5(
    title,
    summary,
    body,
    content='memory_item',
    content_rowid='row_id'
);

INSERT INTO memory_item_fts(rowid, title, summary, body)
SELECT row_id, title, summary, body FROM memory_item;

CREATE TRIGGER memory_item_ai
AFTER INSERT ON memory_item
BEGIN
    INSERT INTO memory_item_fts(rowid, title, summary, body)
    VALUES (new.row_id, new.title, new.summary, new.body);
END;

CREATE TRIGGER memory_item_ad
AFTER DELETE ON memory_item
BEGIN
    INSERT INTO memory_item_fts(memory_item_fts, rowid, title, summary, body)
    VALUES ('delete', old.row_id, old.title, old.summary, old.body);
END;

CREATE INDEX idx_memory_item_project ON memory_item(project_id);
CREATE INDEX idx_memory_item_task ON memory_item(task_id);
CREATE INDEX idx_memory_item_room ON memory_item(room_id);
CREATE INDEX idx_memory_item_scope
    ON memory_item(scope_type, scope_id, created_at DESC, id DESC);
CREATE INDEX idx_memory_item_owner ON memory_item(owner_identity_id, visibility);
CREATE INDEX idx_memory_item_authority
    ON memory_item(authority, retention_priority DESC, created_at DESC);
CREATE INDEX idx_memory_item_source_scope
    ON memory_item(source_scope_type, source_scope_id, source_room_sequence);
CREATE INDEX idx_memory_item_created_at ON memory_item(created_at);

CREATE TRIGGER memory_item_immutable_update
BEFORE UPDATE ON memory_item
BEGIN
    SELECT RAISE(ABORT, 'memory items are append-only');
END;

CREATE TABLE forge_memory_source_binding_new (
    id                  TEXT PRIMARY KEY,
    identity_id         TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    context_scope_id    TEXT NOT NULL REFERENCES agent_context_scope(id) ON DELETE CASCADE,
    scope_type          TEXT NOT NULL CHECK (scope_type IN (
                                'account', 'project', 'room', 'task', 'agent_chat'
                            )),
    scope_id            TEXT NOT NULL,
    account_id          TEXT,
    project_id          TEXT REFERENCES project(id) ON DELETE CASCADE,
    room_id             TEXT REFERENCES room(id) ON DELETE CASCADE,
    task_id             TEXT REFERENCES task(id) ON DELETE CASCADE,
    policy_revision     TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    UNIQUE(identity_id, context_scope_id),
    UNIQUE(identity_id, scope_type, scope_id, policy_revision)
);

INSERT INTO forge_memory_source_binding_new (
    id, identity_id, context_scope_id, scope_type, scope_id, account_id,
    project_id, room_id, task_id, policy_revision, created_at
)
SELECT
    id, identity_id, context_scope_id, scope_type, scope_id, account_id,
    project_id, room_id, task_id, policy_revision, created_at
FROM forge_memory_source_binding;

DROP TABLE forge_memory_source_binding;
ALTER TABLE forge_memory_source_binding_new RENAME TO forge_memory_source_binding;

CREATE TRIGGER forge_memory_source_binding_immutable_update
BEFORE UPDATE ON forge_memory_source_binding
BEGIN
    SELECT RAISE(ABORT, 'memory source bindings are immutable');
END;

CREATE TRIGGER forge_memory_source_binding_immutable_delete
BEFORE DELETE ON forge_memory_source_binding
BEGIN
    SELECT RAISE(ABORT, 'memory source bindings are immutable');
END;

CREATE TABLE context_manifest_new (
    id                       TEXT PRIMARY KEY,
    identity_id              TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    agent_session_id         TEXT REFERENCES agent_session(id) ON DELETE SET NULL,
    context_scope_id         TEXT NOT NULL REFERENCES agent_context_scope(id) ON DELETE CASCADE,
    scope_type               TEXT NOT NULL CHECK (scope_type IN (
                                 'account', 'project', 'room', 'task', 'agent_chat'
                             )),
    scope_id                 TEXT NOT NULL,
    policy_revision          TEXT NOT NULL,
    domain_revision          TEXT NOT NULL,
    lcm_binding_revision     TEXT,
    runtime_manifest_id      TEXT,
    runtime_manifest_fingerprint TEXT,
    combined_fingerprint     TEXT NOT NULL,
    request_fingerprint      TEXT NOT NULL,
    created_at               TEXT NOT NULL
);

INSERT INTO context_manifest_new (
    id, identity_id, agent_session_id, context_scope_id, scope_type, scope_id,
    policy_revision, domain_revision, lcm_binding_revision, runtime_manifest_id,
    runtime_manifest_fingerprint, combined_fingerprint, request_fingerprint,
    created_at
)
SELECT
    id, identity_id, agent_session_id, context_scope_id, scope_type, scope_id,
    policy_revision, domain_revision, lcm_binding_revision, runtime_manifest_id,
    runtime_manifest_fingerprint, combined_fingerprint, request_fingerprint,
    created_at
FROM context_manifest;

DROP TABLE context_manifest;
ALTER TABLE context_manifest_new RENAME TO context_manifest;

CREATE TRIGGER context_manifest_immutable_update
BEFORE UPDATE ON context_manifest
BEGIN
    SELECT RAISE(ABORT, 'context manifests are immutable');
END;

CREATE TRIGGER context_manifest_immutable_delete
BEFORE DELETE ON context_manifest
BEGIN
    SELECT RAISE(ABORT, 'context manifests are immutable');
END;

-- Coordination records share the canonical scope vocabulary.  Their child
-- tables reference these parents by stable ids and remain untouched.
CREATE TABLE agent_commitment_new (
    id                    TEXT PRIMARY KEY,
    owner_identity_id     TEXT NOT NULL REFERENCES agent_identity(id),
    scope_type            TEXT NOT NULL CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent', 'agent_chat'
                          )),
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

INSERT INTO agent_commitment_new (
    id, owner_identity_id, scope_type, scope_id, title, description, status,
    due_at, correlation_id, originating_action_id, originating_task_id,
    evidence_required, cancellation_reason, blocked_reason, completed_at,
    cancelled_at, version, created_at, updated_at
)
SELECT
    id, owner_identity_id, scope_type, scope_id, title, description, status,
    due_at, correlation_id, originating_action_id, originating_task_id,
    evidence_required, cancellation_reason, blocked_reason, completed_at,
    cancelled_at, version, created_at, updated_at
FROM agent_commitment;

DROP TABLE agent_commitment;
ALTER TABLE agent_commitment_new RENAME TO agent_commitment;

CREATE INDEX idx_agent_commitment_owner_status
    ON agent_commitment(owner_identity_id, status, due_at, updated_at DESC);
CREATE INDEX idx_agent_commitment_scope_status
    ON agent_commitment(scope_type, scope_id, status, updated_at DESC);
CREATE INDEX idx_agent_commitment_originating_task
    ON agent_commitment(originating_task_id);

CREATE TABLE agent_inbox_item_new (
    id                    TEXT PRIMARY KEY,
    recipient_identity_id TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent', 'agent_chat'
                          )),
    scope_id              TEXT NOT NULL,
    kind                  TEXT NOT NULL CHECK (kind IN (
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

INSERT INTO agent_inbox_item_new (
    id, recipient_identity_id, scope_type, scope_id, kind, status, title, body,
    payload_json, source_type, source_id, correlation_id, causation_id, dedupe_key,
    read_at, acknowledged_at, version, created_at, updated_at
)
SELECT
    id, recipient_identity_id, scope_type, scope_id, kind, status, title, body,
    payload_json, source_type, source_id, correlation_id, causation_id, dedupe_key,
    read_at, acknowledged_at, version, created_at, updated_at
FROM agent_inbox_item;

DROP TABLE agent_inbox_item;
ALTER TABLE agent_inbox_item_new RENAME TO agent_inbox_item;

CREATE INDEX idx_agent_inbox_recipient_status
    ON agent_inbox_item(recipient_identity_id, status, created_at DESC, id DESC);
CREATE INDEX idx_agent_inbox_scope
    ON agent_inbox_item(scope_type, scope_id, created_at DESC, id DESC);

CREATE TABLE agent_question_new (
    id                    TEXT PRIMARY KEY,
    recipient_identity_id TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type            TEXT NOT NULL CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent', 'agent_chat'
                          )),
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

INSERT INTO agent_question_new (
    id, recipient_identity_id, scope_type, scope_id, status, question, context_json,
    answer, asked_by_type, asked_by_id, answered_by_type, answered_by_id,
    inbox_item_id, due_at, correlation_id, version, answered_at, created_at, updated_at
)
SELECT
    id, recipient_identity_id, scope_type, scope_id, status, question, context_json,
    answer, asked_by_type, asked_by_id, answered_by_type, answered_by_id,
    inbox_item_id, due_at, correlation_id, version, answered_at, created_at, updated_at
FROM agent_question;

DROP TABLE agent_question;
ALTER TABLE agent_question_new RENAME TO agent_question;

CREATE INDEX idx_agent_question_recipient_status
    ON agent_question(recipient_identity_id, status, due_at, created_at DESC);
CREATE INDEX idx_agent_question_scope_status
    ON agent_question(scope_type, scope_id, status, created_at DESC);
CREATE UNIQUE INDEX idx_agent_question_inbox_item
    ON agent_question(inbox_item_id)
    WHERE inbox_item_id IS NOT NULL;

CREATE TABLE agent_action_new (
    id                    TEXT PRIMARY KEY,
    actor_identity_id     TEXT NOT NULL REFERENCES agent_identity(id),
    scope_type            TEXT NOT NULL CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent', 'agent_chat'
                          )),
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

INSERT INTO agent_action_new (
    id, actor_identity_id, scope_type, scope_id, operation, payload_json,
    payload_hash, dedupe_key, correlation_id, causation_id, causation_depth,
    requested_permission, policy_result, policy_reason, status, target_type,
    target_id, outcome_json, version, created_at, updated_at
)
SELECT
    id, actor_identity_id, scope_type, scope_id, operation, payload_json,
    payload_hash, dedupe_key, correlation_id, causation_id, causation_depth,
    requested_permission, policy_result, policy_reason, status, target_type,
    target_id, outcome_json, version, created_at, updated_at
FROM agent_action;

DROP TABLE agent_action;
ALTER TABLE agent_action_new RENAME TO agent_action;

CREATE INDEX idx_agent_action_scope_status
    ON agent_action(scope_type, scope_id, status, created_at DESC);
CREATE INDEX idx_agent_action_actor_status
    ON agent_action(actor_identity_id, status, created_at DESC);

CREATE TABLE agent_wake_budget_window_new (
    identity_id       TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type        TEXT NOT NULL CHECK (scope_type IN (
                              'account', 'project', 'room', 'task', 'agent_chat'
                          )),
    scope_id          TEXT NOT NULL,
    window_started_at TEXT NOT NULL,
    window_seconds    INTEGER NOT NULL DEFAULT 3600 CHECK (window_seconds > 0),
    admitted_count    INTEGER NOT NULL DEFAULT 0 CHECK (admitted_count >= 0),
    version           INTEGER NOT NULL DEFAULT 1,
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (identity_id, scope_type, scope_id)
);

INSERT INTO agent_wake_budget_window_new (
    identity_id, scope_type, scope_id, window_started_at, window_seconds,
    admitted_count, version, updated_at
)
SELECT
    identity_id, scope_type, scope_id, window_started_at, window_seconds,
    admitted_count, version, updated_at
FROM agent_wake_budget_window;

DROP TABLE agent_wake_budget_window;
ALTER TABLE agent_wake_budget_window_new RENAME TO agent_wake_budget_window;

CREATE INDEX idx_agent_wake_budget_window_updated
    ON agent_wake_budget_window(updated_at);

PRAGMA foreign_keys = ON;
