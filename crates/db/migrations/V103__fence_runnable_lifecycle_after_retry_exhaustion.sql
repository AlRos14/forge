-- Once an exact failure budget is exhausted, no caller may reopen the Task as
-- runnable. This database guard closes the race between service preflight and
-- a concurrent retry receipt commit.
CREATE TRIGGER task_lifecycle_retry_exhaustion_guard
BEFORE INSERT ON task_lifecycle_transition
WHEN NEW.to_state IN ('ready', 'active')
 AND EXISTS (
     SELECT 1 FROM task_failure_retry_receipt receipt
     WHERE receipt.task_id = NEW.task_id AND receipt.disposition = 'exhausted'
 )
BEGIN
    SELECT RAISE(ABORT, 'Task retry budget is exhausted; runnable lifecycle states are fenced');
END;
