mod common;

use api_types::{
    ActorRef, ErrorResponse, ExecutionResponse, IntegrationResponse, ProjectResponse, RepoResponse,
    TaskLifecycleState, TaskLifecycleTransitionResponse, TaskResponse,
};
use axum::http::{Method, StatusCode};
use serde_json::json;

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

    for table in [
        "agent_chat",
        "account_main_agent_binding",
        "project_agent_binding",
        "product_genesis_session",
        "project_charter",
        "project_task_governance",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(harness.state.db.pool())
            .await
            .expect("count retired setup rows");
        assert_eq!(count, 0, "normal creation leaves {table} empty");
    }
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
            "executor",
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
            "executor",
            api_types::ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("Agent membership is created");

    let execution: ExecutionResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        &token,
        json!({
            "agent_id": agent_id.clone(),
            "role": "executor",
            "purpose": "implement",
            "prompt": "Implement the exact requested change.",
            "input_artifact_ids": []
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(execution.task_id, task.id);
    assert_eq!(execution.role, "executor");
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
