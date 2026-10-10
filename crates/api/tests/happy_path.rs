#![allow(dead_code, clippy::assertions_on_constants)]
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use api::{build_router, AppState};
use api_types::{
    ActorRef, AgentResponse, CreateTerminalSessionResponse, DaemonRegisterResponse, DaemonResponse,
    ExecutionResponse, ExecutionStatus, GateEvaluationResponse, GateResponse,
    MergeAfterGateResponse, PaginatedResponse, ProjectResponse, RepoResponse,
    ReviewExecutionResponse, ReviewReportVerdict, StartReviewExecutionRequest,
    SubmitReviewReportRequest, SubmitReviewReportResponse, TaskLifecycleResponse,
    TaskLifecycleState, TaskResponse, TerminalAttachTokenResponse, TerminalServerFrame,
    TerminalSessionResponse, TerminalSessionStatus,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use db::{
    CoordinationMode, DomainEventRepo, ProjectHookRunRepo, TaskRepo, ValidationRunRepo,
    ValidationRunStatus, WorkspaceRepo,
};
use events::{EventBus, ForgeEvent, PROJECT_HOOK_RUN_CHANGED_EVENT};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn forge_happy_path_end_to_end() {
    let repo_dir = TestDir::new("forge-happy-repo");
    let repo_path = setup_git_repo(repo_dir.path()).await;
    let default_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"]);

    let workspaces_root = TestDir::new("forge-happy-workspaces");
    let harness = test_app(workspaces_root.path()).await;
    let mut events_rx = harness.event_bus.subscribe();

    let project: ProjectResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Happy Path" }),
        StatusCode::OK,
    )
    .await;
    let project_id = project.id;
    let repo: RepoResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/repos"),
        json!({
            "name": "repo",
            "local_path": repo_path.to_string_lossy(),
            "remote_url": repo_path.to_string_lossy(),
            "default_branch": default_branch
        }),
        StatusCode::OK,
    )
    .await;
    let repo_id = repo.id;
    assert!(repo.local_path.is_some());
    let expected_local_path = repo_path.canonicalize().expect("canonical repo path");
    let returned_local_path = repo
        .local_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .and_then(|path| path.canonicalize().ok());
    assert_eq!(
        returned_local_path.as_deref(),
        Some(expected_local_path.as_path())
    );
    assert_eq!(repo.remote_url, repo_path.to_string_lossy().as_ref());

    let daemon_id = register_daemon_and_report_shell(&harness.app, workspaces_root.path()).await;
    let agent: AgentResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        json!({
            "name": "shell-agent",
            "executor_type": "shell",
            "daemon_id": daemon_id,
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(agent.effective_status.as_deref(), Some("active"));
    let agent_id = agent.id;

    let created_task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "Happy path task",
            "description": "echo hello > greeting.txt && git add . && git commit -m 'hi'",
        }),
        StatusCode::OK,
    )
    .await;
    let task_id = created_task.id.clone();
    assert_eq!(created_task.lifecycle.state, TaskLifecycleState::Ready);
    assert!(serde_json::to_value(&created_task)
        .expect("Task response serializes")
        .get("status")
        .is_none());
    assert_eq!(created_task.version, 1);
    assign_test_user_as_reviewer(&harness.state, &task_id).await;
    assign_agent_as_implementer(&harness.state, &task_id, &agent_id).await;

    let execution: ExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/executions"),
        json!({
            "agent_id": agent_id.clone(),
            "role": "implementer",
            "purpose": "implement",
            "prompt": "echo hello > greeting.txt && git add . && git commit -m 'hi'",
            "input_artifact_ids": []
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(execution.role, "implementer");
    assert_eq!(
        execution.purpose,
        Some(api_types::ExecutionPurpose::Implement)
    );
    let execution_id = execution.id.clone();

    let worktree_path = workspaces_root.path().join(&task_id).join("repo");
    let greeting_path = worktree_path.join("greeting.txt");
    poll_until_workspace_written(&harness.app, &task_id, &greeting_path).await;
    poll_until_execution_completed(&harness.state.db, &execution_id).await;

    let workspace = WorkspaceRepo::get_by_task_id(&*harness.state.db, &task_id)
        .await
        .expect("Task Workspace lookup succeeds")
        .expect("worker Execution created a Workspace");
    let validation = services::ValidationService::new(
        Arc::clone(&harness.state.db),
        Arc::clone(&harness.event_bus),
    )
    .run_command(
        &task_id,
        &workspace.id,
        "test -f greeting.txt",
        0,
        Some(&execution_id),
        None,
    )
    .await
    .expect("explicit deterministic ValidationRun completes");
    assert_eq!(validation.run.status, ValidationRunStatus::Passed);

    let review = submit_human_review(
        &harness.app,
        &task_id,
        ReviewReportVerdict::Pass,
        "The committed file satisfies the requested change.",
    )
    .await;
    assert_eq!(
        review.review_execution.execution.status,
        ExecutionStatus::Completed
    );

    let validations = ValidationRunRepo::list_validation_runs_by_task(&*harness.state.db, &task_id)
        .await
        .expect("ValidationRun history loads");
    assert_eq!(validations.len(), 1);
    assert_eq!(validations[0].status, ValidationRunStatus::Passed);
    assert_eq!(validations[0].command, "test -f greeting.txt");
    assert!(!validations[0].workspace_id.is_empty());
    assert!(!validations[0].commit_sha.is_empty());
    let evidence = validation.evidence;
    assert_eq!(evidence.len(), 1);

    let report_content: Value = serde_json::from_str(
        review
            .report
            .content
            .as_deref()
            .expect("ReviewReport content is available"),
    )
    .expect("ReviewReport content is valid JSON");
    let subject = report_content
        .get("subject")
        .expect("ReviewReport has an exact subject");
    let gate_policy = json!({
        "schema_version": 1,
        "review": {
            "mode": "one_acceptable",
            "required_count": 1,
            "human_required": true,
            "allow_humans": true,
            "allow_agents": false,
            "allowed_actor_refs": [{"kind": "human", "id": "test-user-id"}],
            "task_role_snapshot": null,
            "candidates": [{
                "artifact_id": review.report.id,
                "digest": review.report.digest.as_deref().expect("ReviewReport digest exists"),
                "expected_actor": {"kind": "human", "id": "test-user-id"},
                "required": true,
                "subject": {
                    "workspace_id": subject["workspace_id"],
                    "base_commit_sha": subject["base_commit_sha"],
                    "head_commit_sha": subject["head_commit_sha"],
                    "workspace_snapshot_digest": subject["workspace_snapshot_digest"]
                }
            }]
        },
        "validations": [{
            "validation_run_id": validations[0].id,
            "evidence_id": evidence[0].id,
            "evidence_digest": evidence[0].digest,
            "check_identity": validations[0].check_identity,
            "config_digest": validations[0].config_digest,
            "workspace_id": validations[0].workspace_id,
            "commit_sha": validations[0].commit_sha,
            "workspace_snapshot_digest": validations[0].workspace_snapshot_digest,
            "required_outcome": "passed"
        }],
        "decisions": [],
        "work_units": []
    });
    let gate: GateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/gates"),
        json!({"gate_kind": "merge_readiness", "policy": gate_policy}),
        StatusCode::OK,
    )
    .await;
    let evaluation: GateEvaluationResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{}/evaluate", gate.gate.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evaluation.outcome, "satisfied");
    let events = DomainEventRepo::list_events_after(&*harness.state.db, 0, 100)
        .await
        .expect("GateEvaluation domain events load");
    let evaluation_event = events
        .iter()
        .find(|event| event.event_type == "gate.evaluated" && event.entity_id == evaluation.id)
        .expect("exact GateEvaluation event is durable");
    services::gate_engine::GateEngine::new(
        Arc::clone(&harness.state.db),
        Arc::clone(&harness.event_bus),
    )
    .process_domain_event(evaluation_event)
    .await
    .expect("durable GateEvaluation event applies aggregate lifecycle");
    let ready_to_merge =
        poll_until_task_lifecycle(&harness.app, &task_id, TaskLifecycleState::ReadyToMerge).await;
    assert_eq!(ready_to_merge.state, TaskLifecycleState::ReadyToMerge);

    let candidate_sha = validations[0].commit_sha.clone();
    std::fs::write(
        worktree_path.join("stale.txt"),
        "changed after Gate evaluation\n",
    )
    .expect("stale candidate file writes");
    run_git(&worktree_path, &["add", "."]);
    run_git(
        &worktree_path,
        &["commit", "-m", "change after Gate evaluation"],
    );
    let _: Value = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/merge"),
        json!({"gate_evaluation_id": evaluation.id}),
        StatusCode::CONFLICT,
    )
    .await;
    let still_ready =
        poll_until_task_lifecycle(&harness.app, &task_id, TaskLifecycleState::ReadyToMerge).await;
    assert_eq!(still_ready.version, ready_to_merge.version);
    run_git(&worktree_path, &["reset", "--hard", &candidate_sha]);

    let merge: MergeAfterGateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/merge"),
        json!({"gate_evaluation_id": evaluation.id}),
        StatusCode::OK,
    )
    .await;
    assert!(matches!(merge.outcome.as_str(), "done" | "pull_request"));
    let completed =
        poll_until_task_lifecycle(&harness.app, &task_id, TaskLifecycleState::Done).await;
    assert_eq!(completed.state, TaskLifecycleState::Done);

    let latest_subject = run_git(&repo_path, &["log", "-1", "--format=%s"]);
    assert!(
        latest_subject.contains("hi") || latest_subject.contains(&task_id),
        "latest git subject references the task change: {latest_subject}"
    );
    assert!(
        !workspaces_root.path().join(&task_id).exists(),
        "workspace task directory is cleaned"
    );

    assert_eq!(
        ValidationRunRepo::list_evidence_for_validation_run(&*harness.state.db, &validations[0].id)
            .await
            .expect("Validation Evidence loads")
            .len(),
        1
    );
    assert!(
        db::ReviewRepo::list_by_task(&*harness.state.db, &task_id)
            .await
            .expect("legacy Review rows load")
            .is_empty(),
        "new deterministic checks do not write legacy Review rows"
    );
    let validation_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'validation_run.completed' AND entity_id = ?",
    )
    .bind(&validations[0].id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("durable ValidationRun completion event loads");
    assert_eq!(validation_event_count, 1);

    let listed_tasks: PaginatedResponse<TaskResponse> = empty_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/projects/{project_id}/tasks"),
        StatusCode::OK,
    )
    .await;
    assert!(
        listed_tasks.items.iter().any(|task| task.id == task_id),
        "normal non-automation task remains visible in project task list"
    );
    let persisted_task = TaskRepo::get_by_id(&*harness.state.db, &task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert!(
        !persisted_task.is_automation,
        "happy-path user-created task defaults to non-automation"
    );

    let hook_runs =
        ProjectHookRunRepo::list_recent_for_project(&*harness.state.db, &project_id, 10)
            .await
            .expect("project hook runs load");
    assert!(
        hook_runs.is_empty(),
        "projects without configured hooks must not create project hook runs"
    );

    exercise_terminal_session(&harness, workspaces_root.path(), &project_id, &repo_id).await;

    let events = drain_events(&mut events_rx).await;
    assert_event_type(&events, "task.created");
    assert_event_type(&events, "task.updated");
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "task.assigned"),
        "TaskRole membership changes do not publish the retired singular assignment event"
    );
    assert_event_type(&events, "domain_event.committed");
    assert_event_type(&events, "workspace.cleaned");
    assert!(
        events
            .iter()
            .all(|event| event.event_type != PROJECT_HOOK_RUN_CHANGED_EVENT),
        "no project_hook.run_changed events are emitted when no hooks are configured"
    );
}

