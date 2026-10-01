#![allow(dead_code, clippy::assertions_on_constants)]
use std::{path::Path, sync::Arc, time::Duration};

use api::{build_router, AppState};
use api_types::{
    AgentResponse, DaemonRegisterResponse, DaemonResponse, ProjectResponse, RepoResponse,
    TaskResponse, TransitionTaskResponse,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use events::{EventBus, EventContext, ForgeEvent};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower::ServiceExt;

use db::{now_rfc3339, CreateExecution, ExecutionRepo, ExecutionStatus};

#[tokio::test]
async fn entering_planning_dispatches_assigned_planner_role() {
    let harness = test_app().await;
    let (project_id, _repo_id) = create_project_and_repo(&harness.app).await;
    let _: Value = json_request_with_bearer(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/projects/{project_id}/workflow"),
        &admin_jwt(),
        json!({ "definition": planning_workflow() }),
        StatusCode::OK,
    )
    .await;

    let task: TaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &admin_jwt(),
        json!({ "title": "Needs planning", "description": "split this up" }),
        StatusCode::OK,
    )
    .await;
    let workspace_root = TestDir::new("forge-planning-workspaces");
    let planner_agent_id = create_shell_agent(&harness.app, workspace_root.path()).await;
    let _: Value = json_request_with_bearer(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/tasks/{}/roles/planner", task.id),
        &admin_jwt(),
        json!({ "assignee_type": "agent", "assignee_id": planner_agent_id }),
        StatusCode::OK,
    )
    .await;

    let mut rx = harness.event_bus.subscribe();
    let moved: TransitionTaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/transition", task.id),
        &admin_jwt(),
        json!({ "status": "planning", "version": task.version, "reason": "plan first" }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(moved.task.status, "planning");

    let events = drain_events(&mut rx).await;
    assert!(
        events.iter().any(|event| matches!(
            &event.context,
            EventContext::TaskRoleAgentDispatched { task_id, role, agent_id, state, .. }
                if task_id == &task.id
                && role == "planner"
                && agent_id == &planner_agent_id
                && state == "planning"
        )),
        "missing planner dispatch event; got {events:?}"
    );
}

#[tokio::test]
async fn planning_gate_approval_conflicts_while_planner_execution_is_running() {
    let harness = test_app().await;
    let (project_id, _repo_id) = create_project_and_repo(&harness.app).await;
    let _: Value = json_request_with_bearer(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/projects/{project_id}/workflow"),
        &admin_jwt(),
        json!({ "definition": planning_workflow() }),
        StatusCode::OK,
    )
    .await;

    let task: TaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &admin_jwt(),
        json!({ "title": "Needs planning", "description": "split this up" }),
        StatusCode::OK,
    )
    .await;
    let moved: TransitionTaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/transition", task.id),
        &admin_jwt(),
        json!({ "status": "planning", "version": task.version, "reason": "plan first" }),
        StatusCode::OK,
    )
    .await;
    let now = now_rfc3339();
    ExecutionRepo::create(
        &*harness._state.db,
        CreateExecution {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task.id.clone(),
            agent_id: None,
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "planner".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("still planning".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("running planner execution creates");

    let _: Value = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates/planning/approve", task.id),
        &admin_jwt(),
        json!({ "version": moved.task.version, "reason": null }),
        StatusCode::CONFLICT,
    )
    .await;
}

#[tokio::test]
async fn human_approval_keeps_the_exact_generic_plan_artifact() {
    let harness = test_app().await;
    let (project_id, _repo_id) = create_project_and_repo(&harness.app).await;
    let _: Value = json_request_with_bearer(
        &harness.app,
        Method::PUT,
        &format!("/api/v1/projects/{project_id}/workflow"),
        &admin_jwt(),
        json!({ "definition": plan_approval_workflow() }),
        StatusCode::OK,
    )
    .await;

    let task: TaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        &admin_jwt(),
        json!({ "title": "Review a plan", "description": "keep its authorship" }),
        StatusCode::OK,
    )
    .await;
    let moved: TransitionTaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/transition", task.id),
        &admin_jwt(),
        json!({ "status": "planning", "version": task.version, "reason": "plan first" }),
        StatusCode::OK,
    )
    .await;

    harness
        ._state
        .task_service
        .create_task_role(
            &task.id,
            "planner",
            db::CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("planner role creates");
    harness
        ._state
        .task_service
        .add_task_role_member(
            &task.id,
            "planner",
            api_types::ActorRef::Human("test-user-id".to_owned()),
        )
        .await
        .expect("Human planner membership creates");

    let collaboration = services::CollaborationService::new(
        Arc::clone(&harness._state.db),
        Arc::clone(&harness.event_bus),
    );
    let human_execution = collaboration
        .start_human_plan_execution(&task.id, "test-user-id")
        .await
        .expect("Human Plan Execution starts");
    let artifact = collaboration
        .complete_human_plan_execution(&human_execution.id, "test-user-id", "- [ ] review only\n")
        .await
        .expect("Human Plan Artifact is produced");
    assert_eq!(artifact.producer_execution_id, human_execution.id);
    assert_eq!(
        artifact.producer,
        db::ActorRef::Human("test-user-id".to_owned())
    );

    // A stale physical value is retained for PR13 cleanup, but it is not a
    // source for the API projection or gate behavior.
    sqlx::query("UPDATE task SET plan = ? WHERE id = ?")
        .bind("legacy stale plan")
        .bind(&task.id)
        .execute(harness._state.db.pool())
        .await
        .expect("legacy Task.plan fixture is written");
    let before_approval: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = ? AND kind = 'plan'")
            .bind(&task.id)
            .fetch_one(harness._state.db.pool())
            .await
            .expect("Plan Artifact count loads before approval");

    let approved: TaskResponse = json_request_with_bearer(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates/planning/approve", task.id),
        &admin_jwt(),
        json!({ "version": moved.task.version, "reason": "continue" }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved.status, "in_progress");
    assert_eq!(
        approved
            .plan_artifact
            .as_ref()
            .map(|value| value.artifact_id.as_str()),
        Some(artifact.id.as_str())
    );

    let history: Value = json_request_with_bearer(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/plan", task.id),
        &admin_jwt(),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(history["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(history["artifacts"][0]["artifact_id"], artifact.id);
    assert_eq!(
        history["artifacts"][0]["producer_execution_id"],
        human_execution.id
    );

    let after_approval: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = ? AND kind = 'plan'")
            .bind(&task.id)
            .fetch_one(harness._state.db.pool())
            .await
            .expect("Plan Artifact count loads after approval");
    let legacy_approvals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_plan_approval WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(harness._state.db.pool())
            .await
            .expect("legacy approval count loads");
    assert_eq!(after_approval, before_approval);
    assert_eq!(legacy_approvals, 0);
}

fn planning_workflow() -> Value {
    json!({
        "roles": [{ "name": "planner", "display_name": "Planner", "description": "Plans" }],
        "states": [
            state("todo", "initial", Value::Null, json!({}), trigger("planning")),
            state("planning", "gate", "planner", json!({
                "on_enter": [{ "action": "dispatch_role_agent", "params": {}, "applies_to": "all", "on_failure": "log" }]
            }), trigger("in_progress")),
            state("in_progress", "active", Value::Null, json!({}), trigger("done")),
            state("done", "terminal", Value::Null, json!({}), json!({}))
        ],
        "cancellation_state": null
    })
}

fn plan_approval_workflow() -> Value {
    json!({
        "roles": [{ "name": "planner", "display_name": "Planner", "description": "Plans" }],
        "states": [
            state("todo", "initial", Value::Null, json!({}), trigger("planning")),
            state("planning", "gate", "planner", json!({}), trigger("in_progress")),
            state("in_progress", "active", Value::Null, json!({}), trigger("done")),
            state("done", "terminal", Value::Null, json!({}), json!({}))
        ],
        "cancellation_state": null
    })
}

fn state(name: &str, kind: &str, role: impl Into<Value>, hooks: Value, triggers: Value) -> Value {
    let canonical_phase = match kind {
        "backlog" => "backlog",
        "initial" => "ready",
        "gate" => "review",
        "terminal" => "done",
        _ => "working",
    };
    json!({ "name": name, "kind": kind, "canonical_phase": canonical_phase, "column": name, "display_name": name, "role": role.into(), "hooks": hooks, "gate_config": null, "triggers": triggers, "config": {} })
}

fn trigger(to: &str) -> Value {
    json!({ "accept": { "to": to, "dispatch": null } })
}

async fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<ForgeEvent>) -> Vec<ForgeEvent> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(event)) => events.push(event),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }
    events
}

