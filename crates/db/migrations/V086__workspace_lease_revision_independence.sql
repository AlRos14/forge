-- A WorkspaceLease records the Task revision admitted at issuance for audit
-- provenance. That revision is not itself authority: harmless Task edits must
-- not invalidate a running execution. The remaining checks continue to bind
-- renewal to the live execution, principal, assignment, repository,
-- capability profile, and deterministic governance policy.

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
              AND t.repo_id = NEW.repository_binding_id
              AND e.task_id = t.id AND e.status = 'running' AND e.agent_id = NEW.assigned_principal_id
              AND ((NEW.role = 'reviewer' AND e.role = 'reviewer') OR (NEW.role = 'worker' AND e.role != 'reviewer'))
              AND (
                  (
                      EXISTS (
                          SELECT 1 FROM task_role_assignment ra
                          WHERE ra.task_id = t.id AND ra.role_name = e.role
                      )
                      AND EXISTS (
                          SELECT 1 FROM task_role_assignment ra
                          WHERE ra.task_id = t.id AND ra.role_name = e.role
                            AND ra.assignee_type = NEW.assigned_principal_type
                            AND ra.assignee_id = NEW.assigned_principal_id
                      )
                  )
                  OR (
                      NOT EXISTS (
                          SELECT 1 FROM task_role_assignment ra
                          WHERE ra.task_id = t.id AND ra.role_name = e.role
                      )
                      AND (
                          (t.assignee_type = NEW.assigned_principal_type
                           AND t.assignee_id = NEW.assigned_principal_id)
                          OR ((p.charter_status != 'charter_backed' OR p.charter_setup_required != 0)
                              AND t.assignee_type IS NULL AND t.assignee_id IS NULL)
                      )
                  )
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
