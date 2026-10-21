#![allow(dead_code)]

mod common;

use std::{path::Path, sync::Arc, time::Duration};

use api::AppState;
use api_types::{
    AgentProfileResponse, AgentResponse, AuthResponse, CliProjectionResponse,
    DaemonRegisterResponse, DaemonResponse, ExecutionResponse, ExecutionStatus, GateResponse,
    PaginatedResponse, ProjectResponse, RepoResponse, TaskLifecycleResponse, TaskLifecycleState,
    TaskResponse, WorkUnitIntegrationOutcome, WorkUnitIntegrationResponse, WorkUnitResponse,
    WorkspaceResponse,
};
use cli_adapters::default_registry;
use events::EventBus;
use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::Instant};

struct RunningServer {
    db: Arc<db::SqliteDb>,
    base_url: String,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    async fn start(state: AppState, web_dist: &Path) -> Self {
        let db = Arc::clone(&state.db);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("temporary loopback listener binds");
        let address = listener.local_addr().expect("listener address loads");
        let base_url = format!("http://{address}");
        let web_dist = web_dist.to_path_buf();
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(api::serve_with_listener(
            listener,
            state,
            web_dist,
            async move {
                let _ = shutdown_rx.await;
            },
        ));
        Self {
            db,
            base_url,
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        tokio::time::timeout(Duration::from_secs(5), self.task)
            .await
            .expect("HTTP server stops within the bound")
            .expect("HTTP server task joins")
            .expect("HTTP server shuts down cleanly");
    }
}

#[tokio::test]
async fn s01_authenticated_http_shell_flow_persists_across_server_restart() {
    let temp = common::TestDir::new("pr15-s01-http");
    let database_path = temp.path().join("forge.sqlite3");
    let database_url = format!("sqlite://{}", database_path.display());
    let workspace_root = temp.path().join("workspace-root");
    std::fs::create_dir_all(&workspace_root).expect("temporary workspace root creates");
    let web_dist = temp.path().join("web-dist");
    std::fs::create_dir_all(&web_dist).expect("temporary web root creates");
    std::fs::write(web_dist.join("index.html"), "<html></html>")
        .expect("temporary app shell writes");

    let state = new_unseeded_state(&database_url, &workspace_root).await;
    let server = RunningServer::start(state, &web_dist).await;
    let base = server.base_url.clone();
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("HTTP client builds");

    let health = client
        .get(format!("{base}/healthz"))
        .send()
        .await
        .expect("real HTTP health request completes");
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(health.text().await.expect("health body reads"), "ok");

    let email = "pr15-s01-owner@example.invalid";
    let password = "S01-local-test-password-123";
    let _: AuthResponse = request_json(
        &client,
        Method::POST,
        &base,
        "/api/v1/auth/register",
        None,
        Some(json!({"email": email, "password": password})),
        StatusCode::CREATED,
    )
    .await;
    let auth: AuthResponse = request_json(
        &client,
        Method::POST,
        &base,
        "/api/v1/auth/login",
        None,
        Some(json!({"email": email, "password": password})),
        StatusCode::OK,
    )
    .await;
    let token = auth.access_token;

    let registration: DaemonRegisterResponse = request_json(
        &client,
        Method::POST,
        &base,
        "/api/v1/daemons/register",
        Some(&token),
        Some(json!({
            "machine_id": services::embedded_daemon::embedded_machine_id(),
            "hostname": "pr15-s01-local",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "pr15-s01"
        })),
        StatusCode::OK,
    )
    .await;
    let daemon_id = registration.daemon_id;
    let _: DaemonResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/daemons/{daemon_id}/report"),
        Some(&registration.registration_token),
        Some(json!({
            "detected_clis": [{"kind": "shell", "availability": "authenticated", "path": "/bin/sh"}],
            "runtimes": [{"kind": "local", "workspace_root": workspace_root, "status": "ready"}]
        })),
        StatusCode::OK,
    )
    .await;
    let clis: CliProjectionResponse = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/clis?daemon_id={daemon_id}"),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(clis.items.iter().any(|item| {
        item.daemon_id == daemon_id && item.kind == "shell" && item.availability == "authenticated"
    }));

    let agent: AgentResponse = request_json(
        &client,
        Method::POST,
        &base,
        "/api/v1/agents",
        Some(&token),
        Some(json!({
            "name": "PR15 isolated shell worker",
            "executor_type": "shell",
            "daemon_id": daemon_id
        })),
        StatusCode::OK,
    )
    .await;
    let profiles: Vec<AgentProfileResponse> = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/agents/{}/profiles", agent.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    let profile = profiles
        .iter()
        .find(|profile| profile.executor_type == "shell")
        .expect("shell Agent profile is returned");
    let _: AgentResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/agents/{}/profiles/{}/select", agent.id, profile.id),
        Some(&token),
        Some(json!({"version": agent.version})),
        StatusCode::OK,
    )
    .await;

    let project: ProjectResponse = request_json(
        &client,
        Method::POST,
        &base,
        "/api/v1/projects",
        Some(&token),
        Some(json!({"name": "PR15 S01 HTTP acceptance"})),
        StatusCode::OK,
    )
    .await;
    let repo_root = temp.path().join("repo-root");
    std::fs::create_dir_all(&repo_root).expect("repo parent creates");
    let repo_path = common::setup_git_repo(&repo_root);
    let default_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"]);
    let repo: RepoResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/projects/{}/repos", project.id),
        Some(&token),
        Some(json!({
            "name": "pr15-s01",
            "local_path": repo_path,
            "remote_url": "",
            "default_branch": default_branch
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(repo.name, "pr15-s01");

    let task: TaskResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/projects/{}/tasks", project.id),
        Some(&token),
        Some(json!({"title": "PR15 S01 real HTTP shell flow"})),
        StatusCode::OK,
    )
    .await;
    let _: Value = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/tasks/{}/task-roles", task.id),
        Some(&token),
        Some(json!({"role": "implementer", "coordination_mode": "independent", "policy": {}})),
        StatusCode::OK,
    )
    .await;
    let _: Value = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/tasks/{}/task-roles/implementer/members", task.id),
        Some(&token),
        Some(json!({"actor_ref": {"kind": "agent", "id": agent.id}})),
        StatusCode::OK,
    )
    .await;

    let work_unit_body = json!({
        "title": "Write the local HTTP result",
        "scope": "Create and commit pr15-result.txt in the isolated WorkUnit workspace",
        "role": "implementer",
        "parent_work_unit_id": null,
        "assigned_actor": null,
        "requires_integration": true,
        "provenance": null
    });
    let unauthorized = client
        .post(format!("{base}/api/v1/tasks/{}/work-units", task.id))
        .json(&work_unit_body)
        .send()
        .await
        .expect("unauthorized creation request completes");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let empty_after_rejection: Vec<WorkUnitResponse> = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(
        empty_after_rejection.is_empty(),
        "unauthorized POST writes no WorkUnit"
    );
    let denied_creation_operations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_integration_operation
         WHERE task_id = ? AND kind = 'work_unit_create'",
    )
    .bind(&task.id)
    .fetch_one(server.db.pool())
    .await
    .expect("denied WorkUnit creation operation count loads");
    assert_eq!(denied_creation_operations, 0);

    let created: WorkUnitResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        Some(&token),
        Some(work_unit_body),
        StatusCode::CREATED,
    )
    .await;
    let work_units: Vec<WorkUnitResponse> = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        work_units.len(),
        1,
        "one authorized POST creates one WorkUnit"
    );
    assert_eq!(work_units[0].id, created.id);
    let creation_operation_statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM task_integration_operation
         WHERE task_id = ? AND kind = 'work_unit_create' ORDER BY created_at",
    )
    .bind(&task.id)
    .fetch_all(server.db.pool())
    .await
    .expect("durable WorkUnit creation operation loads");
    assert_eq!(creation_operation_statuses, ["succeeded"]);

    let allocated: WorkUnitResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/work-units/{}/allocation", created.id),
        Some(&token),
        Some(json!({
            "expected_version": created.version,
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent.id}
        })),
        StatusCode::OK,
    )
    .await;
    let _: Value = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/work-units/{}/allocation", created.id),
        Some(&token),
        Some(json!({
            "expected_version": created.version,
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent.id}
        })),
        StatusCode::CONFLICT,
    )
    .await;

    let execution: ExecutionResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/tasks/{}/executions", task.id),
        Some(&token),
        Some(json!({
            "work_unit_id": allocated.id,
            "expected_work_unit_version": allocated.version,
            "purpose": "implement",
            "prompt": "printf 'pr15-http-result\\n' > pr15-result.txt && git add pr15-result.txt && git -c user.name='Forge Test' -c user.email='test@forge.invalid' commit -m 'PR15 HTTP result'",
            "input_artifact_ids": [],
            "overrides": null
        })),
        StatusCode::OK,
    )
    .await;
    assert_eq!(execution.status, ExecutionStatus::Running);
    let completed_execution = poll_execution(
        &client,
        &base,
        &token,
        &task.id,
        &execution.id,
        ExecutionStatus::Completed,
    )
    .await;
    let source_workspace_id = completed_execution
        .workspace_id
        .as_deref()
        .expect("Execution pins its WorkUnit workspace");
    let work_unit_path = workspace::WorkspaceManager::new(workspace_root.clone())
        .work_unit_worktree_path(&task.id, &created.id, source_workspace_id, &repo.id)
        .expect("exact WorkUnit worktree path resolves");
    assert!(
        work_unit_path.join("pr15-result.txt").exists(),
        "shell effect is present at {}",
        work_unit_path.display()
    );
    assert_eq!(
        std::fs::read_to_string(work_unit_path.join("pr15-result.txt"))
            .expect("shell effect exists in isolated worktree")
            .trim(),
        "pr15-http-result"
    );
    let source_sha = run_git(&work_unit_path, &["rev-parse", "HEAD"]);

    let rejected_integration = client
        .post(format!(
            "{base}/api/v1/work-units/{}/integrations",
            created.id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "execution_id": execution.id,
            "idempotency_key": "pr15-s01-before-completion"
        }))
        .send()
        .await
        .expect("premature integration request completes");
    assert_eq!(rejected_integration.status(), StatusCode::BAD_REQUEST);

    let current_work_unit: WorkUnitResponse = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/work-units/{}", created.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    let completed_work_unit: WorkUnitResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/work-units/{}/status", created.id),
        Some(&token),
        Some(json!({
            "expected_version": current_work_unit.version,
            "status": "completed"
        })),
        StatusCode::OK,
    )
    .await;
    let integration_body = json!({
        "execution_id": execution.id,
        "idempotency_key": "pr15-s01-exact-integration-v1"
    });
    let integration: WorkUnitIntegrationResponse = request_json(
        &client,
        Method::POST,
        &base,
        &format!("/api/v1/work-units/{}/integrations", created.id),
        Some(&token),
        Some(integration_body.clone()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(integration.outcome, WorkUnitIntegrationOutcome::Success);
    assert_eq!(integration.source_sha, source_sha);
    let target_sha = integration
        .target_after_sha
        .as_deref()
        .expect("successful integration records its target SHA")
        .to_owned();

    let lifecycle: TaskLifecycleResponse = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(lifecycle.task_id, task.id);
    assert_ne!(lifecycle.state, TaskLifecycleState::Merging);
    assert_ne!(lifecycle.state, TaskLifecycleState::Done);
    let gates: Vec<GateResponse> = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/tasks/{}/gates", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(
        gates.is_empty(),
        "integration does not invent Gate approval"
    );
    let task_workspace: WorkspaceResponse = request_json(
        &client,
        Method::GET,
        &base,
        &format!("/api/v1/tasks/{}/workspace", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        run_git(
            Path::new(&task_workspace.worktree_path),
            &["rev-parse", "HEAD"]
        ),
        target_sha
    );
    assert!(Path::new(&task_workspace.worktree_path)
        .join("pr15-result.txt")
        .exists());

    server.stop().await;

    // This constructs a fresh SqlitePool and a fresh AppState against the same
    // isolated file, so the next HTTP reads cross a real server/database reopen.
    let restarted_state = new_unseeded_state(&database_url, &workspace_root).await;
    let restarted = RunningServer::start(restarted_state, &web_dist).await;
    let persisted_task: TaskResponse = request_json(
        &client,
        Method::GET,
        &restarted.base_url,
        &format!("/api/v1/tasks/{}", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(persisted_task.id, task.id);
    assert_eq!(
        persisted_task
            .workspace
            .as_ref()
            .map(|workspace| workspace.id.as_str()),
        Some(task_workspace.id.as_str())
    );
    let persisted_work_units: Vec<WorkUnitResponse> = request_json(
        &client,
        Method::GET,
        &restarted.base_url,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(persisted_work_units.len(), 1);
    assert_eq!(persisted_work_units[0].id, created.id);
    assert_eq!(persisted_work_units[0].version, completed_work_unit.version);
    let persisted_executions: PaginatedResponse<ExecutionResponse> = request_json(
        &client,
        Method::GET,
        &restarted.base_url,
        &format!("/api/v1/tasks/{}/executions", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert_eq!(persisted_executions.items.len(), 1);
    assert_eq!(persisted_executions.items[0].id, execution.id);
    assert_eq!(
        persisted_executions.items[0].status,
        ExecutionStatus::Completed
    );
    let replay: WorkUnitIntegrationResponse = request_json(
        &client,
        Method::POST,
        &restarted.base_url,
        &format!("/api/v1/work-units/{}/integrations", created.id),
        Some(&token),
        Some(integration_body),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        replay.id, integration.id,
        "stable idempotency key replays one outcome"
    );
    assert_eq!(
        replay.target_after_sha.as_deref(),
        Some(target_sha.as_str())
    );
    let persisted_gates: Vec<GateResponse> = request_json(
        &client,
        Method::GET,
        &restarted.base_url,
        &format!("/api/v1/tasks/{}/gates", task.id),
        Some(&token),
        None,
        StatusCode::OK,
    )
    .await;
    assert!(persisted_gates.is_empty());
    restarted.stop().await;
}

#[tokio::test]
async fn s04_ended_membership_rejects_new_execution_for_preallocated_work_unit() {
    let temp = common::TestDir::new("pr15-s04-ended-membership");
    let harness = common::test_app(temp.path(), "pr15-s04-ended-membership").await;
    let repo_path = common::setup_git_repo(temp.path());
    let (project_id, _repo_id) =
        common::create_project_and_repo(&harness.app, "PR15 S04", &repo_path).await;
    let task: TaskResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Ended membership admission"}),
        StatusCode::OK,
    )
    .await;
    let (agent_id, _) =
        common::create_shell_agents(&harness.app, temp.path(), "pr15-s04-ended-membership").await;
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
    let membership = harness
        .state
        .task_service
        .add_task_role_member(
            &task.id,
            "implementer",
            api_types::ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("active implementer membership is created");
    let work_unit: WorkUnitResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/work-units", task.id),
        json!({
            "title": "Preallocated WorkUnit",
            "scope": "Remain allocated while membership is ended",
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null
        }),
        StatusCode::CREATED,
    )
    .await;
    let allocated: WorkUnitResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/work-units/{}/allocation", work_unit.id),
        json!({
            "expected_version": work_unit.version,
            "role": "implementer",
            "assigned_actor": {"kind": "agent", "id": agent_id}
        }),
        StatusCode::OK,
    )
    .await;
    harness
        .state
        .task_service
        .update_task_role_member(
            &task.id,
            &membership.id,
            membership.version,
            db::RoleMembershipStatus::Ended,
        )
        .await
        .expect("membership is ended after exact allocation");

    let rejected = common::raw_json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        json!({
            "work_unit_id": allocated.id,
            "expected_work_unit_version": allocated.version,
            "purpose": "implement",
            "prompt": "This must not reach the shell provider.",
            "input_artifact_ids": [],
            "overrides": null
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let error_bytes = axum::body::to_bytes(rejected.into_body(), usize::MAX)
        .await
        .expect("rejected admission error body reads");
    let error: api_types::ErrorResponse =
        serde_json::from_slice(&error_bytes).expect("rejected admission error parses");
    assert_eq!(error.code, "invalid_operation");
    assert_eq!(error.message, "WorkUnit is not runnable");
    let execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(harness.state.db.pool())
            .await
            .expect("Execution count loads after rejected admission");
    assert_eq!(execution_count, 0);
}

async fn new_unseeded_state(database_url: &str, workspace_root: &Path) -> AppState {
    let pool = db::create_sqlite_pool(database_url)
        .await
        .expect("isolated file-backed SQLite opens");
    db::run_migrations(&pool)
        .await
        .expect("current migrations apply");
    let db = Arc::new(db::SqliteDb::new(pool));
    let adapters = Arc::new(default_registry());
    services::ensure_default_agents(db.as_ref(), &adapters)
        .await
        .expect("default Agent records initialize");
    let event_bus = Arc::new(EventBus::new(64));
    let merge_service = Arc::new(services::MergeService::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        workspace_root.to_path_buf(),
    ));
    let cleanup_scheduler = Arc::new(services::WorkspaceCleanupScheduler::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        workspace_root.to_path_buf(),
    ));
    AppState::with_adapter_registry_services_and_shutdown(
        db,
        event_bus,
        true,
        adapters,
        merge_service,
        cleanup_scheduler,
        api::state::ShutdownSignal::new(),
        api::state::test_workflows_dir(),
        api::state::test_jwt_secret(),
        api::state::test_bcrypt_cost(),
    )
}

async fn request_json<T: DeserializeOwned>(
    client: &Client,
    method: Method,
    base: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    expected: StatusCode,
) -> T {
    let mut request = client.request(method, format!("{base}{path}"));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.expect("real HTTP request completes");
    let status = response.status();
    let body = response.bytes().await.expect("real HTTP body reads");
    assert_eq!(
        status,
        expected,
        "unexpected HTTP status; body: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("HTTP JSON response parses")
}

async fn poll_execution(
    client: &Client,
    base: &str,
    token: &str,
    task_id: &str,
    execution_id: &str,
    expected: ExecutionStatus,
) -> ExecutionResponse {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let page: PaginatedResponse<ExecutionResponse> = request_json(
            client,
            Method::GET,
            base,
            &format!("/api/v1/tasks/{task_id}/executions"),
            Some(token),
            None,
            StatusCode::OK,
        )
        .await;
        let execution = page
            .items
            .into_iter()
            .find(|execution| execution.id == execution_id)
            .expect("Execution remains listed by its Task");
        if execution.status == expected {
            return execution;
        }
        assert_eq!(
            execution.status,
            ExecutionStatus::Running,
            "Execution terminated before reaching the expected state"
        );
        assert!(
            Instant::now() < deadline,
            "Execution did not reach {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn run_git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("Git command starts");
    assert!(
        output.status.success(),
        "git {} failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
