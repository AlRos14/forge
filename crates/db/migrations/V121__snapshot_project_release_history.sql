-- Materialize every input needed by the public immutable Release reader
-- before Project OS tables are removed in later PR13 migrations.
--
-- This migration copies stored values without parsing or recomputing them.
-- Release GETs will switch to these per-release records before their source
-- Project OS tables are dropped.

CREATE TEMP TABLE _pr13_release_snapshot_assert (
    ok INTEGER NOT NULL CHECK (ok = 1)
);

INSERT INTO _pr13_release_snapshot_assert (ok)
SELECT CASE
    WHEN (SELECT COUNT(*) FROM project_release) =
         (SELECT COUNT(*) FROM project_release r
          JOIN project_milestone m ON m.id = r.milestone_id
          JOIN project_milestone_revision d ON d.id = r.milestone_revision_id
          JOIN project_readiness_snapshot s ON s.id = r.readiness_snapshot_id
         JOIN project_charter_revision cr ON cr.id = r.charter_revision_id
         JOIN project_charter c ON c.id = cr.charter_id
          WHERE m.project_id = r.project_id AND d.milestone_id = m.id
            AND s.project_id = r.project_id AND s.milestone_id = r.milestone_id
            AND c.project_id = r.project_id)
     AND (SELECT COUNT(*) FROM project_release_media_pin p) =
         (SELECT COUNT(*) FROM project_release_media_pin p
          JOIN project_release r ON r.id = p.release_id AND r.project_id = p.project_id
          JOIN media_asset a ON a.id = p.asset_id AND a.project_id = p.project_id)
     AND (SELECT COUNT(*) FROM project_release_reference) =
         (SELECT COUNT(*) FROM project_release_reference rr
          JOIN project_release r ON r.id = rr.release_id)
    THEN 1 ELSE 0 END;

CREATE TABLE project_release_history_milestone (
    release_id   TEXT PRIMARY KEY REFERENCES project_release(id) ON DELETE CASCADE,
    milestone_id TEXT NOT NULL,
    project_id   TEXT NOT NULL,
    milestone_key TEXT NOT NULL,
    display_label TEXT
);
INSERT INTO project_release_history_milestone (
    release_id, milestone_id, project_id, milestone_key, display_label
)
SELECT r.id, m.id, m.project_id, m.milestone_key, m.display_label
FROM project_release r
JOIN project_milestone m ON m.id = r.milestone_id AND m.project_id = r.project_id;

