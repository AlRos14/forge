#![allow(dead_code)]

mod common;

use api_types::{PaginatedResponse, ProjectResponse};
use axum::{http::Method, http::StatusCode};
use serde_json::json;

#[tokio::test]
async fn project_owner_visibility_requires_the_canonical_membership_row() {
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

    let owner_row: (String, String) =
        sqlx::query_as("SELECT user_id, role FROM project_member WHERE project_id = ?")
            .bind(&project.id)
            .fetch_one(harness.state.db.pool())
            .await
            .expect("Project creation writes the canonical owner membership");
    assert_eq!(owner_row, ("test-user-id".to_owned(), "owner".to_owned()));

    // Simulate a legacy/corrupt row that violates INV-040. Authorization fails
    // closed until the forward migration or an explicit repair restores it.
    sqlx::query("DELETE FROM project_member WHERE project_id = ?")
        .bind(&project.id)
        .execute(harness.state.db.pool())
        .await
        .expect("remove owner membership row");

    let missing: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(missing["code"], "not_found");

    let listed: PaginatedResponse<ProjectResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/projects",
        &token,
        StatusCode::OK,
    )
    .await;
    assert!(!listed.items.iter().any(|item| item.id == project.id));
}
