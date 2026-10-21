mod common;

use api_types::{
    ActorRef, AuthResponse, ErrorResponse, ExecutionResponse, IntegrationResponse, ProjectResponse,
    RepoResponse, TaskLifecycleState, TaskLifecycleTransitionResponse, TaskResponse,
    WorkUnitResponse,
};
use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn project_and_task_creation_are_ordinary_v2_operations() {
    let workspace = common::TestDir::new("pr12-ordinary-project");
    let harness = common::test_app(workspace.path(), "pr12-ordinary-project").await;
    let token = common::test_jwt();

    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Ordinary Project"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Ordinary Task"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(task.project_id, project.id);
    assert_eq!(task.lifecycle.state, TaskLifecycleState::Backlog);
    assert!(serde_json::to_value(&task)
        .expect("Task response serializes")
        .get("status")
        .is_none());

    for (method, path, body) in [
        (
            Method::POST,
            "/api/v1/projects".to_owned(),
            json!({"name": "Legacy setup", "project_agent_identity_id": "agent-old"}),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/tasks", project.id),
            json!({"title": "Legacy governance", "governance": {}}),
        ),
    ] {
        let response = common::raw_json_request(&harness.app, method, &path, body).await;
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "legacy fields must be rejected without mutation: {path} returned {}",
            response.status()
        );
    }

    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("Task count");
    assert_eq!(task_count, 1, "unknown legacy input creates no Task");
}

#[tokio::test]
async fn task_scoped_workspace_reset_route_is_retired() {
    let workspace = common::TestDir::new("pr13-task-workspace-reset-retired");
    let harness = common::test_app(workspace.path(), "pr13-task-workspace-reset-retired").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Workspace identity"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Exact Workspace"}),
        StatusCode::OK,
    )
    .await;

    let status = request_status(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/workspace/reset", task.id),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_repo_and_integration_routes_enforce_user_visibility() {
    let workspace = common::TestDir::new("pr12-project-route-visibility");
    let harness = common::test_app(workspace.path(), "pr12-project-route-visibility").await;
    let token_a = common::test_jwt();
    let token_b = common::test_jwt_for_user("user-b", "user-b@example.com", false);
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
    .expect("seed second user");

    let project_a: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_a,
        json!({"name": "Private A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_b,
        json!({"name": "Private B"}),
        StatusCode::OK,
    )
    .await;
    let repo_a: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project_a.id),
        &token_a,
        json!({"remote_url": "https://example.com/a/repo.git"}),
        StatusCode::OK,
    )
    .await;
    let integration_a: IntegrationResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/integration", project_a.id),
        &token_a,
        json!({
            "platform": "github",
            "base_url": "https://api.github.com",
            "owner": "owner-a",
            "repo": "repo-a",
            "credential_env_var": "GITHUB_TOKEN"
        }),
        StatusCode::CREATED,
    )
    .await;

    let visible_to_a: api_types::PaginatedResponse<ProjectResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/projects",
            &token_a,
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        visible_to_a
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![project_a.id.as_str()]
    );
    let visible_to_b: api_types::PaginatedResponse<ProjectResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/projects",
            &token_b,
            StatusCode::OK,
        )
        .await;
    assert_eq!(
        visible_to_b
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![project_b.id.as_str()]
    );

    for (method, path, body) in [
        (
            Method::POST,
            format!("/api/v1/projects/{}/repos", project_a.id),
            Some(json!({"remote_url": "https://example.com/b/forbidden.git"})),
        ),
        (
            Method::GET,
            format!("/api/v1/projects/{}/repos", project_a.id),
            None,
        ),
        (Method::GET, format!("/api/v1/repos/{}", repo_a.id), None),
        (
            Method::PATCH,
            format!("/api/v1/repos/{}", repo_a.id),
            Some(json!({"name": "cross-user-change"})),
        ),
        (
            Method::POST,
            format!("/api/v1/repos/{}/sync", repo_a.id),
            None,
        ),
        (Method::DELETE, format!("/api/v1/repos/{}", repo_a.id), None),
        (
            Method::POST,
            format!("/api/v1/projects/{}/integration", project_a.id),
            Some(json!({
                "platform": "github",
                "base_url": "https://api.github.com",
                "owner": "attacker",
                "repo": "forbidden",
                "credential_env_var": "GITHUB_TOKEN"
            })),
        ),
        (
            Method::GET,
            format!("/api/v1/projects/{}/integration", project_a.id),
            None,
        ),
        (
            Method::PATCH,
            format!("/api/v1/projects/{}/integration", project_a.id),
            Some(json!({"repo": "cross-user-change"})),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/integration/sync", project_a.id),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/v1/projects/{}/integration", project_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/projects/{}/project_hook_runs", project_a.id),
            None,
        ),
        (
            Method::PATCH,
            format!("/api/v1/projects/{}", project_a.id),
            Some(json!({"version": project_a.version, "name": "cross-user-change"})),
        ),
        (
            Method::GET,
            format!(
                "/api/v1/config/mcp?agent=claude&scope=project&project_id={}",
                project_a.id
            ),
            None,
        ),
        (
            Method::POST,
            "/api/v1/config/mcp".to_owned(),
            Some(json!({
                "agent": "claude",
                "scope": "project",
                "project_id": project_a.id,
                "action": "install"
            })),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/pause", project_a.id),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/resume", project_a.id),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/v1/projects/{}", project_a.id),
            None,
        ),
    ] {
        let status = request_status(&harness.app, method, &path, &token_b, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-user request {path}");
    }

    let repo_after: RepoResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/repos/{}", repo_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert_ne!(repo_after.name, "cross-user-change");
    let integration_after: Option<IntegrationResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/integration", project_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        integration_after.expect("integration retained").id,
        integration_a.id
    );
}