#[tokio::test]
async fn request_changes_report_leaves_lifecycle_to_gate_and_orchestration() {
    let repo_dir = TestDir::new("forge-autonomous-repo");
    let repo_path = setup_git_repo(repo_dir.path()).await;
    let default_branch = run_git(&repo_path, &["symbolic-ref", "--short", "HEAD"]);
    let workspaces_root = TestDir::new("forge-autonomous-workspaces");
    let harness = test_app(workspaces_root.path()).await;
    let project: ProjectResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/projects",
        json!({ "name": "Autonomous End to End" }),
        StatusCode::OK,
    )
    .await;
    let project_id = project.id;
    let _: RepoResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/repos"),
        json!({
            "name": "repo",
            "local_path": repo_path.to_string_lossy(),
            "remote_url": repo_path.to_string_lossy(),
            "default_branch": default_branch
        }),
        StatusCode::OK,
    )
    .await;
    let daemon_id = register_daemon_and_report_shell(&harness.app, workspaces_root.path()).await;
    let agent: AgentResponse = json_request(
        &harness.app,
        Method::POST,
        "/api/v1/agents",
        json!({
            "name": "autonomous-shell-agent",
            "executor_type": "shell",
            "daemon_id": daemon_id,
        }),
        StatusCode::OK,
    )
    .await;

    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({
            "title": "Requested changes stay a review fact",
            "description": "printf 'requested changes\\n' > requested-changes.txt && git add requested-changes.txt && git commit -m requested-changes"
        }),
        StatusCode::OK,
    )
    .await;
    assert_eq!(task.lifecycle.state, TaskLifecycleState::Ready);
    assign_test_user_as_reviewer(&harness.state, &task.id).await;
    assign_agent_as_implementer(&harness.state, &task.id, &agent.id).await;

    let execution: ExecutionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/executions", task.id),
        json!({
            "agent_id": agent.id,
            "role": "implementer",
            "purpose": "implement",
            "prompt": "printf 'requested changes\\n' > requested-changes.txt && git add requested-changes.txt && git commit -m requested-changes",
            "input_artifact_ids": []
        }),
        StatusCode::OK,
    )
    .await;
    let first_execution = execution;
    assert_eq!(first_execution.role.to_string(), "implementer");
    poll_until_execution_completed(&harness.state.db, &first_execution.id).await;

    let request_changes = submit_human_review(
        &harness.app,
        &task.id,
        ReviewReportVerdict::RequestChanges,
        "Please add evidence for the requested behavior.",
    )
    .await;
    assert_eq!(
        request_changes.review_execution.execution.status,
        ExecutionStatus::Completed
    );
    assert_eq!(
        request_changes.report.kind,
        api_types::ArtifactKind::ReviewReport
    );
    let report_content: Value = serde_json::from_str(
        request_changes
            .report
            .content
            .as_deref()
            .expect("ReviewReport content is available"),
    )
    .expect("ReviewReport content is valid JSON");
    let subject = &report_content["subject"];
    let gate: GateResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/gates", task.id),
        json!({
            "gate_kind": "review",
            "policy": {
                "schema_version": 1,
                "review": {
                    "mode": "one_acceptable",
                    "required_count": 1,
                    "human_required": true,
                    "allow_humans": true,
                    "allow_agents": false,
                    "allowed_actor_refs": [{"kind": "human", "id": "test-user-id"}],
                    "candidates": [{
                        "artifact_id": request_changes.report.id,
                        "digest": request_changes.report.digest,
                        "expected_actor": {"kind": "human", "id": "test-user-id"},
                        "required": true,
                        "subject": {
                            "workspace_id": subject["workspace_id"],
                            "base_commit_sha": subject["base_commit_sha"],
                            "head_commit_sha": subject["head_commit_sha"],
                            "workspace_snapshot_digest": subject["workspace_snapshot_digest"]
                        }
                    }]
                },
                "validations": [],
                "decisions": [],
                "work_units": []
            }
        }),
        StatusCode::OK,
    )
    .await;
    let evaluation: GateEvaluationResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/gates/{}/evaluate", gate.gate.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_eq!(evaluation.outcome, "unsatisfied");
    let lifecycle: TaskLifecycleResponse = json_request(
        &harness.app,
        Method::GET,
        &format!("/api/v1/tasks/{}/lifecycle", task.id),
        json!(null),
        StatusCode::OK,
    )
    .await;
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);
    let implementer_executions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = 'implementer'",
    )
    .bind(&task.id)
    .fetch_one(harness.state.db.pool())
    .await
    .expect("implementer Execution count loads");
    assert_eq!(implementer_executions, 1);
}

