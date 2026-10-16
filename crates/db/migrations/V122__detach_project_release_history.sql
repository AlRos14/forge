-- ProjectRelease is a historical read contract after PR12. V121 copied the
-- exact snapshot inputs into immutable history tables. This migration removes
-- the reader's foreign keys to Project OS and points media tombstones at the
-- copied pin identities before dropping the source reference/pin tables.
-- The migration receipt commits atomically with the schema cutover so a
-- process restart cannot leave a receipt-less partial rebuild.
PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

DROP TRIGGER IF EXISTS project_release_scope_guard;
DROP TRIGGER IF EXISTS project_release_immutable_update;
DROP TRIGGER IF EXISTS project_release_immutable_delete;
DROP TRIGGER IF EXISTS pr11_retired_insert_project_release;
DROP TRIGGER IF EXISTS pr11_retired_update_project_release;
DROP TRIGGER IF EXISTS project_release_reference_immutable_update;
DROP TRIGGER IF EXISTS project_release_reference_immutable_delete;
DROP TRIGGER IF EXISTS pr11_retired_insert_project_release_reference;
DROP TRIGGER IF EXISTS pr11_retired_update_project_release_reference;
DROP TRIGGER IF EXISTS project_release_media_pin_scope_guard;
DROP TRIGGER IF EXISTS project_release_media_pin_gc_guard_after_insert;
DROP TRIGGER IF EXISTS project_release_media_pin_immutable_delete;
DROP TRIGGER IF EXISTS pr11_retired_insert_project_release_media_pin;
DROP TRIGGER IF EXISTS pr11_retired_update_project_release_media_pin;
DROP TRIGGER IF EXISTS task_media_project_attachment_after_delete;

CREATE TABLE project_release__pr13 (
    id                    TEXT PRIMARY KEY,
    project_id            TEXT NOT NULL REFERENCES project(id) ON DELETE RESTRICT,
    milestone_id          TEXT NOT NULL,
    release_sequence      INTEGER NOT NULL CHECK (release_sequence >= 1),
    release_revision      INTEGER NOT NULL CHECK (release_revision >= 1),
    release_identifier    TEXT NOT NULL,
    milestone_revision_id TEXT NOT NULL,
    readiness_snapshot_id TEXT NOT NULL,
    readiness_digest      TEXT NOT NULL,
    baseline_id           TEXT NOT NULL,
    baseline_revision_id  TEXT NOT NULL,
    baseline_digest       TEXT NOT NULL,
    release_policy_revision TEXT NOT NULL,
    release_policy_digest TEXT NOT NULL,
    summary               TEXT NOT NULL DEFAULT '',
    changelog             TEXT NOT NULL DEFAULT '',
    known_issues_json     TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(known_issues_json)),
    charter_revision_id   TEXT,
    document_revisions_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(document_revisions_json)),
    decision_ids_json     TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(decision_ids_json)),
    task_references_json  TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(task_references_json)),
    validation_references_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(validation_references_json)),
    git_references_json   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(git_references_json)),
    evidence_references_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_references_json)),
    waivers_json          TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(waivers_json)),
    releasing_principal_type TEXT NOT NULL CHECK (length(trim(releasing_principal_type)) > 0),
    releasing_principal_id TEXT NOT NULL CHECK (length(trim(releasing_principal_id)) > 0),
    authorization_basis   TEXT NOT NULL CHECK (length(trim(authorization_basis)) > 0),
    authorization_action  TEXT NOT NULL CHECK (length(trim(authorization_action)) > 0),
    explicit_event        TEXT NOT NULL CHECK (length(trim(explicit_event)) > 0),
    authorization_occurred_at TEXT NOT NULL CHECK (length(trim(authorization_occurred_at)) > 0),
    schema_version        TEXT NOT NULL,
    snapshot_digest       TEXT NOT NULL,
    idempotency_key       TEXT NOT NULL UNIQUE,
    created_at            TEXT NOT NULL,
    UNIQUE (milestone_id, release_revision),
    UNIQUE (project_id, release_identifier)
);
INSERT INTO project_release__pr13 (
    id, project_id, milestone_id, release_sequence, release_revision,
    release_identifier, milestone_revision_id, readiness_snapshot_id,
    readiness_digest, baseline_id, baseline_revision_id, baseline_digest,
    release_policy_revision, release_policy_digest, summary, changelog,
    known_issues_json, charter_revision_id, document_revisions_json,
    decision_ids_json, task_references_json, validation_references_json,
    git_references_json, evidence_references_json, waivers_json,
    releasing_principal_type, releasing_principal_id, authorization_basis,
    authorization_action, explicit_event, authorization_occurred_at,
    schema_version, snapshot_digest, idempotency_key, created_at
)
SELECT
    id, project_id, milestone_id, release_sequence, release_revision,
    release_identifier, milestone_revision_id, readiness_snapshot_id,
    readiness_digest, baseline_id, baseline_revision_id, baseline_digest,
    release_policy_revision, release_policy_digest, summary, changelog,
    known_issues_json, charter_revision_id, document_revisions_json,
    decision_ids_json, task_references_json, validation_references_json,
    git_references_json, evidence_references_json, waivers_json,
    releasing_principal_type, releasing_principal_id, authorization_basis,
    authorization_action, explicit_event, authorization_occurred_at,
    schema_version, snapshot_digest, idempotency_key, created_at
