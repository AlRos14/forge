use db::{create_sqlite_pool, run_migrations};

#[tokio::test]
async fn file_backed_migrations_apply_cleanly() {
    let db_path = std::env::temp_dir().join(format!("forge-migtest-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
}
