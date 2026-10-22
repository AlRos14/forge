#![allow(dead_code)]

mod common;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

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

#[tokio::test]
async fn s02_three_shell_processes_write_concurrently_in_isolated_work_unit_worktrees() {
    let temp = common::TestDir::new("pr15-s02-three-shell-workers");
    let workspace_root = temp.path().join("workspace-root");
    std::fs::create_dir_all(&workspace_root).expect("workspace root creates");
    let barrier_dir = common::TestDir::new_in(temp.path(), "barrier");
    let release_path = barrier_dir.path().join("release");
    let release = ShellWorkerReleaseGuard::new(release_path.clone());
    let harness = common::test_app(&workspace_root, "pr15-s02-three-shell-workers").await;
    let repo_path = common::setup_git_repo(temp.path());
    let base_sha = run_git(&repo_path, &["rev-parse", "HEAD"]);
    let (project_id, _repo_id) =
        common::create_project_and_repo(&harness.app, "PR15 S02 concurrent writers", &repo_path)
            .await;
    let task: TaskResponse = common::json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({"title": "Three isolated shell writers"}),
        StatusCode::OK,
    )
    .await;

    let (agent_a, agent_b) =
        common::create_shell_agents(&harness.app, &workspace_root, "pr15-s02").await;
    let registered_a = db::AgentRepo::get_by_id(&*harness.state.db, &agent_a)
        .await
        .expect("Agent A loads")
        .expect("Agent A exists");
    let daemon_id = registered_a
        .daemon_id
        .expect("shell Agent has embedded daemon");
    let agent_c: AgentResponse = common::json_request_with_bearer(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        &common::admin_jwt(),
        json!({
            "name": "pr15-s02-agent-c",
            "executor_type": "shell",
            "daemon_id": daemon_id
        }),
        StatusCode::OK,
    )
    .await;
    let agent_ids = [agent_a, agent_b, agent_c.id];
    for agent_id in &agent_ids {
        let agent = db::AgentRepo::get_by_id(&*harness.state.db, agent_id)
            .await
            .expect("eligible Agent loads")
            .expect("eligible Agent exists");
        assert!(
            agent.max_concurrent_tasks >= 1,
            "one assigned writer fits its Agent capacity"
        );
    }

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
        .expect("independent implementer TaskRole creates");
    for agent_id in &agent_ids {
        harness
            .state
            .task_service
            .add_task_role_member(
                &task.id,
                "implementer",
                api_types::ActorRef::Agent(agent_id.clone()),
            )
            .await
            .expect("each shell Agent receives explicit implementer membership");
    }

    let labels = ["a", "b", "c"];
    let mut work_units = Vec::with_capacity(labels.len());
    for label in labels {
        work_units.push(
            common::create_allocated_work_unit(
                &harness.app,
                &task.id,
                "implementer",
                &agent_ids[work_units.len()],
                &format!("Write worker {label}"),
                "Write one uniquely named file in the exact WorkUnit worktree",
                true,
            )
            .await,
        );
    }

    let mut executions = Vec::with_capacity(labels.len());
    for (index, label) in labels.iter().enumerate() {
        let work_unit = &work_units[index];
        let execution: ExecutionResponse = common::json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/tasks/{}/executions", task.id),
            json!({
                "work_unit_id": work_unit.id,
                "expected_work_unit_version": work_unit.version,
                "purpose": "implement",
                "prompt": s02_shell_command(
                    label,
                    &release_path,
                    &barrier_dir.path().join(format!("ready-{label}")),
                ),
                "input_artifact_ids": [],
                "overrides": null
            }),
            StatusCode::OK,
        )
        .await;
        assert_eq!(execution.status, ExecutionStatus::Running);
        executions.push(execution);
    }

    let mut workspaces = Vec::with_capacity(executions.len());
    let mut leases = Vec::with_capacity(executions.len());
    let mut execution_rows = Vec::with_capacity(executions.len());
    for index in 0..executions.len() {
        let execution = &executions[index];
        let work_unit = &work_units[index];
        let agent_id = &agent_ids[index];
        let row = db::ExecutionRepo::get_by_id(&*harness.state.db, &execution.id)
            .await
            .expect("Execution row loads")
            .expect("Execution row exists");
        assert_eq!(row.task_id, task.id);
        assert_eq!(row.agent_id.as_deref(), Some(agent_id.as_str()));
        assert_eq!(row.actor_ref(), Some(db::ActorRef::Agent(agent_id.clone())));
        assert_eq!(row.role, "implementer");
        assert_eq!(row.purpose, Some(db::ExecutionPurpose::Implement));
        assert_eq!(row.work_unit_id.as_deref(), Some(work_unit.id.as_str()));
        assert_eq!(row.work_unit_version, Some(work_unit.version));
        let workspace_id = row
            .workspace_id
            .as_deref()
            .expect("Execution pins Workspace");
        assert_eq!(execution.workspace_id.as_deref(), Some(workspace_id));
        let workspace = db::WorkspaceRepo::get_by_id(&*harness.state.db, workspace_id)
            .await
            .expect("Workspace row loads")
            .expect("Workspace row exists");
        assert_eq!(workspace.task_id, task.id);
        assert_eq!(workspace.status, db::WorkspaceStatus::Ready);
        let lease =
            db::WorkspaceLeaseRepo::get_active_for_work_unit(&*harness.state.db, &work_unit.id)
                .await
                .expect("exact active WorkUnit lease loads")
                .expect("each admitted Execution has an active lease");
        assert_eq!(lease.task_id, task.id);
        assert_eq!(lease.work_unit_id.as_deref(), Some(work_unit.id.as_str()));
        assert_eq!(lease.workspace_id.as_deref(), Some(workspace.id.as_str()));
        assert_eq!(lease.execution_id, execution.id);
        assert_eq!(lease.role, "worker");
        assert_eq!(lease.assigned_principal_type, "agent");
        assert_eq!(lease.assigned_principal_id, *agent_id);
        workspaces.push(workspace);
        leases.push(lease);
        execution_rows.push(row);
    }

    let workspace_paths: Vec<_> = workspaces
        .iter()
        .map(|workspace| workspace.worktree_path.clone())
        .collect();
    let workspace_branches: Vec<_> = workspaces
        .iter()
        .map(|workspace| workspace.branch.clone())
        .collect();
    let mut unique_paths = workspace_paths.clone();
    unique_paths.sort();
    unique_paths.dedup();
    let mut unique_branches = workspace_branches.clone();
    unique_branches.sort();
    unique_branches.dedup();
    assert_eq!(unique_paths.len(), 3, "each Execution has its own worktree");
    assert_eq!(
        unique_branches.len(),
        3,
        "each Execution has its own branch"
    );
    for (index, workspace) in workspaces.iter().enumerate() {
        assert!(Path::new(&workspace.worktree_path).is_dir());
        assert_eq!(
            run_git(
                Path::new(&workspace.worktree_path),
                &["branch", "--show-current"]
            ),
            workspace.branch
        );
        for label in labels {
            assert!(
                !Path::new(&workspace.worktree_path)
                    .join(format!("s02-worker-{label}.txt"))
                    .exists(),
                "no worker writes before the shared release"
            );
        }
        assert_eq!(
            execution_rows[index].workspace_id.as_deref(),
            Some(workspace.id.as_str())
        );
        assert_eq!(
            leases[index].workspace_id.as_deref(),
            Some(workspace.id.as_str())
        );
    }

    let readiness_deadline = Instant::now() + Duration::from_secs(20);
    let mut ready_records = vec![None; labels.len()];
    loop {
        for (index, label) in labels.iter().enumerate() {
            if ready_records[index].is_none() {
                ready_records[index] =
                    read_s02_ready_record(&barrier_dir.path().join(format!("ready-{label}")));
            }
        }
        if ready_records.iter().all(Option::is_some) || Instant::now() >= readiness_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let all_ready = ready_records.iter().all(Option::is_some);
    let mut ready_pids: Vec<u32> = ready_records
        .iter()
        .filter_map(|record| record.as_ref().map(|record| record.pid))
        .collect();
    ready_pids.sort_unstable();
    ready_pids.dedup();
    let simultaneous_before_release = all_ready
        && ready_pids.len() == labels.len()
        && ready_records.iter().enumerate().all(|(index, record)| {
            record.as_ref().is_some_and(|record| {
                std::fs::canonicalize(&workspace_paths[index])
                    .is_ok_and(|path| record.worktree_path == path.to_string_lossy())
                    && shell_process_is_alive(record.pid)
                    && !Path::new(&workspace_paths[index])
                        .join(format!("s02-worker-{}.txt", labels[index]))
                        .exists()
            })
        });
    release
        .release_once()
        .expect("shared shell release publishes once");

    let terminal = wait_for_s02_terminal_executions(
        &harness.app,
        &executions
            .iter()
            .map(|execution| execution.id.clone())
            .collect::<Vec<_>>(),
        Duration::from_secs(45),
    )
    .await;
    let terminal = if terminal.is_none() {
        for execution in &executions {
            let current: ExecutionResponse = common::empty_request(
                &harness.app,
                Method::GET,
                &format!("/api/v1/executions/{}", execution.id),
                StatusCode::OK,
            )
            .await;
            if current.status == ExecutionStatus::Running {
                let _: ExecutionResponse = common::empty_request(
                    &harness.app,
                    Method::POST,
                    &format!("/api/v1/executions/{}/cancel", execution.id),
                    StatusCode::OK,
                )
                .await;
            }
        }
        wait_for_s02_terminal_executions(
            &harness.app,
            &executions
                .iter()
                .map(|execution| execution.id.clone())
                .collect::<Vec<_>>(),
            Duration::from_secs(10),
        )
        .await
    } else {
        terminal
    };
    assert!(
        all_ready,
        "all shell processes reached the barrier; records: {ready_records:?}"
    );
    assert!(
        simultaneous_before_release,
        "three distinct shell PIDs were alive in their exact worktrees before release: {ready_records:?}"
    );
    let terminal = terminal.expect("all shell processes terminate before timeout");
    assert_eq!(terminal.len(), 3);
    assert!(terminal
        .iter()
        .all(|execution| execution.status == ExecutionStatus::Completed));

    let mut source_shas = Vec::with_capacity(workspaces.len());
    for (index, workspace) in workspaces.iter().enumerate() {
        let worktree = Path::new(&workspace.worktree_path);
        let own_file = worktree.join(format!("s02-worker-{}.txt", labels[index]));
        assert_eq!(
            std::fs::read_to_string(&own_file)
                .expect("worker's file exists only in its worktree")
                .trim(),
            format!("worker-{}", labels[index])
        );
        for (other_index, other_label) in labels.iter().enumerate() {
            if other_index != index {
                assert!(
                    !worktree
                        .join(format!("s02-worker-{other_label}.txt"))
                        .exists(),
                    "{} is not contaminated by worker {other_label}",
                    workspace.worktree_path
                );
            }
        }
        assert!(run_git(worktree, &["status", "--porcelain"]).is_empty());
        let sha = run_git(worktree, &["rev-parse", "HEAD"]);
        assert_ne!(sha, base_sha);
        let completed_row = db::ExecutionRepo::get_by_id(&*harness.state.db, &executions[index].id)
            .await
            .expect("completed Execution reloads")
            .expect("completed Execution remains persisted");
        assert_eq!(completed_row.after_sha.as_deref(), Some(sha.as_str()));
        source_shas.push(sha);
    }
    let mut unique_source_shas = source_shas.clone();
    unique_source_shas.sort();
    unique_source_shas.dedup();
    assert_eq!(
        unique_source_shas.len(),
        3,
        "each worker produced a distinct Git commit"
    );
    assert_eq!(run_git(&repo_path, &["rev-parse", "HEAD"]), base_sha);

    let mut previous_target_sha: Option<String> = None;
    let mut target_workspace_id: Option<String> = None;
    for index in 0..work_units.len() {
        let current: WorkUnitResponse = common::empty_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/work-units/{}", work_units[index].id),
            StatusCode::OK,
        )
        .await;
        let completed: WorkUnitResponse = common::json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/work-units/{}/status", current.id),
            json!({"expected_version": current.version, "status": "completed"}),
            StatusCode::OK,
        )
        .await;
        assert_eq!(completed.status, api_types::WorkUnitStatus::Completed);
        let integration: WorkUnitIntegrationResponse = common::json_request(
            &harness.app,
            Method::POST,
            &format!("/api/v1/work-units/{}/integrations", completed.id),
            json!({
                "execution_id": executions[index].id,
                "idempotency_key": format!("pr15-s02-integration-{}", labels[index])
            }),
            StatusCode::OK,
        )
        .await;
        assert_eq!(integration.outcome, WorkUnitIntegrationOutcome::Success);
        assert_eq!(integration.source_sha, source_shas[index]);
        if let Some(previous_target_sha) = previous_target_sha.as_deref() {
            assert_eq!(&integration.target_before_sha, previous_target_sha);
        }
        let target_after_sha = integration
            .target_after_sha
            .clone()
            .expect("serial integration records its resulting SHA");
        if let Some(expected_workspace_id) = target_workspace_id.as_deref() {
            assert_eq!(&integration.target_workspace_id, expected_workspace_id);
        }
        target_workspace_id = Some(integration.target_workspace_id.clone());
        previous_target_sha = Some(target_after_sha.clone());
        let target: WorkspaceResponse = common::empty_request(
            &harness.app,
            Method::GET,
            &format!("/api/v1/tasks/{}/workspace", task.id),
            StatusCode::OK,
        )
        .await;
        assert_eq!(integration.target_workspace_id, target.id);
        assert_eq!(
            run_git(Path::new(&target.worktree_path), &["rev-parse", "HEAD"]),
            target_after_sha
        );
    }

    let target: WorkspaceResponse = common::empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/workspace", task.id),
        StatusCode::OK,
    )
    .await;
    for label in labels {
        assert_eq!(
            std::fs::read_to_string(
                Path::new(&target.worktree_path).join(format!("s02-worker-{label}.txt"))
            )
            .expect("serialized integration preserves every worker result")
            .trim(),
            format!("worker-{label}")
        );
    }
    let lifecycle: TaskLifecycleResponse = common::empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        StatusCode::OK,
    )
    .await;
    assert_ne!(lifecycle.state, TaskLifecycleState::Merging);
    assert_ne!(lifecycle.state, TaskLifecycleState::Done);
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

