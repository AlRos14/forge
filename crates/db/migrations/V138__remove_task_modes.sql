PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _v138_expected_tasks (rows INTEGER NOT NULL);
CREATE TEMP TABLE _v138_cutover_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _v138_expected_tasks SELECT COUNT(*) FROM task;

-- V107's supplemental guards still map interactive leases through TaskType.
-- The replacement guards preserve current role and membership checks without
-- that retired classification.
DROP TRIGGER IF EXISTS workspace_lease_scope_guard_insert;
DROP TRIGGER IF EXISTS workspace_lease_active_renewal_guard;
DROP TRIGGER IF EXISTS workspace_lease_interactive_task_role_guard_insert;
DROP TRIGGER IF EXISTS workspace_lease_interactive_task_role_guard_renewal;

-- Upgrade currently valid active read leases to the repository-write policy.
-- Stale active leases are revoked so old authority cannot linger after cutover.
CREATE TEMP TABLE _v138_read_leases_to_upgrade (id TEXT PRIMARY KEY);
INSERT INTO _v138_read_leases_to_upgrade (id)
SELECT wl.id
FROM workspace_lease wl
JOIN task t ON t.id = wl.task_id
JOIN project p ON p.id = t.project_id
JOIN repo r ON r.id = wl.repository_binding_id
JOIN execution e ON e.id = wl.execution_id AND e.task_id = t.id
JOIN work_unit w ON w.id = wl.work_unit_id AND w.task_id = t.id
JOIN workspace_scope ws ON ws.work_unit_id = w.id AND ws.task_id = w.task_id
JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
JOIN task_role tr ON tr.task_id = t.id
JOIN role_membership rm ON rm.task_role_id = tr.id
WHERE wl.status = 'active'
  AND wl.issuing_principal_type = 'system'
  AND wl.issuing_principal_id = 'task-service-scheduler'
  AND wl.assigned_principal_type = 'agent'
  AND wl.capabilities_json = '["repository_read"]'
  AND wl.capability_profile_revision = 'forge.capability-profile/v1'
  AND t.project_id = wl.project_id
  AND t.version = wl.task_version
  AND t.deleted_at IS NULL
  AND t.repo_id = wl.repository_binding_id
  AND r.project_id = t.project_id
  AND e.status = 'running'
  AND e.actor_kind = 'agent'
  AND e.actor_id = wl.assigned_principal_id
  AND e.agent_id = wl.assigned_principal_id
  AND e.work_unit_id = w.id
  AND e.workspace_id = wl.workspace_id
  AND e.role = w.role
  AND w.status = 'open'
  AND ws.workspace_id = wl.workspace_id
  AND ws.scope_kind = 'work_unit'
  AND bound_workspace.repo_id = t.repo_id
  AND bound_workspace.status = 'ready'
  AND wl.role = CASE trim(e.role) WHEN 'reviewer' THEN 'reviewer' ELSE 'worker' END
  AND rm.actor_kind = e.actor_kind
  AND rm.actor_id = e.actor_id
  AND rm.status = 'active'
  AND tr.role = w.role
  AND tr.role = CASE lower(trim(e.role))
      WHEN 'coder' THEN 'implementer'
      WHEN 'worker' THEN 'implementer'
      WHEN 'assignee' THEN 'implementer'
      WHEN 'executor' THEN 'implementer'
      WHEN 'interactive' THEN 'implementer'
      ELSE lower(trim(e.role))
  END
  AND EXISTS (
      SELECT 1 FROM agent_current a
      WHERE a.id = e.actor_id
        AND (
            a.visibility = 'global'
            OR (
                a.visibility = 'account'
                AND a.owner_id IS NOT NULL
                AND (
                    a.owner_id = p.owner_id
                    OR EXISTS (
                        SELECT 1 FROM project_member pm
                        WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                    )
                )
            )
        )
  );

UPDATE workspace_lease
SET capabilities_json = '["repository_write"]',
    capability_profile_digest =
        'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8',
    version = version + 1,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE id IN (SELECT id FROM _v138_read_leases_to_upgrade);

