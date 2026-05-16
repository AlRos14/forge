PRAGMA foreign_keys = OFF;

CREATE TABLE agent_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    executor_type TEXT NOT NULL CHECK (executor_type IN ('shell', 'codex', 'claude_code', 'opencode')),
    model TEXT,
    reasoning_effort TEXT,
    permission_policy TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '[]',
    config_json TEXT NOT NULL DEFAULT '{}',
    daemon_id TEXT REFERENCES daemon(id) ON DELETE SET NULL,
    max_concurrent_tasks INTEGER NOT NULL DEFAULT 1,
    heartbeat_interval_seconds INTEGER NOT NULL DEFAULT 30,
    max_missed_heartbeats INTEGER NOT NULL DEFAULT 3,
    status TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle', 'busy', 'error', 'offline')),
    last_heartbeat_at TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO agent_new (
    id,
    name,
    executor_type,
    model,
    reasoning_effort,
    permission_policy,
    capabilities_json,
    config_json,
    daemon_id,
    max_concurrent_tasks,
    heartbeat_interval_seconds,
    max_missed_heartbeats,
    status,
    last_heartbeat_at,
    is_default,
    version,
    created_at,
    updated_at
)
SELECT
    agent.id,
    agent.name,
    agent_profile.executor_type,
    COALESCE(
        json_extract(agent.config_overrides_json, '$.model'),
        json_extract(agent.config_overrides_json, '$.model_id'),
        json_extract(agent_profile.config_json, '$.model')
    ),
    COALESCE(
        json_extract(agent.config_overrides_json, '$.model_reasoning_effort'),
        json_extract(agent.config_overrides_json, '$.effort'),
        json_extract(agent.config_overrides_json, '$.reasoning_effort'),
        json_extract(agent_profile.config_json, '$.model_reasoning_effort'),
        json_extract(agent_profile.config_json, '$.effort'),
        json_extract(agent_profile.config_json, '$.reasoning_effort')
    ),
    COALESCE(
        json_extract(agent.config_overrides_json, '$.permission_policy'),
        json_extract(agent_profile.config_json, '$.permission_policy')
    ),
    agent_profile.capabilities_json,
    json_patch(agent_profile.config_json, agent.config_overrides_json),
    agent.daemon_id,
    agent.max_concurrent_tasks,
    agent.heartbeat_interval_seconds,
    agent.max_missed_heartbeats,
    agent.status,
    agent.last_heartbeat_at,
    agent_profile.is_default,
    agent.version,
    agent.created_at,
    agent.updated_at
FROM agent
JOIN agent_profile ON agent.profile_id = agent_profile.id;

DROP TABLE agent;
ALTER TABLE agent_new RENAME TO agent;

CREATE TABLE project_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    settings TEXT NOT NULL DEFAULT '{}',
    workflow_definition TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO project_new (id, name, settings, workflow_definition, created_at, updated_at)
SELECT id, name, settings, workflow_definition, created_at, updated_at
FROM project;

DROP TABLE project;
ALTER TABLE project_new RENAME TO project;

DROP TABLE IF EXISTS agent_profile;

PRAGMA foreign_keys = ON;
