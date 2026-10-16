use db::{create_sqlite_pool, run_migrations_from};
use sqlx::SqlitePool;
use std::{fs, path::Path};

fn copy_migrations_through(limit: i64, destination: &Path) {
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
            .expect("migration version");
        if version <= limit {
            fs::copy(
                &path,
                destination.join(path.file_name().expect("migration filename")),
            )
            .expect("migration copies");
        }
    }
}

async fn seed_v124_task_plan(pool: &SqlitePool) {
    let now = "2026-10-13T12:00:00Z";
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES ('pr13-plan-project', 'Task plan history', '{}', '{}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V124 Project fixture");
    sqlx::query(
        "INSERT INTO task (id, project_id, title, task_type, status, plan, created_at, updated_at)
         VALUES ('pr13-plan-task', 'pr13-plan-project', 'Legacy planner', 'planning',
                 'in_progress', 'raw task plan bytes', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("V124 Task fixture");
    sqlx::query(
        "INSERT INTO execution (id, task_id, role, purpose, status, created_at, updated_at)
         VALUES ('pr13-plan-execution', 'pr13-plan-task', 'planner', 'plan', 'completed', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical planner Execution");
    sqlx::query(
        "INSERT INTO task_plan_revision (
             id, task_id, revision, checkpoint, markdown, content_digest,
             checklist_json, warnings_json, source_execution_id, created_at
         ) VALUES ('pr13-plan-revision', 'pr13-plan-task', 3, 'approved',
                   'raw planner markdown', 'plan-digest-3', '{ \"items\" : [1] }',
                   '[\"raw warning\"]', 'pr13-plan-execution', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("V081 Task plan revision");
    sqlx::query(
        "INSERT INTO task_plan_approval (
             id, task_id, plan_revision_id, content_digest, principal_type,
             principal_id, decision, reason, created_at
         ) VALUES ('pr13-plan-approval', 'pr13-plan-task', 'pr13-plan-revision',
                   'plan-digest-3', 'system', 'migration-reviewer', 'approved',
                   'exact historical approval', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("V081 plan approval");
    sqlx::query(
        "INSERT INTO review (
             id, task_id, execution_id, attempt_number, status, step_results_json,
             started_at, finished_at, created_at, updated_at
         ) VALUES ('pr13-plan-review', 'pr13-plan-task', 'pr13-plan-execution',
                   1, 'passed', '{}', ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("legacy Review row for evidence bundle");
    sqlx::query(
        "INSERT INTO review_evidence_bundle (
             id, review_id, task_id, reviewer_execution_id, plan_revision_id,
             plan_digest, base_sha, head_sha, diff_text, diff_digest,
             ci_results_json, fresh_session, created_at
         ) VALUES ('pr13-plan-bundle', 'pr13-plan-review', 'pr13-plan-task',
                   'pr13-plan-execution', 'pr13-plan-revision', 'plan-digest-3',
                   'base-exact', 'head-exact', 'raw diff bytes', 'diff-digest',
                   '{ \"checks\" : [true] }', 0, ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("V081 review evidence bundle");
    sqlx::query(
        "INSERT INTO legacy_task_plan_artifact_migration (
             plan_revision_id, artifact_id, migration_status, details_json
         ) VALUES ('pr13-plan-revision', NULL, 'source_execution_missing',
                   '{\"source\":\"ambiguous legacy row\"}')",
    )
    .execute(pool)
    .await
    .expect("V098 migration provenance with no synthetic Artifact");
}

#[tokio::test]
async fn v125_archives_exact_v081_rows_and_preserves_v098_provenance() {
    let migrations = tempfile::tempdir().expect("migration directory creates");
    copy_migrations_through(124, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("schema through V124 applies");
    seed_v124_task_plan(&pool).await;

    copy_migrations_through(125, migrations.path());
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("V125 Task Plan cutover applies");

    let archived_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pr13_legacy_record_history
         WHERE (source_table, source_identity) IN (
             ('task_plan_revision', 'pr13-plan-revision'),
             ('task_plan_approval', 'pr13-plan-approval'),
             ('review_evidence_bundle', 'pr13-plan-bundle'),
             ('task.plan', 'pr13-plan-task')
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("exact V081 history rows query");
    assert_eq!(archived_rows, 4);

    let plan_json: String = sqlx::query_scalar(
        "SELECT row_json FROM pr13_legacy_record_history
         WHERE source_table = 'task_plan_revision' AND source_identity = 'pr13-plan-revision'",
    )
    .fetch_one(&pool)
    .await
    .expect("V081 plan row archived");
    let plan: serde_json::Value = serde_json::from_str(&plan_json).expect("archived plan JSON");
    assert_eq!(plan["markdown"], "raw planner markdown");
    assert_eq!(plan["checklist_json"], "{ \"items\" : [1] }");
    assert_eq!(plan["source_execution_id"], "pr13-plan-execution");

    let bundle_json: String = sqlx::query_scalar(
        "SELECT row_json FROM pr13_legacy_record_history
         WHERE source_table = 'review_evidence_bundle' AND source_identity = 'pr13-plan-bundle'",
    )
    .fetch_one(&pool)
    .await
    .expect("Review bundle archived");
    let bundle: serde_json::Value = serde_json::from_str(&bundle_json).expect("bundle JSON");
    assert_eq!(bundle["diff_text"], "raw diff bytes");
    assert_eq!(bundle["plan_revision_id"], "pr13-plan-revision");
    assert_eq!(bundle["ci_results_json"], "{ \"checks\" : [true] }");

    let provenance: (Option<String>, String, String) = sqlx::query_as(
        "SELECT artifact_id, migration_status, details_json
         FROM legacy_task_plan_artifact_migration
         WHERE plan_revision_id = 'pr13-plan-revision'",
    )
    .fetch_one(&pool)
    .await
    .expect("V098 receipt remains");
    assert_eq!(provenance.0, None);
    assert_eq!(provenance.1, "source_execution_missing");
    assert_eq!(provenance.2, "{\"source\":\"ambiguous legacy row\"}");
    let provenance_fks: Vec<String> = sqlx::query_scalar(
        "SELECT \"from\" FROM pragma_foreign_key_list('legacy_task_plan_artifact_migration')",
    )
    .fetch_all(&pool)
    .await
    .expect("V098 provenance foreign keys");
    assert_eq!(provenance_fks, vec!["artifact_id"]);

    let retired_objects: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN (
             'task_plan_revision', 'task_plan_approval', 'review_evidence_bundle'
         )",
    )
    .fetch_one(&pool)
    .await
    .expect("retired V081 tables lookup");
    assert_eq!(retired_objects, 0);
    let task_plan_column: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('task') WHERE name = 'plan'")
            .fetch_one(&pool)
            .await
            .expect("legacy Task plan column lookup");
    assert_eq!(task_plan_column, 0);

    let foreign_key_errors: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .expect("foreign_key_check");
    let integrity: String =
        sqlx::query_scalar("SELECT integrity_check FROM pragma_integrity_check")
            .fetch_one(&pool)
            .await
            .expect("integrity_check");
    assert_eq!(foreign_key_errors, 0);
    assert_eq!(integrity, "ok");
}