async fn assign_test_user_as_reviewer(state: &AppState, task_id: &str) {
    state
        .task_service
        .create_task_role(
            task_id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer TaskRole creates");
    state
        .task_service
        .add_task_role_member(
            task_id,
            "reviewer",
            ActorRef::Human("test-user-id".to_owned()),
        )
        .await
        .expect("test Human joins the reviewer TaskRole");
}

async fn assign_agent_as_implementer(state: &AppState, task_id: &str, agent_id: &str) {
    state
        .task_service
        .create_task_role(
            task_id,
            "implementer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("implementer TaskRole creates");
    state
        .task_service
        .add_task_role_member(task_id, "implementer", ActorRef::Agent(agent_id.to_owned()))
        .await
        .expect("Agent joins the implementer TaskRole");
}

async fn submit_human_review(
    app: &Router,
    task_id: &str,
    verdict: ReviewReportVerdict,
    summary: &str,
) -> SubmitReviewReportResponse {
    let review: ReviewExecutionResponse = json_request(
        app,
        Method::POST,
        &format!("/api/v1/tasks/{task_id}/review-executions"),
        serde_json::to_value(StartReviewExecutionRequest { workspace_id: None })
            .expect("review start request serializes"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        review.execution.actor_ref,
        Some(ActorRef::Human("test-user-id".to_owned()))
    );
    assert_eq!(review.execution.role.to_string(), "reviewer");
    assert_eq!(
        review.execution.purpose,
        Some(api_types::ExecutionPurpose::Review)
    );
    json_request(
        app,
        Method::POST,
        &format!("/api/v1/review-executions/{}", review.execution.id),
        serde_json::to_value(SubmitReviewReportRequest {
            verdict,
            summary: summary.to_owned(),
            criteria: vec!["acceptance criteria".to_owned()],
            findings: Vec::new(),
            questions: Vec::new(),
            evidence_ids: Vec::new(),
            artifact_ids: Vec::new(),
        })
        .expect("ReviewReport request serializes"),
        StatusCode::OK,
    )
    .await
}

struct TestHarness {
    app: Router,
    state: Arc<AppState>,
    event_bus: Arc<EventBus>,
    _web_dist_dir: TestDir,
}

async fn test_app(workspace_root: &Path) -> TestHarness {
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
    .expect("seed test user");
    let adapter_registry = Arc::new(cli_adapters::default_registry());
    services::ensure_default_agents(db.as_ref(), &adapter_registry)
        .await
        .expect("default agents upsert");
    let event_bus = Arc::new(EventBus::new(256));
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
    let review_runner = Arc::new(review::ReviewRunner::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        Arc::clone(&adapter_registry),
    ));
    let mut state = AppState::with_adapter_registry_services_and_shutdown(
        db,
        Arc::clone(&event_bus),
        true,
        adapter_registry,
        merge_service,
        cleanup_scheduler,
        review_runner,
        api::state::ShutdownSignal::new(),
        api::state::test_workflows_dir(),
        api::state::test_jwt_secret(),
        api::state::test_bcrypt_cost(),
    );
    let mut config = (*state.effective_config).clone();
    config.terminal.enabled = true;
    state = state.with_effective_config(config);
    let state = Arc::new(state);

    let web_dist_dir = TestDir::new("forge-happy-web");
    std::fs::write(web_dist_dir.path().join("index.html"), "<html></html>").expect("write index");
    let app = build_router((*state).clone(), web_dist_dir.path().to_path_buf());

    TestHarness {
        app,
        state,
        event_bus,
        _web_dist_dir: web_dist_dir,
    }
}

async fn exercise_terminal_session(
    harness: &TestHarness,
    workspace_root: &Path,
    project_id: &str,
    repo_id: &str,
) {
    let task: TaskResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/projects/{project_id}/tasks"),
        json!({ "title": "Terminal happy path", "description": "terminal smoke" }),
        StatusCode::OK,
    )
    .await;
    seed_ready_workspace(&harness.state.db, workspace_root, &task.id, repo_id).await;

    let created: CreateTerminalSessionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/tasks/{}/terminals", task.id),
        json!({ "rows": 24, "cols": 80 }),
        StatusCode::CREATED,
    )
    .await;
    let refreshed: TerminalAttachTokenResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/terminals/{}/attach-token", created.session.id),
        json!({}),
        StatusCode::OK,
    )
    .await;
    assert_ne!(created.attach.attach_token, refreshed.attach_token);

    let mut live_rx = harness
        .state
        .terminal_service
        .attach_client(&created.session.id)
        .await;
    harness
        .state
        .terminal_service
        .handle_terminal_input(&created.session.id, "ZWNobyBmb3JnZS10ZXJtaW5hbC1vawo=")
        .await
        .expect("terminal input accepted");
    wait_for_terminal_output(&mut live_rx, "forge-terminal-ok").await;
    drop(live_rx);

    let mut replay_rx = harness
        .state
        .terminal_service
        .attach_client(&created.session.id)
        .await;
    wait_for_terminal_output(&mut replay_rx, "forge-terminal-ok").await;

    let terminated: TerminalSessionResponse = json_request(
        &harness.app,
        Method::POST,
        &format!("/api/v1/terminals/{}/terminate", created.session.id),
        json!({ "reason": "happy_path" }),
        StatusCode::OK,
    )
    .await;
    assert!(matches!(
        terminated.status,
        TerminalSessionStatus::Terminated
    ));
}

