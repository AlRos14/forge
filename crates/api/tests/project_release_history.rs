#![allow(dead_code)]

mod common;

use api_types::{
    ArtifactRef, AuthorizationProvenance, EvidenceAvailability, EvidencePin, PrincipalKind,
    PrincipalRef, ProjectRelease, ProjectResponse, ReleaseEvidenceAvailability, ReleaseSnapshot,
};
use axum::{http::Method, http::StatusCode};
use serde_json::json;

#[tokio::test]
async fn public_release_get_reads_self_contained_history_after_project_os_drop() {
    let workspace = common::TestDir::new("project-release-history");
    let harness = common::test_app(workspace.path(), "project-release-history").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Historical release project"}),
        StatusCode::OK,
    )
    .await;

    let release = seed_self_contained_release(&harness, &project.id).await;
    let fetched: ProjectRelease = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/releases/{}", project.id, release.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched, release);

    let dropped_project_os_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN (
             'project_milestone', 'project_milestone_revision',
             'project_readiness_snapshot', 'project_charter',
             'project_charter_revision', 'project_execution_baseline',
             'project_execution_baseline_revision', 'project_media_attachment'
         )",
    )
    .fetch_one(harness.state.db.pool())
    .await
    .expect("Project OS objects are physically absent after migrations");
    assert_eq!(dropped_project_os_tables, 0);
}

