-- Interrupted attempts only reserve admission while their exact allocation remains current.
DROP TRIGGER task_dispatch_auto_recovery_guard;

CREATE TRIGGER task_dispatch_auto_recovery_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND NEW.parent_execution_id IS NULL
 AND EXISTS (
     SELECT 1 FROM execution recovery
     WHERE recovery.work_unit_id = NEW.work_unit_id
       AND EXISTS (
           SELECT 1 FROM work_unit w
           WHERE w.id = recovery.work_unit_id AND w.task_id = recovery.task_id
             AND w.status = 'open' AND w.version = recovery.work_unit_version
             AND w.role = recovery.role
             AND w.assigned_actor_kind = recovery.actor_kind
             AND w.assigned_actor_id = recovery.actor_id
       )
       AND recovery.status = 'cancelled'
       AND recovery.resume_policy IN ('auto', 'manual')
       AND recovery.stop_reason IN ('crash_recovery', 'agent_timeout', 'graceful_shutdown')
       AND NOT EXISTS (
           SELECT 1 FROM execution newer
           WHERE newer.work_unit_id = recovery.work_unit_id
             AND (newer.created_at > recovery.created_at
                  OR (newer.created_at = recovery.created_at
                      AND newer.id > recovery.id))
       )
       AND NOT EXISTS (
           SELECT 1 FROM execution child
           WHERE child.parent_execution_id = recovery.id
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_RECOVERY_PENDING');
END;
