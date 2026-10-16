use db::{create_sqlite_pool, run_migrations_from};
use sqlx::SqlitePool;
use std::{fs, path::Path};

fn copy_migrations_between(lower_exclusive: i64, upper_inclusive: i64, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source).expect("migration directory reads") {
        let path = entry.expect("migration entry reads").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("sql") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("migration stem");
        let version = stem
            .strip_prefix('V')
            .and_then(|version| version.split_once("__"))
            .and_then(|(version, _)| version.parse::<i64>().ok())
            .expect("migration has a version");
        if version > lower_exclusive && version <= upper_inclusive {
            fs::copy(
                &path,
                destination.join(path.file_name().expect("migration filename")),
            )
            .expect("migration copies");
        }
    }
}

async fn seed_v120_review_history(pool: &SqlitePool) {
    let now = "2026-10-13T10:00:00Z";
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES ('pr13-review-project', 'Review history', '{}', '{\"kind\":\"old-workflow\"}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V120 Project fixture");
    sqlx::query(
        "INSERT INTO task (id, project_id, title, task_type, status, created_at, updated_at)
         VALUES ('pr13-review-task', 'pr13-review-project', 'Review history', 'implementation', 'todo', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V120 Task fixture creates its TaskLifecycle through the historical trigger");
    sqlx::query(
        "INSERT INTO execution (id, task_id, role, purpose, status, created_at, updated_at)
         VALUES ('pr13-review-execution', 'pr13-review-task', 'reviewer', 'review', 'running', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V120 exact historical reviewer Execution");

    let raw_steps = "{ \"ci_steps\" : [ { \"command\" : \"cargo test\", \"exit_code\" : 7 } ] }";
    sqlx::query(
        "INSERT INTO review (
            id, task_id, execution_id, attempt_number, status, step_results_json,
            started_at, finished_at, created_at, updated_at
         ) VALUES (?, ?, ?, 2, 'failed', ?, ?, ?, ?, ?)",
    )
    .bind("pr13-review-row")
    .bind("pr13-review-task")
    .bind("pr13-review-execution")
    .bind(raw_steps)
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V120 legacy Review fixture");
    sqlx::query(
        "INSERT INTO review_evidence_bundle (
            id, review_id, task_id, reviewer_execution_id, plan_revision_id,
            plan_digest, base_sha, head_sha, diff_text, diff_digest,
            ci_results_json, fresh_session, created_at
         ) VALUES (?, ?, ?, ?, NULL, NULL, 'base-sha', 'head-sha', 'raw diff',
                   'diff-digest', '{\"ci\":[]}', 0, ?)",
    )
    .bind("pr13-review-evidence-bundle")
    .bind("pr13-review-row")
    .bind("pr13-review-task")
    .bind("pr13-review-execution")
    .bind(now)
    .execute(pool)
    .await
    .expect("V120 historical Review evidence");
    sqlx::query(
        "INSERT INTO legacy_review_artifact_migration
            (review_id, artifact_id, migration_status, details_json)
         VALUES ('pr13-review-row', NULL, 'content_unrecoverable', '{\"reason\":\"missing exact subject\"}')",
    )
    .execute(pool)
    .await
    .expect("V099 Review provenance receipt");
    sqlx::query(
        "INSERT INTO legacy_ci_validation_migration
            (review_id, step_index, validation_run_id, migration_status, details_json)
         VALUES ('pr13-review-row', -1, NULL, 'no_steps', '{\"reason\":\"empty exact CI history\"}')",
    )
    .execute(pool)
    .await
    .expect("V099 CI provenance receipt");
}

#[tokio::test]
async fn v131_archives_legacy_review_rows_and_detaches_exact_v099_receipts() {
    let migrations = tempfile::tempdir().expect("migration directory creates");
    copy_migrations_between(0, 120, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("V120 baseline applies");
    seed_v120_review_history(&pool).await;

    for version in 121..=131 {
        copy_migrations_between(version - 1, version, migrations.path());
        run_migrations_from(&pool, migrations.path())
            .await
            .unwrap_or_else(|error| {
                panic!("V{version} applies from its previous receipt: {error}")
            });
    }

    let lifecycle_state: String =
        sqlx::query_scalar("SELECT state FROM task_lifecycle WHERE task_id = 'pr13-review-task'")
            .fetch_one(&pool)
            .await
            .expect("TaskLifecycle remains the state authority after V129");
    assert_eq!(lifecycle_state, "ready");
    let archived_task_status: String = sqlx::query_scalar(
        "SELECT json_extract(row_json, '$.status_projection')
         FROM pr13_legacy_record_history
         WHERE source_table = 'task_legacy_fields'
           AND source_identity = 'pr13-review-task'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact legacy Task status remains historical provenance");
    assert_eq!(archived_task_status, "todo");
    let live_status_column: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('task') WHERE name = 'status'")
            .fetch_one(&pool)
            .await
            .expect("Task status column lookup");
    assert_eq!(live_status_column, 0);

    let legacy_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'review'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy Review table lookup");
    assert_eq!(
        legacy_table_count, 0,
        "old Review storage is physically dropped"
    );

    let archived_steps: String = sqlx::query_scalar(
        "SELECT json_extract(row_json, '$.step_results_json')
         FROM pr13_legacy_record_history
         WHERE source_table = 'review' AND source_identity = 'pr13-review-row'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact Review history reads");
    assert_eq!(
        archived_steps,
        "{ \"ci_steps\" : [ { \"command\" : \"cargo test\", \"exit_code\" : 7 } ] }"
    );
    let archived_bundle: String = sqlx::query_scalar(
        "SELECT json_extract(row_json, '$.diff_text')
         FROM pr13_legacy_record_history
         WHERE source_table = 'review_evidence_bundle'
           AND source_identity = 'pr13-review-evidence-bundle'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact Review evidence history reads");
    assert_eq!(archived_bundle, "raw diff");

    let artifact_receipt: (Option<String>, String, String) = sqlx::query_as(
        "SELECT artifact_id, migration_status, details_json
         FROM legacy_review_artifact_migration WHERE review_id = 'pr13-review-row'",
    )
    .fetch_one(&pool)
    .await
    .expect("V099 Review receipt remains");
    assert_eq!(
        artifact_receipt.0, None,
        "no historical Artifact is invented"
    );
    assert_eq!(artifact_receipt.1, "content_unrecoverable");
    assert_eq!(artifact_receipt.2, "{\"reason\":\"missing exact subject\"}");
    let ci_receipt: (i64, Option<String>, String, String) = sqlx::query_as(
        "SELECT step_index, validation_run_id, migration_status, details_json
         FROM legacy_ci_validation_migration WHERE review_id = 'pr13-review-row'",
    )
    .fetch_one(&pool)
    .await
    .expect("V099 CI receipt remains");
    assert_eq!(
        ci_receipt,
        (
            -1,
            None,
            "no_steps".to_owned(),
            "{\"reason\":\"empty exact CI history\"}".to_owned()
        )
    );

    for table in [
        "legacy_review_artifact_migration",
        "legacy_ci_validation_migration",
    ] {
        let source_fk_count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('{table}') WHERE \"from\" = 'review_id'"
        ))
        .fetch_one(&pool)
        .await
        .expect("V099 source FK lookup");
        assert_eq!(source_fk_count, 0, "{table} no longer depends on Review");
    }

    let review_report_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE kind = 'review_report'")
            .fetch_one(&pool)
            .await
            .expect("target ReviewReport rows count");
    assert_eq!(
        review_report_count, 0,
        "no synthetic ReviewReport was created"
    );
    let fk_violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .expect("foreign_key_check runs");
    assert_eq!(fk_violations, 0);
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check runs");
    assert_eq!(integrity, "ok");
}
