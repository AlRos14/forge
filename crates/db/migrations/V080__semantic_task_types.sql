-- Task type describes purpose/capability. Hierarchy is represented only by
-- parent_task_id. Preserve every Task while replacing the beta-era hybrid
-- task/planning_task/sub_task vocabulary.

PRAGMA foreign_keys = OFF;

CREATE TABLE task_new (
    id                      TEXT PRIMARY KEY,
    project_id              TEXT NOT NULL REFERENCES project(id) ON DELETE CASCADE,
    repo_id                 TEXT REFERENCES repo(id) ON DELETE CASCADE,
    parent_task_id          TEXT REFERENCES task(id) ON DELETE SET NULL,
    assignee_type           TEXT CHECK (assignee_type IN ('agent', 'user')),
    assignee_id             TEXT,
    title                   TEXT NOT NULL,
    description             TEXT,
    task_type               TEXT NOT NULL DEFAULT 'implementation'
                                CHECK (task_type IN ('implementation', 'planning', 'discovery', 'review', 'validation')),
    status                  TEXT NOT NULL DEFAULT 'todo',
    is_automation           INTEGER NOT NULL DEFAULT 0,
    priority                INTEGER NOT NULL DEFAULT 0,
    board_position          REAL NOT NULL DEFAULT 0.0,
    subtask_order           INTEGER,
    task_state_config       TEXT DEFAULT '{}',
    merge_config            TEXT,
    metadata_json           TEXT,
    plan                    TEXT,
    error_annotation        TEXT,
    blocked_json            TEXT,
    failed_json             TEXT,
    entry_barrier_json      TEXT,
    review_passed_at        TEXT,
    archived_at             TEXT,
    deleted_at              TEXT,
    version                 INTEGER NOT NULL DEFAULT 1,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL,
    CHECK (
        (assignee_type IS NULL AND assignee_id IS NULL) OR
        (assignee_type = 'agent') OR
        (assignee_type = 'user' AND assignee_id IS NOT NULL)
    )
);

INSERT INTO task_new (
    id, project_id, repo_id, parent_task_id, assignee_type, assignee_id,
    title, description, task_type, status, is_automation, priority,
    board_position, subtask_order, task_state_config, merge_config,
    metadata_json, plan, error_annotation, blocked_json, failed_json,
    entry_barrier_json, review_passed_at, archived_at, deleted_at, version,
    created_at, updated_at
)
SELECT
    id, project_id, repo_id, parent_task_id, assignee_type, assignee_id,
    title, description,
    CASE task_type
        WHEN 'planning_task' THEN 'planning'
        WHEN 'discovery' THEN 'discovery'
        ELSE 'implementation'
    END,
    status, is_automation, priority, board_position, subtask_order,
    task_state_config, merge_config, metadata_json, plan, error_annotation,
    blocked_json, failed_json, entry_barrier_json, review_passed_at,
    archived_at, deleted_at, version, created_at, updated_at
FROM task;

-- Triggers introduced by the orchestration migrations reference task from
-- other tables. During the brief DROP/RENAME interval their SQL is
-- intentionally unresolved; writable_schema suppresses only that transient
-- validation and is disabled immediately after the replacement is in place.
PRAGMA writable_schema = ON;
DROP TABLE task;
ALTER TABLE task_new RENAME TO task;
PRAGMA writable_schema = OFF;

CREATE TRIGGER task_insert_requires_assignee_id
BEFORE INSERT ON task
WHEN NEW.assignee_type IS NOT NULL
 AND NEW.assignee_id IS NULL
 AND NEW.assignee_type != 'agent'
BEGIN
    SELECT RAISE(ABORT, 'task.assignee_id required when assignee_type is set');
END;

CREATE TRIGGER task_board_revision_after_insert
AFTER INSERT ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;

CREATE TRIGGER task_board_revision_after_delete
AFTER DELETE ON task
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = OLD.project_id;
END;

CREATE TRIGGER task_board_revision_after_update
AFTER UPDATE OF status, board_position, deleted_at, archived_at ON task
WHEN OLD.status IS NOT NEW.status
    OR OLD.board_position IS NOT NEW.board_position
    OR OLD.deleted_at IS NOT NEW.deleted_at
    OR OLD.archived_at IS NOT NEW.archived_at
BEGIN
    UPDATE project SET board_revision = board_revision + 1 WHERE id = NEW.project_id;
END;

