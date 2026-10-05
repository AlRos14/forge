-- `interactive` is an Execution label, not a TaskRole. Once a Task has the
-- canonical role for its operation, its interactive WorkspaceLease must be
-- held by an active Agent member of that role. Keep the old singleton fallback
-- only for Tasks that predate creation of the corresponding TaskRole.

CREATE TRIGGER workspace_lease_interactive_task_role_guard_insert
BEFORE INSERT ON workspace_lease
WHEN NEW.status = 'active'
 AND EXISTS (
     SELECT 1
     FROM task t
     JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
     WHERE t.id = NEW.task_id
       AND lower(trim(e.role)) = 'interactive'
       AND EXISTS (
           SELECT 1
           FROM task_role tr
           WHERE tr.task_id = t.id
             AND tr.role = CASE lower(trim(t.task_type))
                 WHEN 'planning' THEN 'planner'
                 WHEN 'review' THEN 'reviewer'
                 WHEN 'validation' THEN 'validator'
                 WHEN 'discovery' THEN 'investigator'
                 ELSE 'implementer'
             END
       )
       AND (
           NEW.assigned_principal_type != 'agent'
           OR NOT EXISTS (
               SELECT 1
               FROM task_role tr
               JOIN role_membership rm ON rm.task_role_id = tr.id
               WHERE tr.task_id = t.id
                 AND tr.role = CASE lower(trim(t.task_type))
                     WHEN 'planning' THEN 'planner'
                     WHEN 'review' THEN 'reviewer'
                     WHEN 'validation' THEN 'validator'
                     WHEN 'discovery' THEN 'investigator'
                     ELSE 'implementer'
                 END
                 AND rm.actor_kind = 'agent'
                 AND rm.actor_id = NEW.assigned_principal_id
                 AND rm.status = 'active'
           )
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Workspace lease interactive TaskRole membership is stale');
END;

CREATE TRIGGER workspace_lease_interactive_task_role_guard_renewal
BEFORE UPDATE ON workspace_lease
WHEN OLD.status = 'active' AND NEW.status = 'active'
 AND EXISTS (
     SELECT 1
     FROM task t
     JOIN execution e ON e.id = NEW.execution_id AND e.task_id = t.id
     WHERE t.id = NEW.task_id
       AND lower(trim(e.role)) = 'interactive'
       AND EXISTS (
           SELECT 1
           FROM task_role tr
           WHERE tr.task_id = t.id
             AND tr.role = CASE lower(trim(t.task_type))
                 WHEN 'planning' THEN 'planner'
                 WHEN 'review' THEN 'reviewer'
                 WHEN 'validation' THEN 'validator'
                 WHEN 'discovery' THEN 'investigator'
                 ELSE 'implementer'
             END
       )
       AND (
           NEW.assigned_principal_type != 'agent'
           OR NOT EXISTS (
               SELECT 1
               FROM task_role tr
               JOIN role_membership rm ON rm.task_role_id = tr.id
               WHERE tr.task_id = t.id
                 AND tr.role = CASE lower(trim(t.task_type))
                     WHEN 'planning' THEN 'planner'
                     WHEN 'review' THEN 'reviewer'
                     WHEN 'validation' THEN 'validator'
                     WHEN 'discovery' THEN 'investigator'
                     ELSE 'implementer'
                 END
                 AND rm.actor_kind = 'agent'
                 AND rm.actor_id = NEW.assigned_principal_id
                 AND rm.status = 'active'
           )
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Workspace lease renewal authority is stale');
END;
