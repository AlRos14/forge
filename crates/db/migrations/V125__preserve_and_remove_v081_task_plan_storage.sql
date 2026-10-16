-- V081 is historical planning storage. V098 already created exact generic
-- plan Artifacts where source provenance was provable; preserve every V081
-- row and migration receipt before removing the obsolete tables and column.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_plan_expected (row_count INTEGER NOT NULL);
INSERT INTO _pr13_plan_expected
SELECT (SELECT COUNT(*) FROM task_plan_revision)
     + (SELECT COUNT(*) FROM task_plan_approval)
     + (SELECT COUNT(*) FROM review_evidence_bundle)
     + (SELECT COUNT(*) FROM task WHERE plan IS NOT NULL);

CREATE TABLE pr13_legacy_record_history (
    source_table        TEXT NOT NULL,
    source_identity     TEXT NOT NULL,
    row_json            TEXT NOT NULL CHECK (json_valid(row_json)),
    captured_at         TEXT NOT NULL,
    PRIMARY KEY (source_table, source_identity)
);

INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task_plan_revision', id,
       json_object('id', id, 'task_id', task_id, 'revision', revision,
                   'checkpoint', checkpoint, 'markdown', markdown,
                   'content_digest', content_digest, 'checklist_json', checklist_json,
                   'warnings_json', warnings_json,
                   'source_execution_id', source_execution_id,
                   'created_at', created_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task_plan_revision;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task_plan_approval', id,
       json_object('id', id, 'task_id', task_id,
                   'plan_revision_id', plan_revision_id,
                   'content_digest', content_digest,
                   'principal_type', principal_type, 'principal_id', principal_id,
                   'decision', decision, 'reason', reason, 'created_at', created_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task_plan_approval;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'review_evidence_bundle', id,
       json_object('id', id, 'review_id', review_id, 'task_id', task_id,
                   'reviewer_execution_id', reviewer_execution_id,
                   'plan_revision_id', plan_revision_id, 'plan_digest', plan_digest,
                   'base_sha', base_sha, 'head_sha', head_sha,
                   'diff_text', diff_text, 'diff_digest', diff_digest,
                   'ci_results_json', ci_results_json,
                   'fresh_session', fresh_session, 'created_at', created_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM review_evidence_bundle;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'task.plan', id,
       json_object('task_id', id, 'plan', plan),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM task WHERE plan IS NOT NULL;

CREATE TRIGGER pr13_legacy_record_history_immutable_update
BEFORE UPDATE ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is immutable');
END;
CREATE TRIGGER pr13_legacy_record_history_no_insert
BEFORE INSERT ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is migration-only');
END;
CREATE TRIGGER pr13_legacy_record_history_immutable_delete
BEFORE DELETE ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is immutable');
END;

-- Keep V098 migration provenance but remove only its FK to the retired source
-- row. The Artifact FK remains because it names the canonical replacement.
CREATE TABLE legacy_task_plan_artifact_migration__pr13 (
    plan_revision_id TEXT PRIMARY KEY,
    artifact_id      TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    migration_status TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'source_execution_missing',
        'source_execution_unresolved', 'source_execution_output_ambiguous',
        'existing_output_conflict'
    )),
    details_json     TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json))
);
INSERT INTO legacy_task_plan_artifact_migration__pr13
    (plan_revision_id, artifact_id, migration_status, details_json)
SELECT plan_revision_id, artifact_id, migration_status, details_json
FROM legacy_task_plan_artifact_migration;
DROP INDEX idx_legacy_task_plan_artifact_migration_artifact;
DROP TABLE legacy_task_plan_artifact_migration;
ALTER TABLE legacy_task_plan_artifact_migration__pr13
    RENAME TO legacy_task_plan_artifact_migration;
CREATE INDEX idx_legacy_task_plan_artifact_migration_artifact
    ON legacy_task_plan_artifact_migration(artifact_id);

DROP TRIGGER task_plan_revision_immutable;
DROP TRIGGER task_plan_approval_immutable;
DROP TRIGGER review_evidence_bundle_immutable;
DROP TABLE review_evidence_bundle;
DROP TABLE task_plan_approval;
DROP INDEX idx_task_plan_revision_latest;
DROP TABLE task_plan_revision;
ALTER TABLE task DROP COLUMN plan;

CREATE TEMP TABLE _pr13_plan_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_plan_check
SELECT COUNT(*) = (SELECT row_count FROM _pr13_plan_expected)
FROM pr13_legacy_record_history;
INSERT INTO _pr13_plan_check
SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master
    WHERE type = 'table' AND name IN ('task_plan_revision', 'task_plan_approval', 'review_evidence_bundle')
);
INSERT INTO _pr13_plan_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_table_info('task') WHERE name = 'plan'
);
INSERT INTO _pr13_plan_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_foreign_key_list('legacy_task_plan_artifact_migration')
    WHERE "from" = 'plan_revision_id'
);
INSERT INTO _pr13_plan_check SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
DROP TABLE _pr13_plan_check;
DROP TABLE _pr13_plan_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (125, 'preserve_and_remove_v081_task_plan_storage', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
