-- V100's TaskLifecycle and the current board order are authoritative. Retain
-- exact workflow/board operation history in the immutable PR13 archive, then
-- remove the unused workflow fields and transition/move persistence.
-- The explicit transaction is applied on one direct connection with FKs on.
PRAGMA foreign_keys = ON;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_transition_expected (rows INTEGER NOT NULL);
INSERT INTO _pr13_transition_expected SELECT COUNT(*) FROM transition_log;
CREATE TEMP TABLE _pr13_move_expected (rows INTEGER NOT NULL);
INSERT INTO _pr13_move_expected SELECT COUNT(*) FROM task_move_operation;
CREATE TEMP TABLE _pr13_project_workflow_expected (rows INTEGER NOT NULL);
INSERT INTO _pr13_project_workflow_expected SELECT COUNT(*) FROM project;
CREATE TEMP TABLE _pr13_project_defaults_expected (
    invalid_settings INTEGER NOT NULL,
    legacy_defaults INTEGER NOT NULL,
    invalid_task_metadata INTEGER NOT NULL,
    legacy_deferred_dispatch INTEGER NOT NULL
);
INSERT INTO _pr13_project_defaults_expected
SELECT (SELECT COUNT(*) FROM project WHERE json_valid(settings) = 0),
       (SELECT COUNT(*) FROM project
        WHERE json_valid(settings) = 1
          AND json_type(settings, '$.default_role_assignments') IS NOT NULL),
       (SELECT COUNT(*) FROM task
        WHERE metadata_json IS NOT NULL AND json_valid(metadata_json) = 0),
       (SELECT COUNT(*) FROM task
        WHERE json_valid(metadata_json) = 1
          AND json_type(metadata_json, '$.deferred_dispatch') IS NOT NULL);

CREATE TEMP TABLE _pr13_cutover_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_cutover_check
SELECT (SELECT invalid_settings = 0 FROM _pr13_project_defaults_expected);
INSERT INTO _pr13_cutover_check
SELECT (SELECT invalid_task_metadata = 0 FROM _pr13_project_defaults_expected);

DROP TRIGGER pr13_legacy_record_history_no_insert;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_workflow', id,
       json_object('project_id', id,
                   'workflow_definition', workflow_definition,
                   'workflow_template_name', workflow_template_name,
                   'settings_raw', settings,
                   'created_at', created_at,
                   'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project;

INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'project_settings_default_role_assignments', id,
       json_object('project_id', id,
                   'settings_raw', settings,
                   'default_role_assignments',
                       json_extract(settings, '$.default_role_assignments'),
                   'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM project
WHERE json_type(settings, '$.default_role_assignments') IS NOT NULL;

INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task_deferred_dispatch', id,
       json_object('task_id', id,
                   'metadata_raw', metadata_json,
                   'deferred_dispatch', json_extract(metadata_json, '$.deferred_dispatch'),
                   'version', version,
                   'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task
WHERE json_type(metadata_json, '$.deferred_dispatch') IS NOT NULL;

INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'transition_log', id,
       json_object('id', id, 'task_id', task_id, 'from_state', from_state,
                   'to_state', to_state, 'triggered_by', triggered_by,
                   'trigger_reason', trigger_reason,
                   'hook_results_json', hook_results_json,
                   'rejection', rejection, 'created_at', created_at,
                   'trigger_name', trigger_name),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM transition_log;

INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task_move_operation', operation_id,
       json_object('operation_id', operation_id, 'project_id', project_id,
                   'task_id', task_id, 'request_hash', request_hash,
                   'state', state, 'direct_result_json', direct_result_json,
                   'result_json', result_json, 'created_at', created_at,
                   'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task_move_operation;

CREATE TRIGGER pr13_legacy_record_history_no_insert
BEFORE INSERT ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is migration-only');
END;

UPDATE project
SET settings = json_remove(settings, '$.default_role_assignments')
WHERE json_type(settings, '$.default_role_assignments') IS NOT NULL;
UPDATE task
SET metadata_json = json_remove(metadata_json, '$.deferred_dispatch')
WHERE json_type(metadata_json, '$.deferred_dispatch') IS NOT NULL;

DROP INDEX idx_transition_log_task;
DROP INDEX idx_transition_log_rejection;
DROP INDEX idx_transition_log_merge_failed;
DROP TABLE transition_log;
DROP INDEX idx_task_move_operation_project_created;
DROP INDEX idx_task_move_operation_task_created;
DROP TABLE task_move_operation;

ALTER TABLE project DROP COLUMN workflow_template_name;
ALTER TABLE project DROP COLUMN workflow_definition;

INSERT INTO _pr13_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history WHERE source_table = 'transition_log')
       = (SELECT rows FROM _pr13_transition_expected);
INSERT INTO _pr13_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history WHERE source_table = 'task_move_operation')
       = (SELECT rows FROM _pr13_move_expected);
INSERT INTO _pr13_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history WHERE source_table = 'project_workflow')
       = (SELECT rows FROM _pr13_project_workflow_expected);
INSERT INTO _pr13_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history
        WHERE source_table = 'project_settings_default_role_assignments')
       = (SELECT legacy_defaults FROM _pr13_project_defaults_expected);
INSERT INTO _pr13_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history
        WHERE source_table = 'task_deferred_dispatch')
       = (SELECT legacy_deferred_dispatch FROM _pr13_project_defaults_expected);
INSERT INTO _pr13_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('project')
    WHERE name IN ('workflow_definition', 'workflow_template_name')
);
INSERT INTO _pr13_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master
    WHERE type = 'table' AND name IN ('transition_log', 'task_move_operation')
);
INSERT INTO _pr13_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM project
    WHERE json_valid(settings) = 0
       OR json_type(settings, '$.default_role_assignments') IS NOT NULL
);
INSERT INTO _pr13_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM task
    WHERE metadata_json IS NOT NULL AND (
        json_valid(metadata_json) = 0
        OR json_type(metadata_json, '$.deferred_dispatch') IS NOT NULL
    )
);
INSERT INTO _pr13_cutover_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _pr13_cutover_check
SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';

DROP TABLE _pr13_cutover_check;
DROP TABLE _pr13_transition_expected;
DROP TABLE _pr13_move_expected;
DROP TABLE _pr13_project_workflow_expected;
DROP TABLE _pr13_project_defaults_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (130, 'retire_workflow_and_task_move_storage', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
