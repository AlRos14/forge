-- Keep product capabilities separate from the strict PR6 orchestration policy
-- and from its immutable durable wake snapshots.

CREATE TABLE task_role_capability_configuration (
    task_role_id       TEXT PRIMARY KEY REFERENCES task_role(id) ON DELETE CASCADE,
    configuration_json TEXT NOT NULL DEFAULT '{"schema_version":1,"display_label":null,"capabilities":[],"rework":null}'
        CHECK (
            json_valid(configuration_json)
            AND json_type(configuration_json) = 'object'
            AND json_extract(configuration_json, '$.schema_version') = 1
            AND json_type(configuration_json, '$.capabilities') = 'array'
        ),
    updated_at         TEXT NOT NULL
);

INSERT INTO task_role_capability_configuration (
    task_role_id, configuration_json, updated_at
)
SELECT id,
       '{"schema_version":1,"display_label":null,"capabilities":[],"rework":null}',
       updated_at
FROM task_role;

-- Every persisted role receives a parseable, typed configuration before V150
-- snapshots it onto historical Executions. Ordinary roles start with no
-- capabilities; only the exact legacy contracts below are inferred.

CREATE TABLE task_role_capability_migration_audit (
    task_role_id   TEXT NOT NULL REFERENCES task_role(id) ON DELETE CASCADE,
    capability     TEXT NOT NULL CHECK (capability IN ('formal_review', 'orchestration')),
    source         TEXT NOT NULL CHECK (source = 'legacy_role_contract'),
    migration      INTEGER NOT NULL CHECK (migration = 149),
    created_at     TEXT NOT NULL,
    PRIMARY KEY (task_role_id, capability)
);

UPDATE task_role_capability_configuration
SET configuration_json = '{"schema_version":1,"display_label":null,"capabilities":["formal_review"],"rework":null}'
WHERE task_role_id IN (SELECT id FROM task_role WHERE role = 'reviewer');

INSERT INTO task_role_capability_migration_audit (
    task_role_id, capability, source, migration, created_at
)
SELECT id, 'formal_review', 'legacy_role_contract', 149, updated_at
FROM task_role
WHERE role = 'reviewer';

UPDATE task_role_capability_configuration
SET configuration_json = '{"schema_version":1,"display_label":null,"capabilities":["orchestration"],"rework":null}'
WHERE task_role_id IN (SELECT id FROM task_role WHERE role = 'orchestrator');

INSERT INTO task_role_capability_migration_audit (
    task_role_id, capability, source, migration, created_at
)
SELECT id, 'orchestration', 'legacy_role_contract', 149, updated_at
FROM task_role
WHERE role = 'orchestrator';

CREATE TRIGGER task_role_capability_configuration_event
AFTER UPDATE OF configuration_json ON task_role_capability_configuration
WHEN OLD.configuration_json IS NOT NEW.configuration_json
BEGIN
    UPDATE task_role
    SET version = version + 1, updated_at = NEW.updated_at
    WHERE id = NEW.task_role_id;

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
