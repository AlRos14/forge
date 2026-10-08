-- Preserve the canonical TaskRole mapping for interactive WorkspaceLeases.
-- `interactive` is an Execution label; its TaskRole is selected from the
-- Task type, matching the existing V107 contract. Keep every V118 Actor,
-- membership, repository, capability, and lease check unchanged.
DROP TRIGGER IF EXISTS workspace_lease_scope_guard_insert;
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
                  WHEN 'interactive' THEN CASE lower(trim(t.task_type))
                      WHEN 'planning' THEN 'planner'
                      WHEN 'review' THEN 'reviewer'
                      WHEN 'validation' THEN 'validator'
                      WHEN 'discovery' THEN 'investigator'
                      ELSE 'implementer'
                  END
                  ELSE lower(trim(e.role))
              END
              AND CASE NEW.role WHEN 'worker' THEN 'implementer'
                                ELSE lower(trim(NEW.role)) END = tr.role
              AND (
                  EXISTS (
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
              )
              AND json_valid(NEW.capabilities_json)
              AND json_array_length(NEW.capabilities_json) = 1
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  CASE WHEN lower(trim(e.role)) = 'reviewer'
                             OR e.purpose IN ('plan', 'review', 'investigate', 'validate')
                             OR t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  CASE json_extract(NEW.capabilities_json, '$[0]')
                      WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
                      WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
                      ELSE ''
                  END
        ) THEN RAISE(ABORT, 'Workspace lease Task is stale or lacks current TaskRole/Execution authority')
    END;
END;
DROP TRIGGER IF EXISTS workspace_lease_active_renewal_guard;
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
                  WHEN 'interactive' THEN CASE lower(trim(t.task_type))
                      WHEN 'planning' THEN 'planner'
                      WHEN 'review' THEN 'reviewer'
                      WHEN 'validation' THEN 'validator'
                      WHEN 'discovery' THEN 'investigator'
                      ELSE 'implementer'
                  END
                  ELSE lower(trim(e.role))
              END
              AND CASE NEW.role WHEN 'worker' THEN 'implementer'
                                ELSE lower(trim(NEW.role)) END = tr.role
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
              AND json_extract(NEW.capabilities_json, '$[0]') =
                  CASE WHEN lower(trim(e.role)) = 'reviewer'
                             OR e.purpose IN ('plan', 'review', 'investigate', 'validate')
                             OR t.task_type IN ('planning', 'discovery', 'review', 'validation')
                       THEN 'repository_read' ELSE 'repository_write' END
              AND NEW.capability_profile_revision = 'forge.capability-profile/v1'
              AND NEW.capability_profile_digest =
                  CASE json_extract(NEW.capabilities_json, '$[0]')
                      WHEN 'repository_read' THEN 'sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234'
                      WHEN 'repository_write' THEN 'sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8'
                      ELSE ''
                  END
        ) THEN RAISE(ABORT, 'Workspace lease renewal lacks current TaskRole/Execution authority')
    END;
END;