FROM project_release;
DROP TABLE project_release;
ALTER TABLE project_release__pr13 RENAME TO project_release;
CREATE INDEX idx_project_release_project_history
    ON project_release(project_id, created_at DESC, id DESC);

DROP INDEX IF EXISTS idx_media_asset_tombstone_asset;
CREATE TABLE media_asset_tombstone__pr13 (
    id                    TEXT PRIMARY KEY,
    asset_id              TEXT NOT NULL REFERENCES media_asset(id) ON DELETE RESTRICT,
    release_id            TEXT REFERENCES project_release(id) ON DELETE RESTRICT,
    release_pin_id        TEXT REFERENCES project_release_history_media_pin(id) ON DELETE RESTRICT,
    previous_checksum     TEXT,
    previous_availability TEXT NOT NULL,
    availability          TEXT NOT NULL CHECK (availability IN ('redacted', 'purged', 'evidence_unavailable')),
    principal_type        TEXT NOT NULL CHECK (length(trim(principal_type)) > 0),
    principal_id          TEXT NOT NULL CHECK (length(trim(principal_id)) > 0),
    reason                TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    authorization_basis   TEXT NOT NULL CHECK (length(trim(authorization_basis)) > 0),
    authorization_action  TEXT NOT NULL CHECK (length(trim(authorization_action)) > 0),
    explicit_event        TEXT NOT NULL CHECK (length(trim(explicit_event)) > 0),
    authorization_occurred_at TEXT NOT NULL CHECK (length(trim(authorization_occurred_at)) > 0),
    idempotency_key       TEXT NOT NULL UNIQUE,
    mutation_fingerprint  TEXT NOT NULL DEFAULT '',
    created_at            TEXT NOT NULL
);
INSERT INTO media_asset_tombstone__pr13 (
    id, asset_id, release_id, release_pin_id, previous_checksum,
    previous_availability, availability, principal_type, principal_id,
    reason, authorization_basis, authorization_action, explicit_event,
    authorization_occurred_at, idempotency_key, mutation_fingerprint,
    created_at
)
SELECT
    id, asset_id, release_id, release_pin_id, previous_checksum,
    previous_availability, availability, principal_type, principal_id,
    reason, authorization_basis, authorization_action, explicit_event,
    authorization_occurred_at, idempotency_key, mutation_fingerprint,
    created_at
FROM media_asset_tombstone;
DROP TABLE media_asset_tombstone;
ALTER TABLE media_asset_tombstone__pr13 RENAME TO media_asset_tombstone;
CREATE INDEX idx_media_asset_tombstone_asset
    ON media_asset_tombstone(asset_id, created_at DESC, id DESC);
CREATE TRIGGER media_asset_tombstone_immutable_update
BEFORE UPDATE ON media_asset_tombstone BEGIN
    SELECT RAISE(ABORT, 'Media asset tombstones are immutable');
END;
CREATE TRIGGER media_asset_tombstone_immutable_delete
BEFORE DELETE ON media_asset_tombstone
WHEN NOT EXISTS (
    SELECT 1 FROM project_deletion_guard g
    JOIN media_asset a ON a.project_id = g.project_id
    WHERE a.id = OLD.asset_id
)
BEGIN
    SELECT RAISE(ABORT, 'Media asset tombstones are immutable');