struct Harness {
    app: Router,
    event_bus: Arc<EventBus>,
    _state: Arc<AppState>,
    _web_dist_dir: TestDir,
}

async fn test_app() -> Harness {
    let pool = db::create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    db::run_migrations(&pool).await.expect("migrations run");
    let db = Arc::new(db::SqliteDb::new(pool));
    let now = db::now_rfc3339();
    db::UserRepo::create_user(
        &*db,
        &db::User {
            id: "test-user-id".to_owned(),
            email: "test@example.com".to_owned(),
            password_hash: "$2b$04$placeholder".to_owned(),
            display_name: None,
            is_admin: true,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("seed Human planning user");
    let adapter_registry = Arc::new(cli_adapters::default_registry());
    services::ensure_default_agents(db.as_ref(), &adapter_registry)
        .await
        .expect("default agents upsert");
    let event_bus = Arc::new(EventBus::new(64));
    let state = Arc::new(AppState::with_adapter_registry(
        db,
        Arc::clone(&event_bus),
        true,
        adapter_registry,
    ));
    let web_dist_dir = TestDir::new("forge-planning-gate-web");
    std::fs::write(web_dist_dir.path().join("index.html"), "<html></html>").expect("write index");
    let app = build_router((*state).clone(), web_dist_dir.path().to_path_buf());
    Harness {
        app,
        event_bus,
        _state: state,
        _web_dist_dir: web_dist_dir,
    }
}

async fn create_project_and_repo(app: &Router) -> (String, String) {
    let project: ProjectResponse = json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/projects",
        &admin_jwt(),
        json!({ "name": "Planning Gate" }),
        StatusCode::OK,
    )
    .await;
    let repo: RepoResponse = json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/projects/{}/repos", project.id),
        &admin_jwt(),
        json!({ "name": "repo", "remote_url": "https://example.com/repo.git", "default_branch": "main" }),
        StatusCode::OK,
    )
    .await;
    (project.id, repo.id)
}

