mod common;

use api_types::{ErrorResponse, ProjectResponse, TaskResponse};
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn task_delete_rejects_a_parent_with_live_children_without_partial_writes() {
    let workspace = common::TestDir::new("task-delete-integrity");
    let harness = common::test_app(workspace.path(), "task-delete-integrity").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Task delete integrity"}),
        StatusCode::OK,
    )
    .await;
    let parent: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Parent"}),
        StatusCode::OK,
    )
    .await;
    let child: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Child", "parent_task_id": parent.id.clone()}),
        StatusCode::OK,
    )
    .await;

    let response = common::raw_empty_request(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/tasks/{}", parent.id),
    )
    .await;
    let error: ErrorResponse = common::parse_response(response, StatusCode::BAD_REQUEST).await;
    assert_eq!(error.code, "invalid_operation");
    assert!(error.message.contains("live subtasks"));

    let after_parent = db::TaskRepo::get_by_id(&*harness.state.db, &parent.id, false)
        .await
        .expect("parent query succeeds")
        .expect("parent remains live");
    let after_child = db::TaskRepo::get_by_id(&*harness.state.db, &child.id, false)
        .await
        .expect("child query succeeds")
        .expect("child remains live");
    assert_eq!(after_parent.version, parent.version);
    assert_eq!(after_child.version, child.version);
    assert_eq!(
        after_child.parent_task_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(after_child.subtask_order, Some(0));

    let live_orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM task child
         LEFT JOIN task parent ON parent.id = child.parent_task_id
         WHERE child.deleted_at IS NULL
           AND child.parent_task_id IS NOT NULL
           AND (parent.id IS NULL OR parent.deleted_at IS NOT NULL)",
    )
    .fetch_one(harness.state.db.pool())
    .await
    .expect("live parent invariant query succeeds");
    assert_eq!(live_orphans, 0);
}
