-- Preserve the WorkUnit Execution admission guard while making each rejection
-- stable and specific for Rust's DbError classifier. V132/V133 databases apply
-- this replacement without editing historical migration receipts.
DROP TRIGGER IF EXISTS work_unit_execution_admission_guard;

CREATE TRIGGER work_unit_execution_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NEW.status != 'running'
          OR NEW.work_unit_version IS NULL
          OR NEW.actor_kind IS NULL OR NEW.actor_id IS NULL
          OR NEW.purpose IS NULL
          OR (NEW.actor_kind != 'agent' AND NEW.agent_id IS NOT NULL)
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_INVALID')

        WHEN NEW.actor_kind = 'agent' AND NOT EXISTS (
            SELECT 1 FROM agent_current a
            WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_AGENT_MISSING')

        WHEN NOT EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORK_UNIT_MISSING')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.status != 'open'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORK_UNIT_NOT_OPEN')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.version != NEW.work_unit_version
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_VERSION_CONFLICT')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND w.requires_integration = 1
        ) AND (NEW.actor_kind != 'agent' OR NEW.agent_id != NEW.actor_id)
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_INVALID')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND (w.role != NEW.role OR
                   (w.assigned_actor_id IS NOT NULL AND
                    (w.assigned_actor_kind != NEW.actor_kind
                     OR w.assigned_actor_id != NEW.actor_id)))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_VERSION_CONFLICT')

        WHEN EXISTS (
            SELECT 1 FROM work_unit w
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND ((w.requires_integration = 1 AND NOT EXISTS (
                    SELECT 1 FROM workspace_scope ws
                    JOIN workspace bound_workspace ON bound_workspace.id = ws.workspace_id
                    WHERE ws.workspace_id = NEW.workspace_id
                      AND ws.task_id = NEW.task_id
                      AND ws.scope_kind = 'work_unit'
                      AND ws.work_unit_id = w.id
                      AND bound_workspace.status = 'ready'
                      AND bound_workspace.repo_id = (
                          SELECT t.repo_id FROM task t WHERE t.id = w.task_id
                      )
              )) OR (w.requires_integration = 0 AND NEW.workspace_id IS NOT NULL))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_WORKSPACE_UNAVAILABLE')

        WHEN NOT EXISTS (
            SELECT 1 FROM work_unit w
            JOIN task_role tr ON tr.task_id = w.task_id AND tr.role = w.role
            JOIN role_membership rm ON rm.task_role_id = tr.id
            WHERE w.id = NEW.work_unit_id AND w.task_id = NEW.task_id
              AND rm.actor_kind = NEW.actor_kind
              AND rm.actor_id = NEW.actor_id
              AND rm.status = 'active'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_ROLE_MEMBERSHIP_INACTIVE')

        WHEN EXISTS (
            SELECT 1 FROM execution active
            WHERE active.work_unit_id = NEW.work_unit_id
              AND active.status = 'running'
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_ALREADY_RUNNING')

        WHEN EXISTS (
            SELECT 1 FROM work_unit_dependency d
            JOIN work_unit p
              ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
            WHERE d.work_unit_id = NEW.work_unit_id AND d.task_id = NEW.task_id
              AND (p.status != 'completed' OR
                   (p.requires_integration = 1 AND NOT EXISTS (
                       SELECT 1 FROM work_unit_integration i
                       WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                         AND i.outcome = 'success'
                   )))
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_DEPENDENCY_UNSATISFIED')

        WHEN NEW.actor_kind = 'agent' AND EXISTS (
            SELECT 1 FROM agent_current a
            WHERE a.id = NEW.actor_id AND NEW.agent_id = a.id
              AND (SELECT COUNT(*) FROM execution e
                   WHERE e.agent_id = a.id AND e.status = 'running')
                  >= a.max_concurrent_tasks
        )
        THEN RAISE(ABORT, 'FORGE_WU_ADMISSION_AGENT_AT_CAPACITY')
    END;
END;
