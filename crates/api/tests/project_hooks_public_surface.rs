#![allow(dead_code)]
mod common;

use api_types::{ErrorResponse, ProjectResponse};
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn workflow_lifecycle_hook_route_and_project_settings_projection_are_removed() {
    let workspace_root = common::TestDir::new("retired-lifecycle-hook-endpoint");
    let harness = common::test_app(workspace_root.path(), "retired-lifecycle-hook-endpoint").await;
    let project: ProjectResponse = common::json_request(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Ordinary Project" }),
        StatusCode::OK,
    )
    .await;

    let error: ErrorResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/hooks/test", project.id),
        json!({
            "task_id": "legacy-task",
            "event": "before_work",
            "hook_index": 0
        }),
        StatusCode::NOT_FOUND,
    )
    .await;

    assert_eq!(error.code, "not_found");
    assert!(db::ExecutionRepo::list_by_task(
        &*harness.state.db,
        "legacy-task",
        db::PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: db::SortBy::CreatedAt,
            sort_order: db::SortOrder::Desc,
        },
    )
    .await
    .expect("execution lookup succeeds")
    .items
    .is_empty());

    let current: ProjectResponse = common::json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        serde_json::to_value(current.project_hooks).expect("current hooks serialize"),
        serde_json::to_value(project.project_hooks).expect("created hooks serialize")
    );
}
