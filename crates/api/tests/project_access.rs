#![allow(dead_code)]

mod common;

use api_types::{PaginatedResponse, ProjectResponse};
use axum::{http::Method, http::StatusCode};
use serde_json::json;

#[tokio::test]
async fn project_owner_can_list_and_get_without_membership_row() {
    let workspace = common::TestDir::new("project-owner-visibility");
    let harness = common::test_app(workspace.path(), "project-owner-visibility").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Owner visibility project"}),
        StatusCode::OK,
    )
    .await;

    // Direct/API creation labels the owner on the Project itself; membership
    // is not the authority source and may be absent on a legacy row.
    sqlx::query("DELETE FROM project_member WHERE project_id = ?")
        .bind(&project.id)
        .execute(harness.state.db.pool())
        .await
        .expect("remove owner membership row");

    let fetched: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(fetched.id, project.id);

    let listed: PaginatedResponse<ProjectResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/projects",
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(listed.items.iter().any(|item| item.id == project.id));
}