CREATE INDEX idx_task_status_project ON task(status, project_id);
CREATE INDEX idx_task_parent ON task(parent_task_id);
CREATE INDEX idx_task_repo ON task(repo_id);
CREATE INDEX idx_task_assignee ON task(assignee_type, assignee_id);
CREATE INDEX idx_task_parent_subtask_order ON task(parent_task_id, subtask_order, id);
CREATE INDEX idx_task_project_archived ON task(project_id, archived_at);
CREATE INDEX idx_task_project_automation ON task(project_id, is_automation, archived_at, deleted_at);

-- Rebind the V076/V077 WorkspaceLease guards to the semantic vocabulary.
-- The checks remain fail-closed and preserve the exact Task, execution,
-- assignment, capability profile, and approved-baseline authority.
DROP TRIGGER workspace_lease_scope_guard_insert;
CREATE TRIGGER workspace_lease_scope_guard_insert
BEFORE INSERT ON workspace_lease
BEGIN
    SELECT CASE
        WHEN NEW.issuing_principal_type != 'system'
          OR NEW.issuing_principal_id != 'task-service-scheduler'
        THEN RAISE(ABORT, 'Workspace lease may only be issued by the scheduler')
        WHEN EXISTS (
            SELECT 1 FROM project_agent_binding
            WHERE project_id = NEW.project_id AND identity_id = NEW.assigned_principal_id AND state = 'active'
        ) OR EXISTS (
            SELECT 1 FROM account_main_agent_binding
            WHERE identity_id = NEW.assigned_principal_id AND state = 'active'
        ) THEN RAISE(ABORT, 'Orchestration agents cannot receive Workspace leases')
        WHEN NOT EXISTS (
            SELECT 1 FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN execution e ON e.id = NEW.execution_id
            WHERE t.id = NEW.task_id AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version AND t.repo_id = NEW.repository_binding_id
              AND e.task_id = t.id AND e.status = 'running' AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer')
                   OR (NEW.role = 'worker' AND e.role != 'reviewer'))
              AND (
                  (t.assignee_type = NEW.assigned_principal_type AND t.assignee_id = NEW.assigned_principal_id)
                  OR EXISTS (
                      SELECT 1 FROM task_role_assignment ra
                      WHERE ra.task_id = t.id AND ra.role_name = e.role
                        AND ra.assignee_type = NEW.assigned_principal_type
                        AND ra.assignee_id = NEW.assigned_principal_id
                  )
                  OR ((p.charter_status != 'charter_backed' OR p.charter_setup_required != 0)
                      AND t.assignee_type IS NULL AND t.assignee_id IS NULL)
              )
        ) THEN RAISE(ABORT, 'Workspace lease Task is cross-Project or stale')
        WHEN json_array_length(NEW.capabilities_json) != 1
        THEN RAISE(ABORT, 'Workspace lease requires exactly one capability')
        WHEN NEW.capability_profile_revision != 'forge.capability-profile/v1'
          OR NEW.capability_profile_digest != CASE json_extract(NEW.capabilities_json, '$[0]')
              WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
              WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
              WHEN 'read_only' THEN 'sha256:08fe2de40d5f9027b803131fcbe5ab3c885c044836d6e20c2e9319951d2e82f3'
              WHEN 'discovery_read' THEN 'sha256:54502cd9c50b5f43a79e75cd1abdedf5e354393ef1422e6c4932c5716c660c43'
              WHEN 'planning_read' THEN 'sha256:78316b764f1326273f129407de72a33bbcf8db210d3bdfe7154fa1384a7d366d'
              ELSE '' END
        THEN RAISE(ABORT, 'Workspace lease capability profile is invalid')
        WHEN NOT EXISTS (
            SELECT 1 FROM task t
            JOIN project p ON p.id = t.project_id
            LEFT JOIN project_task_governance g ON g.task_id = t.id AND g.project_id = p.id
            LEFT JOIN project_execution_baseline b ON b.id = g.baseline_id AND b.project_id = p.id
            LEFT JOIN project_execution_baseline_revision r ON r.id = g.baseline_revision_id AND r.baseline_id = b.id
            WHERE t.id = NEW.task_id
              AND json_extract(NEW.capabilities_json, '$[0]') = COALESCE(
                  g.capability_class,
                  CASE WHEN t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END)
              AND (
                  p.charter_status != 'charter_backed' OR p.charter_setup_required != 0
                  OR (g.runnable = 1 AND b.lifecycle = 'active' AND b.current_revision_id = r.id
                      AND r.lifecycle = 'approved' AND r.charter_revision_id = p.current_charter_revision_id
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND EXISTS (
                          SELECT 1 FROM project_execution_baseline_approval a
                          WHERE a.baseline_id = b.id AND a.revision_id = r.id
                            AND a.principal_type = 'user'
                            AND a.authorization_action = 'project.execution_baseline.approve'
                            AND a.lifecycle IN ('active', 'consumed')
                            AND a.content_digest = r.content_digest AND a.rendered_digest = r.rendered_digest
                      ))
                  OR (g.runnable = 0 AND g.baseline_id IS NULL AND g.baseline_revision_id IS NULL
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND t.task_type IN ('planning', 'discovery', 'review', 'validation')
                      AND g.capability_class IN ('repository_read', 'read_only', 'discovery_read', 'planning_read'))
              )
        ) THEN RAISE(ABORT, 'Workspace lease requires a runnable user-approved baseline Task')
    END;
