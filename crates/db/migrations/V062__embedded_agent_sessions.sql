CREATE TABLE credential_handle (
    id                  TEXT PRIMARY KEY,
    owner_user_id       TEXT NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL,
    label               TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'configured'
                            CHECK (status IN ('configured', 'invalid', 'revoked')),
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE TABLE protected_credential_secret (
    handle_id           TEXT PRIMARY KEY REFERENCES credential_handle(id) ON DELETE CASCADE,
    ciphertext          BLOB NOT NULL,
    nonce               BLOB NOT NULL,
    key_revision        INTEGER NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

ALTER TABLE agent_identity ADD COLUMN account_permission_ceiling TEXT NOT NULL DEFAULT '{}';
ALTER TABLE agent_identity ADD COLUMN archived_at TEXT;

CREATE TRIGGER agent_profile_credential_guard
BEFORE INSERT ON agent_profile
WHEN NEW.credential_ref IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1 FROM credential_handle WHERE id = NEW.credential_ref
        )
        THEN RAISE(ABORT, 'agent profile credential handle does not exist')
    END;
END;

CREATE TABLE agent_context_scope (
    id                  TEXT PRIMARY KEY,
    identity_id         TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    scope_type          TEXT NOT NULL
                            CHECK (scope_type IN ('account', 'project', 'room', 'task')),
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
    )
);

CREATE INDEX idx_agent_context_scope_scope
    ON agent_context_scope(scope_type, scope_id, identity_id);

CREATE TABLE agent_session (
    id                      TEXT PRIMARY KEY,
    identity_id             TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    profile_id              TEXT NOT NULL REFERENCES agent_profile(id),
    context_scope_id        TEXT NOT NULL REFERENCES agent_context_scope(id) ON DELETE CASCADE,
    backend_kind            TEXT NOT NULL CHECK (backend_kind IN ('cli', 'native')),
    runtime_session_id      TEXT,
    status                  TEXT NOT NULL DEFAULT 'ready'
                                CHECK (status IN (
                                    'starting', 'ready', 'running', 'suspended', 'degraded',
                                    'failed', 'cancelled', 'replaced'
                                )),
    capabilities_json       TEXT NOT NULL DEFAULT '{}',
    connection_status       TEXT NOT NULL DEFAULT 'unknown'
                                CHECK (connection_status IN (
                                    'unknown', 'healthy', 'degraded', 'unavailable'
                                )),
    predecessor_session_id  TEXT REFERENCES agent_session(id) ON DELETE SET NULL,
    replaced_by_session_id  TEXT REFERENCES agent_session(id) ON DELETE SET NULL,
    last_activity_at        TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_agent_session_active_scope
    ON agent_session(identity_id, context_scope_id)
    WHERE status IN ('starting', 'ready', 'running', 'degraded');
CREATE INDEX idx_agent_session_identity_status
    ON agent_session(identity_id, status, updated_at DESC);

CREATE TABLE protected_agent_session_state (
    session_id              TEXT PRIMARY KEY REFERENCES agent_session(id) ON DELETE CASCADE,
    snapshot_ciphertext     BLOB,
    snapshot_nonce          BLOB,
    checkpoint_ciphertext   BLOB,
    checkpoint_nonce        BLOB,
    checkpoint_turn_id      TEXT,
    checkpoint_revision     INTEGER,
    checkpoint_fingerprint  TEXT,
    key_revision            INTEGER NOT NULL,
    state_revision          INTEGER NOT NULL DEFAULT 1,
    updated_at              TEXT NOT NULL
);

CREATE TABLE protected_interaction (
    id                      TEXT PRIMARY KEY,
    session_id              TEXT NOT NULL REFERENCES agent_session(id) ON DELETE CASCADE,
    interaction_kind        TEXT NOT NULL,
    prompt_redacted         TEXT NOT NULL,
    response_ciphertext     BLOB,
    response_nonce          BLOB,
    status                  TEXT NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'answered', 'cancelled', 'expired')),
    expires_at              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

CREATE TABLE agent_connection_health (
    profile_id              TEXT PRIMARY KEY REFERENCES agent_profile(id) ON DELETE CASCADE,
    status                  TEXT NOT NULL
                                CHECK (status IN ('unknown', 'healthy', 'degraded', 'unavailable')),
    capability_status_json  TEXT NOT NULL DEFAULT '{}',
    checked_at              TEXT,
    error_code              TEXT,
    updated_at              TEXT NOT NULL
);

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

-- Archived identities remain addressable from historical foreign keys but are
-- removed from the operational roster composed with the selected profile.
DROP VIEW agent_current;
CREATE VIEW agent_current AS
SELECT
    identity.id,
    identity.name,
    identity.description,
    profile.id AS profile_id,
    profile.backend_kind,
    profile.executor_type,
    profile.provider,
    profile.model,
    profile.reasoning_effort,
    profile.permission_policy,
    profile.prompt_template,
    profile.capabilities_json,
    profile.tool_policy_json,
    profile.config_json,
    profile.credential_ref,
    profile.daemon_id,
    identity.max_concurrent_tasks,
    identity.heartbeat_interval_seconds,
    identity.max_missed_heartbeats,
    identity.status,
    identity.last_heartbeat_at,
    identity.is_default,
    identity.paused,
    identity.owner_id,
    identity.visibility,
    identity.version,
    identity.created_at,
    identity.updated_at
FROM agent_identity AS identity
LEFT JOIN agent_profile AS profile
    ON profile.id = identity.selected_profile_id
WHERE identity.archived_at IS NULL;
