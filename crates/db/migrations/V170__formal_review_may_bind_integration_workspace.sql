-- A formal-review Execution (frozen TaskRole capability `formal_review`) may
-- bind to the Task integration workspace so the exact integrated result can be
-- reviewed before Task merge. Every other legacy (work_unit_id NULL) Execution
-- remains forbidden from that workspace on WorkUnit Tasks.
--
-- The authority snapshot is written by an AFTER INSERT trigger, so the INSERT
-- guard evaluates the same live role/configuration/active-membership join the
-- snapshot trigger uses; a row that passes here necessarily receives a
-- formal_review snapshot. The UPDATE guard reads the immutable snapshot.

DROP TRIGGER execution_legacy_integration_workspace_guard_insert;
DROP TRIGGER execution_legacy_integration_workspace_guard_update;

CREATE TRIGGER execution_legacy_integration_workspace_guard_insert
BEFORE INSERT ON execution
WHEN NEW.work_unit_id IS NULL AND NEW.workspace_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM work_unit w WHERE w.task_id = NEW.task_id)
 AND EXISTS (
     SELECT 1 FROM workspace_scope ws
     WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
       AND ws.scope_kind = 'integration'
 )
 AND NOT (
     NEW.actor_kind IS NOT NULL AND NEW.actor_id IS NOT NULL
     AND EXISTS (
         SELECT 1
         FROM task_role role
         JOIN task_role_capability_configuration configuration
           ON configuration.task_role_id = role.id
         JOIN role_membership membership
           ON membership.task_role_id = role.id
          AND membership.actor_kind = NEW.actor_kind
          AND membership.actor_id = NEW.actor_id
          AND membership.status = 'active'
         JOIN json_each(configuration.configuration_json, '$.capabilities') capability
         WHERE role.task_id = NEW.task_id AND role.role = NEW.role
           AND json_extract(configuration.configuration_json, '$.schema_version') = 1
           AND json_type(configuration.configuration_json, '$.capabilities') = 'array'
           AND capability.type = 'text' AND capability.value = 'formal_review'
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace is not a worker workspace for WorkUnit Tasks');
END;

CREATE TRIGGER execution_legacy_integration_workspace_guard_update
BEFORE UPDATE OF task_id, work_unit_id, workspace_id ON execution
WHEN NEW.work_unit_id IS NULL AND NEW.workspace_id IS NOT NULL
 AND EXISTS (SELECT 1 FROM work_unit w WHERE w.task_id = NEW.task_id)
 AND EXISTS (
     SELECT 1 FROM workspace_scope ws
     WHERE ws.workspace_id = NEW.workspace_id AND ws.task_id = NEW.task_id
       AND ws.scope_kind = 'integration'
 )
 AND NOT EXISTS (
     SELECT 1
     FROM execution_task_role_authority authority
     JOIN json_each(authority.capabilities_json) capability
     WHERE authority.execution_id = NEW.id
       AND capability.type = 'text' AND capability.value = 'formal_review'
 )
BEGIN
    SELECT RAISE(ABORT, 'Task integration workspace is not a worker workspace for WorkUnit Tasks');
END;
