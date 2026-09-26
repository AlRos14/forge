use std::{fs, path::Path};

use db::{create_sqlite_pool, now_rfc3339, run_migrations_from};

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

#[tokio::test]
async fn v090_applies_over_v089_and_preserves_legacy_rows_after_reopen() {
    let temp = tempfile::tempdir().expect("temp dir");
    let migration_dir = temp.path().join("migrations");
    fs::create_dir_all(&migration_dir).expect("migration dir");
    copy_migrations_through(89, &migration_dir);

    let database_path = temp.path().join("forge-v089.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("schema through V089");

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES ('v090-project', 'V090 preservation', '{}', '{}', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy project");
    sqlx::query(
        "INSERT INTO repo (
             id, project_id, name, remote_url, local_path, work_mode,
             default_branch, created_at, updated_at
         ) VALUES ('v090-repo', 'v090-project', 'repo',
                   'https://example.invalid/v090.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy repo");
    sqlx::query(
        "INSERT INTO task (
             id, project_id, repo_id, title, task_type, status, created_at, updated_at
         ) VALUES ('v090-task', 'v090-project', 'v090-repo', 'legacy plan task',
                   'planning', 'in_progress', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy task");
    sqlx::query(
        "INSERT INTO task_plan_revision (
             id, task_id, revision, checkpoint, markdown, content_digest,
             checklist_json, warnings_json, source_execution_id, created_at
         ) VALUES ('v090-plan', 'v090-task', 1, 'approved', '# Existing plan',
                   'sha256:legacy', '[]', '[]', NULL, ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy plan revision");
    let before: (String, String, String, i64) = sqlx::query_as(
        "SELECT id, markdown, content_digest, revision FROM task_plan_revision WHERE id = 'v090-plan'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy row snapshot");

    let migration_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    fs::copy(
        migration_root.join("V090__generic_collaboration_primitives.sql"),
        migration_dir.join("V090__generic_collaboration_primitives.sql"),
    )
    .expect("V090 copied");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("V090 applies over V089");
    pool.close().await;

    let reopened = create_sqlite_pool(&database_url)
        .await
        .expect("database reopens");
    let after: (String, String, String, i64) = sqlx::query_as(
        "SELECT id, markdown, content_digest, revision FROM task_plan_revision WHERE id = 'v090-plan'",
    )
    .fetch_one(&reopened)
    .await
    .expect("legacy plan remains");
    assert_eq!(after, before);
    let latest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&reopened)
        .await
        .expect("latest migration number");
    assert_eq!(latest, 90);
    let foreign_key_issues: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&reopened)
            .await
            .expect("FK check");
    assert!(foreign_key_issues.is_empty());
}