#[tokio::test]
async fn task_and_execution_routes_enforce_project_visibility() {
    let workspace = common::TestDir::new("pr12-task-route-visibility");
    let harness = common::test_app(workspace.path(), "pr12-task-route-visibility").await;
    let token_a = common::test_jwt();
    let token_b = common::test_jwt_for_user("user-b", "user-b@example.com", false);
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
    .expect("seed second user");

    let project_a: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_a,
        json!({"name": "Private A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_b,
        json!({"name": "Private B"}),
        StatusCode::OK,
    )
    .await;
    let repo_a: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project_a.id),
        &token_a,
        json!({"remote_url": "https://example.com/a/repo.git"}),
        StatusCode::OK,
    )
    .await;
    let task_a: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_a.id),
        &token_a,
        json!({"title": "Task A"}),
        StatusCode::OK,
    )
    .await;
    let task_b: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_b.id),
        &token_b,
        json!({"title": "Task B"}),
        StatusCode::OK,
    )
    .await;
    let workspace_id = db::new_uuid_v4();
    db::WorkspaceRepo::create(
        &*harness.state.db,
        db::CreateWorkspace {
            id: workspace_id.clone(),
            task_id: task_a.id.clone(),
            repo_id: repo_a.id,
            worktree_path: workspace
                .path()
                .join("worktree")
                .to_string_lossy()
                .into_owned(),
            branch: "task-a".to_owned(),
            status: db::WorkspaceStatus::Ready,
            before_sha: None,
            created_at: db::now_rfc3339(),
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("create task workspace fixture");
    let execution_id = db::new_uuid_v4();
    db::ExecutionRepo::create(
        &*harness.state.db,
        db::CreateExecution {
            id: execution_id.clone(),
            task_id: task_a.id.clone(),
            agent_id: None,
            actor_ref: Some(db::ActorRef::Human("test-user-id".to_owned())),
            role: "reviewer".to_owned(),
            purpose: Some(db::ExecutionPurpose::Review),
            status: db::ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("historical review fixture".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: db::now_rfc3339(),
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("create execution fixture");

    for (method, path, body) in [
        (Method::GET, format!("/api/v1/tasks/{}", task_a.id), None),
        (
            Method::POST,
            format!("/api/v1/projects/{}/tasks", project_a.id),
            Some(json!({"title": "forbidden create"})),
        ),
        (
            Method::GET,
            format!("/api/v1/projects/{}/tasks", project_a.id),
            None,
        ),
        (
            Method::PATCH,
            format!("/api/v1/tasks/{}", task_a.id),
            Some(json!({"version": task_a.version, "title": "forbidden update"})),
        ),
        (Method::DELETE, format!("/api/v1/tasks/{}", task_a.id), None),
        (
            Method::POST,
            format!("/api/v1/tasks/{}/dependencies", task_a.id),
            Some(json!({"depends_on_id": task_b.id})),
        ),
        (
            Method::GET,
            format!("/api/v1/tasks/{}/dependencies", task_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/tasks/{}/workspace", task_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/tasks/{}/diff", task_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/tasks/{}/executions", task_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/tasks/{}/usage", task_a.id),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/executions/{execution_id}"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/executions/{execution_id}/logs"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/executions/{execution_id}/hook-logs"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/executions/{execution_id}/usage"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/executions/{execution_id}/follow-up"),
            Some(
                json!({"idempotency_key": "pr12-surface-follow-up", "agent_id": "agent-any", "message": "continue"}),
            ),
        ),
        (
            Method::POST,
            format!("/api/v1/executions/{execution_id}/cancel"),
            None,
        ),
        (
            Method::GET,
            format!("/api/v1/workspaces/{workspace_id}/diff"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/tasks/{}/external-links", task_a.id),
            Some(json!({"remote_issue_number": 10})),
        ),
    ] {
        let status = request_status(&harness.app, method, &path, &token_b, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-user request {path}");
    }

    let task_after: TaskResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert_eq!(task_after.title, "Task A");
}

#[tokio::test]
async fn notification_reads_and_mutations_are_limited_to_visible_projects() {
    let workspace = common::TestDir::new("pr12-notification-visibility");
    let harness = common::test_app(workspace.path(), "pr12-notification-visibility").await;
    let token_a = common::test_jwt();
    let token_b = common::test_jwt_for_user("user-b", "user-b@example.com", false);
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
    .expect("seed second user");
    let project_a: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_a,
        json!({"name": "Private A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_b,
        json!({"name": "Private B"}),
        StatusCode::OK,
    )
    .await;
    let notification_a = db::NotificationRepo::create(
        &*harness.state.db,
        db::CreateNotification {
            id: db::new_uuid_v4(),
            project_id: project_a.id.clone(),
            task_id: None,
            event_type: "test.event".to_owned(),
            title: "A only".to_owned(),
            body: None,
            read: false,
            created_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("create Project A notification");
    let notification_b = db::NotificationRepo::create(
        &*harness.state.db,
        db::CreateNotification {
            id: db::new_uuid_v4(),
            project_id: project_b.id.clone(),
            task_id: None,
            event_type: "test.event".to_owned(),
            title: "B only".to_owned(),
            body: Some("private notification body".to_owned()),
            read: false,
            created_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("create Project B notification");

    let visible: api_types::PaginatedResponse<api_types::NotificationResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/notifications?include_total=true",
            &token_a,
            StatusCode::OK,
        )
        .await;
    assert_eq!(visible.items.len(), 1);
    assert_eq!(visible.items[0].id, notification_a.id);
    assert_eq!(visible.total_count, Some(1));
    let unread: api_types::UnreadCountResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        "/api/v1/notifications/unread-count",
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert_eq!(unread.count, 1);

    for (method, path, body) in [
        (
            Method::GET,
            format!("/api/v1/notifications?project_id={}", project_b.id),
            None,
        ),
        (
            Method::GET,
            format!(
                "/api/v1/notifications/unread-count?project_id={}",
                project_b.id
            ),
            None,
        ),
        (
            Method::POST,
            format!(
                "/api/v1/notifications/mark-all-read?project_id={}",
                project_b.id
            ),
            None,
        ),
        (
            Method::PATCH,
            format!("/api/v1/notifications/{}/read", notification_b.id),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/v1/notifications/{}", notification_b.id),
            None,
        ),
    ] {
        let status = request_status(&harness.app, method, &path, &token_a, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "cross-user request {path}");
    }

    let status = request_status(
        &harness.app,
        Method::POST,
        "/api/v1/notifications/mark-all-read",
        &token_a,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let notification_b_read: i64 = sqlx::query_scalar("SELECT read FROM notification WHERE id = ?")
        .bind(&notification_b.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("read B notification state");
    assert_eq!(notification_b_read, 0);

    let visible_to_b: api_types::PaginatedResponse<api_types::NotificationResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/notifications",
            &token_b,
            StatusCode::OK,
        )
        .await;
    assert_eq!(visible_to_b.items.len(), 1);
    assert_eq!(visible_to_b.items[0].id, notification_b.id);
}

#[tokio::test]
async fn agent_roster_filters_private_agents_before_pagination() {
    let workspace = common::TestDir::new("pr12-agent-roster-visibility");
    let harness = common::test_app(workspace.path(), "pr12-agent-roster-visibility").await;
    let token_a = common::test_jwt();
    let token_b = common::test_jwt_for_user("user-b", "user-b@example.com", false);
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
    .expect("seed second user");
    let private_agent =
        common::create_harness_agent(&harness.app, &token_b, "User B private Agent")
            .await
            .agent;

    let visible_to_a: api_types::PaginatedResponse<api_types::AgentResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/agents?limit=100&include_total=true",
            &token_a,
            StatusCode::OK,
        )
        .await;
    assert!(visible_to_a
        .items
        .iter()
        .all(|agent| agent.id != private_agent.id));
    assert_eq!(
        visible_to_a.total_count,
        Some(visible_to_a.items.len() as u64),
        "count metadata must cover the visible roster only"
    );

    let visible_to_b: api_types::PaginatedResponse<api_types::AgentResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/agents?limit=100&include_total=true",
            &token_b,
            StatusCode::OK,
        )
        .await;
    assert!(visible_to_b
        .items
        .iter()
        .any(|agent| agent.id == private_agent.id));
}

async fn request_status(
    app: &Router,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_string(&body).expect("serialize request body"))
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).expect("build request"))
        .await
        .expect("router response")
        .status()
}

#[tokio::test]
async fn repository_provider_settings_are_readable_and_patchable_without_secret_echo() {
    let workspace = common::TestDir::new("pr12-repo-provider-surface");
    let harness = common::test_app(workspace.path(), "pr12-repo-provider-surface").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Provider Project"}),
        StatusCode::OK,
    )
    .await;
    let created: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &token,
        json!({
            "remote_url": "https://example.com/acme/provider.git",
            "work_mode": "pull_request",
            "pr_provider": "github",
            "pr_provider_config": {"polling_interval_seconds": 300, "token": "test-secret"}
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.pr_provider.as_deref(), Some("github"));
    let status = created
        .pr_provider_status
        .as_ref()
        .expect("provider status");
    assert!(status.has_token);
    assert_eq!(status.polling_interval_seconds, 300);
    assert!(!serde_json::to_string(&created)
        .expect("RepoResponse serializes")
        .contains("test-secret"));

    let updated: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", created.id),
        &token,
        json!({
            "work_mode": "pull_request",
            "pr_provider": "github",
            "pr_provider_config": {"polling_interval_seconds": 90}
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated.pr_provider.as_deref(), Some("github"));
    let status = updated.pr_provider_status.expect("updated provider status");
    assert!(
        status.has_token,
        "omitting token preserves the saved credential"
    );
    assert_eq!(status.polling_interval_seconds, 90);

    let listed: api_types::PaginatedResponse<RepoResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/repos", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(listed.items[0].pr_provider.as_deref(), Some("github"));

    let cleared: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/repos/{}", created.id),
        &token,
        json!({"work_mode": "direct_merge", "pr_provider": null, "pr_provider_config": null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(cleared.work_mode, api_types::WorkMode::DirectMerge);
    assert!(cleared.pr_provider.is_none());
    assert!(cleared.pr_provider_status.is_none());
}

#[tokio::test]
async fn issue_integration_does_not_expose_secrets_or_legacy_task_defaults() {
    let workspace = common::TestDir::new("pr12-integration-contract");
    let harness = common::test_app(workspace.path(), "pr12-integration-contract").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Integration Project"}),
        StatusCode::OK,
    )
    .await;
    let created: IntegrationResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/integration", project.id),
        &token,
        json!({
            "platform": "github",
            "base_url": "https://api.github.com",
            "owner": "acme",
            "repo": "product",
            "credential_env_var": "GITHUB_TOKEN",
            "default_implementer": {"kind": "agent", "id": "agent-config"},
        }),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(created.credential_env_var, "GITHUB_TOKEN");
    assert_eq!(
        created.default_implementer,
        Some(ActorRef::Agent("agent-config".to_owned()))
    );
    let response_json = serde_json::to_value(&created).expect("integration response serializes");
    let response_text = response_json.to_string();
    assert!(!response_text.contains("integration-secret"));
    assert!(response_json.get("token").is_none());
    assert!(response_json.get("token_secret_ref").is_none());
    assert!(response_json.get("default_task_state").is_none());
    assert!(response_json.get("default_assignee_id").is_none());

    let patched: IntegrationResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}/integration", project.id),
        &token,
        json!({"default_implementer": {"kind": "human", "id": "human-one"}}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        patched.default_implementer,
        Some(ActorRef::Human("human-one".to_owned()))
    );

    let cleared: IntegrationResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}/integration", project.id),
        &token,
        json!({"default_implementer": null}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(cleared.default_implementer, None);

    let legacy_response = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/integration", project.id),
        json!({
            "platform": "github",
            "base_url": "https://api.github.com",
            "owner": "acme",
            "repo": "legacy",
            "token_secret_ref": "GITHUB_TOKEN",
            "credential_env_var": "GITHUB_TOKEN",
            "default_task_state": "in_progress",
            "default_assignee_type": "agent",
            "default_assignee_id": "agent-legacy",
        }),
    )
    .await;
    assert_eq!(legacy_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let integration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_integration")
        .fetch_one(harness.state.db.pool())
        .await
        .expect("integration count");
    assert_eq!(integration_count, 1, "legacy input creates no integration");

    let read: Option<IntegrationResponse> = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}/integration", project.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        read.expect("integration row remains readable").id,
        created.id
    );
}

#[tokio::test]
async fn retired_rest_mutations_are_unregistered_and_have_no_effect() {
    let workspace = common::TestDir::new("pr12-retired-routes");
    let harness = common::test_app(workspace.path(), "pr12-retired-routes").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Public Surface Project"}),
        StatusCode::OK,
    )
    .await;

    let cases = [
        (Method::PUT, "/api/v1/account/main-agent".to_owned()),
        (
            Method::POST,
            "/api/v1/account/main-agent/product-genesis".to_owned(),
        ),
        (Method::POST, "/api/v1/agent-chats/chat/messages".to_owned()),
        (
            Method::PUT,
            format!("/api/v1/projects/{}/project-agent", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/charter/revisions", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/execution-baseline", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/documents", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/decisions/candidates", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/milestones", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/milestones/primary", project.id),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/agent-handoffs", project.id),
        ),
        (Method::POST, "/api/v1/agents/agent/actions".to_owned()),
        (Method::POST, "/api/v1/agents/agent/commitments".to_owned()),
        (Method::POST, "/api/v1/agents/agent/questions".to_owned()),
        (Method::POST, "/api/v1/memory/backfill".to_owned()),
        (
            Method::POST,
            "/api/v1/mission-control/attention/attention-id/resolve".to_owned(),
        ),
        (Method::POST, "/api/v1/tasks/task-id/recover".to_owned()),
        (Method::POST, "/api/v1/tasks/task-id/transition".to_owned()),
        (
            Method::POST,
            "/api/v1/executions/execution-id/re-execute".to_owned(),
        ),
    ];

    for (method, path) in cases {
        let response = common::json_request_with_bearer::<serde_json::Value>(
            &harness.app,
            method,
            &path,
            &token,
            json!({}),
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(response["code"], "not_found", "{path}");
    }

    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project.id)
        .fetch_one(harness.state.db.pool())
        .await
        .expect("Task count");
    assert_eq!(task_count, 0, "unregistered routes do not create Tasks");
}

#[tokio::test]
async fn lifecycle_merge_readiness_requires_an_exact_gate_evaluation() {
    let workspace = common::TestDir::new("pr12-lifecycle-gate-edge");
    let harness = common::test_app(workspace.path(), "pr12-lifecycle-gate-edge").await;
    let token = common::test_jwt();
    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name": "Lifecycle Project"}),
        StatusCode::OK,
    )
    .await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &token,
        json!({"title": "Gated Task"}),
        StatusCode::OK,
    )
    .await;

    let error: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &token,
        json!({
            "to_state": "ready_to_merge",
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": "pr12-missing-gate-evaluation",
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(error.code, "task_lifecycle.gate_evaluation_required");

    let current: TaskResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        &token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(current.lifecycle.state, TaskLifecycleState::Backlog);
}

#[tokio::test]
async fn lifecycle_mutations_return_exact_transition_identity_and_history() {
    let workspace = common::TestDir::new("pr12-lifecycle-transition");
    let harness = common::test_app(workspace.path(), "pr12-lifecycle-transition").await;
    let token = common::test_jwt();
    let repo_path = common::setup_git_repo(workspace.path());
    let (project_id, _) =
        common::create_project_and_repo(&harness.app, "Lifecycle Project", &repo_path).await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &token,
        json!({"title": "Explicit lifecycle transition"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(task.lifecycle.state, TaskLifecycleState::Ready);

    let incomplete_reason: ErrorResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &token,
        json!({
            "to_state": "active",
            "expected_lifecycle_version": task.lifecycle.version,
            "idempotency_key": "pr12-unpaired-lifecycle-reason",
            "reason_kind": "user_request",
        }),
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(incomplete_reason.code, "domain_error");
    let unchanged: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", task.id),
        &token,
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(unchanged.lifecycle.state, TaskLifecycleState::Ready);
    assert_eq!(unchanged.lifecycle.version, task.lifecycle.version);

    let transition_request = json!({
        "to_state": "active",
        "expected_lifecycle_version": task.lifecycle.version,
        "idempotency_key": "pr12-exact-active-transition",
        "reason_kind": "user_request",
        "reason_ref": "explicit-start",
    });
    let active: TaskLifecycleTransitionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &token,
        transition_request.clone(),
        StatusCode::OK,
    )
    .await;
    assert_eq!(active.lifecycle.state, TaskLifecycleState::Active);
    let transition_id = active
        .transition_id
        .as_deref()
        .expect("exact transition identity is returned");

    let replay: TaskLifecycleTransitionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &token,
        transition_request,
        StatusCode::OK,
    )
    .await;
    assert!(replay.replayed);
    assert_eq!(replay.transition_id.as_deref(), Some(transition_id));

    let transitions: Vec<api_types::TaskLifecycleTransitionFactResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/lifecycle/transitions", task.id),
            &token,
            StatusCode::OK,
        )
        .await;
    let fact = transitions
        .iter()
        .find(|fact| fact.id == transition_id)
        .expect("history contains the exact transition identity");
    assert_eq!(fact.from_state, TaskLifecycleState::Ready);
    assert_eq!(fact.to_state, TaskLifecycleState::Active);
    assert_eq!(fact.reason_kind.as_deref(), Some("user_request"));
    assert_eq!(fact.reason_ref.as_deref(), Some("explicit-start"));
    assert!(!fact.domain_event_id.is_empty());

    let cancelled: TaskLifecycleTransitionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        &token,
        json!({
            "to_state": "cancelled",
            "expected_lifecycle_version": active.lifecycle.version,
            "idempotency_key": "pr12-explicit-cancel",
            "reason_kind": "user_request",
            "reason_ref": "cancel-requested",
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(cancelled.lifecycle.state, TaskLifecycleState::Cancelled);
    assert!(cancelled.transition_id.is_some());
}

#[tokio::test]
async fn project_reads_and_mutations_require_project_visibility() {
    let workspace = common::TestDir::new("pr12-project-authorization");
    let harness = common::test_app(workspace.path(), "pr12-project-authorization").await;
    let token_a = common::test_jwt();
    let auth_b: AuthResponse = common::json_request(
        &harness.app,
        Method::POST,
        "/api/v1/auth/register",
        json!({"email":"pr12-user-b@example.com","password":"password-for-user-b"}),
        StatusCode::CREATED,
    )
    .await;
    let project_a: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token_a,
        json!({"name":"Private Project A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &auth_b.access_token,
        json!({"name":"Private Project B"}),
        StatusCode::OK,
    )
    .await;

    let visible_to_a: api_types::PaginatedResponse<ProjectResponse> =
        common::empty_request_with_bearer(
            &harness.app,
            Method::GET,
            "/api/v1/projects?limit=100&include_total=true",
            &token_a,
            StatusCode::OK,
        )
        .await;
    assert!(visible_to_a
        .items
        .iter()
        .any(|item| item.id == project_a.id));
    assert!(!visible_to_a
        .items
        .iter()
        .any(|item| item.id == project_b.id));
    assert_eq!(visible_to_a.total_count, Some(1));

    let not_visible: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project_a.id),
        &auth_b.access_token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(not_visible["code"], "not_found");

    for (method, path, body) in [
        (
            Method::PATCH,
            format!("/api/v1/projects/{}", project_a.id),
            json!({"version":project_a.version,"name":"Stolen name"}),
        ),
        (
            Method::PATCH,
            format!("/api/v1/projects/{}", project_a.id),
            json!({"version":project_a.version,"project_hooks":[]}),
        ),
    ] {
        let error: serde_json::Value = common::json_request_with_bearer(
            &harness.app,
            method,
            &path,
            &auth_b.access_token,
            body,
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(error["code"], "not_found", "{path}");
    }
    for suffix in ["pause", "resume"] {
        let error: serde_json::Value = common::json_request_with_bearer(
            &harness.app,
            Method::POST,
            &format!("/api/v1/projects/{}/{suffix}", project_a.id),
            &auth_b.access_token,
            json!({}),
            StatusCode::NOT_FOUND,
        )
        .await;
        assert_eq!(error["code"], "not_found");
    }
    assert_eq!(
        project_delete_status(&harness.app, &project_a.id, &auth_b.access_token).await,
        StatusCode::NOT_FOUND,
        "a non-member cannot delete another user's Project"
    );

    let unchanged: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert_eq!(unchanged.name, "Private Project A");
    assert!(!unchanged.paused);
    assert_eq!(unchanged.version, project_a.version);

    let updated: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}", project_a.id),
        &token_a,
        json!({"version":project_a.version,"name":"Updated Project A","project_hooks":[]}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(updated.name, "Updated Project A");
    assert!(updated.project_hooks.is_empty());

    let paused: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/pause", project_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert!(paused.paused);
    let resumed: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/resume", project_a.id),
        &token_a,
        StatusCode::OK,
    )
    .await;
    assert!(!resumed.paused);
    assert_eq!(
        project_delete_status(&harness.app, &project_a.id, &token_a).await,
        StatusCode::NO_CONTENT,
        "the owner can delete their Project"
    );
}

#[tokio::test]
async fn project_visibility_is_separate_from_project_and_task_write_authority() {
    let workspace = common::TestDir::new("pr12-project-write-authority");
    let harness = common::test_app(workspace.path(), "pr12-project-write-authority").await;
    let owner_token = common::test_jwt_for_user("owner-a", "owner-a@example.com", false);
    let admin_token = common::test_jwt_for_user("admin-a", "admin-a@example.com", false);
    let member_token = common::test_jwt_for_user("member-a", "member-a@example.com", false);
    let viewer_token = common::test_jwt_for_user("viewer-a", "viewer-a@example.com", false);
    let outsider_token = common::test_jwt_for_user("user-b", "user-b@example.com", false);

    for (user_id, email) in [
        ("owner-a", "owner-a@example.com"),
        ("admin-a", "admin-a@example.com"),
        ("member-a", "member-a@example.com"),
        ("viewer-a", "viewer-a@example.com"),
        ("user-b", "user-b@example.com"),
    ] {
        let now = db::now_rfc3339();
        db::UserRepo::create_user(
            &*harness.state.db,
            &db::User {
                id: user_id.to_owned(),
                email: email.to_owned(),
                password_hash: "$2b$04$placeholder".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("Project participant user creates");
    }

    let project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &owner_token,
        json!({"name":"Project A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &outsider_token,
        json!({"name":"Project B"}),
        StatusCode::OK,
    )
    .await;
    assert_ne!(project.id, project_b.id);
    for (user_id, role) in [
        ("admin-a", "admin"),
        ("member-a", "member"),
        ("viewer-a", "viewer"),
    ] {
        let now = db::now_rfc3339();
        db::ProjectMemberRepo::add_member(
            &*harness.state.db,
            db::CreateProjectMember {
                id: db::new_uuid_v4(),
                project_id: project.id.clone(),
                user_id: user_id.to_owned(),
                role: role.to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("Project role membership creates");
    }

    let visible_project: ProjectResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &viewer_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(visible_project.id, project.id);
    let invisible_project: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &outsider_token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(invisible_project["code"], "not_found");
    let invisible_project_b: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project_b.id),
        &owner_token,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert_eq!(invisible_project_b["code"], "not_found");

    let owner_task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &owner_token,
        json!({"title":"Owner task"}),
        StatusCode::OK,
    )
    .await;
    let _: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/task-roles", owner_task.id),
        &owner_token,
        json!({"role":"implementer","coordination_mode":"independent"}),
        StatusCode::OK,
    )
    .await;
    let _foreign_actor: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!(
            "/api/v1/tasks/{}/task-roles/implementer/members",
            owner_task.id
        ),
        &owner_token,
        json!({"actor_ref":{"kind":"human","id":"user-b"}}),
        StatusCode::BAD_REQUEST,
    )
    .await;
    let member_task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &member_token,
        json!({"title":"Member task"}),
        StatusCode::OK,
    )
    .await;
    let visible_task: TaskResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}", owner_task.id),
        &viewer_token,
        StatusCode::OK,
    )
    .await;
    assert_eq!(visible_task.id, owner_task.id);

    let _viewer_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project.id),
        &viewer_token,
        json!({"title":"Viewer task"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    let _viewer_update_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}", owner_task.id),
        &viewer_token,
        json!({"version":owner_task.version,"title":"Viewer write"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    let member_update: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}", member_task.id),
        &member_token,
        json!({"version":member_task.version,"title":"Member update"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(member_update.title, "Member update");

    let _member_admin_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}", project.id),
        &member_token,
        json!({"version":project.version,"name":"Member project write"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    let _viewer_admin_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}", project.id),
        &viewer_token,
        json!({"version":project.version,"name":"Viewer project write"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    let admin_project: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/projects/{}", project.id),
        &admin_token,
        json!({"version":project.version,"name":"Admin project update"}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(admin_project.name, "Admin project update");

    let _member_repo_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &member_token,
        json!({"remote_url":"https://example.com/member.git"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    let repo: RepoResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &admin_token,
        json!({"remote_url":"https://example.com/admin.git"}),
        StatusCode::OK,
    )
    .await;
    let _viewer_repo_read: RepoResponse = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/repos/{}", repo.id),
        &viewer_token,
        StatusCode::OK,
    )
    .await;

    let integration_body = json!({
        "platform":"github",
        "base_url":"https://api.github.com",
        "owner":"owner-a",
        "repo":"repo-a",
        "credential_env_var":"GITHUB_TOKEN"
    });
    let _member_integration_denied: serde_json::Value = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/integration", project.id),
        &member_token,
        integration_body.clone(),
        StatusCode::FORBIDDEN,
    )
    .await;
    common::json_request_with_bearer::<IntegrationResponse>(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/integration", project.id),
        &admin_token,
        integration_body,
        StatusCode::CREATED,
    )
    .await;

    let now = db::now_rfc3339();
    let ownerless = db::ProjectRepo::create(
        &*harness.state.db,
        db::CreateProject {
            id: db::new_uuid_v4(),
            name: "Public system Project".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("ownerless Project creates");
    common::empty_request_with_bearer::<ProjectResponse>(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", ownerless.id),
        &outsider_token,
        StatusCode::OK,
    )
    .await;
    for (method, path, body) in [
        (
            Method::PATCH,
            format!("/api/v1/projects/{}", ownerless.id),
            json!({"version":ownerless.version,"name":"World writable"}),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/pause", ownerless.id),
            json!({}),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/repos", ownerless.id),
            json!({"remote_url":"https://example.com/ownerless.git"}),
        ),
        (
            Method::POST,
            format!("/api/v1/projects/{}/tasks", ownerless.id),
            json!({"title":"World writable task"}),
        ),
    ] {
        let response = common::json_request_with_bearer::<serde_json::Value>(
            &harness.app,
            method,
            &path,
            &outsider_token,
            body,
            StatusCode::FORBIDDEN,
        )
        .await;
        let expected_code = if path.ends_with("/tasks") {
            "project_write_forbidden"
        } else {
            "project_admin_required"
        };
        assert_eq!(response["code"], expected_code, "{path}");
    }
    assert_eq!(
        project_delete_status(&harness.app, &ownerless.id, &outsider_token).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        project_delete_status(&harness.app, &project.id, &viewer_token).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn task_parent_links_stay_inside_the_same_project_on_create_and_reparent() {
    let workspace = common::TestDir::new("pr12-task-parent-project-boundary");
    let harness = common::test_app(workspace.path(), "pr12-task-parent-project-boundary").await;
    let token = common::test_jwt();
    let project_a: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name":"Project A"}),
        StatusCode::OK,
    )
    .await;
    let project_b: ProjectResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        &token,
        json!({"name":"Project B"}),
        StatusCode::OK,
    )
    .await;
    let parent_a: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_a.id),
        &token,
        json!({"title":"Parent A"}),
        StatusCode::OK,
    )
    .await;
    let parent_b: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_b.id),
        &token,
        json!({"title":"Parent B"}),
        StatusCode::OK,
    )
    .await;
    let childless_root: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_a.id),
        &token,
        json!({"title":"Childless root"}),
        StatusCode::OK,
    )
    .await;
    let reparent_target: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_a.id),
        &token,
        json!({"title":"Reparent target"}),
        StatusCode::OK,
    )
    .await;

    let child_a: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{}/tasks", project_a.id),
        &token,
        json!({"title":"Child A","parent_task_id":parent_a.id}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(child_a.project_id, project_a.id);
    assert_eq!(
        child_a.parent_task_id.as_deref(),
        Some(parent_a.id.as_str())
    );
    let attached: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::PATCH,
        &format!("/api/v1/tasks/{}", childless_root.id),
        &token,
        json!({
            "version":childless_root.version,
            "parent_task_id":parent_a.id
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        attached.parent_task_id.as_deref(),
        Some(parent_a.id.as_str())
    );
    assert_eq!(attached.subtask_order, Some(1));

    let before_failed_creates: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
            .bind(&project_a.id)
            .fetch_one(harness.state.db.pool())
            .await
            .expect("Task count before failed creates");
    for parent_task_id in [parent_b.id.as_str(), "missing-parent"] {
        let _response = common::json_request_with_bearer::<serde_json::Value>(
            &harness.app,
            Method::POST,
            &format!("/api/v1/projects/{}/tasks", project_a.id),
            &token,
            json!({"title":"Rejected child","parent_task_id":parent_task_id}),
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    let nested_response = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/projects/{}/tasks", project_a.id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({"title":"Nested child","parent_task_id":child_a.id}).to_string(),
                ))
                .expect("nested Task request builds"),
        )
        .await
        .expect("nested Task response");
    assert!(
        matches!(
            nested_response.status(),
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT
        ),
        "nested parent is rejected with current API validation status"
    );
    let after_failed_creates: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
            .bind(&project_a.id)
            .fetch_one(harness.state.db.pool())
            .await
            .expect("Task count after failed creates");
    assert_eq!(after_failed_creates, before_failed_creates);

    let before_rejected_move = db::TaskRepo::get_by_id(&*harness.state.db, &parent_a.id, false)
        .await
        .expect("parent A loads before rejected move")
        .expect("parent A exists");
    let rejected_move = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/v1/tasks/{}", parent_a.id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({
                        "version":parent_a.version,
                        "parent_task_id":reparent_target.id
                    })
                    .to_string(),
                ))
                .expect("nested move request builds"),
        )
        .await
        .expect("nested move response");
    assert!(
        matches!(
            rejected_move.status(),
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT
        ),
        "a Task with children cannot become a subtask"
    );
    let after_rejected_move = db::TaskRepo::get_by_id(&*harness.state.db, &parent_a.id, false)
        .await
        .expect("parent A loads after rejected move")
        .expect("parent A remains");
    assert_eq!(
        after_rejected_move.parent_task_id,
        before_rejected_move.parent_task_id
    );
    assert_eq!(
        after_rejected_move.subtask_order,
        before_rejected_move.subtask_order
    );
    assert_eq!(after_rejected_move.version, before_rejected_move.version);
    let self_parent = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/v1/tasks/{}", parent_a.id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({"version":parent_a.version,"parent_task_id":parent_a.id}).to_string(),
                ))
                .expect("self-parent request builds"),
        )
        .await
        .expect("self-parent response");
    assert!(
        matches!(
            self_parent.status(),
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT
        ),
        "self-parent is rejected"
    );

    let cycle = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/v1/tasks/{}", parent_a.id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({"version":parent_a.version,"parent_task_id":child_a.id}).to_string(),
                ))
                .expect("cycle request builds"),
        )
        .await
        .expect("cycle response");
    assert!(
        matches!(
            cycle.status(),
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT
        ),
        "reparenting a root below its child is rejected as a cycle"
    );
}

