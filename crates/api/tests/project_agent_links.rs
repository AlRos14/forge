#![allow(dead_code)]
mod common;

use api_types::{
    AgentResponse, AuthResponse, ErrorResponse, ProjectAgentLinkResponse, ProjectMemberResponse,
    ProjectResponse, TaskResponse, TaskRoleAssignmentResponse, UserResponse,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn list_project_agents_scopes_to_project_usable_agents() {
    let ws = common::TestDir::new("project-agents-list-ws");
    let harness = common::test_app(ws.path(), "project-agents-list").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-list").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-list@example.com").await;
    let non_member = register_user(app, "non-member-list@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;

    let linked_owner_agent = create_agent(app, &owner_token, "linked-owner-agent").await;
    let unlinked_owner_agent = create_agent(app, &owner_token, "unlinked-owner-agent").await;
    let member_owned_agent = create_agent(app, &member.token, "member-owned-agent").await;
    let _: ProjectAgentLinkResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": linked_owner_agent.id.clone() }),
        StatusCode::CREATED,
    )
    .await;

    let error: ErrorResponse = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/agents"),
        &non_member.token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(error.code, "not_found");

    let agents: Vec<AgentResponse> = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/agents"),
        &member.token,
        StatusCode::OK,
    )
    .await;

    assert!(
        has_agent(&agents, member_owned_agent.id.as_str()),
        "project member must see their own account agent"
    );
    assert!(
        has_agent(&agents, linked_owner_agent.id.as_str()),
        "project member must see explicitly linked account agent"
    );
    assert!(
        !has_agent(&agents, unlinked_owner_agent.id.as_str()),
        "project member must not see another user's unlinked account agent"
    );
}

#[tokio::test]
async fn list_project_agent_links_requires_membership_and_returns_explicit_links_only() {
    let ws = common::TestDir::new("project-agent-links-list-ws");
    let harness = common::test_app(ws.path(), "project-agent-links-list").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-links-list").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-links@example.com").await;
    let non_member = register_user(app, "non-member-links@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;
    let linked_agent = create_agent(app, &owner_token, "explicitly-linked-agent").await;
    let member_owned_agent = create_agent(app, &member.token, "implicit-member-agent").await;
    let link: ProjectAgentLinkResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": linked_agent.id.clone() }),
        StatusCode::CREATED,
    )
    .await;

    let error: ErrorResponse = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &non_member.token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(error.code, "not_found");

    let links: Vec<ProjectAgentLinkResponse> = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &member.token,
        StatusCode::OK,
    )
    .await;

    assert_eq!(
        links.len(),
        1,
        "implicit agent access must not create links"
    );
    assert_eq!(links[0].id, link.id);
    assert_eq!(links[0].project_id, project_id);
    assert_eq!(links[0].agent_id, linked_agent.id);
    assert_ne!(links[0].agent_id, member_owned_agent.id);
}

#[tokio::test]
async fn create_project_agent_link_enforces_admin_visibility_and_uniqueness() {
    let ws = common::TestDir::new("project-agent-link-create-ws");
    let harness = common::test_app(ws.path(), "project-agent-link-create").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-link-create").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-create-link@example.com").await;
    let hidden_owner = register_user(app, "hidden-owner-create-link@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;

    let visible_owner_agent = create_agent(app, &owner_token, "visible-owner-agent").await;
    let member_owned_agent = create_agent(app, &member.token, "member-visible-agent").await;
    let hidden_agent = create_agent(app, &hidden_owner.token, "hidden-other-user-agent").await;

    let link: ProjectAgentLinkResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": visible_owner_agent.id.clone() }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(link.project_id, project_id);
    assert_eq!(link.agent_id, visible_owner_agent.id);
    assert_eq!(link.linked_by_user_id, "test-user-id");

    let member_error: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &member.token,
        json!({ "agent_id": member_owned_agent.id.clone() }),
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(member_error.code, "insufficient_role");

    let hidden_error: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": hidden_agent.id.clone() }),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(hidden_error.code, "not_found");

    let duplicate_error: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": visible_owner_agent.id.clone() }),
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(duplicate_error.code, "agent_already_linked");
}

