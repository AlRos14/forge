use db::{create_sqlite_pool, run_migrations_from};
use std::{
    fs,
    path::PathBuf,
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
async fn migration_runner_allows_foreign_key_pragmas_to_take_effect() {
    let migration_dir = unique_temp_path("pragma-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir");
    fs::write(
        migration_dir.join("V001__initial.sql"),
        r#"
        CREATE TABLE parent (
            id TEXT PRIMARY KEY
        );

        CREATE TABLE child (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL REFERENCES parent(id) ON DELETE CASCADE
        );

        INSERT INTO parent (id) VALUES ('p1');
        INSERT INTO child (id, parent_id) VALUES ('c1', 'p1');
        "#,
    )
    .expect("writes initial migration");
    fs::write(
        migration_dir.join("V002__rebuild_parent.sql"),
        r#"
        PRAGMA foreign_keys = OFF;

        CREATE TABLE parent_new (
            id TEXT PRIMARY KEY
        );

        INSERT INTO parent_new (id)
        SELECT id
        FROM parent;

        DROP TABLE parent;
        ALTER TABLE parent_new RENAME TO parent;

        PRAGMA foreign_keys = ON;
        "#,
    )
    .expect("writes rebuild migration");

    let db_path = unique_temp_path("pragma-migration-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");

    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("migrations apply");

    let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("migration count loads");
    assert_eq!(applied_count, 2);

    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(foreign_key_violations.is_empty());

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}
