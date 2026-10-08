use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    CreateAgentIdentity, CreateAgentProfile, CreateProject, ProjectRepo, SqliteDb,
};
use sqlx::Row;

async fn database() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    SqliteDb::new(pool)
}

#[tokio::test]
async fn retired_binding_writes_are_fenced_while_agent_identity_and_profile_remain() {
    let db = database().await;
    let now = now_rfc3339();
    let account_id = "retired-binding-account".to_owned();
    let project_id = "retired-binding-project".to_owned();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES (?, ?, 'test', NULL, ?, ?)",
    )
    .bind(&account_id)
    .bind("retired-binding-account@example.test")
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("account creates without Main Agent bootstrap");

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "ordinary project".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(account_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project creates without Project Agent bootstrap");

    let identity_id = "ordinary-agent".to_owned();
    let profile_id = new_uuid_v4();
    AgentRepo::create_identity_with_profile(
        &db,
        CreateAgentIdentity {
            id: identity_id.clone(),
            name: "ordinary Agent".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(account_id.clone()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: profile_id.clone(),
            identity_id: identity_id.clone(),
            backend_kind: "native".to_owned(),
            executor_type: "embedded".to_owned(),
            provider: Some("test".to_owned()),
            model: Some("test".to_owned()),
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "{}".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent identity/profile remain writable");

    let main_write = sqlx::query(
        "INSERT INTO account_main_agent_binding (
            id, account_id, identity_id, profile_id, state, version, created_at, updated_at
         ) VALUES ('new-main-binding', ?, ?, ?, 'active', 1, ?, ?)",
    )
    .bind(&account_id)
    .bind(&identity_id)
    .bind(&profile_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await;
    assert!(main_write
        .expect_err("Main Agent binding write is retired")
        .to_string()
        .contains("PR11_OPERATION_RETIRED"));

    let project_write = sqlx::query(
        "INSERT INTO project_agent_binding (
            id, project_id, identity_id, profile_id, state, version,
            autonomy_policy_json, permission_ceiling_json, subscriptions_json,
            wake_budget, created_at, updated_at
         ) VALUES ('new-project-binding', ?, ?, ?, 'active', 1, '{}', '{}', '[]', 1, ?, ?)",
    )
    .bind(&project_id)
    .bind(&identity_id)
    .bind(&profile_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await;
    assert!(project_write
        .expect_err("Project Agent binding write is retired")
        .to_string()
        .contains("PR11_OPERATION_RETIRED"));

    let identity = AgentRepo::get_by_id(&db, &identity_id)
        .await
        .expect("Agent lookup")
        .expect("Agent remains present");
    assert_eq!(identity.id, identity_id);
    let profile_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_profile WHERE id = ? AND identity_id = ?")
            .bind(&profile_id)
            .bind(&identity.id)
            .fetch_one(db.pool())
            .await
            .expect("profile count");
    assert_eq!(profile_count, 1);

    let binding_rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM account_main_agent_binding WHERE id = 'new-main-binding') +
                (SELECT COUNT(*) FROM project_agent_binding WHERE id = 'new-project-binding')",
    )
    .fetch_one(db.pool())
    .await
    .expect("binding rows count");
    assert_eq!(binding_rows, 0);

    let columns = sqlx::query("PRAGMA table_info(project_agent_binding)")
        .fetch_all(db.pool())
        .await
        .expect("binding schema");
    let names = columns
        .iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<Vec<_>>();
    assert!(!names.iter().any(|name| name == "role"));
    assert!(!names.iter().any(|name| name == "is_primary"));
}
