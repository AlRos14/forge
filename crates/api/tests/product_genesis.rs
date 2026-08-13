#![allow(dead_code)]

mod common;

use api_types::{
    AgentChatMessageListResponse, AgentHandoffResponse, ConnectedEmbeddedAgentResponse,
    ErrorResponse, ProductGenesisActiveResponse, ProductGenesisSession,
    ProductGenesisStartResponse, ProjectResponse,
};
use axum::{http::Method, http::StatusCode, Router};
use serde_json::json;
use sqlx::Row;

#[tokio::test]
async fn product_genesis_uses_existing_main_chat_and_is_cancelable() {
    let workspace = common::TestDir::new("product-genesis-routes");
    let harness = common::test_app(workspace.path(), "product-genesis-routes").await;
    let app = &harness.app;
    let token = common::test_jwt();

    let connected = connect_genesis_agent(app, &token, "genesis-main").await;

    let binding: api_types::MainAgentBindingResponse = common::json_request_with_bearer(
        app,
        Method::PUT,
        "/api/v1/account/main-agent",
        &token,
        json!({
            "identity_id": connected.agent.id,
            "profile_id": connected.profile.id,
            "expected_version": 0,
            "autonomy_policy": {}
        }),
        StatusCode::OK,
    )
    .await;

    let started: ProductGenesisStartResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/account/main-agent/product-genesis",
        &token,
        json!({
            "maturity": "production",
            "initial_idea": "A bounded, durable product idea"
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(started.main_chat_id, binding.chat_id);
    assert_eq!(started.session.main_chat_id, binding.chat_id);
    assert!(matches!(
        started.session.lifecycle,
        api_types::ProductGenesisLifecycle::Discovering
    ));
    assert!(matches!(
        started.session.maturity,
        api_types::ProductMaturity::Production
    ));
    assert!(started.admitted_turn_id.is_some());
    assert_eq!(started.session.source_message_ids.len(), 1);

    let instruction = sqlx::query(
        "SELECT source_type, source_id, revision, body, created_by_type
         FROM agent_chat_instruction_revision
         WHERE chat_id = ? AND source_id = ?",
    )
    .bind(&binding.chat_id)
    .bind(&started.session.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("Genesis instruction revision is durable");
    assert_eq!(instruction.get::<String, _>("source_type"), "native");
    assert_eq!(
        instruction.get::<String, _>("source_id"),
        started.session.id
    );
    assert_eq!(
        instruction.get::<String, _>("created_by_type"),
        "product_genesis"
    );
    assert!(instruction
        .get::<String, _>("body")
        .contains("Product Genesis protocol v1"));

    let active: ProductGenesisActiveResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        "/api/v1/account/main-agent/product-genesis/active",
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        active.session.as_ref().map(|session| &session.id),
        Some(&started.session.id)
    );

    let read: ProductGenesisSession = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!(
            "/api/v1/account/main-agent/product-genesis/{}",
            started.session.id
        ),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(read.id, started.session.id);
    assert_eq!(read.prompt_revision, started.session.prompt_revision);

    let duplicate: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/account/main-agent/product-genesis",
        &token,
        json!({ "maturity": "mvp" }),
        StatusCode::CONFLICT,
    )
    .await;
    assert!(duplicate.message.contains("already active"));

    let messages: AgentChatMessageListResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!("/api/v1/agent-chats/{}/messages", binding.chat_id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(messages
        .items
        .iter()
        .any(|message| message.id == started.session.source_message_ids[0]));

    let cancelled: ProductGenesisSession = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!(
            "/api/v1/account/main-agent/product-genesis/{}/cancel",
            started.session.id
        ),
        &token,
        json!({
            "expected_version": started.session.version,
            "reason": "user stopped discovery"
        }),
        StatusCode::OK,
    )
    .await;
    assert!(matches!(
        cancelled.lifecycle,
        api_types::ProductGenesisLifecycle::Cancelled
    ));
    assert_eq!(
        cancelled.failure_reason.as_deref(),
        Some("user stopped discovery")
    );
    let retained_instruction_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_chat_instruction_revision
         WHERE chat_id = ? AND source_id = ?",
    )
    .bind(&binding.chat_id)
    .bind(&started.session.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("terminal Genesis retains immutable instruction history");
    assert_eq!(retained_instruction_count, 1);

    let stale: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!(
            "/api/v1/account/main-agent/product-genesis/{}/cancel",
            started.session.id
        ),
        &token,
        json!({ "expected_version": started.session.version, "reason": "stale" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(stale.message.contains("invalid Product Genesis transition"));

    let empty: ProductGenesisActiveResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        "/api/v1/account/main-agent/product-genesis/active",
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(empty.session.is_none());
}

#[tokio::test]
async fn product_genesis_requires_main_binding_and_hides_cross_account_sessions() {
    let workspace = common::TestDir::new("product-genesis-setup");
    let harness = common::test_app(workspace.path(), "product-genesis-setup").await;
    let app = &harness.app;
    let token = common::test_jwt();

    let missing: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/account/main-agent/product-genesis",
        &token,
        json!({ "maturity": "mvp" }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(missing.message.contains("setup is required"));

    let active: ProductGenesisActiveResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        "/api/v1/account/main-agent/product-genesis/active",
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(active.session.is_none());
}

#[tokio::test]
async fn ready_genesis_uses_normal_project_chat_handoff_and_completes() {
    let workspace = common::TestDir::new("product-genesis-handoff");
    let harness = common::test_app(workspace.path(), "product-genesis-handoff").await;
    let app = &harness.app;
    let token = common::test_jwt();

    let connected = connect_genesis_agent(app, &token, "genesis-handoff").await;
    let binding: api_types::MainAgentBindingResponse = common::json_request_with_bearer(
        app,
        Method::PUT,
        "/api/v1/account/main-agent",
        &token,
        json!({
            "identity_id": connected.agent.id,
            "profile_id": connected.profile.id,
            "expected_version": 0,
            "autonomy_policy": {}
        }),
        StatusCode::OK,
    )
    .await;
    let started: ProductGenesisStartResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/account/main-agent/product-genesis",
        &token,
        json!({ "initial_idea": "A product that needs a Project handoff" }),
        StatusCode::CREATED,
    )
    .await;

    let genesis = services::ProductGenesisService::for_sqlite(harness.state.db.clone());
    // The Main Agent's typed readiness action is persisted before the normal
    // Project API path records its atomic Project/chat/binding result.
    let ready: ProductGenesisSession = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!(
            "/api/v1/account/main-agent/product-genesis/{}/ready",
            started.session.id
        ),
        &token,
        json!({ "expected_version": started.session.version }),
        StatusCode::OK,
    )
    .await;
    assert!(matches!(
        ready.lifecycle,
        api_types::ProductGenesisLifecycle::ReadyForProject
    ));

    let project: ProjectResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({
            "name": "Genesis handoff project",
            "project_agent_identity_id": connected.agent.id,
            "project_agent_profile_id": connected.profile.id,
            "product_genesis_session_id": started.session.id
        }),
        StatusCode::OK,
    )
    .await;

    let project_linked = genesis
        .get(&started.session.id)
        .await
        .expect("Genesis project link remains readable");
    assert_eq!(
        project_linked.project_id.as_deref(),
        Some(project.id.as_str())
    );
    assert!(matches!(
        project_linked.lifecycle,
        api_types::ProductGenesisLifecycle::ReadyForProject
    ));

    let replayed_project: ProjectResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({
            "name": "A replay must not fork Genesis",
            "project_agent_identity_id": connected.agent.id,
            "project_agent_profile_id": connected.profile.id,
            "product_genesis_session_id": started.session.id
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(replayed_project.id, project.id);

    let failed_handoff: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{}/agent-handoffs", project.id),
        &token,
        json!({
            "source_message_id": ready.source_message_ids[0],
            "content": "x".repeat(20_000),
            "dedupe_key": "genesis-handoff-failed-before-persist"
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(failed_handoff.message.contains("bounded"));
    let after_failed_handoff = genesis
        .get(&started.session.id)
        .await
        .expect("failed handoff keeps Genesis durable");
    assert_eq!(
        after_failed_handoff.project_id.as_deref(),
        Some(project.id.as_str())
    );
    assert!(matches!(
        after_failed_handoff.lifecycle,
        api_types::ProductGenesisLifecycle::ReadyForProject
    ));
    assert!(after_failed_handoff
        .failure_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("bounded publication limit")));

    let handoff: AgentHandoffResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{}/agent-handoffs", project.id),
        &token,
        json!({
            // A Genesis handoff may cite the Main turn/response rather than
            // one of the user discovery inputs recorded on the session.
            "source_turn_job_id": started.admitted_turn_id,
            "content": "Approved bounded brief for the Project Agent.",
            "dedupe_key": "genesis-handoff-1"
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(handoff.source_chat_id, binding.chat_id);
    assert_eq!(handoff.target_project_id, project.id);
    assert!(matches!(
        handoff.status,
        api_types::AgentHandoffStatus::Pending | api_types::AgentHandoffStatus::Delivered
    ));

    let completed = genesis
        .get(&started.session.id)
        .await
        .expect("Genesis history remains readable");
    assert!(matches!(
        completed.lifecycle,
        api_types::ProductGenesisLifecycle::HandedOff
    ));
    assert_eq!(completed.project_id.as_deref(), Some(project.id.as_str()));
    assert_eq!(completed.handoff_id.as_deref(), Some(handoff.id.as_str()));
}

async fn connect_genesis_agent(
    app: &Router,
    token: &str,
    name: &str,
) -> ConnectedEmbeddedAgentResponse {
    common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/embedded-agents/connect",
        token,
        json!({
            "name": name,
            "description": "Product Genesis integration fixture",
            "provider": "openai_compatible",
            "base_url": "https://8.8.8.8",
            "model": "genesis-test-model",
            "credential_label": "genesis-test",
            "credential": "fixture-secret",
            "account_permission_ceiling": {
                "permissions": ["read_account", "read_project", "handoff"]
            },
            "tool_policy": {
                "allowed": ["read_account", "read_project", "handoff"]
            }
        }),
        StatusCode::OK,
    )
    .await
}
