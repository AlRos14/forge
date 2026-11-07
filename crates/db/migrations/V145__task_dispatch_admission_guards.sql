-- Close pause and per-Task concurrency races at the Execution admission
-- boundary. These guards apply to parentless automatic WorkUnit admissions;
-- explicit recovery and follow-up children retain their own parent-bound
-- authority and are not silently blocked by a Task pause.
CREATE TRIGGER task_dispatch_admission_guard
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NOT NULL
 AND NEW.parent_execution_id IS NULL
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
 AND NEW.parent_execution_id IS NULL
 AND (
     SELECT COUNT(*) FROM execution e
     WHERE e.task_id = NEW.task_id AND e.status = 'running'
 ) >= 4
BEGIN
    SELECT RAISE(ABORT, 'FORGE_WU_ADMISSION_TASK_AT_CAPACITY');
END;
