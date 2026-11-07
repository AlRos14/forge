-- Freeze current TaskRole authority into each newly admitted exact Execution.
-- The configuration is distinct from policy_json: PR6's durable wake snapshot
-- continues to bind its strict action policy independently.

DROP TRIGGER task_role_capability_configuration_event;

CREATE TRIGGER task_role_capability_configuration_event
AFTER UPDATE OF configuration_json ON task_role_capability_configuration
WHEN OLD.configuration_json IS NOT NEW.configuration_json
BEGIN
    INSERT INTO domain_event (
        id, event_type, entity_type, entity_id, actor_type, actor_id,
        scope_type, scope_id, correlation_id, causation_id, causation_depth,
        dedupe_key, payload_json, created_at
    )
    SELECT
        lower(hex(randomblob(4))) || '-' || lower(hex(randomblob(2))) || '-4' ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' ||
            substr('89ab', 1 + (abs(random()) % 4), 1) ||
            lower(substr(hex(randomblob(2)), 2, 3)) || '-' || lower(hex(randomblob(6))),
        'gate.task_role_changed', 'task_role', role.id, 'system', NULL,
        'task', role.task_id, role.id, NULL, 0,
        'pr15-gate-task-role-capabilities:' || role.id || ':' || role.version,
        json_object(
            'task_role_id', role.id,
            'task_role_version', role.version,
            'configuration', NEW.configuration_json
        ),
        NEW.updated_at
    FROM task_role role WHERE role.id = NEW.task_role_id;
END;

CREATE TABLE execution_task_role_authority (
    execution_id       TEXT PRIMARY KEY REFERENCES execution(id) ON DELETE CASCADE,
    task_role_id       TEXT NOT NULL REFERENCES task_role(id) ON DELETE RESTRICT,
    task_role_version  INTEGER,
    configuration_json TEXT NOT NULL CHECK (
        json_valid(configuration_json) AND json_type(configuration_json) = 'object'
    ),
    capabilities_json  TEXT NOT NULL CHECK (
        json_valid(capabilities_json) AND json_type(capabilities_json) = 'array'
    ),
    membership_id      TEXT NOT NULL REFERENCES role_membership(id) ON DELETE RESTRICT,
    actor_kind         TEXT NOT NULL CHECK (actor_kind IN ('human', 'agent')),
    actor_id           TEXT NOT NULL CHECK (length(trim(actor_id)) > 0),
    source             TEXT NOT NULL CHECK (source IN ('admission', 'legacy_role_contract')),
    captured_at        TEXT NOT NULL
);

CREATE INDEX idx_execution_task_role_authority_role
    ON execution_task_role_authority(task_role_id, task_role_version);

-- Legacy authority is recovered only when the exact TaskRole, Actor and
-- membership interval can be established from durable rows. Its old version
-- is unknowable, so NULL is explicit migration evidence rather than a claim.
INSERT INTO execution_task_role_authority (
    execution_id, task_role_id, task_role_version, configuration_json,
    capabilities_json, membership_id, actor_kind, actor_id, source, captured_at
)
SELECT e.id, role.id, NULL, configuration.configuration_json,
       json_extract(configuration.configuration_json, '$.capabilities'),
       membership.id, e.actor_kind, e.actor_id, 'legacy_role_contract', e.created_at
FROM execution e
JOIN task_role role
  ON role.task_id = e.task_id AND role.role = e.role
JOIN task_role_capability_configuration configuration
  ON configuration.task_role_id = role.id
JOIN role_membership membership
  ON membership.task_role_id = role.id
 AND membership.actor_kind = e.actor_kind
 AND membership.actor_id = e.actor_id
 AND membership.created_at <= e.created_at
 AND (
      (membership.status IN ('active', 'suspended')
       AND (membership.version = 1 OR membership.updated_at <= e.created_at))
      OR (membership.status = 'ended' AND membership.ended_at >= e.created_at)
 )
WHERE e.actor_kind IS NOT NULL AND e.actor_id IS NOT NULL;

CREATE TRIGGER execution_task_role_authority_admission_snapshot
AFTER INSERT ON execution
WHEN NEW.actor_kind IS NOT NULL AND NEW.actor_id IS NOT NULL
BEGIN
    INSERT INTO execution_task_role_authority (
        execution_id, task_role_id, task_role_version, configuration_json,
        capabilities_json, membership_id, actor_kind, actor_id, source, captured_at
    )
    SELECT NEW.id, role.id, role.version, configuration.configuration_json,
           json_extract(configuration.configuration_json, '$.capabilities'),
           membership.id, NEW.actor_kind, NEW.actor_id, 'admission', NEW.created_at
    FROM task_role role
    JOIN task_role_capability_configuration configuration
      ON configuration.task_role_id = role.id
    JOIN role_membership membership
      ON membership.task_role_id = role.id
     AND membership.actor_kind = NEW.actor_kind
     AND membership.actor_id = NEW.actor_id
     AND membership.status = 'active'
    WHERE role.task_id = NEW.task_id AND role.role = NEW.role
      AND json_extract(configuration.configuration_json, '$.schema_version') = 1
      AND json_type(configuration.configuration_json, '$.capabilities') = 'array';
END;

CREATE TRIGGER execution_task_role_authority_immutable_update
BEFORE UPDATE ON execution_task_role_authority
BEGIN
    SELECT RAISE(ABORT, 'Execution TaskRole authority snapshots are immutable');
END;

CREATE TRIGGER execution_task_role_authority_immutable_delete
BEFORE DELETE ON execution_task_role_authority
WHEN EXISTS (SELECT 1 FROM execution WHERE id = OLD.execution_id)
 AND NOT EXISTS (
     SELECT 1 FROM execution e
     JOIN task t ON t.id = e.task_id
     JOIN project_deletion_guard g ON g.project_id = t.project_id
     WHERE e.id = OLD.execution_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Execution TaskRole authority snapshots are immutable');
END;

CREATE TRIGGER execution_task_role_authority_insert_guard
BEFORE INSERT ON execution_task_role_authority
WHEN NOT EXISTS (
    SELECT 1
    FROM execution e
    JOIN task_role role ON role.id = NEW.task_role_id
    JOIN task_role_capability_configuration configuration
      ON configuration.task_role_id = role.id
    JOIN role_membership membership ON membership.id = NEW.membership_id
    WHERE e.id = NEW.execution_id
      AND e.task_id = role.task_id AND e.role = role.role
      AND e.actor_kind = NEW.actor_kind AND e.actor_id = NEW.actor_id
      AND membership.task_role_id = role.id
      AND membership.actor_kind = NEW.actor_kind
      AND membership.actor_id = NEW.actor_id
      AND NEW.configuration_json = configuration.configuration_json
      AND NEW.capabilities_json = json_extract(configuration.configuration_json, '$.capabilities')
      AND (
          (NEW.source = 'admission' AND membership.status = 'active'
           AND NEW.task_role_version = role.version)
          OR NEW.source = 'legacy_role_contract'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Execution TaskRole authority snapshot does not match its exact role and Actor');
END;