#[tokio::test]
async fn delete_project_agent_link_requires_project_admin() {
    let ws = common::TestDir::new("project-agent-link-delete-ws");
    let harness = common::test_app(ws.path(), "project-agent-link-delete").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-link-delete").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-delete-link@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;
    let linked_agent = create_agent(app, &owner_token, "delete-linked-agent").await;
    let _: ProjectAgentLinkResponse = common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        json!({ "agent_id": linked_agent.id.clone() }),
        StatusCode::CREATED,
    )
    .await;

    let member_response = raw_bearer_empty_request(
        app,
        Method::DELETE,
        &format!(
            "/api/v1/projects/{project_id}/agent-links/{}",
            linked_agent.id
        ),
        &member.token,
    )
    .await;
    let member_error: ErrorResponse =
        common::parse_response(member_response, StatusCode::FORBIDDEN).await;
    assert_eq!(member_error.code, "insufficient_role");

    let owner_response = raw_bearer_empty_request(
        app,
        Method::DELETE,
        &format!(
            "/api/v1/projects/{project_id}/agent-links/{}",
            linked_agent.id
        ),
        &owner_token,
    )
    .await;
    assert_eq!(owner_response.status(), StatusCode::NO_CONTENT);

    let links: Vec<ProjectAgentLinkResponse> = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/agent-links"),
        &owner_token,
        StatusCode::OK,
    )
    .await;
    assert!(
        links.iter().all(|link| link.agent_id != linked_agent.id),
        "deleted link must be absent from explicit link list"
    );
}

#[tokio::test]
async fn role_assignment_rejects_unlinked_account_agent() {
    let ws = common::TestDir::new("project-agent-role-unlinked-ws");
    let harness = common::test_app(ws.path(), "project-agent-role-unlinked").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-role-unlinked").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-role-unlinked@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;
    let member_agent = create_agent(app, &member.token, "member-role-agent").await;
    let unlinked_owner_agent = create_agent(app, &owner_token, "unlinked-role-agent").await;
    let task = create_task(app, project_id, "unlinked agent role validation").await;

    let initial_assignment: TaskRoleAssignmentResponse = common::json_request_with_bearer(
        app,
        Method::PUT,
        &format!("/api/v1/tasks/{}/roles/coder", task.id),
        &member.token,
        json!({ "assignee_type": "agent", "assignee_id": member_agent.id.clone() }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(initial_assignment.assignee_type.as_deref(), Some("agent"));
    assert_eq!(
        initial_assignment.assignee_id.as_deref(),
        Some(member_agent.id.as_str())
    );

    let error: ErrorResponse = common::json_request_with_bearer(
        app,
        Method::PUT,
        &format!("/api/v1/tasks/{}/roles/coder", task.id),
        &member.token,
        json!({ "assignee_type": "agent", "assignee_id": unlinked_owner_agent.id.clone() }),
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(error.code, "not_found");

    let roles: TaskRoleAssignmentListResponse = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/tasks/{}/roles", task.id),
        &member.token,
        StatusCode::OK,
    )
    .await;
    let coder = roles
        .items
        .iter()
        .find(|assignment| assignment.role_name == "coder")
        .expect("coder role assignment remains present");
    assert_eq!(coder.assignee_type.as_deref(), Some("agent"));
    assert_eq!(coder.assignee_id.as_deref(), Some(member_agent.id.as_str()));
}

#[tokio::test]
async fn role_assignment_accepts_project_member_user_and_rejects_non_member_user() {
    let ws = common::TestDir::new("project-agent-role-user-ws");
    let harness = common::test_app(ws.path(), "project-agent-role-user").await;
    let app = &harness.app;
    let owner_token = common::test_jwt();
    let project = create_project(app, "project-agent-role-user").await;
    let project_id = project.id.as_str();
    let member = register_user(app, "member-role-user@example.com").await;
    let non_member = register_user(app, "non-member-role-user@example.com").await;

    add_project_member(app, project_id, &member.user_id, "member", &owner_token).await;
    let task = create_task(app, project_id, "project member user role validation").await;

    let assignment: TaskRoleAssignmentResponse = common::json_request_with_bearer(
        app,
        Method::PUT,
        &format!("/api/v1/tasks/{}/roles/reviewer", task.id),
        &owner_token,
        json!({ "assignee_type": "user", "assignee_id": member.user_id.clone() }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(assignment.role_name, "reviewer");
    assert_eq!(assignment.assignee_type.as_deref(), Some("user"));
    assert_eq!(
        assignment.assignee_id.as_deref(),
        Some(member.user_id.as_str())
    );

    let response = raw_bearer_json_request(
        app,
        Method::PUT,
        &format!("/api/v1/tasks/{}/roles/reviewer", task.id),
        &owner_token,
        json!({ "assignee_type": "user", "assignee_id": non_member.user_id.clone() }),
    )
    .await;
    let status = response.status();
    let error: ErrorResponse = parse_body(response).await;
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
        ),
        "assigning a non-member user must fail, got {status} with {error:?}"
    );
    assert!(
        matches!(
            error.code.as_str(),
            "bad_request" | "not_found" | "insufficient_role"
        ),
        "unexpected error code for non-member user assignment: {}",
        error.code
    );

    let roles: TaskRoleAssignmentListResponse = bearer_empty_request(
        app,
        Method::GET,
        &format!("/api/v1/tasks/{}/roles", task.id),
        &owner_token,
        StatusCode::OK,
    )
    .await;
    let reviewer = roles
        .items
        .iter()
        .find(|assignment| assignment.role_name == "reviewer")
        .expect("reviewer role assignment remains present");
    assert_eq!(reviewer.assignee_type.as_deref(), Some("user"));
    assert_eq!(
        reviewer.assignee_id.as_deref(),
        Some(member.user_id.as_str())
    );
}

#[derive(Debug)]
struct RegisteredUser {
    token: String,
    user_id: String,
}

#[derive(Debug, serde::Deserialize)]
struct TaskRoleAssignmentListResponse {
    items: Vec<TaskRoleAssignmentResponse>,
}

async fn register_user(app: &Router, email: &str) -> RegisteredUser {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_string(&json!({
                        "email": email,
                        "password": "Password123!"
                    }))
                    .unwrap(),
                ))
                .expect("build register request"),
        )
        .await
        .expect("router response");
    let auth: AuthResponse = common::parse_response(response, StatusCode::CREATED).await;
    let me: UserResponse = bearer_empty_request(
        app,
        Method::GET,
        "/api/v1/auth/me",
        &auth.access_token,
        StatusCode::OK,
    )
    .await;
    RegisteredUser {
        token: auth.access_token,
        user_id: me.id,
    }
}

