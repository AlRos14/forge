PRAGMA foreign_keys = OFF;

CREATE TABLE repo_new (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    remote_url      TEXT NOT NULL,
    local_path      TEXT,
    work_mode       TEXT NOT NULL DEFAULT 'direct_merge' CHECK(work_mode IN ('direct_merge','pull_request')),
    default_branch  TEXT NOT NULL DEFAULT 'main',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

INSERT INTO repo_new (
    id,
    project_id,
    name,
    remote_url,
    local_path,
    work_mode,
    default_branch,
    created_at,
    updated_at
)
SELECT
    id,
    project_id,
    name,
    COALESCE(remote_url, local_path),
    local_path,
    'direct_merge',
    default_branch,
    created_at,
    updated_at
FROM repo
WHERE COALESCE(remote_url, local_path) IS NOT NULL;

DROP TABLE repo;
ALTER TABLE repo_new RENAME TO repo;

CREATE INDEX idx_repo_project ON repo(project_id);

ALTER TABLE project ADD COLUMN primary_repo_id TEXT REFERENCES repo(id);

UPDATE project
SET primary_repo_id = (
    SELECT id
    FROM repo
    WHERE project_id = project.id
    LIMIT 1
)
WHERE (
    SELECT COUNT(*)
    FROM repo
    WHERE project_id = project.id
) = 1;

CREATE TABLE pr_provider_config (
    id                          TEXT PRIMARY KEY,
    repo_id                     TEXT NOT NULL REFERENCES repo(id),
    provider_type               TEXT NOT NULL,
    base_url                    TEXT,
    polling_interval_seconds    INTEGER NOT NULL DEFAULT 300,
    token_secret_ref            TEXT,
    created_at                  TEXT NOT NULL,
    updated_at                  TEXT NOT NULL
);

CREATE TABLE pr_metadata (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL UNIQUE REFERENCES task(id),
    provider_type       TEXT NOT NULL,
    provider_pr_id      TEXT,
    pr_url              TEXT,
    source_branch       TEXT NOT NULL,
    target_branch       TEXT NOT NULL,
    pr_state            TEXT NOT NULL DEFAULT 'draft',
    merge_status        TEXT NOT NULL DEFAULT 'pending',
    last_synced_at      TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

PRAGMA foreign_keys = ON;