UPDATE workspace_lease
SET status = 'revoked',
    revoked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    version = version + 1,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE status = 'active'
  AND capabilities_json = '["repository_read"]'
  AND id NOT IN (SELECT id FROM _v138_read_leases_to_upgrade);
DROP TABLE _v138_read_leases_to_upgrade;

DROP TRIGGER pr6_task_block_state_event;
DROP TRIGGER pr6_task_created_event;
DROP TRIGGER task_board_revision_after_delete;
DROP TRIGGER task_board_revision_after_insert;
DROP TRIGGER task_board_revision_after_update;
DROP INDEX idx_task_parent;
DROP INDEX idx_task_parent_subtask_order;
DROP INDEX idx_task_project_automation;
DROP INDEX idx_task_repo;

ALTER TABLE task RENAME TO task_v138_old;
CREATE TABLE task (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id           TEXT REFERENCES repo(id) ON DELETE CASCADE,
    parent_task_id    TEXT REFERENCES task(id) ON DELETE SET NULL,
    title             TEXT NOT NULL,
    description       TEXT,
    is_automation     INTEGER NOT NULL DEFAULT 0,
    priority          INTEGER NOT NULL DEFAULT 0,
    board_position    REAL NOT NULL DEFAULT 0.0,
    subtask_order     INTEGER,
    merge_config      TEXT,
    metadata_json     TEXT,
    error_annotation  TEXT,
    blocked_json      TEXT,
    failed_json       TEXT,
    deleted_at        TEXT,
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

INSERT INTO task (
    id, project_id, repo_id, parent_task_id, title, description, is_automation,
    priority, board_position, subtask_order, merge_config, metadata_json,
    error_annotation, blocked_json, failed_json, deleted_at, version,
    created_at, updated_at
)
SELECT id, project_id, repo_id, parent_task_id, title, description, is_automation,
       priority, board_position, subtask_order, merge_config, metadata_json,
       error_annotation, blocked_json, failed_json, deleted_at, version,
       created_at, updated_at
FROM task_v138_old;
DROP TABLE task_v138_old;

CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, deleted_at);
CREATE INDEX idx_task_repo ON task(repo_id);

CREATE TRIGGER pr6_task_block_state_event
AFTER UPDATE OF blocked_json, failed_json ON task
WHEN NEW.deleted_at IS NULL
 AND (OLD.blocked_json IS NOT NEW.blocked_json
      OR OLD.failed_json IS NOT NEW.failed_json)
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    ) VALUES (
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        CASE WHEN NEW.failed_json IS NOT NULL THEN 'task.failed'
             WHEN NEW.blocked_json IS NOT NULL THEN 'task.blocked'
             ELSE 'task.unblocked' END,
        'task', NEW.id, 'system', NULL, 'task', NEW.id,
        NEW.id, NULL, 0, 'pr6-task-block-state:' || NEW.id || ':' || NEW.version,
        json_object(
            'task_version', NEW.version,
            'blocked', NEW.blocked_json IS NOT NULL,
            'failed', NEW.failed_json IS NOT NULL
        ),
        NEW.updated_at
    );
END;

CREATE TRIGGER pr6_task_created_event
AFTER INSERT ON task
BEGIN
    INSERT OR IGNORE INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    ) VALUES (
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'task.created', 'task', NEW.id, 'system', NULL,
        'task', NEW.id, NEW.id, NULL, 0,
        'pr6-task-created:' || NEW.id,
        json_object('task_version', NEW.version),
        NEW.created_at
    );
END;

CREATE TRIGGER task_board_revision_after_delete
AFTER DELETE ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = OLD.project_id;
END;
CREATE TRIGGER task_board_revision_after_insert
AFTER INSERT ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;
CREATE TRIGGER task_board_revision_after_update
AFTER UPDATE OF board_position, deleted_at ON task
WHEN OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;

