-- A WorkUnit Workspace lifecycle transition is the durable cleanup claim.
-- Admission already requires the exact bound Workspace to remain `ready`;
-- this trigger makes `ready -> cleaning` conditional on that same durable
-- boundary having no running Execution, active WorkspaceLease, or Task-wide
-- operation that might still be preparing/integrating the WorkUnit.
CREATE TRIGGER work_unit_workspace_cleanup_lifecycle_guard
BEFORE UPDATE OF status ON workspace
WHEN EXISTS (
    SELECT 1 FROM workspace_scope ws
    WHERE ws.workspace_id = OLD.id AND ws.task_id = OLD.task_id
      AND ws.scope_kind = 'work_unit'
)
AND (
    (NEW.status = 'cleaning' AND OLD.status != 'cleaning' AND (
        OLD.status != 'ready'
        OR NOT EXISTS (
            SELECT 1 FROM workspace_scope ws
            JOIN work_unit wu ON wu.id = ws.work_unit_id AND wu.task_id = ws.task_id
            WHERE ws.workspace_id = OLD.id AND ws.task_id = OLD.task_id
              AND ws.scope_kind = 'work_unit' AND ws.work_unit_id IS NOT NULL
        )
        OR EXISTS (
            SELECT 1 FROM execution e
            WHERE e.workspace_id = OLD.id AND e.status = 'running'
        )
        OR EXISTS (
            SELECT 1 FROM workspace_lease wl
            WHERE wl.workspace_id = OLD.id AND wl.status = 'active'
        )
        OR EXISTS (
            SELECT 1 FROM task_integration_operation op
            WHERE op.task_id = OLD.task_id AND op.status = 'running'
        )
    ))
    OR (OLD.status = 'cleaning' AND NEW.status NOT IN ('cleaning', 'cleaned'))
    OR (NEW.status = 'cleaned' AND OLD.status != 'cleaning')
    OR (OLD.status = 'cleaning' AND NEW.status = 'cleaned' AND (
        NOT EXISTS (
            SELECT 1 FROM workspace_scope ws
            JOIN work_unit wu ON wu.id = ws.work_unit_id AND wu.task_id = ws.task_id
            WHERE ws.workspace_id = OLD.id AND ws.task_id = OLD.task_id
              AND ws.scope_kind = 'work_unit' AND ws.work_unit_id IS NOT NULL
        )
        OR EXISTS (
            SELECT 1 FROM execution e
            WHERE e.workspace_id = OLD.id AND e.status = 'running'
        )
        OR EXISTS (
            SELECT 1 FROM workspace_lease wl
            WHERE wl.workspace_id = OLD.id AND wl.status = 'active'
        )
    ))
)
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit Workspace cleanup lifecycle or authority is invalid');
END;

-- Keep direct lease insertion aligned with the Execution admission trigger:
-- a WorkUnit WorkspaceLease cannot be minted after cleanup has claimed it.
CREATE TRIGGER work_unit_workspace_lease_ready_guard
BEFORE INSERT ON workspace_lease
WHEN NEW.work_unit_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1 FROM workspace_scope ws
    JOIN workspace w ON w.id = ws.workspace_id AND w.task_id = ws.task_id
    WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
      AND ws.scope_kind = 'work_unit' AND ws.work_unit_id = NEW.work_unit_id
      AND w.status = 'ready'
 )
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit WorkspaceLease requires a Ready Workspace');
END;
