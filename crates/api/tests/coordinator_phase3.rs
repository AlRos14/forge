#![allow(dead_code)]

//! REST contract for the phase-3 fallback, micro-fix and replan read models.

mod common;

use axum::http::{Method, StatusCode};
use common::{
    create_project_and_repo, empty_request, empty_request_with_bearer, json_request,
    setup_git_repo, test_app, test_jwt_for_user, TestDir,
};
use serde_json::{json, Value};

struct Fixture {
    harness: common::Harness,
    task_id: String,
    _workspace: TestDir,
    _repo: TestDir,
}

async fn fixture(prefix: &str) -> Fixture {
    let workspace = TestDir::new(&format!("{prefix}-workspaces"));
    let repo_root = TestDir::new(&format!("{prefix}-repo"));
    let repo_path = setup_git_repo(repo_root.path());
    let harness = test_app(workspace.path(), prefix).await;
    let (project_id, _) =
        create_project_and_repo(&harness.app, "Phase three receipts", &repo_path).await;
    let task: api_types::TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Phase three Task"}),
        StatusCode::OK,
    )
    .await;
    Fixture {
        harness,
        task_id: task.id,
        _workspace: workspace,
        _repo: repo_root,
    }
}

#[tokio::test]
async fn task_phase_three_receipts_use_paginated_task_visible_reads() {
    let f = fixture("coordinator-phase3-reads").await;
    for suffix in ["fallback-receipts", "micro-fixes", "coordinator-replans"] {
        let response: Value = empty_request(
            &f.harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/{}?limit=1", f.task_id, suffix),
            StatusCode::OK,
        )
        .await;
        assert_eq!(response["items"], json!([]), "{suffix}");
        assert!(response["next_cursor"].is_null(), "{suffix}");
    }

    let cursor_error: Value = empty_request(
        &f.harness.app,
        Method::GET,
        &format!(
            "/api/v1/tasks/{}/fallback-receipts?cursor=invalid",
            f.task_id
        ),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(cursor_error["code"], "invalid_cursor");
}

#[tokio::test]
async fn phase_three_receipt_reads_hide_tasks_from_non_members() {
    let f = fixture("coordinator-phase3-visibility").await;
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*f.harness.state.db,
        &db::User {
            id: "phase3-outsider".to_owned(),
            email: "phase3-outsider@example.com".to_owned(),
            password_hash: "placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("outsider user creates");
    let token = test_jwt_for_user("phase3-outsider", "phase3-outsider@example.com", false);

    for suffix in ["fallback-receipts", "micro-fixes", "coordinator-replans"] {
        let error: Value = empty_request_with_bearer(
            &f.harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/{}", f.task_id, suffix),
            &token,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(error["code"], "not_found", "{suffix}");
    }
}
