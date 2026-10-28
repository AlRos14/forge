#![allow(dead_code)]
mod common;

use api_types::{AuthResponse, UserResponse};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
};
use serde::de::DeserializeOwned;
use serde_json::json;
use tower::ServiceExt;

// ── helpers ───────────────────────────────────────────────────────────────

async fn register(app: &axum::Router, email: &str, password: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({ "email": email, "password": password }))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn login(app: &axum::Router, email: &str, password: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({ "email": email, "password": password }))
                        .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn register_and_login(app: &axum::Router, email: &str, password: &str) -> AuthResponse {
    parse_body(register(app, email, password).await, StatusCode::CREATED).await
}

async fn parse_body<T: DeserializeOwned>(
    resp: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected status; body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse response body")
}

async fn do_refresh(app: &axum::Router, refresh_token: &str) -> axum::response::Response {
    let body = serde_json::to_string(&json!({ "refresh_token": refresh_token })).unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/refresh")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn do_logout(app: &axum::Router, refresh_token: &str) -> axum::response::Response {
    let body = serde_json::to_string(&json!({ "refresh_token": refresh_token })).unwrap();
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/logout")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

// ── 4.7: full auth flow ───────────────────────────────────────────────────

#[tokio::test]
async fn auth_full_flow() {
    let workspace_root = common::TestDir::new("auth-flow-ws");
    let harness = common::test_app(workspace_root.path(), "auth-flow").await;
    let app = &harness.app;

    let email = "flow@example.com";
    let password = "Password123!";

    // Register
    let reg_resp = register(app, email, password).await;
    assert_eq!(reg_resp.status(), StatusCode::CREATED);

    // Login
    let auth: AuthResponse = parse_body(login(app, email, password).await, StatusCode::OK).await;
    let access = auth.access_token.clone();
    let refresh = auth.refresh_token.clone();

    // Access protected route
    let me_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let _me: UserResponse = parse_body(me_resp, StatusCode::OK).await;

    // Refresh — issues a new pair and invalidates the old token
    let refreshed: AuthResponse = parse_body(do_refresh(app, &refresh).await, StatusCode::OK).await;
    assert_ne!(refreshed.refresh_token, refresh, "new refresh token issued");

    // Old refresh token must now be rejected
    let old_refresh_resp = do_refresh(app, &refresh).await;
    assert_eq!(
        old_refresh_resp.status(),
        StatusCode::UNAUTHORIZED,
        "old refresh token must be invalid"
    );

    // New access token works
    let me_resp2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/auth/me")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", refreshed.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me_resp2.status(), StatusCode::OK);

    // Logout
    let logout_resp = do_logout(app, &refreshed.refresh_token).await;
    assert_eq!(logout_resp.status(), StatusCode::OK);

    // Refresh after logout must fail
    let post_logout = do_refresh(app, &refreshed.refresh_token).await;
    assert_eq!(
        post_logout.status(),
        StatusCode::UNAUTHORIZED,
        "refresh after logout must fail"
    );
}

// ── 4.8: SSE with a short-lived, single-use ticket ────────────────────────

async fn issue_sse_ticket(
    app: &axum::Router,
    access_token: &str,
) -> api_types::EventTicketResponse {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/events/ticket")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    parse_body(resp, StatusCode::OK).await
}

#[tokio::test]
async fn sse_accepts_short_lived_ticket() {
    let workspace_root = common::TestDir::new("auth-sse-ws");
    let harness = common::test_app(workspace_root.path(), "auth-sse").await;
    let app = &harness.app;

    let auth = register_and_login(app, "sse@example.com", "Password123!").await;
    let ticket = issue_sse_ticket(app, &auth.access_token).await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/events?ticket={}", ticket.ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "SSE with a ticket must be accepted"
    );
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("text/event-stream"),
        "response must be SSE stream"
    );

    let replay = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/events?ticket={}", ticket.ticket))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        replay.status(),
        StatusCode::UNAUTHORIZED,
        "SSE tickets must be single-use"
    );
}

#[tokio::test]
async fn sse_rejects_reusable_token_query_param() {
    let workspace_root = common::TestDir::new("auth-sse-query-ws");
    let harness = common::test_app(workspace_root.path(), "auth-sse-query").await;
    let app = &harness.app;

    let auth = register_and_login(app, "sse-query@example.com", "Password123!").await;

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/events?token={}", auth.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "reusable access tokens are not valid SSE query credentials"
    );
}

