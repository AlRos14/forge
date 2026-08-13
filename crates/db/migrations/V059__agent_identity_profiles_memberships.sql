PRAGMA foreign_keys = OFF;

-- Stable product identity replaces the legacy profile-as-agent table. SQLite
-- updates every existing foreign-key reference when the table is renamed, so
-- Task, execution, Conversation, hook, and assignment history keep the same
-- identity UUIDs.
ALTER TABLE agent RENAME TO agent_identity;

CREATE TABLE agent_profile (
    id                         TEXT PRIMARY KEY,
    identity_id                TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    backend_kind               TEXT NOT NULL CHECK (backend_kind IN ('cli', 'native')),
    executor_type              TEXT NOT NULL,
    provider                   TEXT,
    model                      TEXT,
    reasoning_effort           TEXT,
    permission_policy          TEXT,
    prompt_template            TEXT,
    capabilities_json          TEXT NOT NULL DEFAULT '[]',
    tool_policy_json           TEXT NOT NULL DEFAULT '{}',
    config_json                TEXT NOT NULL DEFAULT '{}',
    credential_ref             TEXT,
    daemon_id                  TEXT REFERENCES daemon(id) ON DELETE SET NULL,
    version                    INTEGER NOT NULL DEFAULT 1,
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL
);

CREATE INDEX idx_agent_profile_identity
    ON agent_profile(identity_id, created_at DESC, id DESC);
CREATE INDEX idx_agent_profile_executor
    ON agent_profile(executor_type, backend_kind);

INSERT INTO agent_profile (
    id,
    identity_id,
    backend_kind,
    executor_type,
    provider,
    model,
    reasoning_effort,
    permission_policy,
    prompt_template,
    capabilities_json,
    tool_policy_json,
    config_json,
    credential_ref,
    daemon_id,
    version,
    created_at,
    updated_at
)
SELECT
    lower(hex(randomblob(4))) || '-' ||
        lower(hex(randomblob(2))) || '-4' ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        substr('89ab', 1 + (abs(random()) % 4), 1) ||
        lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
        lower(hex(randomblob(6))),
    id,
    'cli',
    executor_type,
    NULL,
    model,
    reasoning_effort,
    permission_policy,
    prompt_template,
    capabilities_json,
    '{}',
    config_json,
    NULL,
    daemon_id,
    1,
    created_at,
    updated_at
FROM agent_identity;

ALTER TABLE agent_identity
    ADD COLUMN selected_profile_id TEXT REFERENCES agent_profile(id) ON DELETE SET NULL;

UPDATE agent_identity
SET selected_profile_id = (
    SELECT agent_profile.id
    FROM agent_profile
    WHERE agent_profile.identity_id = agent_identity.id
    ORDER BY agent_profile.created_at ASC, agent_profile.id ASC
    LIMIT 1
);

ALTER TABLE agent_identity DROP COLUMN executor_type;
ALTER TABLE agent_identity DROP COLUMN model;
ALTER TABLE agent_identity DROP COLUMN reasoning_effort;
ALTER TABLE agent_identity DROP COLUMN permission_policy;
ALTER TABLE agent_identity DROP COLUMN prompt_template;
ALTER TABLE agent_identity DROP COLUMN capabilities_json;
ALTER TABLE agent_identity DROP COLUMN config_json;
ALTER TABLE agent_identity DROP COLUMN daemon_id;

CREATE TRIGGER agent_profile_immutable
BEFORE UPDATE ON agent_profile
BEGIN
    SELECT RAISE(ABORT, 'agent profiles are immutable');
END;

CREATE TRIGGER agent_identity_selected_profile_guard_insert
BEFORE INSERT ON agent_identity
WHEN NEW.selected_profile_id IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE id = NEW.selected_profile_id
              AND identity_id = NEW.id
        )
        THEN RAISE(ABORT, 'selected profile must belong to identity')
    END;
END;

CREATE TRIGGER agent_identity_selected_profile_guard_update
BEFORE UPDATE OF selected_profile_id ON agent_identity
WHEN NEW.selected_profile_id IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM agent_profile
            WHERE id = NEW.selected_profile_id
              AND identity_id = NEW.id
        )
        THEN RAISE(ABORT, 'selected profile must belong to identity')
    END;
END;

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
    ON profile.id = identity.selected_profile_id;

CREATE TABLE project_agent_membership (
    id                    TEXT PRIMARY KEY,
    project_id            TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    identity_id           TEXT NOT NULL REFERENCES agent_identity(id) ON DELETE CASCADE,
    role                  TEXT NOT NULL DEFAULT 'member',
    is_primary            INTEGER NOT NULL DEFAULT 0,
    state                 TEXT NOT NULL DEFAULT 'active'
                              CHECK (state IN ('active', 'paused', 'archived')),
    permission_ceiling    TEXT NOT NULL DEFAULT '{}',
    autonomy_policy_json  TEXT NOT NULL DEFAULT '{}',
    subscriptions_json    TEXT NOT NULL DEFAULT '[]',
    wake_budget           INTEGER NOT NULL DEFAULT 0 CHECK (wake_budget >= 0),
    primary_session_id    TEXT,
    created_by_user_id    TEXT NOT NULL,
    version               INTEGER NOT NULL DEFAULT 1,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL
);

INSERT INTO project_agent_membership (
    id,
    project_id,
    identity_id,
    role,
    is_primary,
    state,
    permission_ceiling,
    autonomy_policy_json,
    subscriptions_json,
    wake_budget,
    primary_session_id,
    created_by_user_id,
    version,
    created_at,
    updated_at
)
SELECT
    id,
    project_id,
    agent_id,
    'member',
    0,
    'active',
    '{}',
    '{}',
    '[]',
    0,
    NULL,
    linked_by_user_id,
    1,
    created_at,
    updated_at
FROM project_agent_link;

DROP TABLE project_agent_link;

CREATE UNIQUE INDEX idx_project_agent_membership_active_identity
    ON project_agent_membership(project_id, identity_id)
    WHERE state != 'archived';
CREATE UNIQUE INDEX idx_project_agent_membership_primary_steward
    ON project_agent_membership(project_id)
    WHERE state = 'active' AND is_primary = 1 AND role = 'steward';
CREATE INDEX idx_project_agent_membership_identity
    ON project_agent_membership(identity_id, state, project_id);

PRAGMA foreign_keys = ON;
