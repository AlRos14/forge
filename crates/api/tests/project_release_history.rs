#![allow(dead_code)]

mod common;

use api_types::{
    ArtifactRef, AuthorizationProvenance, DecisionRecordState, EvidenceAvailability, EvidencePin,
    PrincipalKind, PrincipalRef, ProjectRelease, ProjectReleaseListResponse, ProjectResponse,
    ReleaseDecisionReference, ReleaseEvidenceAvailability, ReleaseSnapshot, ReleaseTaskReference,
    ReleaseValidationReference,
};
use axum::{http::Method, http::StatusCode};
use serde_json::json;
use std::{fs, path::Path};

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
    sqlx::query(
        "INSERT INTO media_asset_tombstone (
             id, asset_id, release_id, release_pin_id, previous_checksum,
             previous_availability, availability, principal_type, principal_id,
             reason, authorization_basis, authorization_action, explicit_event,
             authorization_occurred_at, idempotency_key, mutation_fingerprint,
             created_at
         ) VALUES (
             'history-tombstone-1', 'history-asset-1', 'history-release-1',
             'history-pin-1', 'asset-checksum-1', 'available', 'evidence_unavailable',
             'user', 'test-user-id', 'source removed', 'exact_user_approval',
             'project.media.purge', 'history-tombstone-event',
             '2026-10-13T12:05:00Z', 'history-tombstone-key', 'fingerprint-1',
             '2026-10-13T12:05:00Z'
         )",
    )
    .execute(harness.state.db.pool())
    .await
    .expect("audited historical evidence tombstone persists");
    let fetched: ProjectRelease = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/releases/{}", project.id, release.id),
        &token,
        StatusCode::OK,
    )
    .await;
    let mut expected_with_availability_overlay = release.clone();
    expected_with_availability_overlay.snapshot.evidence_pins[0].availability_projection =
        ReleaseEvidenceAvailability::EvidenceUnavailable;
    assert_eq!(fetched, expected_with_availability_overlay);
    assert_eq!(
        fetched.snapshot.snapshot_digest, release.snapshot.snapshot_digest,
        "read-time availability overlay does not rewrite the frozen snapshot digest"
    );
    let listed: ProjectReleaseListResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!(
            "/api/v1/projects/{}/milestones/{}/releases",
            project.id, release.milestone_id
        ),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items, vec![expected_with_availability_overlay]);
    assert!(!listed.has_more);
    assert!(listed.next_cursor.is_none());

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

