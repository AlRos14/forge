use db::{create_sqlite_pool, run_migrations_from};
use sqlx::SqlitePool;
use std::{fs, path::Path};

fn copy_migrations_through(limit: i64, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
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

async fn seed_v120_release(pool: &SqlitePool) {
    let now = "2026-10-12T12:00:00Z";
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('pr13-owner', 'pr13@example.test', 'fixture', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Release owner");
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, owner_id, created_at, updated_at)
         VALUES ('pr13-project', 'Historical release', '{}', '{}', 'pr13-owner', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Release Project");

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
            .expect("allow seeding immutable V120 history");
    }

    sqlx::query(
        "INSERT INTO project_charter
         (id, account_id, project_id, project_mode, maturity, lifecycle, created_at, updated_at)
         VALUES ('pr13-charter', 'pr13-owner', 'pr13-project', 'standard', 'mvp', 'attached', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Charter");
    sqlx::query(
        "INSERT INTO project_charter_revision
         (id, charter_id, revision, lifecycle, schema_version, render_version,
          content_json, rendered_view, author_type, author_id, content_digest,
          rendered_digest, created_at)
         VALUES ('pr13-charter-r7', 'pr13-charter', 7, 'approved', 'charter/v1',
                 'render/v3', '{\"raw\": 7}', 'frozen charter bytes', 'user',
                 'pr13-owner', 'charter-content-digest', 'charter-render-digest', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Charter revision");
    sqlx::query(
        "UPDATE project_charter SET current_approved_revision_id = 'pr13-charter-r7'
         WHERE id = 'pr13-charter'",
    )
    .execute(pool)
    .await
    .expect("historical Charter approved revision pointer");
    sqlx::query(
        "INSERT INTO project_milestone
         (id, project_id, milestone_sequence, milestone_key, display_label,
          lifecycle, created_at, updated_at)
         VALUES ('pr13-milestone', 'pr13-project', 4, 'M004', 'Frozen milestone',
                 'released', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical milestone identity");
    sqlx::query(
        "INSERT INTO project_milestone_revision
         (id, milestone_id, revision, base_revision, lifecycle, display_label,
          outcome, included_scope_json, excluded_scope_json, charter_revision_id,
          document_revisions_json, task_selection_json, dependencies_json,
          risks_json, acceptance_checks_json, evidence_requirements_json,
          known_issues_json, change_summary, schema_version, render_version,
          rendered_view, content_digest, rendered_digest, author_type, author_id,
          source_refs_json, created_at)
         VALUES ('pr13-milestone-r9', 'pr13-milestone', 9, 0, 'approved',
                 'Definition label', 'Frozen outcome', '[\"scope\"]', '[\"excluded\"]',
                 'pr13-charter-r7', '[\"doc-rev\"]', '[\"task-ref\"]',
                 '[\"dependency\"]', '[\"risk\"]', '[\"check\"]',
                 '[\"evidence\"]', '[\"issue\"]', 'Frozen change summary',
                 'definition/v4', 'render/v5', 'exact rendered definition',
                 'definition-content-digest', 'definition-rendered-digest',
                 'user', 'pr13-owner', '[\"source\"]', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical milestone definition");
    sqlx::query(
        "INSERT INTO project_execution_baseline
         (id, project_id, lifecycle, created_at, updated_at)
         VALUES ('pr13-baseline', 'pr13-project', 'draft', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical baseline");
    sqlx::query(
        "INSERT INTO project_execution_baseline_revision
         (id, baseline_id, revision, lifecycle, charter_revision_id,
          release_policy_revision, release_policy_digest, schema_version,
          render_version, rendered_view, content_digest, rendered_digest, created_at)
         VALUES ('pr13-baseline-r2', 'pr13-baseline', 2, 'approved',
                 'pr13-charter-r7', 'policy-r4', 'policy-digest-r4',
                 'baseline/v2', 'render/v2', 'frozen baseline bytes',
                 'baseline-digest-r2', 'baseline-render-digest-r2', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical baseline revision");
    sqlx::query(
        "UPDATE project_execution_baseline
         SET lifecycle = 'active', current_revision_id = 'pr13-baseline-r2'
         WHERE id = 'pr13-baseline'",
    )
    .execute(pool)
    .await
    .expect("activate exact historical baseline revision");
    sqlx::query(
        "INSERT INTO project_readiness_snapshot
         (id, project_id, milestone_id, definition_revision_id, baseline_id,
          baseline_revision_id, baseline_digest, release_policy_revision,
          release_policy_digest, input_manifest_json, event_watermark, outcome,
          blocking_reasons_json, check_results_json, waiver_manifest_json,
          evidence_manifest_json, commit_context_json, computing_policy_revision,
          readiness_digest, principal_type, principal_id, authorization_basis,
          authorization_action, authorization_occurred_at, expected_milestone_version,
          explicit_event, idempotency_key, created_at)
         VALUES ('pr13-readiness', 'pr13-project', 'pr13-milestone',
                 'pr13-milestone-r9', 'pr13-baseline', 'pr13-baseline-r2',
                 'baseline-digest-r2', 'policy-r4', 'policy-digest-r4',
                 '[\"raw-input-manifest\"]', 'event-watermark-42', 'ready',
                 '[\"raw-reason\"]', '[\"raw-check\"]', '[\"raw-waiver\"]',
                 '[\"raw-evidence\"]', '{\"commit\":\"abc123\"}',
                 'compute-policy-r6', 'readiness-digest-r6', 'user', 'pr13-owner',
                 'authorization basis', 'project.milestone.release',
                 '2026-10-12T11:59:00Z', 12, 'release approval event',
                 'pr13-readiness-key', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical readiness receipt");
    sqlx::query(
        "INSERT INTO project_release
         (id, project_id, milestone_id, release_sequence, release_revision,
          release_identifier, milestone_revision_id, readiness_snapshot_id,
          readiness_digest, baseline_id, baseline_revision_id, baseline_digest,
          release_policy_revision, release_policy_digest, summary, changelog,
          known_issues_json, charter_revision_id, document_revisions_json,
          decision_ids_json, task_references_json, validation_references_json,
          git_references_json, evidence_references_json, waivers_json,
          releasing_principal_type, releasing_principal_id, authorization_basis,
          authorization_action, explicit_event, authorization_occurred_at,
          schema_version, snapshot_digest, idempotency_key, created_at)
         VALUES ('pr13-release', 'pr13-project', 'pr13-milestone', 6, 1,
                 'M004-r1', 'pr13-milestone-r9', 'pr13-readiness',
                 'readiness-digest-r6', 'pr13-baseline', 'pr13-baseline-r2',
                 'baseline-digest-r2', 'policy-r4', 'policy-digest-r4',
                 'Frozen summary', '[\"line one\",\"line two\"]',
                 '[\"known issue\"]', 'pr13-charter-r7', '[\"raw-doc-ref\"]',
                 '[\"raw-decision-ref\"]', '[\"raw-task-ref\"]',
                 '[\"raw-validation-ref\"]', '[\"raw-git-ref\"]',
                 '[\"raw-evidence-ref\"]', '[\"raw-waiver-ref\"]',
                 'user', 'pr13-owner', 'authorization basis',
                 'project.milestone.release', 'release approval event',
                 '2026-10-12T11:59:00Z', 'forge.milestone-release/v1',
                 'release-snapshot-digest-exact', 'pr13-release-key', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical immutable Release");
    sqlx::query(
        "INSERT INTO project_release_reference
         (release_id, ordinal, reference_kind, record_id, record_version,
          record_state, record_digest, metadata_json)
         VALUES ('pr13-release', 0, 'known_issue', 'known issue', NULL, NULL, NULL, '{}')",
    )
    .execute(pool)
    .await
    .expect("historical Release reference");

    sqlx::query(
        "INSERT INTO media_asset (
             id, project_id, display_filename, content_type, byte_size,
             storage_key, checksum, created_at, updated_at
         ) VALUES ('pr13-asset', 'pr13-project', 'evidence.png', 'image/png', 42,
                   'pr13/evidence.png', 'asset-checksum', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("shared evidence Asset");
    sqlx::query(
        "INSERT INTO project_media_attachment (
             id, project_id, asset_id, attachment_kind, milestone_id,
             caption, evidence_kind, checksum, author_type, author_id,
             created_at, updated_at
         ) VALUES ('pr13-attachment', 'pr13-project', 'pr13-asset', 'evidence',
                   'pr13-milestone', 'frozen evidence', 'screenshot',
                   'attachment-checksum', 'user', 'pr13-owner', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("Project OS evidence attachment");
    sqlx::query(
        "INSERT INTO project_release_media_pin (
             id, project_id, release_id, asset_id, attachment_id,
             asset_checksum, attachment_digest, availability, pin_digest, created_at
         ) VALUES ('pr13-pin', 'pr13-project', 'pr13-release', 'pr13-asset',
                   'pr13-attachment', 'asset-checksum', 'attachment-digest',
                   'available', 'exact-pin-digest', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("exact immutable Release media pin");
}

#[tokio::test]
async fn v121_copies_exact_release_snapshot_and_v122_detaches_project_os_foreign_keys() {
    let temp = tempfile::tempdir().expect("temp dir");
    let v120 = temp.path().join("v120");
    let v122 = temp.path().join("v122");
    let v132 = temp.path().join("v132");
    fs::create_dir_all(&v120).expect("V120 migration dir");
    fs::create_dir_all(&v122).expect("V122 migration dir");
    fs::create_dir_all(&v132).expect("V132 migration dir");
    copy_migrations_through(120, &v120);
    copy_migrations_through(122, &v122);
    copy_migrations_through(132, &v132);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &v120)
        .await
        .expect("V120 baseline");
    seed_v120_release(&pool).await;

    let release_before: (String, String, String, String, String, String) = sqlx::query_as(
        "SELECT r.snapshot_digest, r.document_revisions_json,
                r.decision_ids_json, r.task_references_json,
                r.validation_references_json, r.evidence_references_json
         FROM project_release r WHERE r.id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("V120 Release snapshot");
    let readiness_before: (String, String, String, String) = sqlx::query_as(
        "SELECT input_manifest_json, event_watermark, check_results_json,
                commit_context_json
         FROM project_readiness_snapshot WHERE id = 'pr13-readiness'",
    )
    .fetch_one(&pool)
    .await
    .expect("V120 readiness snapshot");
    let definition_before: (String, String, String, String, String) = sqlx::query_as(
        "SELECT id, display_label, included_scope_json, rendered_view, content_digest
         FROM project_milestone_revision WHERE id = 'pr13-milestone-r9'",
    )
    .fetch_one(&pool)
    .await
    .expect("V120 milestone definition");
    let charter_before: (String, String, String) = sqlx::query_as(
        "SELECT c.id, cr.id, cr.content_digest
         FROM project_charter c JOIN project_charter_revision cr ON cr.charter_id = c.id
         WHERE c.id = 'pr13-charter' AND cr.id = 'pr13-charter-r7'",
    )
    .fetch_one(&pool)
    .await
    .expect("V120 Charter identity");

    run_migrations_from(&pool, &v122)
        .await
        .expect("V121 exact copy and V122 cutover");
    run_migrations_from(&pool, &v132)
        .await
        .expect("V123-V132 destructive cutovers");

    let release_after: (String, String, String, String, String, String) = sqlx::query_as(
        "SELECT snapshot_digest, document_revisions_json, decision_ids_json,
                task_references_json, validation_references_json,
                evidence_references_json
         FROM project_release WHERE id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("detached Release snapshot");
    assert_eq!(release_after, release_before);
    let readiness_after: (String, String, String, String) = sqlx::query_as(
        "SELECT input_manifest_json, event_watermark, check_results_json,
                commit_context_json
         FROM project_release_history_readiness WHERE release_id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("archived readiness identity and bytes");
    assert_eq!(readiness_after, readiness_before);
    let definition_after: (String, String, String, String, String) = sqlx::query_as(
        "SELECT id, display_label, included_scope_json, rendered_view, content_digest
         FROM project_release_history_definition WHERE release_id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("archived definition identity and bytes");
    assert_eq!(definition_after, definition_before);
    let charter_after: (String, String, String) = sqlx::query_as(
        "SELECT charter_id, revision_id, content_digest
         FROM project_release_history_charter WHERE release_id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("archived Charter ArtifactRef");
    assert_eq!(charter_after, charter_before);
    let reference_after: (i64, String, String, String) = sqlx::query_as(
        "SELECT ordinal, reference_kind, record_id, metadata_json
         FROM project_release_history_reference WHERE release_id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("archived exact Release reference");
    assert_eq!(
        reference_after,
        (
            0,
            "known_issue".to_owned(),
            "known issue".to_owned(),
            "{}".to_owned()
        )
    );
    let media_pin_after: (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT id, project_id, release_id, asset_id, attachment_id,
                legacy_task_media_id, asset_checksum, attachment_digest,
                availability, pin_digest, created_at
         FROM project_release_history_media_pin WHERE id = 'pr13-pin'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact media pin survives attachment table removal");
    assert_eq!(
        media_pin_after,
        (
            "pr13-pin".to_owned(),
            "pr13-project".to_owned(),
            "pr13-release".to_owned(),
            "pr13-asset".to_owned(),
            Some("pr13-attachment".to_owned()),
            None,
            "asset-checksum".to_owned(),
            "attachment-digest".to_owned(),
            "available".to_owned(),
            "exact-pin-digest".to_owned(),
            "2026-10-12T12:00:00Z".to_owned(),
        )
    );

    let foreign_keys: Vec<(String, String)> = sqlx::query_as(
        "SELECT \"from\", \"table\" FROM pragma_foreign_key_list('project_release')",
    )
    .fetch_all(&pool)
    .await
    .expect("detached Release foreign keys");
    assert_eq!(
        foreign_keys,
        vec![("project_id".to_owned(), "project".to_owned())]
    );
    let old_release_objects: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN (
             'project_release_reference', 'project_release_media_pin',
             'project_milestone', 'project_milestone_revision',
             'project_readiness_snapshot', 'project_charter',
             'project_charter_revision', 'project_execution_baseline',
             'project_execution_baseline_revision', 'project_media_attachment'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("old Release and Project OS source tables are absent");
    assert_eq!(old_release_objects, 0);
    let retained_snapshot_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN (
             'project_release', 'project_release_history_milestone',
             'project_release_history_definition', 'project_release_history_readiness',
             'project_release_history_charter', 'project_release_history_reference',
             'project_release_history_media_pin'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("self-contained Release history tables remain");
    assert_eq!(retained_snapshot_tables, 7);
    let final_release: (String, String, String, String, String) = sqlx::query_as(
        "SELECT snapshot_digest, milestone_revision_id, readiness_snapshot_id,
                baseline_revision_id, charter_revision_id
         FROM project_release WHERE id = 'pr13-release'",
    )
    .fetch_one(&pool)
    .await
    .expect("Release identity remains after Project OS DROP");
    assert_eq!(
        final_release,
        (
            release_before.0.clone(),
            "pr13-milestone-r9".to_owned(),
            "pr13-readiness".to_owned(),
            "pr13-baseline-r2".to_owned(),
            "pr13-charter-r7".to_owned(),
        )
    );
    let foreign_key_violations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .expect("foreign key check");
    assert_eq!(foreign_key_violations, 0);
    let integrity: String =
        sqlx::query_scalar("SELECT integrity_check FROM pragma_integrity_check")
            .fetch_one(&pool)
            .await
            .expect("integrity check");
    assert_eq!(integrity, "ok");
}