async fn create_project(app: &Router, name: &str) -> ProjectResponse {
    common::json_request(
        app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": name }),
        StatusCode::OK,
    )
    .await
}

async fn add_project_member(
    app: &Router,
    project_id: &str,
    user_id: &str,
    role: &str,
    token: &str,
) -> ProjectMemberResponse {
    common::json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/members"),
        token,
        json!({ "user_id": user_id, "role": role }),
        StatusCode::CREATED,
    )
    .await
}

async fn create_agent(app: &Router, token: &str, name: &str) -> AgentResponse {
    common::json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/agents",
        token,
        json!({ "name": name, "executor_type": "shell" }),
        StatusCode::OK,
    )
    .await
}

async fn create_task(app: &Router, project_id: &str, title: &str) -> TaskResponse {
    common::json_request(
        app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": title }),
        StatusCode::OK,
    )
    .await
}

fn has_agent(agents: &[AgentResponse], agent_id: &str) -> bool {
    agents.iter().any(|agent| agent.id == agent_id)
}

async fn bearer_empty_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let response = raw_bearer_empty_request(app, method, uri, token).await;
    common::parse_response(response, expected_status).await
}

async fn raw_bearer_empty_request(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build empty bearer request"),
        )
        .await
        .expect("router response")
}

async fn raw_bearer_json_request(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .expect("build JSON bearer request"),
        )
        .await
        .expect("router response")
}

async fn parse_body<T>(response: axum::response::Response) -> T
where
    T: DeserializeOwned,
{
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    serde_json::from_slice(&bytes).expect("parse JSON response")
}
