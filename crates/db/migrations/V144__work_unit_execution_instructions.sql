-- A WorkUnit may carry bounded instructions for its assigned harness. This is
-- not an Execution form or a purpose: it is part of the prepared WorkUnit
-- scope and is snapshotted into the automatic Execution summary at admission.
ALTER TABLE work_unit ADD COLUMN execution_instructions TEXT
    CHECK (execution_instructions IS NULL OR length(execution_instructions) <= 65536);

CREATE TRIGGER work_unit_instructions_running_guard
BEFORE UPDATE OF execution_instructions ON work_unit
WHEN EXISTS (SELECT 1 FROM execution e WHERE e.work_unit_id = OLD.id AND e.status = 'running')
 AND NEW.execution_instructions IS NOT OLD.execution_instructions
BEGIN
    SELECT RAISE(ABORT, 'WorkUnit execution instructions cannot change while an Execution is running');
END;