#[tokio::test]
async fn project_deleted_hint_targets_only_pre_delete_recipients() {
    let workspace = common::TestDir::new("pr12-project-deleted-recipients");
    let harness = common::test_app(workspace.path(), "pr12-project-deleted-recipients").await;
    let owner_token = common::test_jwt();
    let member_token = common::test_jwt_for_user("member-b", "member-b@example.com", false);
    let outsider_token = common::test_jwt_for_user("outsider-c", "outsider-c@example.com", false);
    for (user_id, email) in [
        ("member-b", "member-b@example.com"),
        ("outsider-c", "outsider-c@example.com"),
    ] {
        let now = db::now_rfc3339();
        db::UserRepo::create_user(
            &*harness.state.db,
            &db::User {
                id: user_id.to_owned(),
                email: email.to_owned(),
                password_hash: "$2b$04$placeholder".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("Project recipient user creates");
    }
    let now = db::now_rfc3339();
    let project = db::ProjectRepo::create(
        &*harness.state.db,
        db::CreateProject {
            id: db::new_uuid_v4(),
            name: "Deleted Project".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some("test-user-id".to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("private Project creates");
    db::ProjectMemberRepo::add_member(
        &*harness.state.db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: project.id.clone(),
            user_id: "member-b".to_owned(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("member B joins Project");
    let mut events = harness.state.event_bus.subscribe();

    let response = harness
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/v1/projects/{}", project.id))
                .header(header::AUTHORIZATION, format!("Bearer {owner_token}"))
                .body(Body::empty())
                .expect("Project delete request builds"),
        )
        .await
        .expect("Project delete response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(db::ProjectRepo::get_by_id(&*harness.state.db, &project.id)
        .await
        .expect("deleted Project lookup")
        .is_none());

    let mut recipients = std::collections::BTreeSet::new();
    while let Ok(event) = events.try_recv() {
        if event.event_type == "project.deleted" && event.entity_id == project.id {
            if let events::EventContext::ProjectDeleted {
                recipient_user_id, ..
            } = event.context
            {
                recipients.insert(recipient_user_id);
            }
        }
    }
    assert_eq!(
        recipients,
        ["member-b".to_owned(), "test-user-id".to_owned()].into()
    );
    assert!(!recipients.contains("outsider-c"));

    // Deleted Projects no longer grant owner/member visibility, and their
    // deletion notifications are invalidation-only, not membership data.
    let _: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &member_token,
        StatusCode::NOT_FOUND,
    )
    .await;
    let _: serde_json::Value = common::empty_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{}", project.id),
        &outsider_token,
        StatusCode::NOT_FOUND,
    )
    .await;
}

async fn project_delete_status(app: &Router, project_id: &str, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/api/v1/projects/{project_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build Project delete request"),
        )
        .await
        .expect("Project delete response")
        .status()
}

#[tokio::test]
async fn execution_start_requires_an_exact_active_agent_membership_and_purpose() {
    let workspace = common::TestDir::new("pr12-explicit-execution");
    let harness = common::test_app(workspace.path(), "pr12-explicit-execution").await;
    let token = common::test_jwt();
    let repo_path = common::setup_git_repo(workspace.path());
    let (project_id, _repo_id) =
        common::create_project_and_repo(&harness.app, "Execution Project", &repo_path).await;
    let task: TaskResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &token,
        json!({"title": "Execution Task"}),
        StatusCode::OK,
    )
    .await;
    let (agent_id, _) =
        common::create_shell_agents(&harness.app, workspace.path(), "pr12-explicit-execution")
            .await;
    harness
        .state
        .task_service
        .create_task_role(
            &task.id,
            "implementer",
            db::CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("TaskRole is created");
    harness
        .state
        .task_service
        .add_task_role_member(
            &task.id,
            "implementer",
            api_types::ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("Agent membership is created");

    let work_unit: WorkUnitResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        &token,
        json!({
            "title": "Exact executor WorkUnit",
            "scope": "Implement the requested change within this repository scope",
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
        StatusCode::CREATED,
    )
    .await;
    let allocated_work_unit: WorkUnitResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/allocation", work_unit.id),
        &token,
        json!({
            "expected_version": work_unit.version,
            "role": "implementer",
            "assigned_actor": { "kind": "agent", "id": agent_id }
        }),
        StatusCode::OK,
    )
    .await;

    let execution: ExecutionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        &token,
        json!({
            "work_unit_id": allocated_work_unit.id,
            "expected_work_unit_version": allocated_work_unit.version,
            "purpose": "implement",
            "prompt": "Implement the exact requested change.",
            "input_artifact_ids": []
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(execution.task_id, task.id);
    assert_eq!(execution.role, "implementer");
    assert_eq!(
        execution.purpose,
        Some(api_types::ExecutionPurpose::Implement)
    );
    assert_eq!(
        execution.actor_ref,
        Some(api_types::ActorRef::Agent(agent_id.clone()))
    );
    let stored = db::ExecutionRepo::get_by_id(&*harness.state.db, &execution.id)
        .await
        .expect("Execution query succeeds")
        .expect("Execution is stored");
    assert_eq!(stored.harness_session_id, None);
    assert_eq!(stored.parent_execution_id, None);
}