#[tokio::test]
async fn query_token_never_authenticates_rest_or_overrides_a_header() {
    let workspace_root = common::TestDir::new("auth-query-ws");
    let harness = common::test_app(workspace_root.path(), "auth-query").await;
    let app = &harness.app;
    let auth = register_and_login(app, "query@example.com", "Password123!").await;

    let query_only = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/auth/me?token={}", auth.access_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(query_only.status(), StatusCode::UNAUTHORIZED);

    let header_wins = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/auth/me?token=not-a-valid-token")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", auth.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        header_wins.status(),
        StatusCode::OK,
        "an invalid query value must never override a valid Authorization header"
    );
}

#[tokio::test]
async fn sse_rejects_missing_token() {
    let workspace_root = common::TestDir::new("auth-sse-noauth-ws");
    let harness = common::test_app(workspace_root.path(), "auth-sse-noauth").await;

    let resp = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── 4.8b: JWTs use current durable authority, not frozen claims ───────────

async fn admin_list_status(app: &axum::Router, access_token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/admin/users")
                .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn jwt_loses_admin_authority_after_degradation() {
    let workspace_root = common::TestDir::new("auth-degrade-ws");
    let harness = common::test_app(workspace_root.path(), "auth-degrade").await;
    let app = &harness.app;

    let auth = register_and_login(app, "degrade@example.com", "Password123!").await;
    assert_eq!(
        admin_list_status(app, &auth.access_token).await,
        StatusCode::OK,
        "bootstrap user starts as admin"
    );

    let user = db::UserRepo::get_user_by_email(&*harness.state.db, "degrade@example.com")
        .await
        .expect("user lookup")
        .expect("user exists");
    db::UserRepo::set_admin(&*harness.state.db, &user.id, false)
        .await
        .expect("degrade admin");

    assert_eq!(
        admin_list_status(app, &auth.access_token).await,
        StatusCode::FORBIDDEN,
        "the existing JWT must lose administrative authority immediately"
    );
}

#[tokio::test]
async fn jwt_for_deleted_user_is_rejected_and_refresh_fails() {
    let workspace_root = common::TestDir::new("auth-deleted-ws");
    let harness = common::test_app(workspace_root.path(), "auth-deleted").await;
    let app = &harness.app;

    let auth = register_and_login(app, "deleted@example.com", "Password123!").await;
    let user = db::UserRepo::get_user_by_email(&*harness.state.db, "deleted@example.com")
        .await
        .expect("user lookup")
        .expect("user exists");
    db::UserRepo::delete_user(&*harness.state.db, &user.id)
        .await
        .expect("delete user");

    let me = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/auth/me")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", auth.access_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);

    let refresh = do_refresh(app, &auth.refresh_token).await;
    assert_eq!(refresh.status(), StatusCode::UNAUTHORIZED);
}

// ── 4.9: daemon registration requires an authenticated user ────────────────

#[tokio::test]
async fn daemon_register_requires_user_authentication() {
    let workspace_root = common::TestDir::new("auth-daemon-ws");
    let harness = common::test_app(workspace_root.path(), "auth-daemon").await;

    // Deliberately send NO Authorization header
    let resp = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/daemons/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "machine_id": "no-jwt-machine",
                        "hostname":   "host",
                        "os":         "linux",
                        "arch":       "x86_64",
                        "agent_version": "test"
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── 4.10: concurrent refresh — single winner ──────────────────────────────

#[tokio::test]
async fn concurrent_refresh_single_winner() {
    let workspace_root = common::TestDir::new("auth-race-ws");
    let harness = common::test_app(workspace_root.path(), "auth-race").await;
    let app = &harness.app;

    let auth = register_and_login(app, "race@example.com", "Password123!").await;
    let token = auth.refresh_token.clone();

    let body = serde_json::to_string(&json!({ "refresh_token": token })).unwrap();

    let app1 = app.clone();
    let app2 = app.clone();
    let body1 = body.clone();
    let body2 = body.clone();

    let make_req = |b: String| {
        Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/refresh")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b))
            .unwrap()
    };

    let (r1, r2) = tokio::join!(
        tokio::spawn(async move { app1.oneshot(make_req(body1)).await.unwrap() }),
        tokio::spawn(async move { app2.oneshot(make_req(body2)).await.unwrap() }),
    );
    let s1 = r1.unwrap().status();
    let s2 = r2.unwrap().status();

    let successes = [s1, s2].iter().filter(|&&s| s == StatusCode::OK).count();
    let failures = [s1, s2]
        .iter()
        .filter(|&&s| s == StatusCode::UNAUTHORIZED)
        .count();

    assert_eq!(successes, 1, "exactly one concurrent refresh must succeed");
    assert_eq!(failures, 1, "the other concurrent refresh must fail");
}
