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

async fn seed_v123_projection(pool: &SqlitePool) {
    let now = "2026-10-12T12:00:00Z";
    sqlx::query(
        "INSERT INTO project (id, name, created_at, updated_at)
         VALUES ('pr13-session-project', 'Session provenance', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("Project fixture");
    sqlx::query(
        "INSERT INTO task
         (id, project_id, assignee_type, assignee_id, title, created_at, updated_at)
         VALUES ('pr13-session-task', 'pr13-session-project', 'agent',
                 'legacy-agent-token', 'Legacy session row', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("Task fixture");
    sqlx::query(
        "INSERT INTO execution
         (id, task_id, role, status, agent_session_id, executor_config_snapshot_json,
          created_at, updated_at)
         VALUES ('pr13-session-execution', 'pr13-session-task', 'executor', 'failed',
                 ' raw-session-id with spaces ', '{\"legacy\":true}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("ambiguous historical Execution session projection");
    sqlx::query(
        "INSERT INTO execution_session_migration_issue
         (id, execution_id, issue_kind, details_json, created_at)
         VALUES ('pr13-session-issue', 'pr13-session-execution',
                 'historical_session_ambiguous', '{\"raw\":true}', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical ambiguity receipt");
}

#[tokio::test]
async fn v124_preserves_ambiguous_projection_verbatim_without_creating_a_harness_session() {
    let temp = tempfile::tempdir().expect("temp dir");
    let v123 = temp.path().join("v123");
    let v124 = temp.path().join("v124");
    fs::create_dir_all(&v123).expect("V123 migration dir");
    fs::create_dir_all(&v124).expect("V124 migration dir");
    copy_migrations_through(123, &v123);
    copy_migrations_through(124, &v124);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &v123)
        .await
        .expect("upgrade through V123");
    seed_v123_projection(&pool).await;

    run_migrations_from(&pool, &v124)
        .await
        .expect("V124 removes only the projection");

    let archived: (String, String, String, Option<String>) = sqlx::query_as(
        "SELECT execution_id, external_session_id, task_id, executor_config_snapshot_json
         FROM pr13_execution_session_projection_history
         WHERE execution_id = 'pr13-session-execution'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact historical projection");
    assert_eq!(archived.0, "pr13-session-execution");
    assert_eq!(archived.1, " raw-session-id with spaces ");
    assert_eq!(archived.2, "pr13-session-task");
    assert_eq!(archived.3.as_deref(), Some(r#"{"legacy":true}"#));

    let issue_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_session_migration_issue
         WHERE id = 'pr13-session-issue' AND details_json = '{\"raw\":true}'",
    )
    .fetch_one(&pool)
    .await
    .expect("migration issue ledger remains");
    assert_eq!(issue_count, 1);
    let remaining_projection_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('execution')
         WHERE name = 'agent_session_id'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect Execution schema");
    assert_eq!(remaining_projection_columns, 0);
    let harness_session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM harness_session")
        .fetch_one(&pool)
        .await
        .expect("HarnessSession table remains queryable");
    assert_eq!(harness_session_count, 0);
    let foreign_key_violations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .expect("foreign_key_check");
    assert_eq!(foreign_key_violations, 0);
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check");
    assert_eq!(integrity, "ok");
}
