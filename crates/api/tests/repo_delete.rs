mod common;

use api_types::{ErrorResponse, ProjectResponse, RepoResponse, TaskResponse};
use axum::http::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn deleting_task_free_repo_returns_no_content_and_clears_primary_repo() {
    let workspace = common::TestDir::new("repo-delete-empty");
    let harness = common::test_app(workspace.path(), "repo-delete-empty").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Empty Repo Project"}),
        StatusCode::OK,
    )
    .await;
    let repo: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &token,
        json!({"remote_url": "https://example.com/empty.git"}),
        StatusCode::OK,
    )
    .await;

    let selected_project: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        selected_project.primary_repo_id.as_deref(),
        Some(repo.id.as_str())
    );

    let response = common::raw_empty_request(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/repos/{}", repo.id),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read DELETE response body");
    assert!(body.is_empty(), "204 response has no body");

    let missing_repo: ErrorResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/repos/{}", repo.id),
        &token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(missing_repo.code, "not_found");
    let remaining_repo_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM repo WHERE id = ?")
        .bind(&repo.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("Repo count loads");
    assert_eq!(remaining_repo_count, 0);

    let project_after: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(project_after.primary_repo_id, None);
}

#[tokio::test]
async fn deleting_repo_with_task_returns_repo_in_use_and_preserves_state() {
    let workspace = common::TestDir::new("repo-delete-in-use");
    let harness = common::test_app(workspace.path(), "repo-delete-in-use").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Repo In Use Project"}),
        StatusCode::OK,
    )
    .await;
    let repo: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &token,
        json!({"remote_url": "https://example.com/in-use.git"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Uses the Repo"}),
        StatusCode::OK,
    )
    .await;

    let response = common::raw_empty_request(
        &harness.app,
        Method::DELETE,
        &format!("/api/v1/repos/{}", repo.id),
    )
    .await;
    let error: ErrorResponse = common::parse_response(response, StatusCode::CONFLICT).await;
    assert_eq!(error.code, "repo_in_use");

    let repo_after: RepoResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/repos/{}", repo.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(repo_after.id, repo.id);
    let project_after: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        project_after.primary_repo_id.as_deref(),
        Some(repo.id.as_str())
    );
    let task_after: TaskResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(task_after.id, task.id);
}