async fn seed_self_contained_release(
    harness: &common::Harness,
    project_id: &str,
) -> ProjectRelease {
    let now = "2026-10-13T12:00:00Z";
    let milestone_id = "history-milestone-1";
    let milestone_revision_id = "history-milestone-revision-1";
    let readiness_id = "history-readiness-1";
    let baseline_id = "history-baseline-1";
    let baseline_revision_id = "history-baseline-revision-1";
    let charter_id = "history-charter-1";
    let charter_revision_id = "history-charter-revision-1";
    let release_id = "history-release-1";
    let asset_id = "history-asset-1";
    let attachment_id = "history-attachment-1";
    let pin_id = "history-pin-1";
    let action = "project.milestone.release";
    let schema_version = "forge.milestone-release/v1";

    let released_by = PrincipalRef {
        kind: PrincipalKind::User,
        id: "test-user-id".to_owned(),
        display_name: None,
    };
    let authorization = AuthorizationProvenance {
        principal: released_by.clone(),
        authorization_basis: "exact_user_approval".to_owned(),
        action: action.to_owned(),
        event_id: "history-release-event".to_owned(),
        occurred_at: now.to_owned(),
    };
    let evidence_pin = EvidencePin {
        id: pin_id.to_owned(),
        release_id: release_id.to_owned(),
        attachment_id: attachment_id.to_owned(),
        asset_id: asset_id.to_owned(),
        attachment_digest: "attachment-digest-1".to_owned(),
        asset_checksum: "asset-checksum-1".to_owned(),
        availability: EvidenceAvailability::Available,
        availability_projection: ReleaseEvidenceAvailability::Available,
        task_media_id: None,
        stable_project_url: None,
        pinned_at: now.to_owned(),
    };
    let pin_digest = api_types::canonical_digest_with_schema(
        schema_version,
        &serde_json::to_string(&evidence_pin).expect("pin JSON"),
    )
    .expect("pin digest");
    let mut snapshot = ReleaseSnapshot {
        schema_version: schema_version.to_owned(),
        project_id: project_id.to_owned(),
        milestone_id: milestone_id.to_owned(),
        milestone_canonical_id: "M001".to_owned(),
        release_revision: 1,
        release_identity: "M001-r1".to_owned(),
        milestone_definition_revision_id: milestone_revision_id.to_owned(),
        milestone_definition_digest: "definition-digest-1".to_owned(),
        expected_milestone_version: 1,
        display_label: Some("First release".to_owned()),
        summary: "Frozen summary".to_owned(),
        changelog: vec!["first line".to_owned()],
        known_issues: vec![],
        readiness_snapshot_id: readiness_id.to_owned(),
        readiness_digest: "readiness-digest-1".to_owned(),
        source_event_watermark: "event-watermark-42".to_owned(),
        baseline_id: baseline_id.to_owned(),
        baseline_revision_id: baseline_revision_id.to_owned(),
        baseline_digest: "baseline-digest-1".to_owned(),
        charter_revision: ArtifactRef {
            artifact_id: charter_id.to_owned(),
            revision_id: charter_revision_id.to_owned(),
            content_digest: "charter-content-digest".to_owned(),
            render_version: Some("render/v1".to_owned()),
            render_digest: Some("charter-render-digest".to_owned()),
        },
        document_revisions: vec![],
        included_decisions: vec![],
        included_tasks: vec![],
        validation_results: vec![],
        repository_references: vec![],
        evidence_pins: vec![evidence_pin],
        waived_check_ids: vec![],
        release_policy_revision: "policy-r1".to_owned(),
        release_policy_digest: "policy-digest-1".to_owned(),
        released_by,
        authorization,
        released_at: now.to_owned(),
        idempotency_key: "history-release-key".to_owned(),
        snapshot_digest: String::new(),
    };
    let mut digest_input = serde_json::to_value(&snapshot).expect("snapshot JSON");
    digest_input
        .as_object_mut()
        .expect("snapshot object")
        .remove("snapshot_digest");
    snapshot.snapshot_digest =
        api_types::canonical_digest_with_schema(schema_version, &digest_input)
            .expect("snapshot digest");

    let pool = harness.state.db.pool();
    for trigger in [
        "pr11_retired_insert_project_release",
        "project_release_history_snapshot_insert_guard",
        "project_release_history_definition_insert_guard",
        "project_release_history_readiness_insert_guard",
        "project_release_history_charter_insert_guard",
        "project_release_history_media_pin_insert_guard",
    ] {
        sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger}"))
            .execute(pool)
            .await
            .expect("open a test-only immutable-history fixture");
    }

    sqlx::query(
        "INSERT INTO project_release (
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
         ) VALUES (?, ?, ?, 1, 1, 'M001-r1', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                   '[]', ?, '[]', '[]', '[]', '[]', '[]', ?, '[]', 'user',
                   'test-user-id', 'exact_user_approval', ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(release_id)
    .bind(project_id)
    .bind(milestone_id)
    .bind(milestone_revision_id)
    .bind(readiness_id)
    .bind(&snapshot.readiness_digest)
    .bind(baseline_id)
    .bind(baseline_revision_id)
    .bind(&snapshot.baseline_digest)
    .bind(&snapshot.release_policy_revision)
    .bind(&snapshot.release_policy_digest)
    .bind(&snapshot.summary)
    .bind(serde_json::to_string(&snapshot.changelog).expect("changelog JSON"))
    .bind(charter_revision_id)
    .bind(serde_json::to_string(&snapshot.evidence_pins).expect("evidence pin JSON"))
    .bind(action)
    .bind(&snapshot.authorization.event_id)
    .bind(now)
    .bind(schema_version)
    .bind(&snapshot.snapshot_digest)
    .bind(&snapshot.idempotency_key)
    .bind(now)
    .execute(pool)
    .await
    .expect("immutable Release row");

    sqlx::query(
        "INSERT INTO media_asset (
             id, project_id, display_filename, content_type, byte_size,
             storage_key, checksum, created_at, updated_at
         ) VALUES (?, ?, 'evidence.png', 'image/png', 42, 'history/evidence.png', ?, ?, ?)",
    )
    .bind(asset_id)
    .bind(project_id)
    .bind(&snapshot.evidence_pins[0].asset_checksum)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("shared evidence Asset survives Project OS removal");
    sqlx::query(
        "INSERT INTO project_release_history_media_pin (
             id, project_id, release_id, asset_id, attachment_id,
             legacy_task_media_id, asset_checksum, attachment_digest,
             availability, pin_digest, created_at
         ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, 'available', ?, ?)",
    )
    .bind(pin_id)
    .bind(project_id)
    .bind(release_id)
    .bind(asset_id)
    .bind(attachment_id)
    .bind(&snapshot.evidence_pins[0].asset_checksum)
    .bind(&snapshot.evidence_pins[0].attachment_digest)
    .bind(pin_digest)
    .bind(now)
    .execute(pool)
    .await
    .expect("self-contained historical Release pin without a live attachment row");

    sqlx::query(
        "INSERT INTO project_release_history_milestone
         (release_id, milestone_id, project_id, milestone_key, display_label)
         VALUES (?, ?, ?, 'M001', 'First release')",
    )
    .bind(release_id)
    .bind(milestone_id)
    .bind(project_id)
    .execute(pool)
    .await
    .expect("frozen milestone identity");
    sqlx::query(
        "INSERT INTO project_release_history_definition (
             release_id, id, milestone_id, revision, base_revision, base_revision_id,
             lifecycle, display_label, outcome, included_scope_json, excluded_scope_json,
             charter_revision_id, document_revisions_json, task_selection_json,
             dependencies_json, risks_json, acceptance_checks_json,
             evidence_requirements_json, known_issues_json, change_summary,
             schema_version, render_version, rendered_view, content_digest,
             rendered_digest, author_type, author_id, source_refs_json, created_at
         ) VALUES (?, ?, ?, 1, 0, NULL, 'approved', 'First release', 'Outcome',
                   '[]', '[]', ?, '[]', '[]', '[]', '[]', '[]', '[]', '[]', '',
                   'definition/v1', 'render/v1', 'exact rendered bytes', ?,
                   'definition-render-digest', 'user', 'test-user-id', '[]', ?)",
    )
    .bind(release_id)
    .bind(milestone_revision_id)
    .bind(milestone_id)
    .bind(charter_revision_id)
    .bind(&snapshot.milestone_definition_digest)
    .bind(now)
    .execute(pool)
    .await
    .expect("frozen milestone definition");
    sqlx::query(
        "INSERT INTO project_release_history_readiness (
             release_id, id, project_id, milestone_id, definition_revision_id,
             baseline_id, baseline_revision_id, baseline_digest,
             release_policy_revision, release_policy_digest, input_manifest_json,
             event_watermark, outcome, blocking_reasons_json, check_results_json,
             waiver_manifest_json, evidence_manifest_json, commit_context_json,
             computing_policy_revision, readiness_digest, principal_type,
             principal_id, authorization_basis, authorization_action,
             authorization_occurred_at, expected_milestone_version, explicit_event,
             idempotency_key, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '{}', ?, 'ready', '[]', '[]',
                   '[]', '[]', '{}', 'compute-r1', ?, 'user', 'test-user-id',
                   'exact_user_approval', ?, ?, 1, ?, ?, ?)",
    )
    .bind(release_id)
    .bind(readiness_id)
    .bind(project_id)
    .bind(milestone_id)
    .bind(milestone_revision_id)
    .bind(baseline_id)
    .bind(baseline_revision_id)
    .bind(&snapshot.baseline_digest)
    .bind(&snapshot.release_policy_revision)
    .bind(&snapshot.release_policy_digest)
    .bind(&snapshot.source_event_watermark)
    .bind(&snapshot.readiness_digest)
    .bind(action)
    .bind(now)
    .bind(&snapshot.authorization.event_id)
    .bind(&snapshot.idempotency_key)
    .bind(now)
    .execute(pool)
    .await
    .expect("frozen readiness identity");
    sqlx::query(
        "INSERT INTO project_release_history_charter
         (release_id, charter_id, revision_id, content_digest, render_version, rendered_digest)
         VALUES (?, ?, ?, ?, 'render/v1', 'charter-render-digest')",
    )
    .bind(release_id)
    .bind(charter_id)
    .bind(charter_revision_id)
    .bind(&snapshot.charter_revision.content_digest)
    .execute(pool)
    .await
    .expect("frozen Charter ArtifactRef");

    for (trigger, table) in [
        ("pr11_retired_insert_project_release", "project_release"),
        (
            "project_release_history_snapshot_insert_guard",
            "project_release_history_milestone",
        ),
        (
            "project_release_history_definition_insert_guard",
            "project_release_history_definition",
        ),
        (
            "project_release_history_readiness_insert_guard",
            "project_release_history_readiness",
        ),
        (
            "project_release_history_charter_insert_guard",
            "project_release_history_charter",
        ),
    ] {
        sqlx::query(&format!(
            "CREATE TRIGGER {trigger} BEFORE INSERT ON {table}
             BEGIN SELECT RAISE(ABORT, 'immutable historical fixture'); END"
        ))
        .execute(pool)
        .await
        .expect("restore immutable-history write fence");
    }

    ProjectRelease {
        id: release_id.to_owned(),
        project_id: project_id.to_owned(),
        milestone_id: milestone_id.to_owned(),
        release_sequence: 1,
        release_identity: snapshot.release_identity.clone(),
        snapshot,
        version: 1,
        created_at: now.to_owned(),
    }
}
