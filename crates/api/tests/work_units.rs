mod common;

use api_types::{TaskResponse, WorkUnitResponse};
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn work_unit_api_creates_lists_and_rejects_cross_task_parent() {
    let repo_dir = TestDir::new("pr5-work-unit-api-repo");
    let repo_path = setup_git_repo(repo_dir.path());
    let workspace_root = TestDir::new("pr5-work-unit-api-workspaces");
    let harness = test_app(workspace_root.path(), "pr5-work-unit-api").await;
    let (project_id, _) = create_project_and_repo(&harness.app, "PR5 WorkUnits", &repo_path).await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "WorkUnit API scope" }),
        StatusCode::OK,
    )
    .await;
    let other_task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "Other WorkUnit API scope" }),
        StatusCode::OK,
    )
    .await;

    let role = format!("pr5-{}", uuid::Uuid::new_v4());
    let now = db::now_rfc3339();
    db::TaskRoleRepo::create(
        &*harness.state.db,
        db::CreateTaskRole {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task.id.clone(),
            role: role.clone(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("TaskRole exists for WorkUnit allocation context");

    let unit: WorkUnitResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Implement isolated scope",
            "scope": "Implement the endpoint and focused persistence",
            "role": role.clone(),
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(unit.task_id, task.id);
    assert_eq!(unit.status, api_types::WorkUnitStatus::Open);
    assert!(unit.readiness.runnable);
    assert!(unit.readiness.ready_for_allocation);

    let units: Vec<WorkUnitResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        StatusCode::OK,
    )
    .await;
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].id, unit.id);

    let cross_task_parent = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", other_task.id),
        json!({
            "title": "Cross Task parent must be hidden",
            "scope": "Cannot resolve parent in another Task",
            "role": role.clone(),
            "parent_work_unit_id": unit.id,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
    )
    .await;
    assert_eq!(cross_task_parent.status(), StatusCode::NOT_FOUND);

    let unknown_field = raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Reject unknown fields",
            "scope": "Strict request DTO",
            "role": role,
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null,
            "workspace_path": "/tmp/forbidden"
        }),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
