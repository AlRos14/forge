-- Replace the legacy assignee pair with the exact target ActorRef identity.
-- Invalid or unresolved values are retained verbatim in an audit table and
-- do not become a default implementer.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE pr13_project_integration_legacy_defaults (
    integration_id TEXT PRIMARY KEY,
    default_task_state TEXT,
    default_assignee_type TEXT,
    default_assignee_id TEXT,
    captured_at TEXT NOT NULL
);

INSERT INTO pr13_project_integration_legacy_defaults (
    integration_id, default_task_state, default_assignee_type,
    default_assignee_id, captured_at
)
SELECT id, default_task_state, default_assignee_type, default_assignee_id,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_integration;

CREATE TABLE pr13_integration_default_implementer_issue (
    integration_id TEXT NOT NULL,
    source_kind TEXT,
    source_id TEXT,
    issue_kind TEXT NOT NULL,
    captured_at TEXT NOT NULL,
    PRIMARY KEY (integration_id, issue_kind)
);

INSERT INTO pr13_integration_default_implementer_issue (
    integration_id, source_kind, source_id, issue_kind, captured_at
)
SELECT i.id,
       i.default_assignee_type,
       i.default_assignee_id,
       CASE
           WHEN i.default_assignee_type IS NULL THEN 'missing_kind'
           WHEN i.default_assignee_id IS NULL THEN 'missing_actor_id'
           WHEN i.default_assignee_type NOT IN ('agent', 'user') THEN 'unsupported_kind'
           WHEN length(trim(i.default_assignee_id)) = 0
             OR i.default_assignee_id != trim(i.default_assignee_id) THEN 'invalid_actor_id'
           WHEN i.default_assignee_type = 'user'
             AND i.default_assignee_id = 'human' THEN 'reserved_human_sentinel'
           WHEN i.default_assignee_type = 'agent'
             AND NOT EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = i.default_assignee_id)
               THEN 'agent_not_found'
           WHEN i.default_assignee_type = 'user'
             AND NOT EXISTS (SELECT 1 FROM user u WHERE u.id = i.default_assignee_id)
               THEN 'human_not_found'
           ELSE 'invalid_pair'
       END,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project_integration i
WHERE (i.default_assignee_type IS NOT NULL
    OR i.default_assignee_id IS NOT NULL)
  AND NOT (
      i.default_assignee_id IS NOT NULL
      AND length(trim(i.default_assignee_id)) > 0
      AND i.default_assignee_id = trim(i.default_assignee_id)
      AND (
          (i.default_assignee_type = 'agent'
           AND EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = i.default_assignee_id))
          OR
          (i.default_assignee_type = 'user'
           AND i.default_assignee_id != 'human'
           AND EXISTS (SELECT 1 FROM user u WHERE u.id = i.default_assignee_id))
      )
  );

CREATE TRIGGER pr13_project_integration_legacy_defaults_no_insert
BEFORE INSERT ON pr13_project_integration_legacy_defaults
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration provenance is migration-only');
END;
CREATE TRIGGER pr13_project_integration_legacy_defaults_no_update
BEFORE UPDATE ON pr13_project_integration_legacy_defaults
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration provenance is immutable');
END;
CREATE TRIGGER pr13_project_integration_legacy_defaults_no_delete
BEFORE DELETE ON pr13_project_integration_legacy_defaults
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration provenance is immutable');
END;
CREATE TRIGGER pr13_integration_default_implementer_issue_no_insert
BEFORE INSERT ON pr13_integration_default_implementer_issue
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration issue provenance is migration-only');
END;
CREATE TRIGGER pr13_integration_default_implementer_issue_no_update
BEFORE UPDATE ON pr13_integration_default_implementer_issue
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration issue provenance is immutable');
END;
CREATE TRIGGER pr13_integration_default_implementer_issue_no_delete
BEFORE DELETE ON pr13_integration_default_implementer_issue
BEGIN
    SELECT RAISE(ABORT, 'PR13 integration issue provenance is immutable');
END;

CREATE TABLE project_integration__pr13 (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL UNIQUE REFERENCES project(id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('github','gitea')),
    base_url TEXT NOT NULL,
    owner TEXT NOT NULL,
    repo TEXT NOT NULL,
    token_secret_ref TEXT NOT NULL,
    poll_interval_secs INTEGER NOT NULL DEFAULT 300,
    sync_filter TEXT NOT NULL DEFAULT '{}',
    default_implementer_kind TEXT CHECK (default_implementer_kind IN ('agent','human')),
    default_implementer_id TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_polled_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK ((default_implementer_kind IS NULL) = (default_implementer_id IS NULL))
);

INSERT INTO project_integration__pr13 (
    id, project_id, platform, base_url, owner, repo, token_secret_ref,
    poll_interval_secs, sync_filter, default_implementer_kind,
    default_implementer_id, enabled, last_polled_at, created_at, updated_at
)
SELECT i.id, i.project_id, i.platform, i.base_url, i.owner, i.repo,
       i.token_secret_ref, i.poll_interval_secs, i.sync_filter,
       CASE
           WHEN i.default_assignee_type = 'agent'
            AND i.default_assignee_id IS NOT NULL
            AND length(trim(i.default_assignee_id)) > 0
            AND i.default_assignee_id = trim(i.default_assignee_id)
            AND EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = i.default_assignee_id)
               THEN 'agent'
           WHEN i.default_assignee_type = 'user'
            AND i.default_assignee_id IS NOT NULL
            AND length(trim(i.default_assignee_id)) > 0
            AND i.default_assignee_id = trim(i.default_assignee_id)
            AND i.default_assignee_id != 'human'
            AND EXISTS (SELECT 1 FROM user u WHERE u.id = i.default_assignee_id)
               THEN 'human'
           ELSE NULL
       END,
       CASE
           WHEN i.default_assignee_type = 'agent'
            AND i.default_assignee_id IS NOT NULL
            AND length(trim(i.default_assignee_id)) > 0
            AND i.default_assignee_id = trim(i.default_assignee_id)
            AND EXISTS (SELECT 1 FROM agent_identity a WHERE a.id = i.default_assignee_id)
               THEN i.default_assignee_id
           WHEN i.default_assignee_type = 'user'
            AND i.default_assignee_id IS NOT NULL
            AND length(trim(i.default_assignee_id)) > 0
            AND i.default_assignee_id = trim(i.default_assignee_id)
            AND i.default_assignee_id != 'human'
            AND EXISTS (SELECT 1 FROM user u WHERE u.id = i.default_assignee_id)
               THEN i.default_assignee_id
           ELSE NULL
       END,
       i.enabled, i.last_polled_at, i.created_at, i.updated_at
FROM project_integration i;

DROP TABLE project_integration;
ALTER TABLE project_integration__pr13 RENAME TO project_integration;

CREATE TEMP TABLE _pr13_integration_fk_assert (violation INTEGER CHECK (violation = 0));
INSERT INTO _pr13_integration_fk_assert
SELECT 1 FROM pragma_foreign_key_check LIMIT 1;
DROP TABLE _pr13_integration_fk_assert;
CREATE TEMP TABLE _pr13_integration_archive_assert (violation INTEGER CHECK (violation = 0));
INSERT INTO _pr13_integration_archive_assert
SELECT 1 WHERE (SELECT COUNT(*) FROM pr13_project_integration_legacy_defaults)
           != (SELECT COUNT(*) FROM project_integration);
DROP TABLE _pr13_integration_archive_assert;

INSERT INTO _migration(version, name, applied_at)
VALUES (123, 'canonicalize_integration_default_implementer', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