CREATE TABLE project_release_history_definition (
    release_id TEXT PRIMARY KEY REFERENCES project_release(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    milestone_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    base_revision INTEGER NOT NULL,
    base_revision_id TEXT,
    lifecycle TEXT NOT NULL,
    display_label TEXT,
    outcome TEXT NOT NULL,
    included_scope_json TEXT NOT NULL,
    excluded_scope_json TEXT NOT NULL,
    charter_revision_id TEXT,
    document_revisions_json TEXT NOT NULL,
    task_selection_json TEXT NOT NULL,
    dependencies_json TEXT NOT NULL,
    risks_json TEXT NOT NULL,
    acceptance_checks_json TEXT NOT NULL,
    evidence_requirements_json TEXT NOT NULL,
    known_issues_json TEXT NOT NULL,
    change_summary TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    render_version TEXT NOT NULL,
    rendered_view TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    rendered_digest TEXT NOT NULL,
    author_type TEXT NOT NULL,
    author_id TEXT,
    source_refs_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO project_release_history_definition (
    release_id, id, milestone_id, revision, base_revision, base_revision_id,
    lifecycle, display_label, outcome, included_scope_json, excluded_scope_json,
    charter_revision_id, document_revisions_json, task_selection_json,
    dependencies_json, risks_json, acceptance_checks_json,
    evidence_requirements_json, known_issues_json, change_summary, schema_version,
    render_version, rendered_view, content_digest, rendered_digest, author_type,
    author_id, source_refs_json, created_at
)
SELECT r.id,
       d.id, d.milestone_id, d.revision, d.base_revision, d.base_revision_id,
       d.lifecycle, d.display_label, d.outcome, d.included_scope_json,
       d.excluded_scope_json, d.charter_revision_id, d.document_revisions_json,
       d.task_selection_json, d.dependencies_json, d.risks_json,
       d.acceptance_checks_json, d.evidence_requirements_json, d.known_issues_json,
       d.change_summary, d.schema_version, d.render_version, d.rendered_view,
       d.content_digest, d.rendered_digest, d.author_type, d.author_id,
       d.source_refs_json, d.created_at
FROM project_release r
JOIN project_milestone_revision d
  ON d.id = r.milestone_revision_id AND d.milestone_id = r.milestone_id;

CREATE TABLE project_release_history_readiness (
    release_id TEXT PRIMARY KEY REFERENCES project_release(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    milestone_id TEXT NOT NULL,
    definition_revision_id TEXT NOT NULL,
    baseline_id TEXT NOT NULL,
    baseline_revision_id TEXT NOT NULL,
    baseline_digest TEXT NOT NULL,
    release_policy_revision TEXT NOT NULL,
    release_policy_digest TEXT NOT NULL,
    input_manifest_json TEXT NOT NULL,
    event_watermark TEXT NOT NULL,
    outcome TEXT NOT NULL,
    blocking_reasons_json TEXT NOT NULL,
    check_results_json TEXT NOT NULL,
    waiver_manifest_json TEXT NOT NULL,
    evidence_manifest_json TEXT NOT NULL,
    commit_context_json TEXT NOT NULL,
    computing_policy_revision TEXT NOT NULL,
    readiness_digest TEXT NOT NULL,
    principal_type TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    authorization_basis TEXT NOT NULL,
    authorization_action TEXT NOT NULL,
    authorization_occurred_at TEXT NOT NULL,
    expected_milestone_version INTEGER NOT NULL,
    explicit_event TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO project_release_history_readiness (
    release_id, id, project_id, milestone_id, definition_revision_id,
    baseline_id, baseline_revision_id, baseline_digest, release_policy_revision,
    release_policy_digest, input_manifest_json, event_watermark, outcome,
    blocking_reasons_json, check_results_json, waiver_manifest_json,
    evidence_manifest_json, commit_context_json, computing_policy_revision,
    readiness_digest, principal_type, principal_id, authorization_basis,
    authorization_action, authorization_occurred_at, expected_milestone_version,
    explicit_event, idempotency_key, created_at
)
SELECT r.id,
       s.id, s.project_id, s.milestone_id, s.definition_revision_id,
       s.baseline_id, s.baseline_revision_id, s.baseline_digest,
       s.release_policy_revision, s.release_policy_digest, s.input_manifest_json,
       s.event_watermark, s.outcome, s.blocking_reasons_json, s.check_results_json,
       s.waiver_manifest_json, s.evidence_manifest_json, s.commit_context_json,
       s.computing_policy_revision, s.readiness_digest, s.principal_type,
       s.principal_id, s.authorization_basis, s.authorization_action,
       s.authorization_occurred_at, s.expected_milestone_version, s.explicit_event,
       s.idempotency_key, s.created_at
FROM project_release r
JOIN project_readiness_snapshot s
  ON s.id = r.readiness_snapshot_id AND s.project_id = r.project_id
 AND s.milestone_id = r.milestone_id;

CREATE TABLE project_release_history_charter (
    release_id TEXT PRIMARY KEY REFERENCES project_release(id) ON DELETE CASCADE,
    charter_id TEXT NOT NULL,
    revision_id TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    render_version TEXT NOT NULL,
    rendered_digest TEXT NOT NULL
);
INSERT INTO project_release_history_charter (
    release_id, charter_id, revision_id, content_digest, render_version, rendered_digest
)
SELECT r.id, c.id, cr.id, cr.content_digest, cr.render_version, cr.rendered_digest
FROM project_release r
JOIN project_charter_revision cr ON cr.id = r.charter_revision_id
JOIN project_charter c ON c.id = cr.charter_id AND c.project_id = r.project_id;

CREATE TABLE project_release_history_reference (
    release_id TEXT NOT NULL REFERENCES project_release(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    reference_kind TEXT NOT NULL,
    record_id TEXT NOT NULL,
    record_version TEXT,
    record_state TEXT,
    record_digest TEXT,
    metadata_json TEXT NOT NULL,
    PRIMARY KEY (release_id, ordinal)
);
INSERT INTO project_release_history_reference (
    release_id, ordinal, reference_kind, record_id, record_version,
    record_state, record_digest, metadata_json
)
SELECT r.id, rr.ordinal, rr.reference_kind, rr.record_id, rr.record_version,
       rr.record_state, rr.record_digest, rr.metadata_json
FROM project_release r
JOIN project_release_reference rr ON rr.release_id = r.id;

CREATE TABLE project_release_history_media_pin (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    release_id TEXT NOT NULL REFERENCES project_release(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES media_asset(id) ON DELETE RESTRICT,
    attachment_id TEXT,
    legacy_task_media_id TEXT,
    asset_checksum TEXT NOT NULL,
    attachment_digest TEXT NOT NULL,
    availability TEXT NOT NULL,
    pin_digest TEXT NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO project_release_history_media_pin (
    id, project_id, release_id, asset_id, attachment_id, legacy_task_media_id,
    asset_checksum, attachment_digest, availability, pin_digest, created_at
)
SELECT p.id, p.project_id, p.release_id, p.asset_id, p.attachment_id,
       p.legacy_task_media_id, p.asset_checksum, p.attachment_digest,
       p.availability, p.pin_digest, p.created_at
FROM project_release_media_pin p;

CREATE INDEX idx_project_release_history_media_pin_release
    ON project_release_history_media_pin(release_id, id);
CREATE INDEX idx_project_release_history_media_pin_asset
    ON project_release_history_media_pin(asset_id, availability, created_at DESC);

DROP TABLE _pr13_release_snapshot_assert;
