mod common;

use api_types::TaskResponse;
use axum::http::{Method, StatusCode};
use db::{ExecutionRepo, ExecutionStatus, ExecutionUsageRepo};
use serde_json::Value;

#[tokio::test]
async fn task_response_exposes_execution_aggregates_without_latest_identity() {
    let workspace_root = common::TestDir::new("pr12-execution-observability");
    let harness = common::test_app(workspace_root.path(), "pr12-execution-observability").await;
    let repo_path = common::setup_git_repo(workspace_root.path());
    let (project_id, _) =
        common::create_project_and_repo(&harness.app, "Execution Observability", &repo_path).await;
    let task: TaskResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        serde_json::json!({ "title": "Execution observability" }),
        StatusCode::OK,
    )
    .await;

    let execution_id = db::new_uuid_v4();
    let started_at = "2026-04-30T12:00:00+00:00".to_owned();
    let stopped_at = "2026-04-30T12:00:10+00:00".to_owned();
    ExecutionRepo::create(
        &*harness.state.db,
        db::CreateExecution {
            id: execution_id.clone(),
            task_id: task.id.clone(),
            agent_id: None,
            actor_ref: Some(db::ActorRef::Human("test-user-id".to_owned())),
            purpose: Some(db::ExecutionPurpose::General),
            harness_session_id: None,
            role: "interactive".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(stopped_at.clone()),
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: Some(stopped_at.clone()),
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: started_at,
            updated_at: stopped_at,
        },
    )
    .await
    .expect("exact Human Execution creates");
    ExecutionUsageRepo::upsert(
        &*harness.state.db,
        db::UpsertExecutionUsage {
            execution_id,
            provider: "anthropic".to_owned(),
            model: "claude-test".to_owned(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
            cost_usd: Some(0.12),
        },
    )
    .await
    .expect("usage records");

    let response: Value = common::empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        StatusCode::OK,
    )
    .await;
    let observability = response
        .get("execution_observability")
        .expect("Task includes aggregate execution observability");

    assert_eq!(observability["execution_count"], 1);
    assert_eq!(observability["total_runtime_seconds"], 10.0);
    assert_eq!(observability["total_input_tokens"], 100);
    assert_eq!(observability["total_output_tokens"], 50);
    assert_eq!(observability["total_tokens"], 165);
    assert_eq!(observability["total_cost_usd"], 0.12);
    assert!(observability.get("latest_execution_id").is_none());
    assert!(observability.get("latest_execution_status").is_none());
}
