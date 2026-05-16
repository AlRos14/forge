-- Agent Profile: named executor configurations
CREATE TABLE agent_profile (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    executor_type TEXT NOT NULL,
    variant TEXT NOT NULL DEFAULT 'DEFAULT',
    config_json TEXT NOT NULL DEFAULT '{}',
    capabilities_json TEXT NOT NULL DEFAULT '[]',
    is_default INTEGER NOT NULL DEFAULT 0,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(executor_type, variant)
);

CREATE TABLE host (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    machine_id TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('online', 'offline')),
    last_seen_at TEXT,
    agent_version TEXT,
    labels_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE runtime (
    id TEXT PRIMARY KEY,
    host_id TEXT NOT NULL REFERENCES host(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    workspace_root TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ready', 'degraded', 'offline')),
    labels_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE workspace (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL UNIQUE REFERENCES task(id) ON DELETE CASCADE,
    repo_id TEXT NOT NULL REFERENCES repo(id),
    worktree_path TEXT NOT NULL,
    branch TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('creating', 'ready', 'error', 'cleaning', 'cleaned')),
    before_sha TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_workspace_task ON workspace(task_id);

-- Recreate agent table with new schema
CREATE TABLE agent_new (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    profile_id TEXT NOT NULL REFERENCES agent_profile(id) ON DELETE CASCADE,
    host_id TEXT NOT NULL REFERENCES host(id),
    max_concurrent_tasks INTEGER NOT NULL DEFAULT 1,
    heartbeat_interval_seconds INTEGER NOT NULL DEFAULT 30,
    max_missed_heartbeats INTEGER NOT NULL DEFAULT 3,
    status TEXT NOT NULL DEFAULT 'idle' CHECK (status IN ('idle', 'busy', 'error', 'offline')),
    last_heartbeat_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

DROP INDEX IF EXISTS idx_task_agent;
DROP TABLE IF EXISTS agent_skill;
DROP TABLE IF EXISTS agent;
ALTER TABLE agent_new RENAME TO agent;

ALTER TABLE execution ADD COLUMN executor_config_snapshot_json TEXT;
ALTER TABLE execution ADD COLUMN workspace_id TEXT REFERENCES workspace(id);

ALTER TABLE project ADD COLUMN default_profile_id TEXT REFERENCES agent_profile(id) ON DELETE SET NULL;
