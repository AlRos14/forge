PRAGMA foreign_keys = OFF;

CREATE TABLE agent_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    executor_type TEXT NOT NULL CHECK (executor_type IN ('shell', 'codex', 'claude_code', 'cursor', 'opencode', 'gemini', 'null')),
    model TEXT,
    reasoning_effort TEXT,
    permission_policy TEXT,
    prompt_template TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '[]',
    config_json TEXT NOT NULL DEFAULT '{}',
    daemon_id TEXT REFERENCES daemon(id) ON DELETE SET NULL,
    max_concurrent_tasks INTEGER NOT NULL DEFAULT 1,
    heartbeat_interval_seconds INTEGER NOT NULL DEFAULT 30,
    max_missed_heartbeats INTEGER NOT NULL DEFAULT 3,
    status TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle', 'busy', 'error', 'offline')),
    last_heartbeat_at TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    paused INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    owner_id TEXT,
    visibility TEXT NOT NULL DEFAULT 'global' CHECK (visibility IN ('global', 'account'))
);

INSERT INTO agent_new (
    id,
    name,
    description,
    executor_type,
    model,
    reasoning_effort,
    permission_policy,
    prompt_template,
    capabilities_json,
    config_json,
    daemon_id,
    max_concurrent_tasks,
    heartbeat_interval_seconds,
    max_missed_heartbeats,
    status,
    last_heartbeat_at,
    is_default,
    paused,
    version,
    created_at,
    updated_at,
    owner_id,
    visibility
)
SELECT
    id,
    name,
    description,
    executor_type,
    model,
    reasoning_effort,
    permission_policy,
    prompt_template,
    capabilities_json,
    config_json,
    daemon_id,
    max_concurrent_tasks,
    heartbeat_interval_seconds,
    max_missed_heartbeats,
    status,
    last_heartbeat_at,
    is_default,
    paused,
    version,
    created_at,
    updated_at,
    owner_id,
    visibility
FROM agent;

DROP TABLE agent;
ALTER TABLE agent_new RENAME TO agent;

PRAGMA foreign_keys = ON;
