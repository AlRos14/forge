#![allow(dead_code)]

mod common;

use std::sync::Arc;

use api_types::{AgentSessionResponse, ErrorResponse};
use axum::http::{Method, StatusCode};
use db::{
    AgentContextScopeRepo, AgentRepo, AgentSessionRepo, AgentStatus, CreateAgentContextScope,
    CreateAgentIdentity, CreateAgentProfile, CreateAgentSession,
};
use serde_json::json;

#[tokio::test]
async fn historical_native_session_is_readable_but_interaction_runtime_fails_closed() {
    let workspace = common::TestDir::new("protected-interactions");
    let harness = common::test_app(workspace.path(), "protected-interactions").await;
    let session_id = seed_owned_native_session(&harness.state.db).await;

    let sessions: Vec<AgentSessionResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/agents/interaction-identity/sessions",
        &common::test_jwt(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    assert_eq!(sessions[0].backend_kind, "native");
    assert_eq!(sessions[0].status, "ready");

    let interactions: ErrorResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/agent-sessions/{session_id}/interactions"),
        &common::test_jwt(),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(interactions.code, "agent_runtime.retired");

    let answer: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/agent-sessions/{session_id}/interactions/old-request/answer"),
        &common::test_jwt(),
        json!({"expected_version": 1, "values": []}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(answer.code, "agent_runtime.retired");

    let cancel: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/agent-sessions/{session_id}/interactions/old-request/cancel"),
        &common::test_jwt(),
        json!({"expected_version": 1}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(cancel.code, "agent_runtime.retired");

    let stored_status: String = sqlx::query_scalar("SELECT status FROM agent_session WHERE id = ?")
        .bind(&session_id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("historical session remains stored");
    assert_eq!(stored_status, "ready");
}

async fn seed_owned_native_session(db: &Arc<db::SqliteDb>) -> String {
    let now = db::now_rfc3339();
    AgentRepo::create_identity_with_profile(
        db.as_ref(),
        CreateAgentIdentity {
            id: "interaction-identity".to_owned(),
            name: "Historical interaction identity".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some("test-user-id".to_owned()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: "interaction-profile".to_owned(),
            identity_id: "interaction-identity".to_owned(),
            backend_kind: "native".to_owned(),
            executor_type: "embedded".to_owned(),
            provider: Some("historical".to_owned()),
            model: Some("historical-model".to_owned()),
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
    .expect("historical identity/profile creates");
    AgentContextScopeRepo::create_context_scope(
        db.as_ref(),
        CreateAgentContextScope {
            id: "interaction-scope".to_owned(),
            identity_id: "interaction-identity".to_owned(),
            scope_type: "account".to_owned(),
            scope_id: "test-user-id".to_owned(),
            project_id: None,
            task_id: None,
            task_role: None,
            workspace_access: "deny".to_owned(),
            authority_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("historical context scope creates");
    AgentSessionRepo::create_agent_session(
        db.as_ref(),
        CreateAgentSession {
            id: "interaction-session".to_owned(),
            identity_id: "interaction-identity".to_owned(),
            profile_id: "interaction-profile".to_owned(),
            context_scope_id: "interaction-scope".to_owned(),
            backend_kind: "native".to_owned(),
            runtime_session_id: Some("retired-runtime-session".to_owned()),
            status: "ready".to_owned(),
            capabilities_json: "{}".to_owned(),
            connection_status: "healthy".to_owned(),
            predecessor_session_id: None,
            last_activity_at: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("historical session creates");
    "interaction-session".to_owned()
}