async fn create_shell_agent(app: &Router, workspace_root: &Path) -> String {
    let registration: DaemonRegisterResponse = json_request(
        app,
        Method::POST,
        "/api/v1/daemons/register",
        json!({
            "machine_id": services::embedded_daemon::embedded_machine_id(),
            "hostname": "planning-host",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "test"
        }),
        StatusCode::OK,
    )
    .await;
    let daemon_id = registration.daemon_id;
    let _: DaemonResponse = json_request_with_bearer(
        app,
        Method::POST,
        &format!("/api/v1/daemons/{daemon_id}/report"),
        &registration.registration_token,
        json!({
            "detected_clis": [{ "kind": "shell", "availability": "authenticated", "path": "/bin/sh" }],
            "runtimes": [{ "kind": "local", "workspace_root": workspace_root.to_string_lossy(), "status": "ready" }]
        }),
        StatusCode::OK,
    )
    .await;
    let agent: AgentResponse = json_request_with_bearer(
        app,
        Method::POST,
        "/api/v1/agents",
        &admin_jwt(),
        json!({
            "name": "planner-agent",
            "executor_type": "shell",
            "daemon_id": daemon_id,
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(agent.effective_status.as_deref(), Some("active"));
    agent.id
}

fn admin_jwt() -> String {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({
        "sub": "test-user-id",
        "email": "test@example.com",
        "is_admin": true,
        "iat": now,
        "exp": now + 900,
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b"test-jwt-secret-for-development"),
    )
    .expect("encode test jwt")
}

async fn json_request<T>(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .expect("build request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        expected_status,
        "body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON")
}

async fn json_request_with_bearer<T>(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
    expected_status: StatusCode,
) -> T
where
    T: DeserializeOwned,
{
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .expect("build request"),
        )
        .await
        .expect("router response");
    parse_response(response, expected_status).await
}

async fn empty_request<T>(app: &Router, method: Method, uri: &str, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("router response");
    parse_response(response, expected_status).await
}

async fn parse_response<T>(response: axum::response::Response, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    assert_eq!(
        status,
        expected_status,
        "body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON")
}

struct TestDir {
    path: std::path::PathBuf,
}

impl TestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("temp dir creates");
        Self { path }
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
