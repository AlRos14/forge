CREATE TABLE project_integration (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL UNIQUE REFERENCES project(id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('github','gitea')),
    base_url TEXT NOT NULL,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    token_secret_ref TEXT NOT NULL,
    poll_interval_secs INTEGER NOT NULL DEFAULT 300,
    sync_filter TEXT NOT NULL DEFAULT '{}',
    default_task_state TEXT,
    default_assignee_type TEXT CHECK (default_assignee_type IN ('agent','user')),
    default_assignee_id TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_polled_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((default_assignee_type IS NULL) = (default_assignee_id IS NULL))
);

CREATE TABLE task_external_link (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    integration_id TEXT NOT NULL REFERENCES project_integration(id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    remote_owner TEXT NOT NULL,
    remote_repo TEXT NOT NULL,
    remote_issue_number INTEGER NOT NULL,
    remote_url TEXT NOT NULL,
    global_id TEXT NOT NULL UNIQUE,
    synced_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
