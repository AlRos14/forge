mod common;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn request_with_bearer(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_string(&body).unwrap())
    } else {
        Body::empty()
    };
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn project_scope_without_project_id_is_rejected_before_cwd_mutation() {
    let workspace = common::TestDir::new("mcp-config-project-scope");
    let harness = common::test_app(workspace.path(), "mcp-config-project-scope").await;
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*harness.state.db,
        &db::User {
            id: "user-b".to_owned(),
            email: "user-b@example.com".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("second user seeds");
    let token = common::test_jwt_for_user("user-b", "user-b@example.com", false);

    let (status, body) = request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/config/mcp",
        &token,
        Some(json!({
            "agent": "claude",
            "scope": "project",
            "action": "install"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "project_id_required");

    let (status, body) = request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/config/mcp?agent=claude&scope=project",
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "project_id_required");

    let (status, body) = request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/config/mcp",
        &token,
        Some(json!({
            "agent": "claude",
            "scope": "project",
            "project_id": "   ",
            "action": "install"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "project_id_required");
}

#[tokio::test]
async fn claude_local_scope_without_project_id_fails_explicitly() {
    let workspace = common::TestDir::new("mcp-config-claude-local");
    let harness = common::test_app(workspace.path(), "mcp-config-claude-local").await;
    let token = common::test_jwt();

    let (status, body) = request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/config/mcp",
        &token,
        Some(json!({
            "agent": "claude",
            "scope": "local",
            "action": "install"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "project_id_required");
}

#[tokio::test]
async fn server_global_scope_still_requires_admin() {
    let workspace = common::TestDir::new("mcp-config-server-scope");
    let harness = common::test_app(workspace.path(), "mcp-config-server-scope").await;
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*harness.state.db,
        &db::User {
            id: "user-b".to_owned(),
            email: "user-b@example.com".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("second user seeds");
    let token = common::test_jwt_for_user("user-b", "user-b@example.com", false);

    let (status, body) = request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/config/mcp",
        &token,
        Some(json!({
            "agent": "claude",
            "scope": "user",
            "action": "install"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "admin_required");
}