async fn seed_ready_workspace(
    db: &Arc<db::SqliteDb>,
    workspace_root: &Path,
    task_id: &str,
    repo_id: &str,
) {
    let now = db::now_rfc3339();
    let worktree_path = workspace_root.join(task_id).join("terminal-repo");
    std::fs::create_dir_all(&worktree_path).expect("terminal worktree creates");
    db::WorkspaceRepo::create(
        &**db,
        db::CreateWorkspace {
            id: db::new_uuid_v4(),
            task_id: task_id.to_owned(),
            repo_id: repo_id.to_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: workspace::task_branch_name(task_id),
            status: db::WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("terminal workspace creates");
}

async fn wait_for_terminal_output(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<TerminalServerFrame>,
    needle: &str,
) {
    let mut collected = String::new();
    for _ in 0..20 {
        if let Ok(Some(TerminalServerFrame::Output { data })) =
            tokio::time::timeout(Duration::from_millis(250), rx.recv()).await
        {
            collected.push_str(&String::from_utf8_lossy(&decode_base64_standard(&data)));
            if collected.contains(needle) {
                return;
            }
        }
    }
    panic!("terminal output did not contain {needle}; collected {collected:?}");
}

fn decode_base64_standard(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0_u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => continue,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    out
}

async fn setup_git_repo(path: &Path) -> std::path::PathBuf {
    let repo_path = path.join("repo");
    std::fs::create_dir_all(&repo_path).expect("repo dir creates");
    run_git(&repo_path, &["init"]);
    run_git(&repo_path, &["config", "user.email", "test@forge.dev"]);
    run_git(&repo_path, &["config", "user.name", "Forge Test"]);
    std::fs::write(repo_path.join("README.md"), "# Happy Path\n").expect("README writes");
    run_git(&repo_path, &["add", "-A"]);
    run_git(&repo_path, &["commit", "-m", "initial commit"]);
    repo_path
}

async fn register_daemon_and_report_shell(app: &Router, workspace_root: &Path) -> String {
    let registration: DaemonRegisterResponse = json_request(
        app,
        Method::POST,
        "/api/v1/daemons/register",
        json!({
            "machine_id": services::embedded_daemon::embedded_machine_id(),
            "hostname": "happy-path-host",
            "os": "linux",
            "arch": "x86_64",
            "agent_version": "happy-path-test",
            "labels": { "suite": "happy_path" }
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
            "detected_clis": [{
                "kind": "shell",
                "availability": "authenticated",
                "path": "/bin/sh"
            }],
            "runtimes": [{
                "kind": "local",
                "workspace_root": workspace_root.to_string_lossy(),
                "status": "ready"
            }]
        }),
        StatusCode::OK,
    )
    .await;

    daemon_id
}

async fn poll_until_execution_completed(db: &Arc<db::SqliteDb>, execution_id: &str) {
    for _ in 0..100 {
        if let Some(execution) = db::ExecutionRepo::get_by_id(&**db, execution_id)
            .await
            .expect("execution lookup")
        {
            if execution.status == db::ExecutionStatus::Completed {
                return;
            }
            if execution.status != db::ExecutionStatus::Running {
                let log = execution
                    .logs_path
                    .as_deref()
                    .and_then(|path| std::fs::read_to_string(path).ok());
                panic!(
                    "execution ended in unexpected status: {:?}; error: {:?}; logs: {:?}",
                    execution.status, execution.error, log
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("execution did not complete within timeout");
}

async fn poll_until_workspace_written(app: &Router, task_id: &str, greeting_path: &Path) {
    for _ in 0..100 {
        if greeting_path.exists() {
            return;
        }
        let execution = single_execution_for_task(app, task_id).await;
        if execution.status != ExecutionStatus::Running {
            panic!(
                "execution ended before writing workspace output: {:?}; error: {:?}; logs: {:?}",
                execution.status,
                execution.error,
                execution
                    .logs_path
                    .as_deref()
                    .and_then(|path| std::fs::read_to_string(path).ok())
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        greeting_path.exists(),
        "greeting.txt was not written before execution stopped"
    );
}

async fn poll_until_task_lifecycle(
    app: &Router,
    task_id: &str,
    expected_state: TaskLifecycleState,
) -> TaskLifecycleResponse {
    for _ in 0..100 {
        let lifecycle: TaskLifecycleResponse = empty_request(
            app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}/lifecycle"),
            StatusCode::OK,
        )
        .await;
        if lifecycle.state == expected_state {
            return lifecycle;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("task did not reach lifecycle {expected_state:?} within timeout");
}

async fn single_execution_for_task(app: &Router, task_id: &str) -> ExecutionResponse {
    let executions: PaginatedResponse<ExecutionResponse> = empty_request(
        app,
        Method::GET,
        &format!("/api/v1/tasks/{task_id}/executions"),
        StatusCode::OK,
    )
    .await;
    assert_eq!(executions.items.len(), 1);
    executions.items.into_iter().next().unwrap()
}

async fn poll_until_follow_up_execution(
    app: &Router,
    task_id: &str,
    parent_execution_id: &str,
) -> ExecutionResponse {
    for _ in 0..100 {
        let executions: PaginatedResponse<ExecutionResponse> = empty_request(
            app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}/executions"),
            StatusCode::OK,
        )
        .await;
        if let Some(execution) = executions
            .items
            .into_iter()
            .find(|execution| execution.parent_execution_id.as_deref() == Some(parent_execution_id))
        {
            return execution;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("worker follow-up execution was not recorded");
}

async fn poll_until_fresh_worker_execution(
    app: &Router,
    task_id: &str,
    previous_execution_id: &str,
) -> ExecutionResponse {
    for _ in 0..100 {
        let executions: PaginatedResponse<ExecutionResponse> = empty_request(
            app,
            Method::GET,
            &format!("/api/v1/tasks/{task_id}/executions"),
            StatusCode::OK,
        )
        .await;
        if let Some(execution) = executions
            .items
            .into_iter()
            .find(|execution| execution.role == "worker" && execution.id != previous_execution_id)
        {
            return execution;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("fresh worker execution was not recorded after the ReviewReport");
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

fn assert_event_type(events: &[ForgeEvent], event_type: &str) {
    assert!(
        events.iter().any(|event| event.event_type == event_type),
        "missing event {event_type}; got {:?}",
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
    );
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
    let response = raw_json_request(app, method, uri, body).await;
    parse_response(response, expected_status).await
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
                .expect("build authorized JSON request"),
        )
        .await
        .expect("router response");
    parse_response(response, expected_status).await
}

async fn empty_request<T>(app: &Router, method: Method, uri: &str, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let response = raw_empty_request(app, method, uri).await;
    parse_response(response, expected_status).await
}

fn test_jwt() -> String {
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

async fn raw_json_request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Value,
) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {}", test_jwt()))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .expect("build JSON request"),
        )
        .await
        .expect("router response")
}

async fn raw_empty_request(app: &Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {}", test_jwt()))
                .body(Body::empty())
                .expect("build empty request"),
        )
        .await
        .expect("router response")
}

async fn parse_response<T>(response: axum::response::Response, expected_status: StatusCode) -> T
where
    T: DeserializeOwned,
{
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    assert_eq!(
        status,
        expected_status,
        "unexpected response status with body: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("parse JSON response")
}

fn run_git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git command runs");
    assert!(
        output.status.success(),
        "git {} failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct TestDir {
    path: PathBuf,
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

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
