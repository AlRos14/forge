-- Task pause and the per-Task admission ceiling apply to every WorkUnit
-- Execution, including explicit and automatic parent-bound recovery children.
-- V145 protected only parentless admissions; replace those guards in place.
DROP TRIGGER task_dispatch_admission_guard;
DROP TRIGGER task_dispatch_concurrency_guard;

CREATE TRIGGER task_dispatch_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND EXISTS (
     SELECT 1 FROM task_dispatch d
     WHERE d.task_id = NEW.task_id AND d.state IN ('paused', 'cancelled')
 )
BEGIN
    SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_TASK_PAUSED');
END;

CREATE TRIGGER task_dispatch_concurrency_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND (
     SELECT COUNT(*) FROM execution e
     WHERE e.task_id = NEW.task_id AND e.status = 'running'
 ) >= 4
BEGIN
    SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_TASK_AT_CAPACITY');
END;

-- Preflight is a read snapshot. A concurrent CrashRecovery may turn a
-- Running parent into an exact Auto-recovery candidate after preflight has
-- read the WorkUnit but before ordinary admission commits. Fence that race at
-- the durable boundary: only the parent-bound recovery path may consume it.
CREATE TRIGGER task_dispatch_auto_recovery_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND NEW.parent_execution_id IS NULL
 AND EXISTS (
     SELECT 1 FROM execution recovery
     WHERE recovery.work_unit_id = NEW.work_unit_id
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