#[derive(Debug, Clone)]
struct S02ReadyRecord {
    pid: u32,
    worktree_path: String,
}

struct ShellWorkerReleaseGuard {
    release_path: PathBuf,
}

impl ShellWorkerReleaseGuard {
    fn new(release_path: PathBuf) -> Self {
        Self { release_path }
    }

    fn release_once(&self) -> std::io::Result<()> {
        use std::io::Write;

        let mut release = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.release_path)?;
        release.write_all(b"released\n")
    }
}

impl Drop for ShellWorkerReleaseGuard {
    fn drop(&mut self) {
        // A panic before the explicit release must still let any already
        // started shell process leave its barrier before temporary data drops.
        let _ = std::fs::write(&self.release_path, b"released during cleanup\n");
    }
}

fn s02_shell_command(label: &str, release_path: &Path, ready_path: &Path) -> String {
    let release_path = shell_quote(&release_path.to_string_lossy());
    let ready_path = shell_quote(&ready_path.to_string_lossy());
    let output_file = shell_quote(&format!("s02-worker-{label}.txt"));
    let output = shell_quote(&format!("worker-{label}"));
    format!(
        "set -eu\nready={ready_path}\nrelease={release_path}\nready_tmp=\"$ready.tmp\"\ntrap 'rm -f \"$ready\" \"$ready_tmp\"' EXIT HUP INT TERM\nprocess_worktree=$(pwd -P)\nprintf '%s\\n%s\\n' \"$$\" \"$process_worktree\" > \"$ready_tmp\"\nmv \"$ready_tmp\" \"$ready\"\npolls=0\nwhile [ ! -f \"$release\" ]; do\n  polls=$((polls + 1))\n  if [ \"$polls\" -ge 6000 ]; then echo 'S02 barrier timed out' >&2; exit 124; fi\n  sleep 0.01\ndone\nprintf '%s\\n' {output} > {output_file}\ngit add -- {output_file}\ngit -c user.name='Forge PR15 S02' -c user.email='pr15-s02@example.invalid' commit -m 'PR15 S02 worker {label}'\n"
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn read_s02_ready_record(path: &Path) -> Option<S02ReadyRecord> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut lines = contents.lines();
    Some(S02ReadyRecord {
        pid: lines.next()?.parse().ok()?,
        worktree_path: lines.next()?.to_owned(),
    })
}

fn shell_process_is_alive(pid: u32) -> bool {
    std::process::Command::new("sh")
        .args([
            "-c",
            "kill -0 \"$1\" 2>/dev/null",
            "s02-pid-check",
            &pid.to_string(),
        ])
        .status()
        .is_ok_and(|status| status.success())
}

async fn wait_for_s02_terminal_executions(
    app: &axum::Router,
    execution_ids: &[String],
    timeout: Duration,
) -> Option<Vec<ExecutionResponse>> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut executions = Vec::with_capacity(execution_ids.len());
        for execution_id in execution_ids {
            executions.push(
                common::empty_request::<ExecutionResponse>(
                    app,
                    Method::GET,
                    &format!("/api/v1/executions/{execution_id}"),
                    StatusCode::OK,
                )
                .await,
            );
        }
        if executions
            .iter()
            .all(|execution| execution.status != ExecutionStatus::Running)
        {
            return Some(executions);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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
