-- PR8's old Review rows have been superseded by exact ReviewReport Artifacts,
-- ValidationRuns, and immutable migration receipts. Preserve every remaining
-- source row verbatim as stored column values, then remove the old runtime
-- persistence and detach the two V099 provenance ledgers from the source FK.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TEMP TABLE _pr13_review_expected (rows INTEGER NOT NULL);
INSERT INTO _pr13_review_expected SELECT COUNT(*) FROM review;

CREATE TEMP TABLE _pr13_review_cutover_check (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (
    SELECT 1
    FROM sqlite_master source
    JOIN pragma_foreign_key_list(source.name) fk
    WHERE source.type = 'table'
      AND fk."table" = 'review'
      AND source.name NOT IN (
          'legacy_review_artifact_migration',
          'legacy_ci_validation_migration'
      )
);

DROP TRIGGER pr13_legacy_record_history_no_insert;
INSERT INTO pr13_legacy_record_history (source_table, source_identity, row_json, captured_at)
SELECT 'review', id,
       json_object('id', id, 'task_id', task_id, 'execution_id', execution_id,
                   'attempt_number', attempt_number, 'status', status,
                   'step_results_json', step_results_json,
                   'started_at', started_at, 'finished_at', finished_at,
                   'created_at', created_at, 'updated_at', updated_at),
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM review;
CREATE TRIGGER pr13_legacy_record_history_no_insert
BEFORE INSERT ON pr13_legacy_record_history
BEGIN
    SELECT RAISE(ABORT, 'PR13 legacy history is migration-only');
END;

CREATE TABLE legacy_review_artifact_migration__pr13 (
    review_id         TEXT PRIMARY KEY,
    artifact_id       TEXT REFERENCES artifact(id) ON DELETE RESTRICT,
    migration_status  TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'source_execution_missing',
        'source_execution_unresolved', 'wrong_task', 'actor_missing',
        'content_unrecoverable', 'ambiguous', 'existing_output_conflict',
        'status_verdict_mismatch', 'terminal_timestamp_missing', 'subject_unavailable'
    )),
    details_json      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json))
);
INSERT INTO legacy_review_artifact_migration__pr13
    (review_id, artifact_id, migration_status, details_json)
SELECT review_id, artifact_id, migration_status, details_json
FROM legacy_review_artifact_migration;
DROP TABLE legacy_review_artifact_migration;
ALTER TABLE legacy_review_artifact_migration__pr13
    RENAME TO legacy_review_artifact_migration;
CREATE INDEX idx_legacy_review_artifact_migration_artifact
    ON legacy_review_artifact_migration(artifact_id);

CREATE TABLE legacy_ci_validation_migration__pr13 (
    review_id         TEXT NOT NULL,
    step_index        INTEGER NOT NULL,
    validation_run_id TEXT REFERENCES validation_run(id) ON DELETE RESTRICT,
    migration_status  TEXT NOT NULL CHECK (migration_status IN (
        'migrated', 'mapped_duplicate', 'no_steps', 'source_execution_missing',
        'source_execution_unresolved', 'wrong_task', 'check_identity_missing',
        'status_missing', 'timestamps_missing', 'workspace_missing',
        'commit_identity_missing', 'output_missing', 'ambiguous',
        'insufficient_provenance'
    )),
    details_json      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(details_json)),
    PRIMARY KEY (review_id, step_index)
);
INSERT INTO legacy_ci_validation_migration__pr13
    (review_id, step_index, validation_run_id, migration_status, details_json)
SELECT review_id, step_index, validation_run_id, migration_status, details_json
FROM legacy_ci_validation_migration;
DROP TABLE legacy_ci_validation_migration;
ALTER TABLE legacy_ci_validation_migration__pr13
    RENAME TO legacy_ci_validation_migration;
CREATE INDEX idx_legacy_ci_validation_migration_run
    ON legacy_ci_validation_migration(validation_run_id);

DROP TABLE review;

INSERT INTO _pr13_review_cutover_check
SELECT (SELECT COUNT(*) FROM pr13_legacy_record_history WHERE source_table = 'review')
       = (SELECT rows FROM _pr13_review_expected);
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'review'
);
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (
    SELECT 1
    FROM sqlite_master source
    JOIN pragma_foreign_key_list(source.name) fk
    WHERE source.type = 'table' AND fk."table" = 'review'
);
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_foreign_key_list('legacy_review_artifact_migration')
    WHERE "from" = 'review_id'
);
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (
    SELECT 1 FROM pragma_foreign_key_list('legacy_ci_validation_migration')
    WHERE "from" = 'review_id'
);
INSERT INTO _pr13_review_cutover_check
SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO _pr13_review_cutover_check
SELECT (SELECT integrity_check FROM pragma_integrity_check LIMIT 1) = 'ok';

DROP TABLE _pr13_review_cutover_check;
DROP TABLE _pr13_review_expected;

INSERT INTO _migration (version, name, applied_at)
VALUES (131, 'archive_and_drop_legacy_review_storage', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