#[tokio::test]
async fn v120_release_history_survives_v121_v132_and_public_get_and_list() {
    let workspace = common::TestDir::new("project-release-v120-upgrade");
    let v120_dir = workspace.path().join("migrations-v120");
    let v132_dir = workspace.path().join("migrations-v132");
    fs::create_dir_all(&v120_dir).expect("V120 migration directory");
    fs::create_dir_all(&v132_dir).expect("V132 migration directory");
    copy_migrations_through(120, &v120_dir);
    copy_migrations_through(132, &v132_dir);

    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("migration fixture pool");
    db::run_migrations_from(&pool, &v120_dir)
        .await
        .expect("fresh database reaches V120");
    let expected = seed_authentic_v120_release_history(&pool).await;
    assert_eq!(
        expected.len(),
        2,
        "fixture has multiple releases per milestone"
    );

    let captured_release_rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, snapshot_digest, document_revisions_json,
                decision_ids_json, evidence_references_json
         FROM project_release WHERE project_id = 'v120-history-project'
         ORDER BY release_revision",
    )
    .fetch_all(&pool)
    .await
    .expect("capture exact V120 public Release bytes");
    assert_eq!(captured_release_rows.len(), expected.len());
    for (row, release) in captured_release_rows.iter().zip(expected.iter()) {
        assert_eq!(row.0, release.id);
        assert_eq!(row.1, release.snapshot.snapshot_digest);
        assert_eq!(
            row.2,
            serde_json::to_string(&release.snapshot.document_revisions).unwrap()
        );
        assert_eq!(
            row.3,
            serde_json::to_string(&release.snapshot.included_decisions).unwrap()
        );
        let mut frozen_pins = release.snapshot.evidence_pins.clone();
        for pin in &mut frozen_pins {
            pin.availability_projection = ReleaseEvidenceAvailability::Available;
        }
        assert_eq!(row.4, serde_json::to_string(&frozen_pins).unwrap());
    }

    db::run_migrations_from(&pool, &v132_dir)
        .await
        .expect("V121-V132 cutover");
    db::run_migrations_from(&pool, &v132_dir)
        .await
        .expect("migration restart is idempotent");

    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version BETWEEN 121 AND 132")
            .fetch_one(&pool)
            .await
            .expect("PR13 migration receipts");
    assert_eq!(receipt_count, 12);
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign_key_check after PR13");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check after PR13");
    assert_eq!(integrity, "ok");

    for table in [
        "project_milestone",
        "project_milestone_revision",
        "project_readiness_snapshot",
        "project_charter",
        "project_charter_revision",
        "project_execution_baseline",
        "project_execution_baseline_revision",
        "project_media_attachment",
    ] {
        let present: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("retired Project OS table lookup");
        assert_eq!(
            present, 0,
            "reader operates after {table} is physically dropped"
        );
    }

    let harness = common::test_app_with_pool(pool.clone(), workspace.path(), "release-v120").await;
    let token = common::test_jwt();
    for release in &expected {
        let fetched: ProjectRelease = common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            &format!(
                "/api/v1/projects/{}/releases/{}",
                release.project_id, release.id
            ),
            &token,
            StatusCode::OK,
        )
        .await;
        assert_eq!(fetched, *release, "V120 snapshot survives the public GET");
    }
    let listed: ProjectReleaseListResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/projects/v120-history-project/milestones/v120-history-milestone/releases",
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items, expected);
    assert!(!listed.has_more);
    assert!(listed.next_cursor.is_none());
    assert_eq!(
        listed.items[1].snapshot.evidence_pins[0].availability_projection,
        ReleaseEvidenceAvailability::EvidenceUnavailable,
        "tombstone availability is projected without changing frozen digest"
    );

    let migrated_release_rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, snapshot_digest, document_revisions_json,
                decision_ids_json, evidence_references_json
         FROM project_release WHERE project_id = 'v120-history-project'
         ORDER BY release_revision",
    )
    .fetch_all(&pool)
    .await
    .expect("capture Release fields after migration");
    assert_eq!(migrated_release_rows, captured_release_rows);
    let references_per_release: Vec<(String, i64)> = sqlx::query_as(
        "SELECT release_id, COUNT(*) FROM project_release_history_reference
         GROUP BY release_id ORDER BY release_id",
    )
    .fetch_all(&pool)
    .await
    .expect("archived Release references");
    assert_eq!(references_per_release.len(), 2);
    assert!(references_per_release.iter().all(|(_, count)| *count == 5));

    sqlx::query("DROP TRIGGER project_release_immutable_update")
        .execute(&pool)
        .await
        .expect("test-only digest corruption permission");
    sqlx::query(
        "UPDATE project_release SET snapshot_digest = 'incorrect-digest'
         WHERE id = 'v120-release-r1'",
    )
    .execute(&pool)
    .await
    .expect("corrupt a copied digest for the fail-closed check");
    let corrupt_digest = common::raw_empty_request(
        &harness.app,
        Method::GET,
        "/api/v1/projects/v120-history-project/releases/v120-release-r1",
    )
    .await;
    assert_ne!(
        corrupt_digest.status(),
        StatusCode::OK,
        "incorrect frozen digest must not produce a public Release"
    );

    sqlx::query("DROP TRIGGER project_release_history_definition_delete_guard")
        .execute(&pool)
        .await
        .expect("test-only incomplete snapshot permission");
    sqlx::query(
        "DELETE FROM project_release_history_definition
         WHERE release_id = 'v120-release-r2'",
    )
    .execute(&pool)
    .await
    .expect("remove a required historical definition for the fail-closed check");
    let incomplete_snapshot = common::raw_empty_request(
        &harness.app,
        Method::GET,
        "/api/v1/projects/v120-history-project/releases/v120-release-r2",
    )
    .await;
    assert_ne!(
        incomplete_snapshot.status(),
        StatusCode::OK,
        "incomplete archive must not return a fabricated Release"
    );
}

