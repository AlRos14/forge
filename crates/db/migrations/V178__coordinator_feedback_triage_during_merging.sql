-- Permit the workspace-free coordinator Execution that triages external PR
-- feedback while the Task remains Merging. This is narrowly scoped to an
-- unallocated Execution and a current external_feedback request; every other
-- V106 lifecycle admission rule remains unchanged.
DROP TRIGGER execution_lifecycle_admission_guard_insert;
CREATE TRIGGER execution_lifecycle_admission_guard_insert
BEFORE INSERT ON execution
WHEN NEW.status = 'running'
 AND NOT EXISTS (
     SELECT 1 FROM task_lifecycle lifecycle
     WHERE lifecycle.task_id = NEW.task_id
       AND (
           lifecycle.state NOT IN ('done', 'cancelled', 'merging')
           OR (
               lifecycle.state = 'merging'
               AND NEW.work_unit_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM task_coordination_request request
                   WHERE request.task_id = NEW.task_id
                     AND request.kind = 'external_feedback'
                     AND request.state IN ('requested', 'running', 'uncertain')
                     AND (request.execution_id = NEW.id OR request.execution_id IS NULL)
               )
           )
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Running Execution admission requires a non-terminal Task lifecycle');
END;

DROP TRIGGER execution_lifecycle_admission_guard_update;
CREATE TRIGGER execution_lifecycle_admission_guard_update
BEFORE UPDATE OF status ON execution
WHEN NEW.status = 'running'
 AND OLD.status != 'running'
 AND NOT EXISTS (
     SELECT 1 FROM task_lifecycle lifecycle
     WHERE lifecycle.task_id = NEW.task_id
       AND (
           lifecycle.state NOT IN ('done', 'cancelled', 'merging')
           OR (
               lifecycle.state = 'merging'
               AND NEW.work_unit_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM task_coordination_request request
                   WHERE request.task_id = NEW.task_id
                     AND request.kind = 'external_feedback'
                     AND request.state IN ('requested', 'running', 'uncertain')
                     AND (request.execution_id = NEW.id OR request.execution_id IS NULL)
               )
           )
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Running Execution admission requires a non-terminal Task lifecycle');
END;
