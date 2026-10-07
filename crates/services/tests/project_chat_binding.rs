use std::sync::Arc;

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    CreateAgentIdentity, CreateAgentProfile, CreateProject, CreateProjectMember,
    ProjectAgentBindingRepo, ProjectMemberRepo, ProjectRepo, SqliteDb,
};
use events::EventBus;
use services::{AgentChatService, SendAgentChatMessageInput};

async fn database() -> Arc<SqliteDb> {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    Arc::new(SqliteDb::new(pool))
}

async fn project(db: &SqliteDb, id: &str) {
    let now = now_rfc3339();
    sqlx::query(
        "INSERT OR IGNORE INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES (?, ?, 'test', NULL, ?, ?)",
    )
    .bind("user-1")
    .bind("user-1@example.test")
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("test user creates");
    ProjectRepo::create(
        db,
        CreateProject {
            id: id.to_owned(),
            name: id.to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some("user-1".to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("project creates");
}

async fn external_agent(db: &SqliteDb, identity_id: &str) {
    let now = now_rfc3339();
    AgentRepo::create_identity_with_profile(
        db,
        CreateAgentIdentity {
            id: identity_id.to_owned(),
            name: identity_id.to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some("user-1".to_owned()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: new_uuid_v4(),
            identity_id: identity_id.to_owned(),
            backend_kind: "cli".to_owned(),
            executor_type: "codex".to_owned(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("external Agent creates");
}

#[tokio::test]
async fn project_chat_never_infers_worker_as_binding() {
    let db = database().await;
    project(&db, "project-worker-primary").await;
    external_agent(&db, "worker-identity").await;
    ProjectMemberRepo::add_member(
        db.as_ref(),
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: "project-worker-primary".to_owned(),
            user_id: "user-1".to_owned(),
            role: "owner".to_owned(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project member creates");
    let binding =
        ProjectAgentBindingRepo::get_active_project_binding(db.as_ref(), "project-worker-primary")
            .await
            .expect("binding lookup")
            .expect("project always has one singular binding");
    assert_eq!(binding.state, "agent_setup_required");
    assert_eq!(binding.identity_id, None);

    let chats = AgentChatService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let chat = chats
        .ensure_project_chat("project-worker-primary")
        .await
        .expect("Project Chat creates");
    let error = chats
        .send_message(SendAgentChatMessageInput {
            actor_user_id: "user-1".to_owned(),
            chat_id: chat.id.clone(),
            content: "must not route to worker".to_owned(),
            dedupe_key: Some("worker-primary-denial".to_owned()),
        })
        .await
        .expect_err("a primary Worker must not infer a Project binding");
    assert!(error.to_string().contains("not ready"));

    let turns: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat_turn_job WHERE chat_id = ?")
            .bind(chat.id)
            .fetch_one(db.pool())
            .await
            .expect("turn count");
    assert_eq!(turns, 0, "denied routing must not admit a turn");
}
