#![allow(dead_code)]

mod common;

use api_types::{
    AgentResponse, AgentUsageResponse, DaemonRegisterResponse, ErrorResponse, TaskResponse,
};
use axum::http::Method;
use db::{CreateExecution, ExecutionRepo, ExecutionStatus};
use serde_json::json;

#[tokio::test]
async fn unpinned_remote_agent_usage_uses_the_actual_execution_observation() {
    let workspace = common::TestDir::new("agent-usage-workspace");
    let harness = common::test_app(workspace.path(), "agent-usage").await;
    let app = &harness.app;
    let repo_root = common::TestDir::new("agent-usage-repo");
    let repo_path = common::setup_git_repo(repo_root.path());
    let (project_id, _repo_id) =
        common::create_project_and_repo(app, "agent-usage", &repo_path).await;

    let agent: AgentResponse = common::json_request(
        app,
        Method::POST,
        "/api/v1/agents",
        json!({
            "name": "unpinned-codex",
            "executor_type": "codex",
            "config_json": { "env": { "CODEX_HOME": "/home/alex/.codex" } }
        }),
        axum::http::StatusCode::OK,
    )
    .await;

    let task: TaskResponse = common::json_request(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({
            "title": "observe remote usage",
            "description": "usage test"
        }),
        axum::http::StatusCode::OK,
    )
    .await;

    let execution_id = db::new_uuid_v4();
    let now = db::now_rfc3339();
    ExecutionRepo::create(
        &*harness.state.db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: task.id,
            agent_id: Some(agent.id.clone()),
            role: "executor".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: Some(now.clone()),
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                json!({
                    "executor_type": "codex",
                    "config": { "env": { "CODEX_HOME": "/home/alex/.codex" } },
                    "resolved_daemon_id": "daemon-7",
                    "agent_daemon_id": null
                })
                .to_string(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("execution creates");

    sqlx::query(
        "INSERT INTO account_usage_snapshot
         (id, account_key, executor_type, daemon_id, source, usage_json,
          captured_at, stale_after, execution_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(db::new_uuid_v4())
    .bind("codex@daemon-7:home=/home/alex/.codex")
    .bind("codex")
    .bind("daemon-7")
    .bind("provider_event")
    .bind(json!({ "rateLimits": { "primary": { "usedPercent": 17 } } }).to_string())
    .bind(&now)
    .bind("2099-01-01T00:00:00Z")
    .bind(execution_id)
    .execute(harness.state.db.pool())
    .await
    .expect("usage snapshot inserts");

    let usage: AgentUsageResponse = common::empty_request_with_bearer(
        app,
        Method::GET,
        &format!("/api/v1/agents/{}/usage", agent.id),
        &common::test_jwt(),
        axum::http::StatusCode::OK,
    )
    .await;

    assert!(usage.available);
    assert_eq!(
        usage.account_key.as_deref(),
        Some("codex@daemon-7:home=/home/alex/.codex")
    );
    assert_eq!(usage.daemon_id.as_deref(), Some("daemon-7"));
    assert_eq!(usage.source.as_deref(), Some("provider_event"));
    assert!(!usage.manual_refresh_supported);
    assert!(!usage.shared_account);
}

#[tokio::test]
async fn remote_agent_usage_refresh_is_explicitly_unsupported() {
    let workspace = common::TestDir::new("agent-usage-refresh-workspace");
    let harness = common::test_app(workspace.path(), "agent-usage-refresh").await;
    let app = &harness.app;

    let unpinned: AgentResponse = common::json_request(
        app,
        Method::POST,
        "/api/v1/agents",
        json!({
            "name": "unpinned-refresh",
            "executor_type": "codex",
            "config_json": { "env": { "CODEX_HOME": "/home/alex/.codex" } }
        }),
        axum::http::StatusCode::OK,
    )
    .await;
    let unpinned_error: ErrorResponse = common::empty_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/agents/{}/usage/refresh", unpinned.id),
        &common::test_jwt(),
        axum::http::StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(unpinned_error.code, "usage_refresh_unsupported");

    let registration: DaemonRegisterResponse = common::json_request(
        app,
        Method::POST,
        "/api/v1/daemons/register",
        json!({
            "machine_id": services::embedded_daemon::embedded_machine_id(),
            "hostname": "agent-usage-refresh-host",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "test"
        }),
        axum::http::StatusCode::OK,
    )
    .await;
    let pinned: AgentResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/agents",
        &common::admin_jwt(),
        json!({
            "name": "pinned-refresh",
            "executor_type": "codex",
            "daemon_id": registration.daemon_id,
            "config_json": { "env": { "CODEX_HOME": "/home/alex/.codex" } }
        }),
        axum::http::StatusCode::OK,
    )
    .await;
    let pinned_error: ErrorResponse = common::empty_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/agents/{}/usage/refresh", pinned.id),
        &common::test_jwt(),
        axum::http::StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(pinned_error.code, "usage_refresh_unsupported");
}