fn copy_migrations_through(limit: i64, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../db/migrations");
    for entry in fs::read_dir(source).expect("migration directory") {
        let path = entry.expect("migration entry").path();
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((version, _)) = stem.split_once("__") else {
            continue;
        };
        let version = version
            .strip_prefix('V')
            .expect("migration prefix")
            .parse::<i64>()
            .expect("migration number");
        if version <= limit {
            fs::copy(&path, destination.join(path.file_name().expect("filename")))
                .expect("migration copied");
        }
    }
}

async fn seed_authentic_v120_release_history(pool: &sqlx::SqlitePool) -> Vec<ProjectRelease> {
    const PROJECT: &str = "v120-history-project";
    const MILESTONE: &str = "v120-history-milestone";
    const CHARTER: &str = "v120-history-charter";
    const CHARTER_REVISION: &str = "v120-history-charter-r7";
    const BASELINE: &str = "v120-history-baseline";
    const BASELINE_REVISION: &str = "v120-history-baseline-r2";
    const ASSET: &str = "v120-history-shared-asset";
    const ATTACHMENT: &str = "v120-history-shared-attachment";
    const NOW: &str = "2026-10-13T10:00:00Z";
    const SCHEMA: &str = "forge.milestone-release/v1";
    const ACTION: &str = "project.milestone.release";

    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('test-user-id', 'test@example.com', 'fixture', ?, ?)",
    )
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 release owner");
    sqlx::query(
        "INSERT INTO project (
             id, name, settings, workflow_definition, owner_id, created_at, updated_at
         ) VALUES (?, 'V120 immutable history', '{}', '{}', 'test-user-id', ?, ?)",
    )
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 Project owned by test principal");
    sqlx::query(
        "INSERT INTO project_member (id, project_id, user_id, role, created_at, updated_at)
         VALUES ('v120-history-owner-membership', ?, 'test-user-id', 'owner', ?, ?)",
    )
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 Project owner membership");

    for trigger in [
        "pr11_retired_insert_project_charter",
        "pr11_retired_update_project_charter",
        "pr11_retired_insert_project_charter_revision",
        "pr11_retired_insert_project_execution_baseline",
        "pr11_retired_update_project_execution_baseline",
        "pr11_retired_insert_project_execution_baseline_revision",
        "pr11_retired_update_project_execution_baseline_revision",
        "pr11_retired_insert_project_milestone",
        "pr11_retired_insert_project_milestone_revision",
        "pr11_retired_insert_project_readiness_snapshot",
        "pr11_retired_insert_project_release",
        "pr11_retired_insert_project_release_reference",
        "pr11_retired_insert_project_release_media_pin",
    ] {
        sqlx::query(&format!("DROP TRIGGER IF EXISTS {trigger}"))
            .execute(pool)
            .await
            .expect("open V120-only immutable history fixture");
    }

    sqlx::query(
        "INSERT INTO project_charter (
             id, account_id, project_id, project_mode, maturity, lifecycle,
             created_at, updated_at
         ) VALUES (?, 'test-user-id', ?, 'standard', 'mvp', 'attached', ?, ?)",
    )
    .bind(CHARTER)
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 Charter identity");
    sqlx::query(
        "INSERT INTO project_charter_revision (
             id, charter_id, revision, lifecycle, schema_version, render_version,
             content_json, rendered_view, author_type, author_id,
             content_digest, rendered_digest, created_at
         ) VALUES (?, ?, 7, 'approved', 'charter/v1', 'render/v7',
                   '{ \"raw\" : { \"revision\" : 7 } }', 'frozen charter bytes',
                   'user', 'test-user-id', 'charter-content-digest',
                   'charter-render-digest', ?)",
    )
    .bind(CHARTER_REVISION)
    .bind(CHARTER)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 exact Charter revision");
    sqlx::query("UPDATE project_charter SET current_approved_revision_id = ? WHERE id = ?")
        .bind(CHARTER_REVISION)
        .bind(CHARTER)
        .execute(pool)
        .await
        .expect("V120 approved Charter pointer");

    sqlx::query(
        "INSERT INTO project_milestone (
             id, project_id, milestone_sequence, milestone_key, display_label,
             lifecycle, created_at, updated_at
         ) VALUES (?, ?, 4, 'M004', 'Frozen milestone identity', 'released', ?, ?)",
    )
    .bind(MILESTONE)
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 milestone");
    sqlx::query(
        "INSERT INTO project_execution_baseline (id, project_id, lifecycle, created_at, updated_at)
         VALUES (?, ?, 'active', ?, ?)",
    )
    .bind(BASELINE)
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 baseline");
    sqlx::query(
        "INSERT INTO project_execution_baseline_revision (
             id, baseline_id, revision, lifecycle, charter_revision_id,
             release_policy_revision, release_policy_digest, schema_version,
             render_version, rendered_view, content_digest, rendered_digest, created_at
         ) VALUES (?, ?, 2, 'approved', ?, 'policy-r4', 'policy-digest-r4',
                   'baseline/v2', 'render/v2', 'frozen baseline bytes',
                   'baseline-content-digest', 'baseline-render-digest', ?)",
    )
    .bind(BASELINE_REVISION)
    .bind(BASELINE)
    .bind(CHARTER_REVISION)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("V120 baseline revision");
    sqlx::query("UPDATE project_execution_baseline SET current_revision_id = ? WHERE id = ?")
        .bind(BASELINE_REVISION)
        .bind(BASELINE)
        .execute(pool)
        .await
        .expect("V120 current baseline pointer");

    sqlx::query(
        "INSERT INTO media_asset (
             id, project_id, display_filename, content_type, byte_size,
             storage_key, checksum, created_at, updated_at
         ) VALUES (?, ?, 'shared-evidence.png', 'image/png', 84,
                   'history/shared-evidence.png', 'shared-asset-checksum', ?, ?)",
    )
    .bind(ASSET)
    .bind(PROJECT)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("shared V120 evidence asset");
    sqlx::query(
        "INSERT INTO project_media_attachment (
             id, project_id, asset_id, attachment_kind, milestone_id,
             caption, evidence_kind, checksum, author_type, author_id,
             created_at, updated_at
         ) VALUES (?, ?, ?, 'evidence', ?, 'frozen proof', 'screenshot',
                   'shared-attachment-digest', 'user', 'test-user-id', ?, ?)",
    )
    .bind(ATTACHMENT)
    .bind(PROJECT)
    .bind(ASSET)
    .bind(MILESTONE)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .expect("shared V120 Project OS attachment");

    let released_by = PrincipalRef {
        kind: PrincipalKind::User,
        id: "test-user-id".to_owned(),
        display_name: None,
    };
    let authorization_for = |event_id: String, occurred_at: &str| AuthorizationProvenance {
        principal: released_by.clone(),
        authorization_basis: "explicit-user-approval".to_owned(),
        action: ACTION.to_owned(),
        event_id,
        occurred_at: occurred_at.to_owned(),
    };
    let task_ref = ReleaseTaskReference {
        task_id: "historical-task-without-canonical-row".to_owned(),
        task_version: 7,
        task_type: "implementation".to_owned(),
        task_state: "active".to_owned(),
        acceptance_check_ids: vec!["check-frozen-1".to_owned()],
    };
    let document_ref = ArtifactRef {
        artifact_id: "historical-document".to_owned(),
        revision_id: "historical-document-r3".to_owned(),
        content_digest: "document-content-digest-r3".to_owned(),
        render_version: Some("document-render-v2".to_owned()),
        render_digest: Some("document-render-digest-r3".to_owned()),
    };

    let mut expected = Vec::new();
    for (revision, release_id, readiness_id, pin_id, tombstone) in [
        (
            1_i64,
            "v120-release-r1",
            "v120-readiness-r1",
            "v120-pin-r1",
            false,
        ),
        (
            2_i64,
            "v120-release-r2",
            "v120-readiness-r2",
            "v120-pin-r2",
            true,
        ),
    ] {
        let definition_id = format!("v120-milestone-definition-r{revision}");
        let readiness_digest = format!("readiness-digest-r{revision}");
        let event_id = format!("v120-release-event-r{revision}");
        let released_at = if revision == 1 {
            "2026-10-13T10:10:00Z"
        } else {
            "2026-10-13T10:20:00Z"
        };
        let authorization = authorization_for(event_id.clone(), released_at);
        let validation = ReleaseValidationReference {
            validation_id: format!("historical-validation-r{revision}"),
            result_digest: format!("validation-result-digest-r{revision}"),
            evaluated_at: released_at.to_owned(),
            principal: released_by.clone(),
            authorization: authorization.clone(),
            status: api_types::AcceptanceCheckResultStatus::Pass,
            result: "all frozen checks passed".to_owned(),
            input_digest: format!("validation-input-digest-r{revision}"),
            governing_revision_ids: vec![definition_id.clone()],
        };
        let decision = ReleaseDecisionReference {
            decision_id: format!("historical-decision-r{revision}"),
            state: DecisionRecordState::Active,
            digest: format!("decision-digest-r{revision}"),
            rationale: "historical rationale bytes".to_owned(),
            authorization: authorization.clone(),
            affected_milestone_id: Some(MILESTONE.to_owned()),
            affected_check_id: Some("check-frozen-1".to_owned()),
        };
        let pin = EvidencePin {
            id: pin_id.to_owned(),
            release_id: release_id.to_owned(),
            attachment_id: ATTACHMENT.to_owned(),
            asset_id: ASSET.to_owned(),
            attachment_digest: "shared-attachment-digest".to_owned(),
            asset_checksum: "shared-asset-checksum".to_owned(),
            availability: EvidenceAvailability::Available,
            availability_projection: ReleaseEvidenceAvailability::Available,
            task_media_id: None,
            stable_project_url: Some("/api/v1/media/shared-history-asset".to_owned()),
            pinned_at: released_at.to_owned(),
        };
        let mut snapshot = ReleaseSnapshot {
            schema_version: SCHEMA.to_owned(),
            project_id: PROJECT.to_owned(),
            milestone_id: MILESTONE.to_owned(),
            milestone_canonical_id: "M004".to_owned(),
            release_revision: revision,
            release_identity: format!("M004-r{revision}"),
            milestone_definition_revision_id: definition_id.clone(),
            milestone_definition_digest: format!("milestone-definition-digest-r{revision}"),
            expected_milestone_version: 12 + revision,
            display_label: Some("Frozen milestone identity".to_owned()),
            summary: format!("historical summary revision {revision}"),
            changelog: vec![
                format!("line {revision}: exact release bytes"),
                "second nontrivial line".to_owned(),
            ],
            known_issues: vec![format!("known issue revision {revision}")],
            readiness_snapshot_id: readiness_id.to_owned(),
            readiness_digest: readiness_digest.clone(),
            source_event_watermark: format!("event-watermark-r{revision}-opaque"),
            baseline_id: BASELINE.to_owned(),
            baseline_revision_id: BASELINE_REVISION.to_owned(),
            baseline_digest: "baseline-content-digest".to_owned(),
            charter_revision: ArtifactRef {
                artifact_id: CHARTER.to_owned(),
                revision_id: CHARTER_REVISION.to_owned(),
                content_digest: "charter-content-digest".to_owned(),
                render_version: Some("render/v7".to_owned()),
                render_digest: Some("charter-render-digest".to_owned()),
            },
            document_revisions: vec![document_ref.clone()],
            included_decisions: vec![decision.clone()],
            included_tasks: vec![task_ref.clone()],
            validation_results: vec![validation.clone()],
            repository_references: Vec::new(),
            evidence_pins: vec![pin.clone()],
            waived_check_ids: vec![format!("waiver-r{revision}")],
            release_policy_revision: "policy-r4".to_owned(),
            release_policy_digest: "policy-digest-r4".to_owned(),
            released_by: released_by.clone(),
            authorization: authorization.clone(),
            released_at: released_at.to_owned(),
            idempotency_key: format!("v120-release-key-r{revision}"),
            snapshot_digest: String::new(),
        };
        let mut digest_input = serde_json::to_value(&snapshot).expect("snapshot JSON");
        digest_input
            .as_object_mut()
            .expect("snapshot object")
            .remove("snapshot_digest");
        snapshot.snapshot_digest = api_types::canonical_digest_with_schema(SCHEMA, &digest_input)
            .expect("authentic V120 snapshot digest");

        sqlx::query(
            "INSERT INTO project_milestone_revision (
                 id, milestone_id, revision, base_revision, lifecycle, display_label,
                 outcome, included_scope_json, excluded_scope_json, charter_revision_id,
                 document_revisions_json, task_selection_json, dependencies_json,
                 risks_json, acceptance_checks_json, evidence_requirements_json,
                 known_issues_json, change_summary, schema_version, render_version,
                 rendered_view, content_digest, rendered_digest, author_type,
                 author_id, source_refs_json, created_at
             ) VALUES (?, ?, ?, 0, 'approved', ?, 'frozen outcome',
                       '[ \"scope\" ]', '[ \"excluded\" ]', ?, ?, '[ \"task\" ]',
                       '[ \"dependency\" ]', '[ \"risk\" ]', '[ \"check\" ]',
                       '[ \"evidence\" ]', '[ \"known\" ]', 'exact change bytes',
                       'definition/v4', 'render/v5', 'frozen rendered definition',
                       ?, 'definition-render-digest', 'user', 'test-user-id',
                       '[ \"source\" ]', ?)",
        )
        .bind(&definition_id)
        .bind(MILESTONE)
        .bind(revision)
        .bind(snapshot.display_label.as_deref())
        .bind(CHARTER_REVISION)
        .bind(serde_json::to_string(&snapshot.document_revisions).unwrap())
        .bind(&snapshot.milestone_definition_digest)
        .bind(NOW)
        .execute(pool)
        .await
        .expect("exact V120 milestone revision");

        sqlx::query(
            "INSERT INTO project_readiness_snapshot (
                 id, project_id, milestone_id, definition_revision_id, baseline_id,
                 baseline_revision_id, baseline_digest, release_policy_revision,
                 release_policy_digest, input_manifest_json, event_watermark, outcome,
                 blocking_reasons_json, check_results_json, waiver_manifest_json,
                 evidence_manifest_json, commit_context_json, computing_policy_revision,
                 readiness_digest, principal_type, principal_id, authorization_basis,
                 authorization_action, authorization_occurred_at,
                 expected_milestone_version, explicit_event, idempotency_key, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, '[ { \"opaque\" : true } ]', ?, 'ready',
                       '[ \"raw reason\" ]', '[ { \"raw check\" : 1 } ]',
                       '[ \"raw waiver\" ]', '[ \"raw evidence\" ]',
                       '{ \"commit\" : \"abc123\" }', 'compute-r6', ?, 'user',
                       'test-user-id', 'explicit-user-approval', ?, ?, ?, ?, ?, ?)",
        )
        .bind(readiness_id)
        .bind(PROJECT)
        .bind(MILESTONE)
        .bind(&definition_id)
        .bind(BASELINE)
        .bind(BASELINE_REVISION)
        .bind(&snapshot.baseline_digest)
        .bind(&snapshot.release_policy_revision)
        .bind(&snapshot.release_policy_digest)
        .bind(&snapshot.source_event_watermark)
        .bind(&snapshot.readiness_digest)
        .bind(ACTION)
        .bind(released_at)
        .bind(snapshot.expected_milestone_version)
        .bind(&event_id)
        .bind(&snapshot.idempotency_key)
        .bind(NOW)
        .execute(pool)
        .await
        .expect("exact V120 readiness receipt");

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
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       '[]', ?, ?, 'user', 'test-user-id', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(release_id)
        .bind(PROJECT)
        .bind(MILESTONE)
        .bind(5 + revision)
        .bind(revision)
        .bind(&snapshot.release_identity)
        .bind(&definition_id)
        .bind(readiness_id)
        .bind(&snapshot.readiness_digest)
        .bind(BASELINE)
        .bind(BASELINE_REVISION)
        .bind(&snapshot.baseline_digest)
        .bind(&snapshot.release_policy_revision)
        .bind(&snapshot.release_policy_digest)
        .bind(&snapshot.summary)
        .bind(serde_json::to_string(&snapshot.changelog).unwrap())
        .bind(serde_json::to_string(&snapshot.known_issues).unwrap())
        .bind(CHARTER_REVISION)
        .bind(serde_json::to_string(&snapshot.document_revisions).unwrap())
        .bind(serde_json::to_string(&snapshot.included_decisions).unwrap())
        .bind(serde_json::to_string(&snapshot.included_tasks).unwrap())
        .bind(serde_json::to_string(&snapshot.validation_results).unwrap())
        .bind(serde_json::to_string(&snapshot.evidence_pins).unwrap())
        .bind(serde_json::to_string(&snapshot.waived_check_ids).unwrap())
        .bind(&snapshot.authorization.authorization_basis)
        .bind(ACTION)
        .bind(&event_id)
        .bind(released_at)
        .bind(SCHEMA)
        .bind(&snapshot.snapshot_digest)
        .bind(&snapshot.idempotency_key)
        .bind(released_at)
        .execute(pool)
        .await
        .expect("authentic immutable V120 Release row");

        let reference_rows = [
            (
                "task",
                task_ref.task_id.clone(),
                Some(task_ref.task_version.to_string()),
                Some(task_ref.task_state.clone()),
                None,
                serde_json::to_string(&task_ref).unwrap(),
            ),
            (
                "validation",
                validation.validation_id.clone(),
                Some(validation.evaluated_at.clone()),
                Some("pass".to_owned()),
                Some(validation.result_digest.clone()),
                serde_json::to_string(&validation).unwrap(),
            ),
            (
                "document",
                document_ref.revision_id.clone(),
                None,
                Some("approved".to_owned()),
                Some(document_ref.content_digest.clone()),
                serde_json::to_string(&document_ref).unwrap(),
            ),
            (
                "decision",
                decision.decision_id.clone(),
                None,
                Some("active".to_owned()),
                Some(decision.digest.clone()),
                serde_json::to_string(&decision).unwrap(),
            ),
            (
                "known_issue",
                snapshot.known_issues[0].clone(),
                None,
                None,
                None,
                "{}".to_owned(),
            ),
        ];
        for (ordinal, (kind, record_id, version, state, digest, metadata)) in
            reference_rows.into_iter().enumerate()
        {
            sqlx::query(
                "INSERT INTO project_release_reference (
                     release_id, ordinal, reference_kind, record_id,
                     record_version, record_state, record_digest, metadata_json
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(release_id)
            .bind(ordinal as i64)
            .bind(kind)
            .bind(record_id)
            .bind(version)
            .bind(state)
            .bind(digest)
            .bind(metadata)
            .execute(pool)
            .await
            .expect("exact V120 historical typed reference");
        }

        let pin_digest =
            api_types::canonical_digest_with_schema(SCHEMA, &serde_json::to_string(&pin).unwrap())
                .expect("V120 media pin digest");
        sqlx::query(
            "INSERT INTO project_release_media_pin (
                 id, project_id, release_id, asset_id, attachment_id,
                 asset_checksum, attachment_digest, availability, pin_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 'available', ?, ?)",
        )
        .bind(pin_id)
        .bind(PROJECT)
        .bind(release_id)
        .bind(ASSET)
        .bind(ATTACHMENT)
        .bind(&pin.asset_checksum)
        .bind(&pin.attachment_digest)
        .bind(pin_digest)
        .bind(released_at)
        .execute(pool)
        .await
        .expect("exact V120 release media pin");
        if tombstone {
            sqlx::query(
                "INSERT INTO media_asset_tombstone (
                     id, asset_id, release_id, release_pin_id, previous_checksum,
                     previous_availability, availability, principal_type, principal_id,
                     reason, authorization_basis, authorization_action, explicit_event,
                     authorization_occurred_at, idempotency_key, mutation_fingerprint,
                     created_at
                 ) VALUES ('v120-history-tombstone', ?, ?, ?, ?, 'available',
                           'evidence_unavailable', 'user', 'test-user-id',
                           'historical source unavailable', 'explicit-user-approval',
                           'project.media.purge', 'v120-history-tombstone-event',
                           '2026-10-13T10:30:00Z', 'v120-history-tombstone-key',
                           'v120-history-tombstone-fingerprint', '2026-10-13T10:30:00Z')",
            )
            .bind(ASSET)
            .bind(release_id)
            .bind(pin_id)
            .bind(&pin.asset_checksum)
            .execute(pool)
            .await
            .expect("V120 tombstone is rekeyed to archived pin by migration");
        }

        let mut public_snapshot = snapshot.clone();
        if tombstone {
            public_snapshot.evidence_pins[0].availability_projection =
                ReleaseEvidenceAvailability::EvidenceUnavailable;
        }
        expected.push(ProjectRelease {
            id: release_id.to_owned(),
            project_id: PROJECT.to_owned(),
            milestone_id: MILESTONE.to_owned(),
            release_sequence: 5 + revision,
            release_identity: snapshot.release_identity.clone(),
            snapshot: public_snapshot,
            version: revision,
            created_at: released_at.to_owned(),
        });
    }
    expected
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
