use db::{create_sqlite_pool, run_migrations, run_migrations_from};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("forge-{name}-{}-{nanos}", std::process::id()))
}

#[tokio::test]
async fn file_backed_migrations_apply_cleanly() {
    let db_path = unique_temp_path("migtest").with_extension("db");
    let _ = std::fs::remove_file(&db_path);
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
}

#[tokio::test]
async fn cursor_executor_backfill_runs_when_version_53_was_used_by_old_migration() {
    let migration_dir = unique_temp_path("cursor-backfill-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir creates");
    copy_migrations_up_to(52, &migration_dir);

    let db_path = unique_temp_path("cursor-backfill-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");

    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("baseline migrations apply");

    sqlx::query(
        "INSERT INTO _migration (version, name, applied_at) VALUES (53, 'integration_credentials', '2026-05-25T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("old conflicting migration marker inserts");

    run_migrations(&pool)
        .await
        .expect("current migrations backfill cursor executor type");

    let migration_name: String =
        sqlx::query_scalar("SELECT name FROM _migration WHERE version = 54")
            .fetch_one(&pool)
            .await
            .expect("V054 migration applied");
    assert_eq!(migration_name, "cursor_executor_type_backfill");

    sqlx::query(
        "INSERT INTO agent (id, name, executor_type, created_at, updated_at) VALUES ('cursor-agent', 'Cursor', 'cursor', 'now', 'now')",
    )
    .execute(&pool)
    .await
    .expect("cursor executor type is accepted");

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

fn copy_migrations_up_to(max_version: i64, destination: &Path) {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source_dir).expect("migration dir reads") {
        let entry = entry.expect("migration entry reads");
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(version) = migration_version(filename) else {
            continue;
        };
        if version <= max_version {
            fs::copy(&path, destination.join(filename)).expect("migration copies");
        }
    }
}

fn migration_version(filename: &str) -> Option<i64> {
    filename.strip_prefix('V')?.split_once("__")?.0.parse().ok()
}