CREATE TRIGGER workspace_lease_scope_guard_insert
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.issuing_principal_type != 'system'
          OR NEW.issuing_principal_id != 'task-service-scheduler'
        THEN RAISE(ABORT, 'Workspace lease may only be issued by the scheduler')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version
              AND t.deleted_at IS NULL
              AND t.repo_id = NEW.repository_binding_id
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  WHEN 'interactive' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND NEW.role = CASE trim(e.role)
                  WHEN 'reviewer' THEN 'reviewer'
                  ELSE 'worker'
              END
              AND EXISTS (
                  SELECT 1 FROM agent_current a
                  WHERE a.id = e.actor_id
                    AND (
                        a.visibility = 'global'
                        OR (
                            a.visibility = 'account'
                            AND a.owner_id IS NOT NULL
                            AND (
                                a.owner_id = p.owner_id
                                OR EXISTS (
                                    SELECT 1 FROM project_member pm
                                    WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                                )
                            )
                        )
                    )
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') = 'repository_write'
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
        ) THEN RAISE(ABORT, 'Workspace lease Task is stale or lacks current TaskRole/Execution authority')
    END;
END;

CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
        WHEN NOT EXISTS (
            SELECT 1
            FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN repo r ON r.id = NEW.repository_binding_id
            JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
            JOIN task_role tr ON tr.task_id = t.id
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE t.id = NEW.task_id
              AND t.project_id = NEW.project_id
              AND t.deleted_at IS NULL
              AND t.repo_id = NEW.repository_binding_id
              AND r.project_id = t.project_id
              AND e.status = 'running'
              AND e.actor_kind = 'agent'
              AND e.actor_id = NEW.assigned_principal_id
              AND e.agent_id = NEW.assigned_principal_id
              AND NEW.assigned_principal_type = 'agent'
              AND rm.actor_kind = e.actor_kind
              AND rm.actor_id = e.actor_id
              AND rm.status = 'active'
              AND tr.role = CASE lower(trim(e.role))
                  WHEN 'coder' THEN 'implementer'
                  WHEN 'worker' THEN 'implementer'
                  WHEN 'assignee' THEN 'implementer'
                  WHEN 'executor' THEN 'implementer'
                  WHEN 'interactive' THEN 'implementer'
                  ELSE lower(trim(e.role))
              END
              AND NEW.role = CASE trim(e.role)
                  WHEN 'reviewer' THEN 'reviewer'
                  ELSE 'worker'
              END
              AND EXISTS (
                  SELECT 1 FROM agent_current a
                  WHERE a.id = e.actor_id
                    AND (
                        a.visibility = 'global'
                        OR (
                            a.visibility = 'account'
                            AND a.owner_id IS NOT NULL
                            AND (
                                a.owner_id = p.owner_id
                                OR EXISTS (
                                    SELECT 1 FROM project_member pm
                                    WHERE pm.project_id = p.id AND pm.user_id = a.owner_id
                                )
                            )
                        )
                    )
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') = 'repository_write'
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
        ) THEN RAISE(ABORT, 'Workspace lease renewal lacks current TaskRole/Execution authority')
    END;
END;

INSERT INTO _v138_cutover_check SELECT
    (SELECT COUNT(*) FROM task) = (SELECT rows FROM _v138_expected_tasks);
INSERT INTO _v138_cutover_check SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('task') WHERE name IN ('task_type', 'read_only')
);
INSERT INTO _v138_cutover_check SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master
    WHERE type = 'trigger' AND (sql LIKE '%task_type%' OR sql LIKE '%repository_read%')
);
INSERT INTO _v138_cutover_check SELECT NOT EXISTS (
    SELECT 1 FROM workspace_lease
    WHERE status = 'active' AND capabilities_json = '["repository_read"]'
);
INSERT INTO _v138_cutover_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _v138_cutover_check
SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';

DROP TABLE _v138_cutover_check;
DROP TABLE _v138_expected_tasks;
INSERT INTO _migration (version, name, applied_at)
VALUES (138, 'remove_task_modes', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
