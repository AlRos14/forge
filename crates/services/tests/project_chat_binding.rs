use db::{create_sqlite_pool, now_rfc3339, run_migrations, CreateProject, ProjectRepo, SqliteDb};
use std::sync::Arc;

#[tokio::test]
async fn new_project_has_no_agent_chat_or_project_binding() {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let db = Arc::new(SqliteDb::new(pool));
    let now = now_rfc3339();
    ProjectRepo::create(
        db.as_ref(),
        CreateProject {
            id: "project-without-agent-os".to_owned(),
            name: "Ordinary Project".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("ordinary Project creates");

    for table in ["agent_chat", "project_agent_binding"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .expect("count retired rows");
        assert_eq!(count, 0, "Project creation must not create {table}");
    }

    let denied = sqlx::query(
        "INSERT INTO agent_chat
         (id, kind, account_id, project_id, status, created_at, updated_at)
         VALUES ('forbidden-project-chat', 'project', NULL, 'project-without-agent-os',
                 'active', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await;
    assert!(denied.is_err(), "retired Chat writes fail at SQLite");
    let chats: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat")
        .fetch_one(db.pool())
        .await
        .expect("chat count after rejected write");
    assert_eq!(chats, 0);
}