END;

DROP TRIGGER workspace_lease_active_renewal_guard;
CREATE TRIGGER workspace_lease_active_renewal_guard
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
BEGIN
    SELECT CASE
        WHEN NEW.expires_at <= OLD.expires_at OR NEW.updated_at IS OLD.updated_at
        THEN RAISE(ABORT, 'Workspace lease renewal must extend expiry')
        WHEN EXISTS (
            SELECT 1 FROM project_agent_binding
            WHERE project_id = NEW.project_id AND identity_id = NEW.assigned_principal_id AND state = 'active'
        ) OR EXISTS (
            SELECT 1 FROM account_main_agent_binding
            WHERE identity_id = NEW.assigned_principal_id AND state = 'active'
        ) THEN RAISE(ABORT, 'Orchestration agents cannot receive Workspace leases')
        WHEN NOT EXISTS (
            SELECT 1 FROM task t
            JOIN project p ON p.id = t.project_id
            JOIN execution e ON e.id = NEW.execution_id
            LEFT JOIN project_task_governance g ON g.task_id = t.id AND g.project_id = p.id
            LEFT JOIN project_execution_baseline b ON b.id = g.baseline_id AND b.project_id = p.id
            LEFT JOIN project_execution_baseline_revision r ON r.id = g.baseline_revision_id AND r.baseline_id = b.id
            WHERE t.id = NEW.task_id AND t.project_id = NEW.project_id
              AND t.version = NEW.task_version AND t.repo_id = NEW.repository_binding_id
              AND e.task_id = t.id AND e.status = 'running' AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer') OR (NEW.role = 'worker' AND e.role != 'reviewer'))
              AND (
                  (t.assignee_type = NEW.assigned_principal_type AND t.assignee_id = NEW.assigned_principal_id)
                  OR EXISTS (
                      SELECT 1 FROM task_role_assignment ra
                      WHERE ra.task_id = t.id AND ra.role_name = e.role
                        AND ra.assignee_type = NEW.assigned_principal_type
                        AND ra.assignee_id = NEW.assigned_principal_id
                  )
                  OR ((p.charter_status != 'charter_backed' OR p.charter_setup_required != 0)
                      AND t.assignee_type IS NULL AND t.assignee_id IS NULL)
              )
              AND json_extract(NEW.capabilities_json, '$[0]') = COALESCE(
                  g.capability_class,
                  CASE WHEN t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END)
              AND (
                  p.charter_status != 'charter_backed' OR p.charter_setup_required != 0
                  OR (g.runnable = 1 AND b.lifecycle = 'active' AND b.current_revision_id = r.id
                      AND r.lifecycle = 'approved' AND r.charter_revision_id = p.current_charter_revision_id
                      AND EXISTS (SELECT 1 FROM project_execution_baseline_approval a
                                  WHERE a.baseline_id = b.id AND a.revision_id = r.id
                                    AND a.lifecycle IN ('active', 'consumed')
                                    AND a.content_digest = r.content_digest AND a.rendered_digest = r.rendered_digest))
                  OR (g.runnable = 0 AND g.baseline_id IS NULL AND g.baseline_revision_id IS NULL
                      AND g.charter_revision_id = p.current_charter_revision_id
                      AND t.task_type IN ('planning', 'discovery', 'review', 'validation')
                      AND g.capability_class IN ('repository_read', 'read_only', 'discovery_read', 'planning_read'))
              )
        ) THEN RAISE(ABORT, 'Workspace lease renewal authority is stale')
    END;
END;

PRAGMA foreign_keys = ON;
