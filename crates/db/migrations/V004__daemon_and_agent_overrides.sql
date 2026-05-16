DROP TABLE IF EXISTS host;

CREATE TABLE daemon (
    id TEXT PRIMARY KEY,
    machine_id TEXT NOT NULL UNIQUE,
    hostname TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    agent_version TEXT,
    labels_json TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL CHECK (status IN ('online','offline')),
    last_report_at TEXT,
    registration_token_hash TEXT,
    detected_clis_json TEXT NOT NULL DEFAULT '[]',
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

DROP TABLE IF EXISTS runtime;

CREATE TABLE runtime (
    id TEXT PRIMARY KEY,
    daemon_id TEXT NOT NULL REFERENCES daemon(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    workspace_root TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ready', 'degraded', 'offline')),
    labels_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

DROP TABLE IF EXISTS agent;

CREATE TABLE agent (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    profile_id TEXT NOT NULL REFERENCES agent_profile(id) ON DELETE CASCADE,
    daemon_id TEXT NOT NULL REFERENCES daemon(id),
    max_concurrent_tasks INTEGER NOT NULL DEFAULT 1,
    heartbeat_interval_seconds INTEGER NOT NULL DEFAULT 30,
    max_missed_heartbeats INTEGER NOT NULL DEFAULT 3,
    status TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle', 'busy', 'error', 'offline')),
    last_heartbeat_at TEXT,
    config_overrides_json TEXT NOT NULL DEFAULT '{}',
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
