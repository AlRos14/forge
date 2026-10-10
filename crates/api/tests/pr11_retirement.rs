mod common;

use api_types::{ErrorResponse, ProjectResponse, TaskResponse};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

#[tokio::test]
async fn project_and_task_creation_need_no_main_agent_or_project_os_setup() {
    let workspace = common::TestDir::new("pr11-normal-project");
    let harness = common::test_app(workspace.path(), "pr11-normal-project").await;
    let token = common::test_jwt();

    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Empty PR11 Project"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Ordinary Task"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(task.project_id, project.id);

    let retired_governance: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Must not use Project Task Governance", "governance": {}}),
        StatusCode::GONE,
    )
    .await;
    assert_eq!(retired_governance.code, "operation_retired");
    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("Task count");
    assert_eq!(task_count, 1, "retired governance does not create a Task");

    for table in [
        "agent_chat",
        "account_main_agent_binding",
        "project_agent_binding",
        "product_genesis_session",
        "project_charter",
        "project_task_governance",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(harness.state.db.pool())
            .await
            .expect("count retired setup rows");
        assert_eq!(
            count, 0,
            "normal Project/Task creation leaves {table} empty"
        );
    }
}

#[tokio::test]
async fn retired_rest_mutations_return_stable_gone_errors() {
    let workspace = common::TestDir::new("pr11-retired-routes");
    let harness = common::test_app(workspace.path(), "pr11-retired-routes").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "PR11 Route Project"}),
        StatusCode::OK,
    )
    .await;

    let cases = [
        (Method::PUT, "/api/v1/account/main-agent".to_owned()),
        (
            Method::POST,
            "/api/v1/account/main-agent/product-genesis".to_owned(),
        ),
        (Method::POST, "/api/v1/agent-chats/chat/messages".to_owned()),
        (
            Method::PUT,
            format!("/api/v1/projects/{}/project-agent", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/charter/revisions", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/execution-baseline", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/documents", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/decisions/candidates", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/milestones", project.id),
        ),
        (
            Method::POST,
            format!(
                "/api/v1/projects/{}/milestones/legacy-milestone/readiness",
                project.id
            ),
        ),
        (
            Method::POST,
            format!(
                "/api/v1/projects/{}/milestones/legacy-milestone/transition",
                project.id
            ),
        ),
        (
            Method::POST,
            format!(
                "/api/v1/projects/{}/milestones/legacy-milestone/release",
                project.id
            ),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/milestones/primary", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/agent-handoffs", project.id),
        ),
        (Method::POST, "/api/v1/agents/agent/actions".to_owned()),
        (Method::POST, "/api/v1/agents/agent/commitments".to_owned()),
        (Method::POST, "/api/v1/agents/agent/questions".to_owned()),
        (Method::POST, "/api/v1/memory/backfill".to_owned()),
        (
            Method::POST,
            "/api/v1/mission-control/attention/attention-id/resolve".to_owned(),
        ),
    ];

    for (method, path) in cases {
        let error: ErrorResponse = common::json_request_with_bearer(
            &harness.app,
            method,
            &path,
            &token,
            json!({}),
            StatusCode::GONE,
        )
        .await;
        assert_eq!(error.code, "operation_retired", "{path}");
    }

    // An old Project Agent assignment in the create payload also fails before
    // Project creation; the same endpoint still accepts an ordinary Project.
    let error: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({
            "name": "Must not create with a Project Agent",
            "project_agent_identity_id": "agent-old"
        }),
        StatusCode::GONE,
    )
    .await;
    assert_eq!(error.code, "operation_retired");

    let history: Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/agent-chats",
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(
        history.is_object(),
        "historical chat read surface remains available"
    );
}
