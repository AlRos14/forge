#![allow(dead_code)]

mod common;

use api_types::{ErrorResponse, PaginatedResponse, ProjectOverview, ProjectResponse};
use axum::{http::Method, http::StatusCode};
use serde_json::json;

#[tokio::test]
async fn project_overview_returns_truthful_setup_projection() {
    let workspace = common::TestDir::new("project-overview-setup");
    let harness = common::test_app(workspace.path(), "project-overview-setup").await;
    let token = common::test_jwt();

    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Overview setup project"}),
        StatusCode::OK,
    )
    .await;

    // Ownership is authoritative even if the best-effort membership insert
    // was lost; the Overview must not lock the Project owner out.
    sqlx::query("DELETE FROM project_member WHERE project_id = ?")
        .bind(&project.id)
        .execute(harness.state.db.pool())
        .await
        .expect("remove redundant owner membership");

    let overview: ProjectOverview = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/overview", project.id),
        &token,
        StatusCode::OK,
    )
    .await;

    assert_eq!(overview.project_id, project.id);
    assert_eq!(overview.project_name, "Overview setup project");
    assert_eq!(
        overview.charter_state,
        api_types::ProjectCharterState::CharterSetupRequired
    );
    assert_eq!(
        overview.projection_state,
        api_types::OverviewProjectionState::Stale
    );
    assert!(overview.current_charter.is_none());
    assert!(overview.active_milestones.is_empty());
    assert!(overview
        .next_action
        .as_deref()
        .is_some_and(|value| value.contains("Charter")));
    assert_eq!(overview.task_counts.total, 0);
    assert_eq!(overview.check_summary.required_total, 0);
}

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

#[tokio::test]
async fn project_overview_does_not_probe_an_unknown_project() {
    let workspace = common::TestDir::new("project-overview-auth");
    let harness = common::test_app(workspace.path(), "project-overview-auth").await;

    let error: ErrorResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/projects/not-a-project/overview",
        &common::test_jwt(),
        StatusCode::NOT_FOUND,
    )
    .await;

    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("project"));
}

#[tokio::test]
async fn project_overview_denies_a_non_member_without_probing_project_rows() {
    let workspace = common::TestDir::new("project-overview-denied");
    let harness = common::test_app(workspace.path(), "project-overview-denied").await;
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &common::test_jwt(),
        json!({"name": "Private overview project"}),
        StatusCode::OK,
    )
    .await;
    let error: ErrorResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/overview", project.id),
        &jwt_for_user("different-user-id", "different@example.com"),
        StatusCode::NOT_FOUND,
    )
    .await;

    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("project"));
}

fn jwt_for_user(user_id: &str, email: &str) -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_secs();
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &json!({
            "sub": user_id,
            "email": email,
            "is_admin": false,
            "iat": now,
            "exp": now + 900,
        }),
        &EncodingKey::from_secret(b"test-jwt-secret-for-development"),
    )
    .expect("encode test jwt")
}
