-- A launch may finish workspace preparation after Task cancellation commits.
-- Serialize its final DB admission against the aggregate lifecycle transition:
-- the insert/update wins first and cancellation sees it, or cancellation wins
-- first and no Running Execution can be admitted afterward.
CREATE TRIGGER execution_lifecycle_admission_guard_insert
BEFORE INSERT ON execution
WHEN NEW.status = 'running'
 AND NOT EXISTS (
     SELECT 1 FROM task_lifecycle lifecycle
     WHERE lifecycle.task_id = NEW.task_id
       AND lifecycle.state NOT IN ('done', 'cancelled', 'merging')
 )
BEGIN
    SELECT RAISE(ABORT, 'Running Execution admission requires a non-terminal Task lifecycle');
END;

CREATE TRIGGER execution_lifecycle_admission_guard_update
BEFORE UPDATE OF status ON execution
WHEN NEW.status = 'running'
 AND OLD.status != 'running'
 AND NOT EXISTS (
     SELECT 1 FROM task_lifecycle lifecycle
     WHERE lifecycle.task_id = NEW.task_id
       AND lifecycle.state NOT IN ('done', 'cancelled', 'merging')
 )
BEGIN
    SELECT RAISE(ABORT, 'Running Execution admission requires a non-terminal Task lifecycle');
END;