END;

DROP TABLE project_release_reference;
DROP TABLE project_release_media_pin;

CREATE TRIGGER project_release_immutable_update
BEFORE UPDATE ON project_release BEGIN
    SELECT RAISE(ABORT, 'Project releases are immutable');
END;
CREATE TRIGGER project_release_immutable_delete
BEFORE DELETE ON project_release BEGIN
    SELECT RAISE(ABORT, 'Project releases are immutable');
END;
CREATE TRIGGER pr11_retired_insert_project_release
BEFORE INSERT ON project_release BEGIN
    SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
END;
CREATE TRIGGER project_release_history_snapshot_insert_guard
BEFORE INSERT ON project_release_history_milestone BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_snapshot_update_guard
BEFORE UPDATE ON project_release_history_milestone BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_snapshot_delete_guard
BEFORE DELETE ON project_release_history_milestone BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_definition_insert_guard
BEFORE INSERT ON project_release_history_definition BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_definition_update_guard
BEFORE UPDATE ON project_release_history_definition BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_definition_delete_guard
BEFORE DELETE ON project_release_history_definition BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_readiness_insert_guard
BEFORE INSERT ON project_release_history_readiness BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_readiness_update_guard
BEFORE UPDATE ON project_release_history_readiness BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_readiness_delete_guard
BEFORE DELETE ON project_release_history_readiness BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_charter_insert_guard
BEFORE INSERT ON project_release_history_charter BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_charter_update_guard
BEFORE UPDATE ON project_release_history_charter BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_charter_delete_guard
BEFORE DELETE ON project_release_history_charter BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_reference_insert_guard
BEFORE INSERT ON project_release_history_reference BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_reference_update_guard
BEFORE UPDATE ON project_release_history_reference BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_reference_delete_guard
BEFORE DELETE ON project_release_history_reference BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_media_pin_insert_guard
BEFORE INSERT ON project_release_history_media_pin BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_media_pin_update_guard
BEFORE UPDATE ON project_release_history_media_pin BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER project_release_history_media_pin_delete_guard
BEFORE DELETE ON project_release_history_media_pin BEGIN
    SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
END;
CREATE TRIGGER task_media_project_attachment_after_delete
AFTER UPDATE OF deleted_at ON task_media
WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL
BEGIN
    UPDATE project_media_attachment
    SET deleted_at = NEW.deleted_at,
        availability = 'purged',
        updated_at = NEW.deleted_at,
        version = version + 1
    WHERE task_media_id = NEW.id AND deleted_at IS NULL;

    UPDATE media_asset
    SET gc_state = CASE
        WHEN EXISTS (
            SELECT 1 FROM project_release_history_media_pin p
            WHERE p.asset_id = NEW.asset_id
        ) THEN 'referenced' ELSE 'gc_candidate' END,
        gc_candidate_at = CASE
        WHEN EXISTS (
            SELECT 1 FROM project_release_history_media_pin p
            WHERE p.asset_id = NEW.asset_id
        ) THEN NULL ELSE NEW.deleted_at END,
        deleted_at = CASE
        WHEN EXISTS (
            SELECT 1 FROM project_release_history_media_pin p
            WHERE p.asset_id = NEW.asset_id
        ) THEN NULL ELSE NEW.deleted_at END,
        gc_lease_owner = NULL,
        gc_lease_expires_at = NULL,
        version = version + 1,
        updated_at = NEW.deleted_at
    WHERE id = NEW.asset_id;
END;

CREATE TEMP TABLE _pr13_foreign_key_assert (violation INTEGER CHECK (violation = 0));
INSERT INTO _pr13_foreign_key_assert
SELECT 1 FROM pragma_foreign_key_check LIMIT 1;
DROP TABLE _pr13_foreign_key_assert;
CREATE TEMP TABLE _pr13_integrity_assert (result TEXT CHECK (result = 'ok'));
INSERT INTO _pr13_integrity_assert SELECT integrity_check FROM pragma_integrity_check;
DROP TABLE _pr13_integrity_assert;

INSERT INTO _migration(version, name, applied_at)
VALUES (122, 'detach_project_release_history', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
COMMIT;
PRAGMA foreign_keys = ON;
