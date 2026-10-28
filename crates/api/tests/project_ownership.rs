mod common;

use api_types::{ProjectMemberResponse, ProjectResponse};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use serde_json::json;
use tower::ServiceExt;

async fn seed_user(harness: &common::Harness, id: &str, email: &str) {
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*harness.state.db,
        &db::User {
            id: id.to_owned(),
            email: email.to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("user seeds");
}

async fn request(
    harness: &common::Harness,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if let Some(body) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_string(&body).unwrap())
    } else {
        Body::empty()
    };
    harness
        .app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("response parses")
}

#[tokio::test]
async fn rest_project_creation_writes_owner_and_canonical_membership_atomically() {
    let workspace = common::TestDir::new("project-owner-create");
    let harness = common::test_app(workspace.path(), "project-owner-create").await;
    seed_user(&harness, "owner-a", "owner-a@example.com").await;
    let token = common::test_jwt_for_user("owner-a", "owner-a@example.com", false);

    let project: ProjectResponse = json(
        request(
            &harness,
            Method::POST,
            "/api/v1/projects",
            &token,
            Some(json!({ "name": "Atomic owner" })),
        )
        .await,
        StatusCode::OK,
    )
    .await;

    let (owner_id, membership_role): (Option<String>, String) = sqlx::query_as(
        "SELECT p.owner_id, pm.role FROM project p \
         JOIN project_member pm ON pm.project_id = p.id AND pm.user_id = p.owner_id \
         WHERE p.id = ?",
    )
    .bind(&project.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("canonical owner membership exists");
    assert_eq!(owner_id.as_deref(), Some("owner-a"));
    assert_eq!(membership_role, "owner");
}

#[tokio::test]
async fn rest_owner_transfer_and_removal_preserve_one_canonical_owner() {
    let workspace = common::TestDir::new("project-owner-transfer");
    let harness = common::test_app(workspace.path(), "project-owner-transfer").await;
    seed_user(&harness, "owner-a", "owner-a@example.com").await;
    seed_user(&harness, "owner-b", "owner-b@example.com").await;
    let owner_a = common::test_jwt_for_user("owner-a", "owner-a@example.com", false);

    let project: ProjectResponse = json(
        request(
            &harness,
            Method::POST,
            "/api/v1/projects",
            &owner_a,
            Some(json!({ "name": "Transfer owner" })),
        )
        .await,
        StatusCode::OK,
    )
    .await;

    let member: ProjectMemberResponse = json(
        request(
            &harness,
            Method::POST,
            &format!("/api/v1/projects/{}/members", project.id),
            &owner_a,
            Some(json!({ "user_id": "owner-b", "role": "owner" })),
        )
        .await,
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(member.role, "owner");

    let removal = request(
        &harness,
        Method::DELETE,
        &format!("/api/v1/projects/{}/members/owner-a", project.id),
        &owner_a,
        None,
    )
    .await;
    assert_eq!(removal.status(), StatusCode::NO_CONTENT);

    let (owner_id, old_membership): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT p.owner_id, \
                (SELECT role FROM project_member WHERE project_id = p.id AND user_id = 'owner-a') \
         FROM project p WHERE p.id = ?",
    )
    .bind(&project.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("project owner loads");
    assert_eq!(owner_id.as_deref(), Some("owner-b"));
    assert_eq!(old_membership, None);

    let owner_b = common::test_jwt_for_user("owner-b", "owner-b@example.com", false);
    let last_owner = request(
        &harness,
        Method::DELETE,
        &format!("/api/v1/projects/{}/members/owner-b", project.id),
        &owner_b,
        None,
    )
    .await;
    assert_eq!(
        last_owner.status(),
        StatusCode::CONFLICT,
        "the last canonical owner cannot be removed"
    );
}

#[tokio::test]
async fn ownerless_historical_project_is_never_claimed_automatically() {
    let workspace = common::TestDir::new("project-ownerless");
    let harness = common::test_app(workspace.path(), "project-ownerless").await;
    seed_user(&harness, "reader", "reader@example.com").await;
    let now = db::now_rfc3339();
    sqlx::query(
        "INSERT INTO project (id, name, settings, owner_id, created_at, updated_at) \
         VALUES ('ownerless', 'Ownerless', '{}', NULL, ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(harness.state.db.pool())
    .await
    .unwrap();

    let reader = common::test_jwt_for_user("reader", "reader@example.com", false);
    let project: ProjectResponse = json(
        request(
            &harness,
            Method::GET,
            "/api/v1/projects/ownerless",
            &reader,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(project.owner_id, None);

    let mutation = request(
        &harness,
        Method::PATCH,
        "/api/v1/projects/ownerless",
        &reader,
        Some(json!({ "version": project.version, "name": "Still ownerless" })),
    )
    .await;
    assert_eq!(
        mutation.status(),
        StatusCode::FORBIDDEN,
        "ownerless visibility is not write authority"
    );
    let owner: Option<String> =
        sqlx::query_scalar("SELECT owner_id FROM project WHERE id = 'ownerless'")
            .fetch_one(harness.state.db.pool())
            .await
            .unwrap();
    assert_eq!(owner, None);
}
