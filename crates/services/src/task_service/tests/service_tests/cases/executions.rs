use super::super::*;
use api_types::ActorRef;
use db::{CollaborationRepo, CoordinationMode, ProjectMemberRepo, TaskLifecycleRepo, TaskRoleRepo};
use serde_json::Value;

async fn add_human_reviewer(
    db: &db::SqliteDb,
    service: &TaskService,
    task: &db::Task,
) -> (String, db::RoleMembership) {
    if TaskRoleRepo::get_by_task_and_role(db, &task.id, "reviewer")
        .await
        .expect("reviewer role lookup succeeds")
        .is_none()
    {
        service
            .create_task_role(
                &task.id,
                "reviewer",
                CoordinationMode::Collaborative,
                "{}".to_owned(),
            )
            .await
            .expect("reviewer TaskRole creates");
    }
    let user_id = seed_human_user(db).await;
    let now = now_rfc3339();
    ProjectMemberRepo::add_member(
        db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: task.project_id.clone(),
            user_id: user_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Human joins the Task project");
    let membership = service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human(user_id.clone()))
        .await
        .expect("Human joins the reviewer TaskRole");
    (user_id, membership)
}

async fn assign_legacy_human_reviewer(db: &db::SqliteDb, task_id: &str, user_id: &str) {
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO task_role_assignment
         (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
         VALUES (?, ?, 'reviewer', 'user', ?, ?, ?)",
    )
    .bind(db::new_uuid_v4())
    .bind(task_id)
    .bind(user_id)
    .bind(now.clone())
    .bind(now)
    .execute(db.pool())
    .await
    .expect("legacy Human reviewer projection fixture writes");
}

fn human_review_request(
    summary: &str,
    evidence_ids: Vec<String>,
    artifact_ids: Vec<String>,
) -> api_types::SubmitReviewReportRequest {
    api_types::SubmitReviewReportRequest {
        verdict: api_types::ReviewReportVerdict::Pass,
        summary: summary.to_owned(),
        criteria: vec!["correctness".to_owned()],
        findings: Vec::new(),
        questions: Vec::new(),
        evidence_ids,
        artifact_ids,
    }
}

fn run_workspace_git(path: &std::path::Path, args: &[&str]) -> String {
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

struct RoutingTestEventHandler;

#[async_trait::async_trait]
impl crate::daemon_transport::DaemonExecutionEventHandler for RoutingTestEventHandler {
    async fn handle_log(
        &self,
        _daemon_id: &str,
        _notification: api_types::ExecutionLogNotification,
    ) -> crate::Result<()> {
        Ok(())
    }

    async fn handle_terminal(
        &self,
        _daemon_id: &str,
        _notification: api_types::ExecutionTerminalNotification,
    ) -> crate::Result<()> {
        Ok(())
    }
}

fn routing_test_registry() -> Arc<crate::daemon_transport::DaemonConnectionRegistry> {
    Arc::new(crate::daemon_transport::DaemonConnectionRegistry::new(
        Arc::new(EventBus::new(16)),
        Arc::new(RoutingTestEventHandler),
    ))
}

async fn add_remote_executor_daemon(db: &db::SqliteDb, machine_id: &str) -> String {
    let now = now_rfc3339();
    let daemon_id = db::new_uuid_v4();
    db::DaemonRepo::upsert_by_machine_id(
        db,
        db::UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: machine_id.to_owned(),
            hostname: machine_id.to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: db::DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("additional daemon creates");
    db::DaemonRepo::update_report(
        db,
        db::UpdateDaemonReport {
            id: daemon_id.clone(),
            detected_clis_json: r#"[{"kind":"shell","availability":"authenticated"}]"#.to_owned(),
            labels_json: None,
            status: db::DaemonStatus::Online,
            last_report_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("additional daemon reports shell adapter");
    daemon_id
}

async fn change_agent_daemon(
    db: &db::SqliteDb,
    agent_id: &str,
    daemon_id: Option<&str>,
) -> db::Agent {
    let agent = AgentRepo::get_by_id(db, agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    AgentRepo::update(
        db,
        db::UpdateAgent {
            id: agent.id.clone(),
            expected_version: agent.version,
            name: None,
            description: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: None,
            config_json: None,
            daemon_id: Some(daemon_id.map(ToOwned::to_owned)),
            max_concurrent_tasks: None,
            heartbeat_interval_seconds: None,
            max_missed_heartbeats: None,
            status: None,
            last_heartbeat_at: None,
            is_default: None,
            paused: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Agent daemon binding updates")
}

fn register_routing_test_daemon(
    registry: &crate::daemon_transport::DaemonConnectionRegistry,
    daemon_id: &str,
) -> tokio::sync::mpsc::Receiver<api_types::DaemonFrame> {
    let (connection, outbound) =
        crate::daemon_transport::DaemonConnection::new(daemon_id.to_owned());
    registry.register(daemon_id.to_owned(), connection);
    outbound
}

async fn answer_routing_test_daemon(
    registry: Arc<crate::daemon_transport::DaemonConnectionRegistry>,
    daemon_id: String,
    mut outbound: tokio::sync::mpsc::Receiver<api_types::DaemonFrame>,
    wait: std::time::Duration,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut methods = Vec::new();
    loop {
        let frame = match tokio::time::timeout_at(deadline, outbound.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) | Err(_) => break,
        };
        let api_types::DaemonFrame::Request { id, method, params } = frame else {
            continue;
        };
        methods.push(method.clone());
        let result = match method.as_str() {
            api_types::METHOD_PROTOCOL_CAPABILITIES => serde_json::json!({
                "schema_version": 1,
                "features": [api_types::DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1]
            }),
            api_types::METHOD_EXECUTION_START => serde_json::json!({
                "execution_id": params["execution_id"],
                "accepted": true
            }),
            api_types::METHOD_EXECUTION_CANCEL => serde_json::json!({
                "execution_id": params["execution_id"],
                "cancelled": true
            }),
            _ => serde_json::json!({}),
        };
        registry.dispatch_incoming(&daemon_id, api_types::DaemonFrame::Response { id, result });
        if method == api_types::METHOD_EXECUTION_START
            || method == api_types::METHOD_EXECUTION_CANCEL
        {
            break;
        }
    }
    methods
}

#[tokio::test]
async fn start_execution_uses_frozen_daemon_after_agent_is_rebound() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let agent_at_admission = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let daemon_a = agent_at_admission
        .daemon_id
        .clone()
        .expect("initial daemon binding");
    let daemon_b = add_remote_executor_daemon(&db, "frozen-host-replacement").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let registry = routing_test_registry();
    let _outbound_a = register_routing_test_daemon(&registry, &daemon_a);
    let outbound_b = register_routing_test_daemon(&registry, &daemon_b);
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_daemon_connections(Arc::clone(&registry));

    let snapshot = crate::task_service::config::build_executor_config_snapshot(
        &db,
        &task,
        &agent_at_admission,
        None,
        None,
    )
    .await
    .expect("Execution snapshot builds")
    .expect("Agent snapshot exists");
    let snapshot_value: Value = serde_json::from_str(&snapshot).expect("snapshot parses");
    assert_eq!(snapshot_value["resolved_daemon_id"], daemon_a);
    let (execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("frozen host start".to_owned()),
        Some(snapshot),
    )
    .await;
    change_agent_daemon(&db, &agent_id, Some(&daemon_b)).await;
    registry.unregister(&daemon_a);
    let outbound_a = register_routing_test_daemon(&registry, &daemon_a);

    let responder_a = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_a.clone(),
        outbound_a,
        std::time::Duration::from_secs(2),
    ));
    let responder_b = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_b,
        outbound_b,
        std::time::Duration::from_millis(150),
    ));
    let result = service
        .start_execution(execution.id)
        .await
        .expect("Start dispatches through the admitted daemon");
    assert!(result.accepted);
    assert_eq!(
        responder_a.await.expect("daemon A responder joins"),
        vec![
            api_types::METHOD_PROTOCOL_CAPABILITIES,
            api_types::METHOD_EXECUTION_START
        ]
    );
    assert!(
        responder_b
            .await
            .expect("daemon B observer joins")
            .is_empty(),
        "mutable Agent daemon B received no Execution request"
    );
}

#[tokio::test]
async fn unpinned_start_fails_closed_when_frozen_daemon_disappears() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let daemon_a = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists")
        .daemon_id
        .expect("initial daemon binding");
    change_agent_daemon(&db, &agent_id, None).await;
    let daemon_b = add_remote_executor_daemon(&db, "unpinned-host-fallback").await;
    sqlx::query("UPDATE daemon SET created_at = '2000-01-01T00:00:00Z' WHERE id = ?")
        .bind(&daemon_a)
        .execute(db.pool())
        .await
        .expect("daemon A is first for admission");
    sqlx::query("UPDATE daemon SET created_at = '2001-01-01T00:00:00Z' WHERE id = ?")
        .bind(&daemon_b)
        .execute(db.pool())
        .await
        .expect("daemon B follows daemon A");
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let registry = routing_test_registry();
    let outbound_b = register_routing_test_daemon(&registry, &daemon_b);
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_daemon_connections(Arc::clone(&registry));
    let agent_at_admission = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("unpinned Agent loads")
        .expect("unpinned Agent exists");
    let snapshot = crate::task_service::config::build_executor_config_snapshot(
        &db,
        &task,
        &agent_at_admission,
        None,
        None,
    )
    .await
    .expect("Execution snapshot builds")
    .expect("Agent snapshot exists");
    let snapshot_value: Value = serde_json::from_str(&snapshot).expect("snapshot parses");
    assert_eq!(snapshot_value["resolved_daemon_id"], daemon_a);
    assert!(snapshot_value["agent_daemon_id"].is_null());
    let (execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("frozen unpinned host start".to_owned()),
        Some(snapshot),
    )
    .await;

    let daemon_a_row = db::DaemonRepo::get_by_id(&*db, &daemon_a)
        .await
        .expect("daemon A loads")
        .expect("daemon A exists");
    db::DaemonRepo::update_report(
        &*db,
        db::UpdateDaemonReport {
            id: daemon_a.clone(),
            detected_clis_json: daemon_a_row.detected_clis_json,
            labels_json: None,
            status: db::DaemonStatus::Offline,
            last_report_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("daemon A goes offline after admission");
    let current_agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("current Agent loads")
        .expect("current Agent exists");
    assert_eq!(
        crate::agent_service::resolve_daemon_for_agent(&db, &current_agent)
            .await
            .expect("current unpinned resolver now selects B")
            .id,
        daemon_b
    );

    let responder_b = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_b,
        outbound_b,
        std::time::Duration::from_millis(150),
    ));
    let error = service
        .start_execution(execution.id)
        .await
        .expect_err("unavailable frozen daemon fails closed");
    assert!(matches!(
        error,
        ServiceError::DaemonUnavailable { daemon_id } if daemon_id == daemon_a
    ));
    assert!(
        responder_b
            .await
            .expect("daemon B observer joins")
            .is_empty(),
        "daemon B was not selected as an implicit replacement"
    );
}

#[tokio::test]
async fn cancel_execution_uses_frozen_daemon_after_agent_is_rebound() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let daemon_a = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists")
        .daemon_id
        .expect("initial daemon binding");
    let daemon_b = add_remote_executor_daemon(&db, "cancel-host-replacement").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let registry = routing_test_registry();
    let outbound_a = register_routing_test_daemon(&registry, &daemon_a);
    let outbound_b = register_routing_test_daemon(&registry, &daemon_b);
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_daemon_connections(Arc::clone(&registry));
    let agent_at_admission = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let snapshot = crate::task_service::config::build_executor_config_snapshot(
        &db,
        &task,
        &agent_at_admission,
        None,
        None,
    )
    .await
    .expect("Execution snapshot builds")
    .expect("Agent snapshot exists");
    let (execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("frozen host cancel".to_owned()),
        Some(snapshot),
    )
    .await;
    change_agent_daemon(&db, &agent_id, Some(&daemon_b)).await;

    let responder_a = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_a,
        outbound_a,
        std::time::Duration::from_secs(2),
    ));
    let responder_b = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_b,
        outbound_b,
        std::time::Duration::from_millis(150),
    ));
    service
        .cancel_execution_with_provider(&execution, "test cancellation")
        .await
        .expect("Cancel is sent through the admitted daemon");
    assert_eq!(
        responder_a.await.expect("daemon A responder joins"),
        vec![api_types::METHOD_EXECUTION_CANCEL]
    );
    assert!(
        responder_b
            .await
            .expect("daemon B observer joins")
            .is_empty(),
        "mutable Agent daemon B received no Cancel request"
    );
}

async fn create_claimed_review_execution(
    db: &db::SqliteDb,
    event_bus: Arc<EventBus>,
    project_id: &str,
    repo_id: &str,
    agent_id: &str,
    workspace_root: &std::path::Path,
) -> (TaskService, db::Task, db::Execution, db::Workspace) {
    let service = TaskService::new(Arc::new(db.clone()), event_bus)
        .with_workspace_root(workspace_root.to_path_buf())
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let now = now_rfc3339();
    let task = TaskRepo::create(
        db,
        db::CreateTask {
            id: db::new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "Read-only exact Review fixture".to_owned(),
            description: Some("Review a dirty Workspace snapshot".to_owned()),
            task_type: "review".to_owned(),
            status: "in_progress".to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Review fixture Task creates");
    service
        .create_task_role(
            &task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer TaskRole creates");
    service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(agent_id.to_owned()))
        .await
        .expect("Agent joins the reviewer TaskRole");
    let (workspace, workspace_created_by_attempt) =
        crate::task_service::workspace::prepare_workspace_owned(
            db,
            workspace_root,
            &task,
            &task.id,
            None,
        )
        .await
        .expect("review Workspace prepares");
    let agent = AgentRepo::get_by_id(db, agent_id)
        .await
        .expect("review Agent loads")
        .expect("review Agent exists");
    let executor_config_snapshot_json =
        crate::task_service::config::build_executor_config_snapshot(db, &task, &agent, None, None)
            .await
            .expect("review executor snapshot builds");
    let now = now_rfc3339();
    let execution = service
        .create_running_execution(
            db::CreateExecution {
                id: db::new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(agent_id.to_owned()),
                actor_ref: Some(db::ActorRef::Agent(agent_id.to_owned())),
                purpose: Some(db::ExecutionPurpose::Review),
                harness_session_id: None,
                role: "reviewer".to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some("Review the exact Workspace subject".to_owned()),
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
            workspace_created_by_attempt,
        )
        .await
        .expect("running Review Execution and WorkspaceLease create");
    let workspace = db::WorkspaceRepo::get_by_id(db, &workspace.id)
        .await
        .expect("Workspace loads")
        .expect("Workspace exists");
    (service, task, execution, workspace)
}

async fn create_running_repository_execution(
    db: &db::SqliteDb,
    service: &TaskService,
    task: &db::Task,
    agent_id: &str,
    role: &str,
    purpose: db::ExecutionPurpose,
    workspace_root: &std::path::Path,
    summary: Option<String>,
    executor_config_snapshot_json: Option<String>,
) -> (db::Execution, db::Workspace) {
    if TaskRoleRepo::get_by_task_and_role(db, &task.id, role)
        .await
        .expect("TaskRole lookup succeeds")
        .is_none()
    {
        service
            .create_task_role(
                &task.id,
                role,
                CoordinationMode::Collaborative,
                "{}".to_owned(),
            )
            .await
            .expect("Execution TaskRole creates");
    }
    if crate::task_service::current_role_memberships_authoritative(db, &task.id, role)
        .await
        .expect("TaskRole memberships load")
        .is_some_and(|memberships| {
            crate::task_service::active_agent_membership(&memberships, agent_id).is_none()
        })
        || crate::task_service::current_role_memberships_authoritative(db, &task.id, role)
            .await
            .expect("TaskRole memberships reload")
            .is_none()
    {
        service
            .add_task_role_member(&task.id, role, ActorRef::Agent(agent_id.to_owned()))
            .await
            .expect("Agent joins the Execution TaskRole");
    }
    let (workspace, created_by_attempt) = crate::task_service::workspace::prepare_workspace_owned(
        db,
        workspace_root,
        task,
        &task.id,
        None,
    )
    .await
    .expect("Execution Workspace prepares");
    let now = now_rfc3339();
    let execution = service
        .create_running_execution(
            db::CreateExecution {
                id: db::new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(agent_id.to_owned()),
                actor_ref: Some(db::ActorRef::Agent(agent_id.to_owned())),
                purpose: Some(purpose),
                harness_session_id: None,
                role: role.to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary,
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
            created_by_attempt,
        )
        .await
        .expect("Running Execution receives its exact WorkspaceLease");
    (execution, workspace)
}

const REVIEW_EXECUTOR_RESULT: &str = "FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"The frozen workspace subject passes.\",\"criteria\":[\"correctness\"],\"findings\":[],\"questions\":[],\"evidence_considered\":[]}";

struct ReviewOutputExecutor {
    mutate_worktree: bool,
    commit_mutation: bool,
    remove_git_marker: bool,
}

struct NeverRunExecutor;

#[async_trait]
impl TaskExecutor for NeverRunExecutor {
    async fn execute(
        &self,
        _ctx: ExecutionContext,
    ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
        panic!("persisted ReviewReport recovery must not invoke the reviewer executor")
    }

    async fn cancel(
        &self,
        _execution_id: &str,
    ) -> std::result::Result<(), executors::ExecutorError> {
        Ok(())
    }
}

#[async_trait]
impl TaskExecutor for ReviewOutputExecutor {
    async fn execute(
        &self,
        ctx: ExecutionContext,
    ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
        assert!(executors::is_worktree_read_only(&ctx.agent_config));
        let worktree = std::path::Path::new(&ctx.worktree_path);
        if self.mutate_worktree {
            std::fs::write(worktree.join("README.md"), "reviewer mutation\n")
                .expect("reviewer modifies tracked file");
            std::fs::write(worktree.join("reviewer-created.tmp"), "reviewer file\n")
                .expect("reviewer creates an untracked file");
            if self.commit_mutation {
                run_workspace_git(worktree, &["config", "user.name", "Review Test"]);
                run_workspace_git(worktree, &["config", "user.email", "review@test.invalid"]);
                run_workspace_git(worktree, &["add", "README.md"]);
                run_workspace_git(worktree, &["commit", "-m", "reviewer mutation"]);
            }
        }
        if self.remove_git_marker {
            std::fs::remove_file(worktree.join(".git"))
                .expect("fake reviewer removes the linked worktree marker");
        }
        Ok(executors::ExecutionResult {
            status: ExecutionOutcome::Completed,
            assistant_output: Some(REVIEW_EXECUTOR_RESULT.to_owned()),
            summary: Some("Review completed".to_owned()),
            ..Default::default()
        })
    }

    async fn cancel(
        &self,
        _execution_id: &str,
    ) -> std::result::Result<(), executors::ExecutorError> {
        Ok(())
    }
}

fn make_dirty_review_subject(worktree: &std::path::Path) -> (String, String, String, String) {
    let head = run_workspace_git(worktree, &["rev-parse", "HEAD"]);
    std::fs::write(worktree.join("README.md"), "staged review subject\n")
        .expect("staged subject content writes");
    run_workspace_git(worktree, &["add", "README.md"]);
    std::fs::write(worktree.join("README.md"), "unstaged review subject\n")
        .expect("unstaged subject content writes");
    std::fs::write(
        worktree.join("user-subject.txt"),
        "pre-review untracked content\n",
    )
    .expect("pre-review untracked file writes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            worktree.join("README.md"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("pre-review tracked permissions set");
    }
    let staged_diff = run_workspace_git(worktree, &["diff", "--cached", "--binary"]);
    let tracked_diff = run_workspace_git(worktree, &["diff", "--binary", "HEAD", "--"]);
    let status = run_workspace_git(worktree, &["status", "--porcelain"]);
    (head, staged_diff, tracked_diff, status)
}

async fn assert_review_report_has_subject(
    db: &db::SqliteDb,
    execution_id: &str,
    expected_digest: &str,
) {
    let subject = db::ExecutionRepo::get_review_execution_subject(db, execution_id)
        .await
        .expect("frozen subject lookup succeeds")
        .expect("Review subject is persisted");
    assert_eq!(subject.workspace_snapshot_digest, expected_digest);
    let report = db::CollaborationRepo::get_execution_artifact_output(
        db,
        execution_id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("ReviewReport lookup succeeds")
    .expect("ReviewReport is materialized");
    let content: serde_json::Value = serde_json::from_str(
        report
            .content
            .as_deref()
            .expect("ReviewReport content is inline"),
    )
    .expect("ReviewReport JSON parses");
    assert_eq!(
        content["subject"]["workspace_snapshot_digest"].as_str(),
        Some(expected_digest)
    );
    assert_eq!(content["subject"]["workspace_id"], subject.workspace_id);
    assert_eq!(
        content["subject"]["head_commit_sha"],
        subject.head_commit_sha
    );
}

#[tokio::test]
async fn pr8_local_review_preserves_dirty_tracked_and_untracked_subject() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let workspace_root = TempDir::new().expect("Review workspace root creates");
    let (service, _task, execution, workspace) = create_claimed_review_execution(
        &db,
        Arc::clone(&event_bus),
        &project_id,
        &repo_id,
        &agent_id,
        workspace_root.path(),
    )
    .await;
    let worktree = std::path::Path::new(&workspace.worktree_path);
    let (head, staged_diff, tracked_diff, status) = make_dirty_review_subject(worktree);
    let snapshot_digest = crate::ValidationService::snapshot_digest(&workspace.worktree_path)
        .await
        .expect("pre-review snapshot digest computes");

    let completed = service
        .run_execution(
            execution.id.clone(),
            &ReviewOutputExecutor {
                mutate_worktree: false,
                commit_mutation: false,
                remove_git_marker: false,
            },
        )
        .await
        .expect("no-op local reviewer completes");

    assert_eq!(completed.status, ExecutionStatus::Completed);
    assert_eq!(git::get_current_sha(worktree).await.unwrap(), head);
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "unstaged review subject\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("user-subject.txt")).unwrap(),
        "pre-review untracked content\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(worktree.join("README.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert_eq!(
        run_workspace_git(worktree, &["diff", "--cached", "--binary"]),
        staged_diff
    );
    assert_eq!(
        run_workspace_git(worktree, &["diff", "--binary", "HEAD", "--"]),
        tracked_diff
    );
    assert_eq!(
        run_workspace_git(worktree, &["status", "--porcelain"]),
        status
    );
    assert_eq!(
        crate::ValidationService::snapshot_digest(&workspace.worktree_path)
            .await
            .expect("post-review snapshot digest computes"),
        snapshot_digest
    );
    assert_review_report_has_subject(&db, &execution.id, &snapshot_digest).await;
}

#[tokio::test]
async fn pr8_local_review_discards_reviewer_mutations_and_restores_exact_subject() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let workspace_root = TempDir::new().expect("Review workspace root creates");
    let (service, _task, execution, workspace) = create_claimed_review_execution(
        &db,
        Arc::clone(&event_bus),
        &project_id,
        &repo_id,
        &agent_id,
        workspace_root.path(),
    )
    .await;
    let worktree = std::path::Path::new(&workspace.worktree_path);
    let (head, staged_diff, tracked_diff, status) = make_dirty_review_subject(worktree);
    let snapshot_digest = crate::ValidationService::snapshot_digest(&workspace.worktree_path)
        .await
        .expect("pre-review snapshot digest computes");

    let completed = service
        .run_execution(
            execution.id.clone(),
            &ReviewOutputExecutor {
                mutate_worktree: true,
                commit_mutation: true,
                remove_git_marker: false,
            },
        )
        .await
        .expect("mutating reviewer result completes after exact restoration");

    assert_eq!(completed.status, ExecutionStatus::Completed);
    assert_eq!(git::get_current_sha(worktree).await.unwrap(), head);
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "unstaged review subject\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("user-subject.txt")).unwrap(),
        "pre-review untracked content\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(worktree.join("README.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!worktree.join("reviewer-created.tmp").exists());
    assert_eq!(
        run_workspace_git(worktree, &["diff", "--cached", "--binary"]),
        staged_diff
    );
    assert_eq!(
        run_workspace_git(worktree, &["diff", "--binary", "HEAD", "--"]),
        tracked_diff
    );
    assert_eq!(
        run_workspace_git(worktree, &["status", "--porcelain"]),
        status
    );
    assert_eq!(
        crate::ValidationService::snapshot_digest(&workspace.worktree_path)
            .await
            .expect("post-review snapshot digest computes"),
        snapshot_digest
    );
    assert_review_report_has_subject(&db, &execution.id, &snapshot_digest).await;
}

#[tokio::test]
async fn pr8_local_review_restore_failure_fails_execution_without_report() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let workspace_root = TempDir::new().expect("Review workspace root creates");
    let (service, _task, execution, workspace) = create_claimed_review_execution(
        &db,
        Arc::clone(&event_bus),
        &project_id,
        &repo_id,
        &agent_id,
        workspace_root.path(),
    )
    .await;
    let worktree = std::path::Path::new(&workspace.worktree_path);
    let _ = make_dirty_review_subject(worktree);

    let failed = service
        .run_execution(
            execution.id.clone(),
            &ReviewOutputExecutor {
                mutate_worktree: false,
                commit_mutation: false,
                remove_git_marker: true,
            },
        )
        .await
        .expect("restore failure terminalizes the Review Execution");

    assert_eq!(failed.status, ExecutionStatus::Failed);
    let error = failed.error.expect("restore diagnostic is persisted");
    assert!(error.contains("pre-review state backup retained at"));
    let backup_path = error
        .split("pre-review state backup retained at ")
        .nth(1)
        .expect("diagnostic names isolated backup path");
    let backup_path = std::path::Path::new(backup_path);
    assert!(backup_path.join("index").exists());
    assert!(backup_path.join("tracked.patch").exists());
    assert!(backup_path
        .join("untracked")
        .join("user-subject.txt")
        .exists());
    assert!(db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("ReviewReport lookup succeeds")
    .is_none());
    std::fs::remove_dir_all(backup_path).expect("diagnostic snapshot is cleaned after assertions");
}

fn create_workspace_checkout(
    source: &std::path::Path,
    destination: &std::path::Path,
    file_name: &str,
    content: &str,
) {
    std::fs::create_dir_all(destination.parent().expect("workspace parent exists"))
        .expect("workspace parent creates");
    let output = std::process::Command::new("git")
        .args(["clone", "--no-hardlinks"])
        .arg(source)
        .arg(destination)
        .output()
        .expect("git clone runs");
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    run_workspace_git(destination, &["checkout", "-b", "review-fixture"]);
    run_workspace_git(destination, &["config", "user.email", "test@forge.dev"]);
    run_workspace_git(destination, &["config", "user.name", "Forge Test"]);
    std::fs::write(destination.join(file_name), content).expect("workspace change writes");
    run_workspace_git(destination, &["add", file_name]);
    run_workspace_git(destination, &["commit", "-m", "workspace review fixture"]);
}

async fn create_workspace_record(
    db: &db::SqliteDb,
    task: &db::Task,
    repo_id: &str,
    worktree_path: &std::path::Path,
) -> db::Workspace {
    let now = now_rfc3339();
    db::WorkspaceRepo::create(
        db,
        db::CreateWorkspace {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            repo_id: repo_id.to_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: run_workspace_git(worktree_path, &["branch", "--show-current"]),
            status: db::WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("exact Workspace records")
}

async fn create_remote_execution_fixture(
    db: &db::SqliteDb,
    task_id: &str,
    agent_id: &str,
    purpose: db::ExecutionPurpose,
) -> Execution {
    let now = now_rfc3339();
    db::ExecutionRepo::create(
        db,
        db::CreateExecution {
            id: db::new_uuid_v4(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.to_owned())),
            purpose: Some(purpose.clone()),
            harness_session_id: None,
            role: if purpose == db::ExecutionPurpose::Plan {
                "planner".to_owned()
            } else {
                "coder".to_owned()
            },
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
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
    .expect("remote Execution fixture creates")
}

#[tokio::test]
async fn pr8_agent_review_execution_materializes_one_exact_review_report() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let execution = db::ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            role: "reviewer".to_owned(),
            purpose: Some(db::ExecutionPurpose::Review),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            harness_session_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: Some("aaaaaaa000000000000000000000000000000000".to_owned()),
            after_sha: Some("bbbbbbb000000000000000000000000000000000".to_owned()),
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent reviewer Execution creates");
    assert_eq!(
        execution.actor_ref(),
        Some(db::ActorRef::Agent(agent_id.clone()))
    );

    let service = crate::CollaborationService::new(Arc::clone(&db), event_bus);
    let output = "Review notes.\nFORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"The exact subject is sound.\",\"criteria\":[\"correctness\"],\"findings\":[],\"questions\":[],\"evidence_considered\":[]}";
    let report = service
        .create_review_report_from_execution(&execution.id, output)
        .await
        .expect("structured result becomes a ReviewReport Artifact");
    let retry = service
        .create_review_report_from_execution(&execution.id, output)
        .await
        .expect("identical output retry reuses its Artifact");
    assert_eq!(report.id, retry.id);
    assert!(matches!(
        &report.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Agent(id) }
            if execution_id == &execution.id && id == &agent_id
    ));
    let contradictory = output.replace("The exact subject is sound.", "A different conclusion.");
    assert!(service
        .create_review_report_from_execution(&execution.id, &contradictory)
        .await
        .is_err());
    assert_eq!(
        db::ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution lookup succeeds")
            .expect("Execution remains")
            .status,
        ExecutionStatus::Running
    );
}

#[tokio::test]
async fn pr8_human_review_uses_plural_active_memberships_over_legacy_projection() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (human_a, membership_a) = add_human_reviewer(&db, &service, &task).await;
    let (human_b, membership_b) = add_human_reviewer(&db, &service, &task).await;

    assert_eq!(membership_a.status, db::RoleMembershipStatus::Active);
    assert_eq!(membership_b.status, db::RoleMembershipStatus::Active);
    let projected_user: Option<String> = sqlx::query_scalar(
        "SELECT assignee_id FROM task_role_assignment
         WHERE task_id = ? AND role_name = 'reviewer'",
    )
    .bind(&task.id)
    .fetch_optional(db.pool())
    .await
    .expect("legacy projection lookup succeeds");
    assert_eq!(projected_user.as_deref(), Some(human_a.as_str()));

    sqlx::query("DELETE FROM task_role_assignment WHERE task_id = ? AND role_name = 'reviewer'")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("legacy projection is absent for the membership-only check");
    let execution_a = service
        .start_human_review_execution(&task.id, &human_a, None)
        .await
        .expect("active Human starts without any singleton row");
    assert_eq!(
        execution_a.actor_ref(),
        Some(db::ActorRef::Human(human_a.clone()))
    );
    assert_eq!(execution_a.role, "reviewer");
    assert_eq!(execution_a.purpose, Some(db::ExecutionPurpose::Review));
    assert!(execution_a.workspace_id.is_none());
    assert!(
        db::ExecutionRepo::get_review_execution_subject(&*db, &execution_a.id)
            .await
            .expect("workspace-free Review subject lookup succeeds")
            .is_none()
    );

    assign_legacy_human_reviewer(&db, &task.id, &human_a).await;
    let execution_b = service
        .start_human_review_execution(&task.id, &human_b, None)
        .await
        .expect("singleton projection to A does not block active member B");
    assert_eq!(execution_b.actor_ref(), Some(db::ActorRef::Human(human_b)));
    assert_eq!(execution_b.role, "reviewer");
    assert_eq!(execution_b.purpose, Some(db::ExecutionPurpose::Review));
    let reused_a = service
        .start_human_review_execution(&task.id, &human_a, None)
        .await
        .expect("exact running Human review lookup reuses A's Execution");
    assert_eq!(reused_a.id, execution_a.id);
}

#[tokio::test]
async fn pr8_suspended_and_ended_humans_cannot_start_review() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (suspended_user, suspended_membership) = add_human_reviewer(&db, &service, &task).await;
    let (ended_user, ended_membership) = add_human_reviewer(&db, &service, &task).await;
    service
        .update_task_role_member(
            &task.id,
            &suspended_membership.id,
            suspended_membership.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("Human membership suspends");
    service
        .update_task_role_member(
            &task.id,
            &ended_membership.id,
            ended_membership.version,
            db::RoleMembershipStatus::Ended,
        )
        .await
        .expect("Human membership ends");

    for user_id in [suspended_user, ended_user] {
        let error = service
            .start_human_review_execution(&task.id, &user_id, None)
            .await
            .expect_err("inactive Human cannot start a fresh Review Execution");
        assert!(matches!(error, ServiceError::AuthorizationDenied { .. }));
    }
}

#[tokio::test]
async fn pr8_human_review_uses_legacy_only_without_replacement_task_role() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let user_id = seed_human_user(&db).await;
    let replacement_task =
        seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .create_task_role(
            &replacement_task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("replacement reviewer TaskRole exists with no members");
    assign_legacy_human_reviewer(&db, &replacement_task.id, &user_id).await;
    let denied = service
        .start_human_review_execution(&replacement_task.id, &user_id, None)
        .await
        .expect_err("contradictory singleton cannot grant replacement-role authority");
    assert!(matches!(denied, ServiceError::AuthorizationDenied { .. }));

    let legacy_task =
        seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    assign_legacy_human_reviewer(&db, &legacy_task.id, &user_id).await;
    let execution = service
        .start_human_review_execution(&legacy_task.id, &user_id, None)
        .await
        .expect("bounded singleton fallback works before replacement TaskRole exists");
    assert_eq!(execution.actor_ref(), Some(db::ActorRef::Human(user_id)));
}

#[tokio::test]
async fn pr8_completed_human_report_retry_survives_later_membership_change() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (user_id, membership) = add_human_reviewer(&db, &service, &task).await;
    let execution = service
        .start_human_review_execution(&task.id, &user_id, None)
        .await
        .expect("active Human starts Review Execution");
    let request = human_review_request("Exact report", Vec::new(), Vec::new());
    let (_, report) = service
        .submit_human_review_report(&execution.id, &user_id, request)
        .await
        .expect("Human submits exact ReviewReport");
    service
        .update_task_role_member(
            &task.id,
            &membership.id,
            membership.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("membership changes after the report completes");
    let (_, retry) = service
        .submit_human_review_report(
            &execution.id,
            &user_id,
            human_review_request("Exact report", Vec::new(), Vec::new()),
        )
        .await
        .expect("same completed report replays without current membership authority");
    assert_eq!(retry.id, report.id);
}

#[tokio::test]
async fn pr8_running_human_report_reconciles_after_membership_and_workspace_change() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)))
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (user_id, membership) = add_human_reviewer(&db, &service, &task).await;
    let workspace_root = TempDir::new().expect("Human Review workspace root creates");
    let worktree = workspace_root.path().join("worktree");
    create_workspace_checkout(
        repo_dir.path(),
        &worktree,
        "review-subject.txt",
        "before report\n",
    );
    let workspace = create_workspace_record(&db, &task, &repo_id, &worktree).await;
    let execution = service
        .start_human_review_execution(&task.id, &user_id, Some(&workspace.id))
        .await
        .expect("Human starts an exact workspace-bound Review");
    let frozen_subject = db::ExecutionRepo::get_review_execution_subject(&*db, &execution.id)
        .await
        .expect("frozen subject lookup succeeds")
        .expect("Human Review subject is frozen at start");
    let request = human_review_request("Exact report", Vec::new(), Vec::new());
    let human_output = format!(
        "FORGE_RESULT: {}",
        json!({
            "schema_version": 1,
            "kind": "review",
            "verdict": "pass",
            "summary": "Exact report",
            "criteria": ["correctness"],
            "findings": [],
            "questions": [],
            "evidence_considered": [],
        })
    );
    let report = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&service.event_bus))
        .create_human_review_report_from_execution(
            &execution.id,
            &human_output,
            Vec::new(),
            Vec::new(),
        )
        .await
        .expect("ReviewReport commits while Execution remains Running");
    assert_eq!(
        db::ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    service
        .update_task_role_member(
            &task.id,
            &membership.id,
            membership.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("membership is revoked after the report commit");
    std::fs::write(
        worktree.join("review-subject.txt"),
        "workspace changed later\n",
    )
    .expect("Workspace changes after the historical report commit");
    run_workspace_git(&worktree, &["add", "review-subject.txt"]);
    run_workspace_git(&worktree, &["commit", "-m", "later workspace change"]);

    let (completed, retried_report) = service
        .submit_human_review_report(&execution.id, &user_id, request)
        .await
        .expect("exact historical report reconciles without current authority or Workspace");
    assert_eq!(completed.status, ExecutionStatus::Completed);
    assert_eq!(retried_report.id, report.id);
    assert_eq!(retried_report.digest, report.digest);
    assert_eq!(
        db::ExecutionRepo::get_review_execution_subject(&*db, &execution.id)
            .await
            .unwrap()
            .unwrap(),
        frozen_subject
    );
    let output_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_artifact_output
         WHERE execution_id = ? AND kind = 'review_report'",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("ReviewReport output count reads");
    assert_eq!(output_count, 1);
    let artifact_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'artifact.created' AND entity_id = ?",
    )
    .bind(&report.id)
    .fetch_one(db.pool())
    .await
    .expect("ReviewReport event count reads");
    assert_eq!(artifact_event_count, 1);
}

#[tokio::test]
async fn pr8_conflicting_human_report_retry_fails_without_replacing_artifact() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (user_id, _membership) = add_human_reviewer(&db, &service, &task).await;
    let execution = service
        .start_human_review_execution(&task.id, &user_id, None)
        .await
        .expect("Human starts workspace-free Review");
    let original_request = human_review_request("Exact report", Vec::new(), Vec::new());
    let output = format!(
        "FORGE_RESULT: {}",
        json!({
            "schema_version": 1,
            "kind": "review",
            "verdict": "pass",
            "summary": "Exact report",
            "criteria": ["correctness"],
            "findings": [],
            "questions": [],
            "evidence_considered": [],
        })
    );
    let report = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&service.event_bus))
        .create_human_review_report_from_execution(&execution.id, &output, Vec::new(), Vec::new())
        .await
        .expect("original PASS report is durable before terminalization");
    let mut conflict = original_request;
    conflict.verdict = api_types::ReviewReportVerdict::RequestChanges;
    let error = service
        .submit_human_review_report(&execution.id, &user_id, conflict)
        .await
        .expect_err("conflicting verdict cannot replace an existing ReviewReport");
    assert!(matches!(error, ServiceError::Db(db::DbError::Check(_))));
    let still_running = db::ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_running.status, ExecutionStatus::Running);
    let existing = CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(existing.id, report.id);
    assert_eq!(existing.digest, report.digest);
    let output_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_artifact_output
         WHERE execution_id = ? AND kind = 'review_report'",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(output_count, 1);
}

#[tokio::test]
async fn pr8_running_agent_report_reconciles_without_relaunch_after_workspace_change() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let workspace_root = TempDir::new().expect("Review workspace root creates");
    let (service, _task, execution, workspace) = create_claimed_review_execution(
        &db,
        Arc::clone(&event_bus),
        &project_id,
        &repo_id,
        &agent_id,
        workspace_root.path(),
    )
    .await;
    let execution = service
        .freeze_review_subject_and_inputs(execution)
        .await
        .expect("Agent Review subject freezes before cognitive output");
    let report = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .create_review_report_from_execution(&execution.id, REVIEW_EXECUTOR_RESULT)
        .await
        .expect("Agent ReviewReport commits while Execution remains Running");
    let worktree = std::path::Path::new(&workspace.worktree_path);
    std::fs::write(worktree.join("README.md"), "changed after report\n")
        .expect("Workspace changes after the historical report commit");
    run_workspace_git(worktree, &["add", "README.md"]);
    run_workspace_git(worktree, &["commit", "-m", "later workspace change"]);

    let completed = service
        .run_execution(execution.id.clone(), &NeverRunExecutor)
        .await
        .expect("exact historical Agent report reconciles without a new reviewer run");
    assert_eq!(completed.status, ExecutionStatus::Completed);
    let persisted = CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(persisted.id, report.id);
    assert_eq!(persisted.digest, report.digest);
    let output_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_artifact_output
         WHERE execution_id = ? AND kind = 'review_report'",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(output_count, 1);
}

#[tokio::test]
async fn pr8_review_subject_uses_exact_workunit_workspace_and_matching_evidence() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .create_task_role(
            &task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer TaskRole creates");
    service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("Agent joins reviewer TaskRole");

    let workspaces_root = TempDir::new().expect("workspace root creates");
    let integration_path = workspaces_root.path().join("integration");
    create_workspace_checkout(
        repo_dir.path(),
        &integration_path,
        "integration-change.txt",
        "integration head\n",
    );
    let integration = create_workspace_record(&db, &task, &repo_id, &integration_path).await;

    let now = now_rfc3339();
    let work_unit_id = db::new_uuid_v4();
    db::WorkUnitRepo::create(
        &*db,
        db::CreateWorkUnit {
            id: work_unit_id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Review isolated workspace".to_owned(),
            scope: "Exact Review subject fixture".to_owned(),
            role: "reviewer".to_owned(),
            assigned_actor: Some(db::ActorRef::Agent(agent_id.clone())),
            requires_integration: true,
            provenance: None,
            created_by: db::ActorRef::Agent(agent_id.clone()),
            created_at: now.clone(),
        },
        db::CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: work_unit_id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent_id.clone()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: work_unit_id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("work-unit-created:{work_unit_id}")),
            payload_json: json!({ "work_unit_id": work_unit_id, "task_id": task.id }).to_string(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("WorkUnit creates");
    let work_unit_path = workspaces_root.path().join("work-unit");
    create_workspace_checkout(
        repo_dir.path(),
        &work_unit_path,
        "work-unit-change.txt",
        "work-unit head\n",
    );
    let work_unit_workspace = db::WorkUnitWorkspaceRepo::create_for_work_unit(
        &*db,
        db::CreateWorkUnitWorkspace {
            workspace: db::CreateWorkspace {
                id: db::new_uuid_v4(),
                task_id: task.id.clone(),
                repo_id: repo_id.clone(),
                worktree_path: work_unit_path.to_string_lossy().into_owned(),
                branch: run_workspace_git(&work_unit_path, &["branch", "--show-current"]),
                status: db::WorkspaceStatus::Ready,
                before_sha: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            work_unit_id,
        },
    )
    .await
    .expect("WorkUnit Workspace creates");

    let validation = crate::ValidationService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let integration_check = validation
        .run_command(
            &task.id,
            &integration.id,
            "printf integration",
            0,
            None,
            None,
        )
        .await
        .expect("integration Workspace ValidationRun finishes");
    let work_unit_check = validation
        .run_command(
            &task.id,
            &work_unit_workspace.id,
            "printf work-unit",
            0,
            None,
            None,
        )
        .await
        .expect("WorkUnit Workspace ValidationRun finishes");
    let integration_diff = crate::DiffService::new(Arc::clone(&db))
        .task_diff(&task.id)
        .await
        .expect("Task diff resolves its canonical integration Workspace");
    let work_unit_diff = crate::DiffService::new(Arc::clone(&db))
        .workspace_diff(&work_unit_workspace.id)
        .await
        .expect("exact WorkUnit diff resolves");
    assert_ne!(integration_diff.head_sha, work_unit_diff.head_sha);
    assert_ne!(integration_diff.diff, work_unit_diff.diff);

    let now = now_rfc3339();
    let execution = db::ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            role: "reviewer".to_owned(),
            purpose: Some(db::ExecutionPurpose::Review),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: Some(work_unit_workspace.id.clone()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Agent Review Execution uses the exact WorkUnit Workspace");
    let frozen = service
        .freeze_review_subject_and_inputs(execution)
        .await
        .expect("Review subject freezes against its exact workspace_id");
    let subject = db::ExecutionRepo::get_review_execution_subject(&*db, &frozen.id)
        .await
        .expect("durable Review subject lookup succeeds")
        .expect("durable Review subject persists");
    assert_eq!(subject.workspace_id, work_unit_workspace.id);
    assert_eq!(subject.base_commit_sha, work_unit_diff.base_sha);
    assert_eq!(subject.head_commit_sha, work_unit_diff.head_sha);
    assert_eq!(
        subject.workspace_snapshot_digest,
        work_unit_check.run.workspace_snapshot_digest
    );
    assert_ne!(subject.head_commit_sha, integration_diff.head_sha);
    let evidence_inputs = db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &frozen.id)
        .await
        .expect("Review Evidence inputs load");
    assert_eq!(evidence_inputs.len(), work_unit_check.evidence.len());
    assert!(evidence_inputs
        .iter()
        .all(|input| input.evidence_id == work_unit_check.evidence[0].id));
    assert!(evidence_inputs
        .iter()
        .all(|input| input.evidence_id != integration_check.evidence[0].id));

    std::fs::write(
        work_unit_path.join("uncommitted-after-freeze.txt"),
        "changed\n",
    )
    .expect("post-freeze Workspace change writes");
    let output = "FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"The exact WorkUnit subject is sound.\",\"criteria\":[\"correctness\"],\"findings\":[],\"questions\":[],\"evidence_considered\":[]}";
    assert!(crate::CollaborationService::new(Arc::clone(&db), event_bus)
        .create_review_report_from_execution(&frozen.id, output)
        .await
        .is_err());
    assert!(db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &frozen.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("ReviewReport lookup succeeds")
    .is_none());
}

#[tokio::test]
async fn pr8_human_review_subject_change_rejects_report_without_pins() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (user_id, _) = add_human_reviewer(&db, &service, &task).await;
    let workspace_root = TempDir::new().expect("Human review workspace root creates");
    let worktree_path = workspace_root.path().join("human-review");
    create_workspace_checkout(
        repo_dir.path(),
        &worktree_path,
        "reviewed-commit.txt",
        "initial reviewed commit\n",
    );
    let workspace = create_workspace_record(&db, &task, &repo_id, &worktree_path).await;
    let execution = service
        .start_human_review_execution(&task.id, &user_id, Some(&workspace.id))
        .await
        .expect("Human Review freezes its exact Workspace subject at start");
    let subject = db::ExecutionRepo::get_review_execution_subject(&*db, &execution.id)
        .await
        .expect("Review subject lookup succeeds")
        .expect("Human Review subject persists");
    assert_eq!(subject.workspace_id, workspace.id);
    assert_eq!(
        subject.base_commit_sha,
        execution.before_sha.clone().unwrap()
    );
    assert_eq!(
        subject.head_commit_sha,
        execution.after_sha.clone().unwrap()
    );
    assert_eq!(subject.workspace_snapshot_digest.len(), 64);

    std::fs::write(
        worktree_path.join("after-review-start.txt"),
        "new content\n",
    )
    .expect("post-start working tree change writes");
    assert!(service
        .submit_human_review_report(
            &execution.id,
            &user_id,
            human_review_request("Stale subject", Vec::new(), Vec::new()),
        )
        .await
        .is_err());
    assert_eq!(
        db::ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("Execution lookup succeeds")
            .expect("Execution remains")
            .status,
        ExecutionStatus::Running
    );
    assert!(db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("ReviewReport lookup succeeds")
    .is_none());
    assert!(
        db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &execution.id)
            .await
            .expect("Evidence input lookup succeeds")
            .is_empty()
    );
    assert!(
        db::CollaborationRepo::list_execution_artifact_inputs(&*db, &execution.id)
            .await
            .expect("Artifact input lookup succeeds")
            .is_empty()
    );
}

#[tokio::test]
async fn pr8_human_review_inputs_and_report_commit_atomically_and_retry_exactly() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(64));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (user_id, membership) = add_human_reviewer(&db, &service, &task).await;
    let workspace_root = TempDir::new().expect("Human review workspace root creates");
    let worktree_path = workspace_root.path().join("human-review-atomic");
    create_workspace_checkout(
        repo_dir.path(),
        &worktree_path,
        "reviewed-commit.txt",
        "reviewed subject\n",
    );
    let workspace = create_workspace_record(&db, &task, &repo_id, &worktree_path).await;
    let validation = crate::ValidationService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let check = validation
        .run_command(&task.id, &workspace.id, "printf pass", 0, None, None)
        .await
        .expect("exact ValidationRun creates Evidence");
    let plan_execution =
        create_remote_execution_fixture(&db, &task.id, &agent_id, db::ExecutionPurpose::Plan).await;
    let collaboration = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let artifact_a = collaboration
        .create_plan_artifact_from_execution(&plan_execution.id, "Exact pinned plan input")
        .await
        .expect("same-Task Plan Artifact input creates");
    let execution = service
        .start_human_review_execution(&task.id, &user_id, Some(&workspace.id))
        .await
        .expect("Human Review freezes exact Workspace subject");

    let artifact_events_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'artifact.created' AND scope_id = ?",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("pre-submit artifact event count reads");
    let missing_artifact = "missing-review-input-artifact".to_owned();
    for artifact_ids in [
        vec![missing_artifact.clone()],
        vec![artifact_a.id.clone(), missing_artifact.clone()],
    ] {
        assert!(service
            .submit_human_review_report(
                &execution.id,
                &user_id,
                human_review_request(
                    "Rejected before commit",
                    vec![check.evidence[0].id.clone()],
                    artifact_ids,
                ),
            )
            .await
            .is_err());
        assert!(
            db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &execution.id)
                .await
                .expect("failed Evidence pins read")
                .is_empty()
        );
        assert!(
            db::CollaborationRepo::list_execution_artifact_inputs(&*db, &execution.id)
                .await
                .expect("failed Artifact pins read")
                .is_empty()
        );
        assert!(db::CollaborationRepo::get_execution_artifact_output(
            &*db,
            &execution.id,
            db::ArtifactKind::ReviewReport,
        )
        .await
        .expect("failed ReviewReport lookup succeeds")
        .is_none());
    }
    assert!(service
        .submit_human_review_report(
            &execution.id,
            &user_id,
            human_review_request(
                "Duplicate reference",
                vec![check.evidence[0].id.clone(), check.evidence[0].id.clone()],
                Vec::new(),
            ),
        )
        .await
        .is_err());
    assert!(
        db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &execution.id)
            .await
            .expect("duplicate Evidence pins read")
            .is_empty()
    );
    assert!(
        db::CollaborationRepo::list_execution_artifact_inputs(&*db, &execution.id)
            .await
            .expect("duplicate Artifact pins read")
            .is_empty()
    );

    let request = human_review_request(
        "Exact report",
        vec![check.evidence[0].id.clone()],
        vec![artifact_a.id.clone()],
    );
    let (_, report) = service
        .submit_human_review_report(&execution.id, &user_id, request)
        .await
        .expect("valid exact inputs and report commit together");
    let report_content: serde_json::Value = serde_json::from_str(
        report
            .content
            .as_deref()
            .expect("ReviewReport stores inline content"),
    )
    .expect("ReviewReport content is valid JSON");
    assert_eq!(
        report_content["subject"]["workspace_snapshot_digest"].as_str(),
        Some(check.run.workspace_snapshot_digest.as_str())
    );
    let evidence_inputs =
        db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &execution.id)
            .await
            .expect("committed Evidence pins read");
    let artifact_inputs =
        db::CollaborationRepo::list_execution_artifact_inputs(&*db, &execution.id)
            .await
            .expect("committed Artifact pins read");
    assert_eq!(evidence_inputs.len(), 1);
    assert_eq!(artifact_inputs.len(), 1);
    assert_eq!(evidence_inputs[0].evidence_id, check.evidence[0].id);
    assert_eq!(artifact_inputs[0].artifact_id, artifact_a.id);
    let artifact_events_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'artifact.created' AND scope_id = ?",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("post-submit artifact event count reads");
    assert_eq!(artifact_events_after, artifact_events_before + 1);

    service
        .update_task_role_member(
            &task.id,
            &membership.id,
            membership.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("Human authority changes after completed report");
    let (_, retry) = service
        .submit_human_review_report(
            &execution.id,
            &user_id,
            human_review_request(
                "Exact report",
                vec![check.evidence[0].id.clone()],
                vec![artifact_a.id.clone()],
            ),
        )
        .await
        .expect("same completed report reuses its output after membership change");
    assert_eq!(retry.id, report.id);
    assert!(service
        .submit_human_review_report(
            &execution.id,
            &user_id,
            human_review_request(
                "Conflicting completed report",
                vec![check.evidence[0].id.clone()],
                vec![artifact_a.id.clone()],
            ),
        )
        .await
        .is_err());
    assert_eq!(
        db::ValidationRunRepo::list_execution_evidence_inputs(&*db, &execution.id)
            .await
            .expect("final Evidence pin set reads")
            .len(),
        1
    );
    assert_eq!(
        db::CollaborationRepo::list_execution_artifact_inputs(&*db, &execution.id)
            .await
            .expect("final Artifact pin set reads")
            .len(),
        1
    );
    let final_artifact_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'artifact.created' AND scope_id = ?",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("final artifact event count reads");
    assert_eq!(final_artifact_events, artifact_events_after);
}

fn completed_remote_notification(
    execution_id: &str,
    assistant_output: Option<&str>,
) -> api_types::ExecutionTerminalNotification {
    api_types::ExecutionTerminalNotification {
        execution_id: execution_id.to_owned(),
        exit_code: Some(0),
        signal: None,
        error: None,
        ts: now_rfc3339(),
        status: Some("completed".to_owned()),
        agent_session_id: None,
        summary: Some("short summary only".to_owned()),
        assistant_output: assistant_output.map(str::to_owned),
        after_sha: None,
        usage: None,
        account_usage: None,
        failure_class: None,
        retry_at: None,
        resolved_candidate: None,
        route_attempts: None,
    }
}

#[tokio::test]
async fn run_execution_dispatches_shell_adapter_and_updates_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "shell", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    sqlx::query("UPDATE task SET description = 'printf service-run-ok' WHERE id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("shell prompt is configured");
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("shell Agent loads")
        .expect("shell Agent exists");
    let executor_config_snapshot_json =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("normalized shell executor snapshot builds");
    let (execution, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("printf service-run-ok".to_owned()),
        executor_config_snapshot_json,
    )
    .await;

    let registry = Arc::new(cli_adapters::default_registry());
    let executor = executors::FallbackExecutor::new(registry);
    let execution = service
        .run_execution(execution.id, &executor)
        .await
        .expect("execution runs");

    assert_eq!(
        execution.status,
        ExecutionStatus::Completed,
        "{execution:#?}"
    );
    let snapshot: Value = serde_json::from_str(
        execution
            .executor_config_snapshot_json
            .as_deref()
            .expect("execution retains its immutable harness snapshot"),
    )
    .expect("harness snapshot is valid JSON");
    assert_eq!(snapshot["executor_type"], "shell");
    assert_eq!(
        snapshot["routing"]["selected_candidate_key"],
        executors::candidate_key(&executors::ExecutorKind::Shell, &snapshot["config"])
    );
    let logs_path = execution.logs_path.expect("logs path recorded");
    assert!(
        logs_path.contains(&format!(
            "/.forge/logs/{}/{}/",
            task.project_id, workspace.task_id
        )),
        "logs path should live under durable project/task log dir, got {logs_path}"
    );
    let logs = executors::LogReader::read(std::path::Path::new(&logs_path), 0, 100)
        .await
        .expect("logs read");
    assert!(logs.entries.iter().any(|entry| {
        entry.payload.get("line").and_then(|line| line.as_str()) == Some("service-run-ok")
    }));
}

#[derive(Default)]
struct CursorUsageRecordingExecutor {
    executions: std::sync::atomic::AtomicUsize,
    usage_observations: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl executors::TaskExecutor for CursorUsageRecordingExecutor {
    async fn execute(
        &self,
        _ctx: ExecutionContext,
    ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
        self.executions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(executors::ExecutionResult {
            status: ExecutionOutcome::Completed,
            ..Default::default()
        })
    }

    async fn cancel(
        &self,
        _execution_id: &str,
    ) -> std::result::Result<(), executors::ExecutorError> {
        Ok(())
    }

    async fn observe_usage(
        &self,
        _kind: executors::ExecutorKind,
        _config: &Value,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Option<executors::UsageObservation>, executors::ExecutorError> {
        self.usage_observations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(executors::UsageObservation {
            value: serde_json::json!({"plan": "fixture"}),
            source: Some("cursor_poll".to_owned()),
        }))
    }
}

#[tokio::test]
async fn run_cursor_execution_does_not_poll_or_persist_usage() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let executor = Arc::new(CursorUsageRecordingExecutor::default());
    let task_executor: Arc<dyn executors::TaskExecutor> = executor.clone();
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_task_executor(task_executor);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "cursor", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Cursor Agent loads")
        .expect("Cursor Agent exists");
    let snapshot =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("Cursor snapshot builds");
    let (execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("complete Cursor work".to_owned()),
        snapshot,
    )
    .await;
    let execution_id = execution.id.clone();

    let execution = service
        .run_execution(execution_id.clone(), executor.as_ref())
        .await
        .expect("Cursor execution completes");

    assert_eq!(execution.status, ExecutionStatus::Completed);
    assert_eq!(
        executor
            .executions
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        executor
            .usage_observations
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the service must not launch an Execution-owned Cursor usage helper"
    );
    let usage_snapshots: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_usage_snapshot WHERE execution_id = ?")
            .bind(execution_id)
            .fetch_one(db.pool())
            .await
            .expect("usage snapshot count reads");
    assert_eq!(
        usage_snapshots, 0,
        "no stale or fixture usage is persisted for this Execution"
    );
}

#[tokio::test]
async fn run_execution_emits_terminal_execution_event() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (running_execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("Emit terminal event".to_owned()),
        Some(r#"{"executor_type":"shell","config":{}}"#.to_owned()),
    )
    .await;
    let mut rx = event_bus.subscribe();

    let execution = service
        .run_execution(running_execution.id.clone(), &NoDiffExecutor)
        .await
        .expect("execution runs");

    assert_eq!(execution.status, ExecutionStatus::Completed);
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let event = rx.recv().await.expect("terminal event emits");
            if event.event_type == "execution.completed" {
                break event;
            }
        }
    })
    .await
    .expect("execution.completed event received");
    assert_eq!(event.entity_id, running_execution.id);
    match event.context {
        EventContext::ExecutionCompleted { task_id } => assert_eq!(task_id, task.id),
        other => panic!("unexpected event context: {other:?}"),
    }
}

#[tokio::test]
async fn run_execution_rejects_when_terminal_active_in_workspace() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let terminal_activity = Arc::new(TerminalActivityTracker::default());
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_terminal_activity_tracker(Arc::clone(&terminal_activity))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (running_execution, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("Terminal blocks execution".to_owned()),
        Some(r#"{"executor_type":"shell","config":{}}"#.to_owned()),
    )
    .await;
    assert!(terminal_activity.try_mark_active(&workspace.id).await);
    let executor = CountingExecutor::default();

    let error = service
        .run_execution(running_execution.id, &executor)
        .await
        .expect_err("active terminal rejects execution");

    assert!(matches!(
        error,
        ServiceError::TerminalActiveExecution { .. }
    ));
    assert_eq!(
        executor.calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "executor must not launch while a terminal is active"
    );
}

#[tokio::test]
async fn run_execution_batches_execution_log_events() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(128));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let (running_execution, _workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("Batch execution logs".to_owned()),
        Some(r#"{"executor_type":"shell","config":{}}"#.to_owned()),
    )
    .await;
    let mut rx = event_bus.subscribe();

    let execution = service
        .run_execution(
            running_execution.id.clone(),
            &BurstLogExecutor { count: 55 },
        )
        .await
        .expect("execution runs");

    assert_eq!(execution.status, ExecutionStatus::Completed);
    let log_events = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let mut events = Vec::new();
        while events.len() < 2 {
            let event = rx.recv().await.expect("execution log event emits");
            if event.event_type == "execution.log" {
                events.push(event);
            }
        }
        events
    })
    .await
    .expect("batched execution.log events received");

    assert_eq!(log_events.len(), 2);
    let mut total_logs = 0;
    let mut saw_multi_log_event = false;
    for event in log_events {
        assert_eq!(event.entity_id, running_execution.id);
        match event.context {
            EventContext::ExecutionLog { task_id, log, logs } => {
                assert_eq!(task_id, task.id);
                assert!(!log.is_null());
                let logs = logs.expect("batched logs included");
                saw_multi_log_event |= logs.len() > 1;
                total_logs += logs.len();
            }
            other => panic!("unexpected event context: {other:?}"),
        }
    }
    assert_eq!(total_logs, 55);
    assert!(saw_multi_log_event);
}

#[tokio::test]
async fn launch_execution_creates_interactive_execution_and_workspace() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = service
        .create_task(
            project_id,
            "Launch interactive",
            Some("printf launch-ok".to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("task creates");

    let launched = service
        .launch_execution(task.id.clone(), agent_id.clone(), None, None)
        .await
        .expect("interactive launch succeeds");

    assert_eq!(launched.task.status, "in_progress".to_owned());
    assert_eq!(launched.execution.role, "interactive".to_owned());
    assert_eq!(launched.execution.status, ExecutionStatus::Running);
    assert_eq!(
        launched.execution.agent_id.as_deref(),
        Some(agent_id.as_str())
    );
    assert_eq!(launched.workspace.task_id, task.id);
}

#[tokio::test]
async fn interactive_workspace_lease_uses_the_canonical_task_role_when_present() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let role_agent_id = seed_agent(&db).await;
    let stale_legacy_agent_id = seed_agent(&db).await;
    let task = service
        .create_task(
            project_id,
            "Interactive launch follows TaskRole",
            Some("printf role-authority".to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("task creates");
    service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("canonical implementer role creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(role_agent_id))
        .await
        .expect("canonical implementer member is active");
    sqlx::query(
        "UPDATE task
         SET assignee_type = 'agent', assignee_id = ?, version = version + 1
         WHERE id = ?",
    )
    .bind(&stale_legacy_agent_id)
    .bind(&task.id)
    .execute(db.pool())
    .await
    .expect("legacy assignee projection is made contradictory");

    let result = service
        .launch_execution(task.id.clone(), stale_legacy_agent_id, None, None)
        .await;

    let error = match result {
        Ok(_) => panic!("legacy assignee cannot bypass TaskRole"),
        Err(error) => error,
    };
    assert!(error
        .to_string()
        .contains("membership does not authorize this WorkspaceLease principal"));
    let failed_execution_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id = ? AND status = 'failed'",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("failed Execution count reads");
    let running_execution_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id = ? AND status = 'running'",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("running Execution count reads");
    let lease_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_lease WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("WorkspaceLease count reads");
    assert_eq!(failed_execution_count, 1);
    assert_eq!(running_execution_count, 0);
    assert_eq!(lease_count, 0);
}

#[tokio::test]
async fn dispatch_initial_role_execution_creates_execution_and_spawns() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_executor(Arc::new(NoDiffExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let execution = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::CODER,
            db::ExecutionPurpose::Implement,
            "implement the task".to_owned(),
        )
        .await
        .expect("initial role dispatch succeeds");

    assert_eq!(execution.role, crate::workflow::default_roles::CODER);
    assert_eq!(execution.status, ExecutionStatus::Running);
    assert_eq!(execution.agent_id.as_deref(), Some(agent_id.as_str()));
    assert_eq!(execution.summary.as_deref(), Some("implement the task"));

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let current = ExecutionRepo::get_by_id(&*db, &execution.id)
                .await
                .expect("execution loads")
                .expect("execution exists");
            if current.status == ExecutionStatus::Completed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("spawned execution completes");
}

#[tokio::test]
async fn plan_executions_create_immutable_artifacts_for_agent_and_human_authors() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_executor(Arc::new(PlanOutputExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let human_id = seed_human_user(&db).await;
    let now = now_rfc3339();
    ProjectMemberRepo::add_member(
        &*db,
        CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: human_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Human Project membership creates");
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::PLANNING.to_owned(),
    )
    .await;
    service
        .create_task_role(
            &task.id,
            crate::workflow::default_roles::PLANNER,
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("planner TaskRole creates");
    service
        .add_task_role_member(
            &task.id,
            crate::workflow::default_roles::PLANNER,
            ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("Agent planner membership creates");
    service
        .add_task_role_member(
            &task.id,
            crate::workflow::default_roles::PLANNER,
            ActorRef::Human(human_id.clone()),
        )
        .await
        .expect("Human planner membership creates");

    let execution = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::PLANNER,
            db::ExecutionPurpose::Plan,
            "plan the task".to_owned(),
        )
        .await
        .expect("Plan Execution dispatch succeeds");

    let completed = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let current = ExecutionRepo::get_by_id(&*db, &execution.id)
                .await
                .expect("execution loads")
                .expect("execution exists");
            if current.status == ExecutionStatus::Completed {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Plan Execution completes");
    assert_eq!(completed.purpose, Some(db::ExecutionPurpose::Plan));
    assert_eq!(
        completed.actor_ref(),
        Some(db::ActorRef::Agent(agent_id.clone()))
    );

    let first = CollaborationRepo::get_execution_artifact_output(
        &*db,
        &completed.id,
        db::ArtifactKind::Plan,
    )
    .await
    .expect("Plan output loads")
    .expect("Plan output exists");
    assert_eq!(first.task_id, task.id);
    assert!(matches!(
        &first.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Agent(id) }
            if execution_id == &completed.id && id == &agent_id
    ));
    assert_eq!(first.content.as_deref(), Some("- [x] verify plan\n"));

    let downstream = db::ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: db::new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            role: crate::workflow::default_roles::CODER.to_owned(),
            purpose: Some(db::ExecutionPurpose::Implement),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: Some(completed.id.clone()),
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("Implement from the selected plan".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("downstream Execution creates");
    let collaboration = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus));
    collaboration
        .pin_execution_artifact_input(&downstream.id, &first.id)
        .await
        .expect("downstream input pins exact Artifact");
    sqlx::query("UPDATE execution SET status = 'completed', updated_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(&downstream.id)
        .execute(db.pool())
        .await
        .expect("downstream fixture completes");

    let second_execution = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::PLANNER,
            db::ExecutionPurpose::Plan,
            "plan the task again".to_owned(),
        )
        .await
        .expect("second Plan Execution dispatches");
    let second_execution = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let current = ExecutionRepo::get_by_id(&*db, &second_execution.id)
                .await
                .expect("second Execution loads")
                .expect("second Execution exists");
            if current.status == ExecutionStatus::Completed {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("second Plan Execution completes");
    let second = CollaborationRepo::get_execution_artifact_output(
        &*db,
        &second_execution.id,
        db::ArtifactKind::Plan,
    )
    .await
    .expect("second Plan output loads")
    .expect("second Plan output exists");
    assert_ne!(first.id, second.id);
    assert_eq!(first.content.as_deref(), Some("- [x] verify plan\n"));
    assert!(matches!(
        &second.producer,
        db::ArtifactProducer::Execution { execution_id, .. }
            if execution_id == &second_execution.id
    ));

    let retry = collaboration
        .create_plan_artifact_from_execution(&completed.id, "- [x] verify plan\n")
        .await
        .expect("same Plan output retry succeeds");
    assert_eq!(retry.id, first.id);

    let pinned = CollaborationRepo::list_execution_artifact_inputs(&*db, &downstream.id)
        .await
        .expect("pinned inputs load");
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].artifact_id, first.id);
    assert_eq!(pinned[0].digest, first.digest);

    let human_execution = collaboration
        .start_human_plan_execution(&task.id, &human_id)
        .await
        .expect("Human Plan Execution starts");
    assert_eq!(human_execution.purpose, Some(db::ExecutionPurpose::Plan));
    assert_eq!(
        human_execution.actor_ref(),
        Some(db::ActorRef::Human(human_id.clone()))
    );
    assert!(human_execution.agent_id.is_none());
    assert!(human_execution.harness_session_id.is_none());
    let human_artifact = collaboration
        .complete_human_plan_execution(
            &human_execution.id,
            &human_id,
            "- [ ] Human-authored plan\n",
        )
        .await
        .expect("Human Plan Execution completes");
    assert_eq!(human_artifact.task_id, task.id);
    assert!(matches!(
        &human_artifact.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Human(id) }
            if execution_id == &human_execution.id && id == &human_id
    ));

    let artifact_count_before_lifecycle_observation: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = ? AND kind = 'plan'")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Plan Artifact count loads before lifecycle observation");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("Task lifecycle loads")
        .expect("Task lifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Active);
    let artifact_count_after_lifecycle_observation: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = ? AND kind = 'plan'")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Plan Artifact count loads after lifecycle observation");
    assert_eq!(
        artifact_count_after_lifecycle_observation, artifact_count_before_lifecycle_observation,
        "aggregate progress observes planning activity without rewriting its artifact"
    );

    let revision_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_plan_revision")
        .fetch_one(db.pool())
        .await
        .expect("legacy revisions count");
    let approval_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_plan_approval")
        .fetch_one(db.pool())
        .await
        .expect("legacy approvals count");
    assert_eq!(revision_count, 0);
    assert_eq!(approval_count, 0);
    let legacy_task_plan: Option<String> = sqlx::query_scalar("SELECT plan FROM task WHERE id = ?")
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("physical legacy Task.plan field remains");
    assert!(legacy_task_plan.is_none());
}

#[tokio::test]
async fn plan_artifact_output_is_idempotent_across_sqlite_connections() {
    let database_dir = TempDir::new().expect("database temp dir creates");
    let database_url = format!("sqlite://{}", database_dir.path().join("pr7.db").display());
    let first_pool = db::create_sqlite_pool(&database_url)
        .await
        .expect("first pool creates");
    db::run_migrations(&first_pool)
        .await
        .expect("migrations run");
    let first_db = Arc::new(db::SqliteDb::new(first_pool));
    let second_db = Arc::new(db::SqliteDb::new(
        db::create_sqlite_pool(&database_url)
            .await
            .expect("second pool creates"),
    ));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&first_db).await;
    let agent_id = seed_agent(&first_db).await;
    let task = seed_task_with_status(&first_db, &project_id, &repo_id, "planning".to_owned()).await;
    let now = now_rfc3339();
    let execution_id = db::new_uuid_v4();
    db::ExecutionRepo::create(
        &*first_db,
        db::CreateExecution {
            id: execution_id.clone(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Plan),
            harness_session_id: None,
            role: "planner".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("plan complete".to_owned()),
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
    .expect("Plan Execution creates");

    let first_service =
        crate::CollaborationService::new(Arc::clone(&first_db), Arc::new(EventBus::new(8)));
    let second_service =
        crate::CollaborationService::new(Arc::clone(&second_db), Arc::new(EventBus::new(8)));
    let markdown = "# Same exact plan\n";
    let (first, second) = tokio::join!(
        first_service.create_plan_artifact_from_execution(&execution_id, markdown),
        second_service.create_plan_artifact_from_execution(&execution_id, markdown),
    );
    let first = first.expect("first process materializes the output");
    let second = second.expect("second process observes the same output");
    assert_eq!(first.id, second.id);

    let output_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_artifact_output
         WHERE execution_id = ? AND kind = 'plan'",
    )
    .bind(&execution_id)
    .fetch_one(first_db.pool())
    .await
    .expect("one output binding remains");
    let artifact_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = ? AND kind = 'plan'")
            .bind(&task.id)
            .fetch_one(first_db.pool())
            .await
            .expect("plan Artifacts count");
    assert_eq!(output_count, 1);
    assert_eq!(artifact_count, 1);
}

#[tokio::test]
async fn execution_start_event_and_initial_artifact_inputs_commit_atomically() {
    let database_dir = TempDir::new().expect("database temp dir creates");
    let database_url = format!(
        "sqlite://{}",
        database_dir.path().join("start-inputs.db").display()
    );
    let first_pool = db::create_sqlite_pool(&database_url)
        .await
        .expect("first pool creates");
    db::run_migrations(&first_pool)
        .await
        .expect("migrations run");
    let first_db = Arc::new(db::SqliteDb::new(first_pool));
    let second_db = Arc::new(db::SqliteDb::new(
        db::create_sqlite_pool(&database_url)
            .await
            .expect("second pool creates"),
    ));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&first_db).await;
    let agent_id = seed_agent(&first_db).await;
    let task = seed_task_with_status(&first_db, &project_id, &repo_id, "planning".to_owned()).await;
    let producer_id = db::new_uuid_v4();
    db::ExecutionRepo::create(
        &*first_db,
        db::CreateExecution {
            id: producer_id.clone(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Plan),
            harness_session_id: None,
            role: "planner".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Plan producer Execution creates");
    let artifact =
        crate::CollaborationService::new(Arc::clone(&first_db), Arc::new(EventBus::new(8)))
            .create_plan_artifact_from_execution(&producer_id, "# Selected plan\n")
            .await
            .expect("Plan Artifact creates");

    let execution_id = db::new_uuid_v4();
    let input = db::CreateExecution {
        id: execution_id.clone(),
        task_id: task.id.clone(),
        agent_id: Some(agent_id.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
        purpose: Some(db::ExecutionPurpose::Implement),
        harness_session_id: None,
        role: "coder".to_owned(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: Some(producer_id),
        agent_session_id: None,
        agent_message_id: None,
        last_activity_at: None,
        summary: Some("uses selected plan".to_owned()),
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: None,
        workspace_id: None,
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    };
    let event = crate::task_service::execution_domain_event(&input, "execution.started");

    // Hold a read snapshot on another pool across the write. It sees neither
    // half until that transaction is released and a fresh snapshot begins.
    let mut observer = second_db.pool().begin().await.expect("observer begins");
    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&execution_id)
    .fetch_one(&mut *observer)
    .await
    .expect("observer establishes snapshot");
    assert_eq!(before, 0);

    db::ExecutionRepo::create_with_artifact_inputs_and_event(
        &*first_db,
        input,
        vec![artifact.id.clone()],
        event,
    )
    .await
    .expect("Execution, input, and event commit");

    let snapshot_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&execution_id)
    .fetch_one(&mut *observer)
    .await
    .expect("old snapshot remains readable");
    let snapshot_input_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_artifact_input WHERE execution_id = ?")
            .bind(&execution_id)
            .fetch_one(&mut *observer)
            .await
            .expect("old snapshot input count loads");
    assert_eq!(snapshot_event_count, 0);
    assert_eq!(snapshot_input_count, 0);
    observer.rollback().await.expect("old snapshot closes");

    let committed_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&execution_id)
    .fetch_one(second_db.pool())
    .await
    .expect("committed event loads from second pool");
    let committed_inputs =
        db::CollaborationRepo::list_execution_artifact_inputs(&*second_db, &execution_id)
            .await
            .expect("committed inputs load from second pool");
    assert_eq!(committed_event_count, 1);
    assert_eq!(committed_inputs.len(), 1);
    assert_eq!(committed_inputs[0].artifact_id, artifact.id);
    assert_eq!(committed_inputs[0].digest, artifact.digest);

    let failed_execution_id = db::new_uuid_v4();
    let failed_input = db::CreateExecution {
        id: failed_execution_id.clone(),
        task_id: task.id.clone(),
        agent_id: Some(agent_id.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_id)),
        purpose: Some(db::ExecutionPurpose::Implement),
        harness_session_id: None,
        role: "coder".to_owned(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: None,
        agent_session_id: None,
        agent_message_id: None,
        last_activity_at: None,
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: None,
        workspace_id: None,
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    };
    let failed_event =
        crate::task_service::execution_domain_event(&failed_input, "execution.started");
    let result = db::ExecutionRepo::create_with_artifact_inputs_and_event(
        &*first_db,
        failed_input,
        vec![artifact.id, "missing-artifact".to_owned()],
        failed_event,
    )
    .await;
    assert!(
        result.is_err(),
        "an invalid second input rejects the whole write"
    );
    assert!(
        db::ExecutionRepo::get_by_id(&*second_db, &failed_execution_id)
            .await
            .expect("failed Execution lookup succeeds")
            .is_none()
    );
    let failed_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&failed_execution_id)
    .fetch_one(second_db.pool())
    .await
    .expect("failed start event count loads");
    let failed_input_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_artifact_input WHERE execution_id = ?")
            .bind(&failed_execution_id)
            .fetch_one(second_db.pool())
            .await
            .expect("failed input count loads");
    assert_eq!(failed_event_count, 0);
    assert_eq!(failed_input_count, 0);
}

#[tokio::test]
async fn remote_plan_completion_materializes_output_before_terminal_state_and_reuses_on_retry() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "planning".to_owned()).await;
    let execution =
        create_remote_execution_fixture(&db, &task.id, &agent_id, db::ExecutionPurpose::Plan).await;
    let full_output = "# Complete plan\n- Inspect the migration\n- Verify recovery\n";
    let notification = completed_remote_notification(&execution.id, Some(full_output));

    let completed = service
        .complete_remote_execution(notification.clone(), None)
        .await
        .expect("remote Plan Execution completes with its full output");
    assert_eq!(completed.status, ExecutionStatus::Completed);
    assert_eq!(completed.summary.as_deref(), Some("short summary only"));
    let artifact = db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::Plan,
    )
    .await
    .expect("Plan output lookup succeeds")
    .expect("remote Plan output is materialized");
    assert_eq!(artifact.content.as_deref(), Some(full_output));
    assert!(matches!(
        &artifact.producer,
        db::ArtifactProducer::Execution { execution_id, actor: db::ActorRef::Agent(id) }
            if execution_id == &execution.id && id == &agent_id
    ));

    service
        .complete_remote_execution(notification, None)
        .await
        .expect("same remote terminal notification retries");
    let retry_artifact = db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::Plan,
    )
    .await
    .expect("retried Plan output lookup succeeds")
    .expect("retry reuses the output");
    assert_eq!(retry_artifact.id, artifact.id);
    let artifact_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'artifact.created' AND entity_id = ?",
    )
    .bind(&artifact.id)
    .fetch_one(db.pool())
    .await
    .expect("Artifact event count loads");
    let terminal_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.completed' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("terminal event count loads");
    assert_eq!(artifact_event_count, 1);
    assert_eq!(terminal_event_count, 1);
}

#[tokio::test]
async fn remote_plan_completion_requires_full_output_and_non_plan_completion_is_unchanged() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "planning".to_owned()).await;
    let plan_execution =
        create_remote_execution_fixture(&db, &task.id, &agent_id, db::ExecutionPurpose::Plan).await;

    let error = service
        .complete_remote_execution(
            completed_remote_notification(&plan_execution.id, None),
            None,
        )
        .await
        .expect_err("summary alone cannot complete a remote Plan Execution");
    assert!(error.to_string().contains("complete assistant result"));
    let still_running = db::ExecutionRepo::get_by_id(&*db, &plan_execution.id)
        .await
        .expect("Plan Execution lookup succeeds")
        .expect("Plan Execution remains persisted");
    assert_eq!(still_running.status, ExecutionStatus::Running);
    assert!(db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &plan_execution.id,
        db::ArtifactKind::Plan,
    )
    .await
    .expect("missing Plan output lookup succeeds")
    .is_none());

    let implementation =
        create_remote_execution_fixture(&db, &task.id, &agent_id, db::ExecutionPurpose::Implement)
            .await;
    let completed = service
        .complete_remote_execution(
            completed_remote_notification(&implementation.id, None),
            None,
        )
        .await
        .expect("non-Plan remote Execution still completes without full assistant output");
    assert_eq!(completed.status, ExecutionStatus::Completed);
}

#[tokio::test]
async fn role_dispatch_runs_required_before_work_hook_before_execution_creation() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_executor(Arc::new(NoDiffExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let settings = json!({
        "lifecycle_hooks": {
            "before_work": [{
                "type": "script",
                "command": "printf required-ok > required-hook.out; exit 0",
                "timeout_seconds": 5,
                "blocking": true
            }]
        }
    });
    sqlx::query("UPDATE project SET settings = ?, updated_at = ? WHERE id = ?")
        .bind(settings.to_string())
        .bind(now_rfc3339())
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project settings update");
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(
                &task.id,
                crate::workflow::default_roles::CODER,
                Some(agent_id.clone()),
                None,
            ),
            false,
            false,
        )
        .await
        .expect("coder role assignment succeeds");

    let launched = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::CODER,
            db::ExecutionPurpose::Implement,
            "run the required before-work hook".to_owned(),
        )
        .await
        .expect("required hook passes and role dispatch succeeds");

    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions list");
    assert_eq!(executions.items.len(), 1);
    assert_eq!(
        executions.items[0].role,
        crate::workflow::default_roles::CODER,
        "this dispatch is explicitly requested through the legacy coder role label"
    );
    assert_eq!(
        executions.items[0].agent_id.as_deref(),
        Some(agent_id.as_str())
    );
    assert_eq!(launched.id, executions.items[0].id);

    let workspace =
        WorkspaceRepo::get_by_id(&*db, executions.items[0].workspace_id.as_deref().unwrap())
            .await
            .expect("workspace loads")
            .expect("workspace exists");
    let marker = std::fs::read_to_string(
        std::path::Path::new(&workspace.worktree_path).join("required-hook.out"),
    )
    .expect("required hook marker exists");
    assert_eq!(marker, "required-ok");
}

#[tokio::test]
async fn role_dispatch_blocks_lifecycle_when_required_before_work_hook_fails() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_executor(Arc::new(NoDiffExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let settings = json!({
        "lifecycle_hooks": {
            "before_work": [{
                "type": "script",
                "command": "echo preflight-out; echo preflight-err >&2; exit 9",
                "timeout_seconds": 5,
                "blocking": true
            }]
        }
    });
    sqlx::query("UPDATE project SET settings = ?, updated_at = ? WHERE id = ?")
        .bind(settings.to_string())
        .bind(now_rfc3339())
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project settings update");
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(
                &task.id,
                crate::workflow::default_roles::CODER,
                Some(agent_id.clone()),
                None,
            ),
            false,
            false,
        )
        .await
        .expect("coder role assignment succeeds");

    let result = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::CODER,
            db::ExecutionPurpose::Implement,
            "run the required before-work hook".to_owned(),
        )
        .await;
    assert!(
        result.is_err(),
        "required hook failure rejects role dispatch"
    );
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions list");
    assert!(
        executions.items.is_empty(),
        "no execution should be created"
    );

    let blocked = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("task lifecycle lookup succeeds")
        .expect("task lifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
    let annotation: serde_json::Value = serde_json::from_str(
        blocked
            .error_annotation
            .as_deref()
            .expect("blocking annotation is recorded"),
    )
    .expect("annotation parses");
    assert_eq!(annotation["type"], "before_work_hook_failed");
    assert_eq!(annotation["artifact"]["kind"], "hook");
    assert_eq!(annotation["hook"]["exit_code"], 9);
    assert_eq!(annotation["hook"]["stdout"], "preflight-out\n");
    assert!(annotation["hook"]["stderr"]
        .as_str()
        .expect("hook stderr is captured")
        .contains("preflight-err\n"));
    let recovery_actions = annotation["recovery_actions"]
        .as_array()
        .expect("recovery actions array");
    assert!(recovery_actions.iter().any(|value| value == "retry_hook"));
    assert!(recovery_actions
        .iter()
        .any(|value| value == "update_workspace_and_retry_hook"));
    assert!(recovery_actions
        .iter()
        .any(|value| value == "skip_hook_once"));
    assert!(recovery_actions.iter().any(|value| value == "cancel_task"));
    let log_path = annotation["hook"]["log_path"]
        .as_str()
        .expect("hook log path recorded");
    assert!(
        std::path::Path::new(log_path).exists(),
        "hook log path should exist: {log_path}"
    );
}

#[tokio::test]
async fn retired_workflow_recovery_actions_cannot_run_hooks_or_dispatch() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let before = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle loads")
        .expect("lifecycle exists");

    for action in [
        api_types::RecoveryAction::RetryHook,
        api_types::RecoveryAction::UpdateWorkspaceAndRetryHook,
        api_types::RecoveryAction::SkipHookOnce,
    ] {
        let error = service
            .recover_task(task.id.clone(), action, None, None)
            .await
            .expect_err("legacy workflow hook recovery is retired");
        assert!(error
            .to_string()
            .contains("legacy workflow recovery actions are retired"));
    }

    let current = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task exists");
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads")
        .expect("lifecycle exists");
    assert_eq!(current.status, task.status);
    assert_eq!(lifecycle.state, before.state);
    assert_eq!(lifecycle.version, before.version);
    assert!(ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("execution list loads")
    .items
    .is_empty());
}

#[tokio::test]
async fn dispatch_initial_role_execution_runs_reviewer_when_agent_is_busy_on_same_task() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_executor(Arc::new(ReviewOutputExecutor {
            mutate_worktree: false,
            commit_mutation: false,
            remove_git_marker: false,
        }))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    sqlx::query("UPDATE task SET task_type = 'review' WHERE id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("fixture marks this Task as a read-only Review operation");

    TaskRoleAssignmentRepo::assign(
        &*db,
        role_assignment_input(
            &task.id,
            crate::workflow::default_roles::REVIEWER,
            Some(agent_id.clone()),
            None,
        ),
    )
    .await
    .expect("reviewer assignment created");

    let execution = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            crate::workflow::default_roles::REVIEWER,
            db::ExecutionPurpose::Review,
            "review the task".to_owned(),
        )
        .await
        .expect("reviewer dispatch succeeds");

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let current = ExecutionRepo::get_by_id(&*db, &execution.id)
                .await
                .expect("execution loads")
                .expect("execution exists");
            if current.status == ExecutionStatus::Completed {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reviewer execution completes");
}

#[tokio::test]
async fn failed_review_execution_does_not_mutate_legacy_review_or_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Review),
            harness_session_id: None,
            role: crate::workflow::default_roles::REVIEWER.to_owned(),
            status: ExecutionStatus::Failed,
            stop_reason: Some(db::StopReason::ExecutorFailed),
            stopped_by: Some("system:executor".to_owned()),
            resume_policy: Some(db::ResumePolicy::Manual),
            stopped_at: Some(now.clone()),
            parent_execution_id: None,
            // This historical fixture has no HarnessSession authority.
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("reviewer quit before verdict".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some("claude-code exited with status exit status: 1".to_owned()),
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("failed reviewer execution creates");
    ReviewRepo::create(
        &*db,
        db::CreateReview {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            execution_id: execution.id.clone(),
            attempt_number: 1,
            status: ReviewStatus::Running,
            step_results_json: json!({ "ci_steps": [] }).to_string(),
            started_at: now.clone(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("running review creates");

    let lifecycle_before = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle loads")
        .expect("lifecycle exists");
    service
        .maybe_cascade_executor_completion(&execution.id)
        .await
        .expect("legacy completion check is harmless");

    let reviews = ReviewRepo::list_by_task(&*db, &task.id)
        .await
        .expect("reviews load");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].status, ReviewStatus::Running);
    assert!(reviews[0].finished_at.is_none());
    let current = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(current.status, "in_progress");
    let lifecycle_after = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads")
        .expect("lifecycle exists");
    assert_eq!(lifecycle_after.state, lifecycle_before.state);
    assert_eq!(lifecycle_after.version, lifecycle_before.version);
    let current_execution = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("execution loads")
        .expect("execution exists");
    assert_eq!(
        current_execution.resume_policy,
        Some(db::ResumePolicy::Manual),
        "retry ownership is not assigned by the retired review cascade"
    );
}

#[tokio::test]
async fn legacy_review_approval_gate_does_not_authorize_a_decision_or_lifecycle_move() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let mut workflow = crate::workflow::default_workflow::default_workflow();
    workflow
        .states
        .iter_mut()
        .find(|state| state.name == crate::workflow::default_states::REVIEW)
        .and_then(|state| state.gate_config.as_mut())
        .expect("review gate config")
        .requires_user_approval = Some(true);
    sqlx::query("UPDATE project SET workflow_definition = ? WHERE id = ?")
        .bind(serde_json::to_string(&workflow).expect("workflow serializes"))
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project workflow updates");
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
                    actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
                    purpose: Some(db::ExecutionPurpose::Review),
                    harness_session_id: None,
            role: crate::workflow::default_roles::REVIEWER.to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            // This historical fixture has no HarnessSession authority.
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("Looks good.\nFORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass\",\"summary\":\"clear\",\"findings\":[],\"questions\":[]}".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("completed reviewer execution creates");
    ReviewRepo::create(
        &*db,
        db::CreateReview {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            execution_id: execution.id.clone(),
            attempt_number: 1,
            status: ReviewStatus::Running,
            step_results_json: json!({ "ci_steps": [] }).to_string(),
            started_at: now.clone(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("running review creates");

    let lifecycle_before = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle loads")
        .expect("lifecycle exists");
    service
        .maybe_cascade_executor_completion(&execution.id)
        .await
        .expect("legacy completion check is harmless");

    let reviews = ReviewRepo::list_by_task(&*db, &task.id)
        .await
        .expect("reviews load");
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].status, ReviewStatus::Running);
    assert!(reviews[0].finished_at.is_none());
    let current = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(current.status, "in_progress");
    let lifecycle_after = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads")
        .expect("lifecycle exists");
    assert_eq!(lifecycle_after.state, lifecycle_before.state);
    assert_eq!(lifecycle_after.version, lifecycle_before.version);
    assert!(db::GateRepo::list_active_gate_policies(&*db, &task.id)
        .await
        .expect("Gate policies load")
        .is_empty());
    let comments = TaskCommentRepo::list_comments(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Asc,
        },
    )
    .await
    .expect("comments list");
    assert!(comments.items.is_empty());
}

#[tokio::test]
async fn follow_up_execution_reuses_explicit_harness_session() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "claude_code", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let message = "Please continue with the remaining edge cases".to_owned();
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("parent execution".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"claude_code","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let parent_execution = ExecutionRepo::record_harness_session_result(
        &*db,
        &parent_execution.id,
        "test-session",
        &now_rfc3339(),
    )
    .await
    .expect("parent harness session activates");

    let result = service
        .follow_up_execution(parent_execution.id.clone(), message.clone(), None, None)
        .await
        .expect("follow-up succeeds");

    assert_eq!(
        result.execution.parent_execution_id.as_deref(),
        Some(parent_execution.id.as_str())
    );
    assert_eq!(result.execution.summary.as_deref(), Some(message.as_str()));
    assert_eq!(result.execution.role, "interactive".to_owned());
    assert_eq!(
        result.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        result.execution.agent_session_id.as_deref(),
        Some("test-session")
    );
    let snapshot: serde_json::Value = serde_json::from_str(
        result
            .execution
            .executor_config_snapshot_json
            .as_deref()
            .expect("snapshot exists"),
    )
    .expect("snapshot is valid json");
    assert_eq!(
        snapshot["dispatch"]["execution_policy"],
        "explicit_harness_session"
    );
    assert!(snapshot["config"]
        .get("resume_session_id")
        .is_none_or(serde_json::Value::is_null));
}

#[tokio::test]
async fn legacy_execution_session_id_never_authorizes_resume_or_actions() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("historical-shaped execution creates");
    sqlx::query("UPDATE execution SET agent_session_id = 'legacy-thread' WHERE id = ?")
        .bind(&execution.id)
        .execute(db.pool())
        .await
        .expect("legacy session projection is seeded");
    let historical = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("execution loads")
        .expect("execution exists");

    let legacy_only =
        crate::task_service::resumable_external_session(&db, &historical, Some(&agent_id), None)
            .await
            .expect("legacy projection lookup succeeds without becoming authority");
    assert_eq!(legacy_only, None);

    sqlx::query(
        "INSERT INTO execution_session_migration_issue
         (id, execution_id, issue_kind, details_json, created_at)
         VALUES (?, ?, 'historical_session_ambiguous', '{}', ?)",
    )
    .bind(new_uuid_v4())
    .bind(&execution.id)
    .bind(now_rfc3339())
    .execute(db.pool())
    .await
    .expect("historical ambiguity marker is inserted");

    let ambiguous =
        crate::task_service::resumable_external_session(&db, &historical, Some(&agent_id), None)
            .await
            .expect("ambiguous lookup fails closed without error");
    assert_eq!(ambiguous, None);

    let workflow = crate::workflow::default_workflow::default_workflow();
    let annotation = api_types::TaskBlockingAnnotation {
        annotation_type: api_types::FailureKind::ExecutorFailed,
        blocking_reason: "executor failed".to_owned(),
        blocked_by: Some("system".to_owned()),
        blocked_at: Some(now_rfc3339()),
        blocked_execution_id: Some(historical.id.clone()),
        artifact: None,
        message: None,
        hook: None,
        recovery_actions: vec![api_types::RecoveryAction::ResumeSession],
    };
    let actions =
        crate::task_service::action_resolver::resolve_execution_actions_with_session_state(
            &task,
            &workflow,
            &[historical],
            Some(&annotation),
            Some(&std::collections::HashSet::new()),
        );
    let session_follow_up = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::SessionFollowUp)
        .expect("session follow-up action exists");
    assert!(!session_follow_up.enabled);
    let workflow_resume = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::WorkflowResume)
        .expect("workflow resume action exists");
    assert!(!workflow_resume.enabled);
}

#[tokio::test]
async fn role_follow_up_keeps_active_lineage_agent_over_legacy_projection() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_task_executor(Arc::new(NoDiffExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(&task.id, "coder", Some(agent_a.clone()), None),
            false,
            false,
        )
        .await
        .expect("initial role assignment succeeds");
    let role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
        .await
        .expect("TaskRole loads")
        .expect("TaskRole exists");
    service
        .update_task_role(
            &task.id,
            "implementer",
            role.version,
            Some(CoordinationMode::Collaborative),
            None,
        )
        .await
        .expect("coordination mode updates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("second membership creates");
    let workspace_id = seed_workspace_for_task(&db, &task, workspace_root.path()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_b.clone()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("parent execution".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
            ),
            workspace_id: Some(workspace_id),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let follow_up = service
        .dispatch_role_follow_up(
            &task.id,
            "coder",
            parent_execution.id.clone(),
            "continue current work".to_owned(),
            "test",
            db::ExecutionPurpose::Implement,
        )
        .await
        .expect("role follow-up succeeds");

    assert_eq!(follow_up.agent_id.as_deref(), Some(agent_b.as_str()));
    let historical_parent = ExecutionRepo::get_by_id(&*db, &parent_execution.id)
        .await
        .expect("historical parent loads")
        .expect("historical parent exists");
    assert_eq!(
        historical_parent.agent_id.as_deref(),
        Some(agent_b.as_str())
    );
}

#[tokio::test]
async fn role_follow_up_does_not_reuse_suspended_lineage_agent() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_task_executor(Arc::new(NoDiffExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(&task.id, "coder", Some(agent_a.clone()), None),
            false,
            false,
        )
        .await
        .expect("initial role assignment succeeds");
    let role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
        .await
        .expect("TaskRole loads")
        .expect("TaskRole exists");
    service
        .update_task_role(
            &task.id,
            "implementer",
            role.version,
            Some(CoordinationMode::Collaborative),
            None,
        )
        .await
        .expect("coordination mode updates");
    let membership_b = service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("second membership creates");
    service
        .update_task_role_member(
            &task.id,
            &membership_b.id,
            membership_b.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("lineage membership suspends");
    let workspace_id = seed_workspace_for_task(&db, &task, workspace_root.path()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_b.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_b.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("parent execution".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{}}"#.to_owned(),
            ),
            workspace_id: Some(workspace_id),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let parent_execution = ExecutionRepo::record_harness_session_result(
        &*db,
        &parent_execution.id,
        "suspended-lineage-thread",
        &now_rfc3339(),
    )
    .await
    .expect("parent harness session activates");

    let follow_up = service
        .dispatch_role_follow_up(
            &task.id,
            "coder",
            parent_execution.id.clone(),
            "continue with the current eligible Agent".to_owned(),
            "test",
            db::ExecutionPurpose::Implement,
        )
        .await
        .expect("role follow-up succeeds");

    assert_eq!(follow_up.agent_id.as_deref(), Some(agent_a.as_str()));
    assert_ne!(
        follow_up.harness_session_id,
        parent_execution.harness_session_id
    );
    assert!(follow_up.agent_session_id.is_none());
    let historical_parent = ExecutionRepo::get_by_id(&*db, &parent_execution.id)
        .await
        .expect("historical parent loads")
        .expect("historical parent exists");
    assert_eq!(
        historical_parent.agent_id.as_deref(),
        Some(agent_b.as_str())
    );
}

#[tokio::test]
async fn follow_up_rejects_a_running_repository_role_without_mutating_task() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "claude_code", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Failed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("failed-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some("failed".to_owned()),
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"claude_code","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("failed parent creates");
    let running = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"claude_code","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("running execution creates");

    let result = service
        .follow_up_execution(parent.id, "continue".to_owned(), None, None)
        .await;

    assert!(matches!(
        result,
        Err(ServiceError::InvalidOperation { message })
            if message.contains("repository execution already running")
                && message.contains(&running.id)
    ));
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions list");
    assert_eq!(executions.items.len(), 2);
    let unchanged = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(unchanged.version, task.version);
    assert!(unchanged.error_annotation.is_none());
}

#[tokio::test]
async fn follow_up_execution_codex_resumes_explicit_harness_session() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let message = "Please continue with the remaining edge cases".to_owned();
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("parent execution".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{"resume_fallback_prompt":"do not send this full prompt"},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#
                    .to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let parent_execution = ExecutionRepo::record_harness_session_result(
        &*db,
        &parent_execution.id,
        "codex-thread",
        &now_rfc3339(),
    )
    .await
    .expect("parent harness session activates");

    let result = service
        .follow_up_execution(parent_execution.id.clone(), message.clone(), None, None)
        .await
        .expect("follow-up succeeds");

    assert_eq!(result.execution.summary.as_deref(), Some(message.as_str()));
    assert_eq!(
        result.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        result.execution.agent_session_id.as_deref(),
        Some("codex-thread")
    );
    let snapshot: serde_json::Value = serde_json::from_str(
        result
            .execution
            .executor_config_snapshot_json
            .as_deref()
            .expect("snapshot exists"),
    )
    .expect("snapshot is valid json");
    assert_eq!(
        snapshot["dispatch"]["execution_policy"],
        "explicit_harness_session"
    );
    assert!(snapshot["config"]
        .get("resume_thread_id")
        .is_none_or(serde_json::Value::is_null));
    assert!(snapshot["config"]
        .get("resume_thread_in_place")
        .is_none_or(serde_json::Value::is_null));
    assert!(snapshot["config"]
        .get("resume_fallback_prompt")
        .is_none_or(serde_json::Value::is_null));
}

#[tokio::test]
async fn follow_up_execution_rejects_running_parent() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("running parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn follow_up_on_cancelled_execution_reuses_active_harness_session() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Cancelled,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("cancelled parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let parent_execution = ExecutionRepo::record_harness_session_result(
        &*db,
        &parent_execution.id,
        "test-session",
        &now_rfc3339(),
    )
    .await
    .expect("parent harness session activates");

    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await;

    let child = result.expect("follow-up succeeds");
    assert_eq!(
        child.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        child.execution.agent_session_id.as_deref(),
        Some("test-session")
    );
}

#[tokio::test]
async fn follow_up_on_cancelled_execution_without_session_starts_new_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Cancelled,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("cancelled parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let parent_id = parent_execution.id.clone();
    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await;

    let child = result.expect("follow-up starts a fresh execution");
    assert_eq!(
        child.execution.parent_execution_id.as_deref(),
        Some(parent_id.as_str())
    );
    assert!(child.execution.harness_session_id.is_none());
    assert!(child.execution.agent_session_id.is_none());
}

#[tokio::test]
async fn follow_up_execution_with_legacy_projection_starts_fresh_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("completed parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    sqlx::query("UPDATE execution SET agent_session_id = 'legacy-thread' WHERE id = ?")
        .bind(&parent_execution.id)
        .execute(db.pool())
        .await
        .expect("legacy projection is added to the historical execution");

    let parent_id = parent_execution.id.clone();
    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await;

    let child = result.expect("follow-up starts a fresh execution");
    assert_eq!(
        child.execution.parent_execution_id.as_deref(),
        Some(parent_id.as_str())
    );
    assert!(child.execution.harness_session_id.is_none());
    assert!(child.execution.agent_session_id.is_none());
    assert_eq!(
        crate::task_service::execution::harness_invocation_for_execution(
            &db,
            &child.execution,
            None,
        )
        .await
        .expect("legacy projection does not change Start into Resume"),
        api_types::HarnessInvocation::Start
    );
}

#[tokio::test]
async fn follow_up_execution_rejects_terminal_task() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "done".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("completed parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn follow_up_execution_on_blocked_task() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: None,
            blocked_json: Some(Some(
                r#"{"reason":"test block","created_at":"2026-04-28T00:00:00Z","kind":"ci_failed"}"#
                    .to_owned(),
            )),
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("set blocked_json");
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id)),
            purpose: Some(db::ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("completed parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");
    let parent_execution = ExecutionRepo::record_harness_session_result(
        &*db,
        &parent_execution.id,
        "test-session",
        &now_rfc3339(),
    )
    .await
    .expect("parent harness session activates");

    let result = service
        .follow_up_execution(parent_execution.id, "continue".to_owned(), None, None)
        .await
        .expect("follow-up succeeds");

    assert_eq!(result.execution.role, "interactive".to_owned());
    assert_eq!(result.task.status, "in_progress".to_owned());
}

#[tokio::test]
async fn follow_up_execution_rejects_executor_mismatch() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let shell_agent_id = seed_agent(&db).await;
    let codex_agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(shell_agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("completed parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            Some(codex_agent_id),
            None,
        )
        .await;

    assert!(matches!(
        result,
        Err(ServiceError::InvalidOperation { message })
            if message.contains("same executor type")
    ));
}

#[tokio::test]
async fn re_execute_uses_current_membership_not_legacy_projection() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(&task.id, "coder", Some(agent_a.clone()), None),
            false,
            false,
        )
        .await
        .expect("initial role assignment succeeds");
    let role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
        .await
        .expect("TaskRole loads")
        .expect("TaskRole exists");
    service
        .update_task_role(
            &task.id,
            "implementer",
            role.version,
            Some(CoordinationMode::Collaborative),
            None,
        )
        .await
        .expect("coordination mode updates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("second membership creates");
    let projection = TaskRoleAssignmentRepo::get_by_task_and_role(&*db, &task.id, "coder")
        .await
        .expect("legacy projection loads")
        .expect("legacy projection exists");
    assert_eq!(projection.assignee_id.as_deref(), Some(agent_a.as_str()));
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_b.clone()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Cancelled,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("cancelled parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let result = service
        .re_execute_execution(parent_execution.id.clone())
        .await
        .expect("re-execute succeeds");

    assert_eq!(result.execution.role, "coder".to_owned());
    assert_eq!(result.execution.status, ExecutionStatus::Running);
    assert_eq!(result.execution.agent_id.as_deref(), Some(agent_b.as_str()));
    assert_eq!(
        result.execution.parent_execution_id.as_deref(),
        Some(parent_execution.id.as_str())
    );
    assert_eq!(result.execution.agent_session_id, None);
    let historical_parent = ExecutionRepo::get_by_id(&*db, &parent_execution.id)
        .await
        .expect("historical parent loads")
        .expect("historical parent exists");
    assert_eq!(
        historical_parent.agent_id.as_deref(),
        Some(agent_b.as_str())
    );
}

#[tokio::test]
async fn re_execute_rejects_running_parent() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("running parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("parent execution creates");

    let result = service.re_execute_execution(parent_execution.id).await;

    assert!(matches!(
        result,
        Err(ServiceError::InvalidOperation { message })
            if message.contains("re-execute requires")
    ));
}

#[tokio::test]
async fn re_execute_rejects_concurrent_running_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let parent_execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Cancelled,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("cancelled parent".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("parent execution creates");
    ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("running sibling".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("running execution creates");

    let result = service.re_execute_execution(parent_execution.id).await;

    assert!(matches!(
        result,
        Err(ServiceError::InvalidOperation { message })
            if message.contains("already running")
    ));
}

#[tokio::test]
async fn interactive_execution_completion_does_not_trigger_review_cascade() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = service
        .create_task(
            project_id,
            "Interactive no cascade",
            Some("printf no-cascade".to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("task creates");
    let launched = service
        .launch_execution(task.id.clone(), agent_id, None, None)
        .await
        .expect("launch succeeds");

    let registry = Arc::new(cli_adapters::default_registry());
    let executor = executors::AdapterExecutor::new(registry);
    let execution = service
        .run_execution(launched.execution.id.clone(), &executor)
        .await
        .expect("interactive execution runs");
    assert_eq!(execution.status, ExecutionStatus::Completed);

    service
        .maybe_cascade_executor_completion(&launched.execution.id)
        .await
        .expect("cascade check succeeds");

    let current = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(current.status, "in_progress".to_owned());
}

#[tokio::test]
async fn recover_reexecute_without_blocked_execution_dispatches_current_state_role() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_task_executor(Arc::new(PendingExecutor))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("authoritative implementer TaskRole creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("Agent joins the authoritative implementer TaskRole");
    TaskRoleAssignmentRepo::assign(
        &*db,
        role_assignment_input(
            &task.id,
            crate::workflow::default_roles::CODER,
            Some(agent_id.clone()),
            None,
        ),
    )
    .await
    .expect("coder assignment created");
    let annotation = json!({
        "type": "recovery_required",
        "blocking_reason": "crash_recovery",
        "blocked_by": "system:crash_recovery",
        "blocked_at": now_rfc3339(),
        "message": "Recovered after server restart",
        "recovery_actions": ["reexecute", "reset_to_initial", "cancel_task"],
    })
    .to_string();
    TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(annotation)),
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task recovery annotation saved");

    let recovered = service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::Reexecute,
            Some("test".to_owned()),
            Some("resume current work".to_owned()),
        )
        .await
        .expect("reexecute recovers");

    assert_eq!(recovered.status, "in_progress");
    assert_eq!(recovered.error_annotation, None);
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 20,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions load");
    assert_eq!(executions.items.len(), 1);
    assert_eq!(
        executions.items[0].role, "implementer",
        "re-execution follows the authoritative TaskRole, not the legacy coder projection"
    );
    assert_eq!(executions.items[0].status, ExecutionStatus::Running);
    assert!(executions.items[0]
        .summary
        .as_deref()
        .unwrap_or_default()
        .contains("resume current work"));
}

struct PendingExecutor;

#[async_trait::async_trait]
impl TaskExecutor for PendingExecutor {
    async fn execute(
        &self,
        _ctx: ExecutionContext,
    ) -> std::result::Result<ExecutionResult, ExecutorError> {
        std::future::pending::<std::result::Result<ExecutionResult, ExecutorError>>().await
    }

    async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
        Ok(())
    }
}

#[tokio::test]
async fn parent_execution_completion_does_not_recursively_dispatch_subtask_work() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let subtask = seed_subtask_with_status(&db, &task, "child", "todo".to_owned(), 0).await;
    let workspace = seed_workspace_with_plan(&db, &task, "- [x] parent work\n").await;

    let plan_execution =
        create_remote_execution_fixture(&db, &task.id, &agent_id, db::ExecutionPurpose::Plan).await;
    let plan_artifact =
        crate::CollaborationService::new(Arc::clone(&db), Arc::new(EventBus::new(8)))
            .create_plan_artifact_from_execution(&plan_execution.id, "# Exact task plan\n")
            .await
            .expect("Plan Artifact creates");

    let mut harness_capabilities = api_types::HarnessCapabilities::unknown();
    harness_capabilities.resume = api_types::CapabilitySupport::Native;
    let harness_session_id = db::new_uuid_v4();
    let now = now_rfc3339();
    db::HarnessSessionRepo::create(
        &*db,
        db::CreateHarnessSession {
            id: harness_session_id.clone(),
            agent_id: agent_id.clone(),
            harness_kind: "codex".to_owned(),
            external_session_id: Some("test-session".to_owned()),
            profile_id: None,
            profile_snapshot_json: json!({
                "executor_type": "codex",
                "config": {},
                "credential_ref": null
            })
            .to_string(),
            capabilities_snapshot_json: serde_json::to_string(&harness_capabilities.snapshot())
                .expect("HarnessSession capabilities serialize"),
            workspace_id: Some(workspace.id.clone()),
            status: db::HarnessSessionStatus::Active,
            predecessor_session_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            last_activity_at: Some(now.clone()),
        },
    )
    .await
    .expect("active HarnessSession creates");

    let parent_input = db::CreateExecution {
        id: db::new_uuid_v4(),
        task_id: task.id.clone(),
        agent_id: Some(agent_id.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
        purpose: Some(db::ExecutionPurpose::Implement),
        harness_session_id: Some(harness_session_id.clone()),
        role: crate::workflow::default_roles::CODER.to_owned(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: None,
        agent_session_id: Some("test-session".to_owned()),
        agent_message_id: None,
        last_activity_at: None,
        summary: Some("implemented the change".to_owned()),
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: Some(
            r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
        ),
        workspace_id: Some(workspace.id.clone()),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let execution = db::ExecutionRepo::create_with_artifact_inputs_and_event(
        &*db,
        parent_input.clone(),
        vec![plan_artifact.id.clone()],
        crate::task_service::execution_domain_event(&parent_input, "execution.started"),
    )
    .await
    .expect("parent Execution and exact Plan input create atomically")
    .0;
    sqlx::query("UPDATE execution SET status = 'completed', updated_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(&execution.id)
        .execute(db.pool())
        .await
        .expect("parent Execution completes");

    service
        .maybe_cascade_executor_completion(&execution.id)
        .await
        .expect("legacy completion check is harmless");

    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 20,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions load");
    assert_eq!(executions.items.len(), 2);
    assert!(executions
        .items
        .iter()
        .any(|item| { item.id == execution.id && item.status == ExecutionStatus::Completed }));
    assert!(executions
        .items
        .iter()
        .any(|item| item.id == plan_execution.id));
    assert!(executions
        .items
        .iter()
        .all(|item| item.id == execution.id || item.id == plan_execution.id));
    let parent_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("parent lifecycle loads")
        .expect("parent lifecycle exists");
    assert_eq!(parent_lifecycle.state, db::TaskLifecycleState::Active);
    let subtask_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &subtask.id)
        .await
        .expect("subtask lifecycle loads")
        .expect("subtask lifecycle exists");
    assert_eq!(subtask_lifecycle.state, db::TaskLifecycleState::Ready);
}

#[tokio::test]
async fn execution_completion_does_not_implicitly_publish_a_comment_or_move_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let now = now_rfc3339();
    let execution = ExecutionRepo::create(
        &*db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "executor".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: Some("test-session".to_owned()),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("implemented the change".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("execution creates");

    let lifecycle_before = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle loads")
        .expect("lifecycle exists");
    service
        .maybe_cascade_executor_completion(&execution.id)
        .await
        .expect("legacy completion check is harmless");

    let comments = TaskCommentRepo::list_comments(
        &*db,
        &task.id,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Asc,
        },
    )
    .await
    .expect("comments list");
    assert!(comments.items.is_empty());
    let lifecycle_after = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads")
        .expect("lifecycle exists");
    assert_eq!(lifecycle_after.state, lifecycle_before.state);
    assert_eq!(lifecycle_after.version, lifecycle_before.version);
}

async fn seed_workspace_with_plan(db: &SqliteDb, task: &Task, plan: &str) -> Workspace {
    let workspace_dir = std::env::temp_dir()
        .join(format!("forge-guard-plan-{}", new_uuid_v4()))
        .join(&task.id);
    let worktree_path = workspace_dir.join("worktree");
    std::fs::create_dir_all(&worktree_path).expect("worktree creates");
    std::fs::write(workspace_dir.join("plan.md"), plan).expect("plan writes");
    git::init(&worktree_path).await.expect("git init succeeds");
    std::fs::write(worktree_path.join("README.md"), "# Test\n").expect("readme writes");
    git::commit_all(&worktree_path, "initial commit")
        .await
        .expect("initial commit creates");
    WorkspaceRepo::create(
        db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            repo_id: task.repo_id.clone().unwrap(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: ::workspace::task_branch_name(&task.id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("workspace creates")
}

#[tokio::test]
async fn claim_task_records_execution_permission_policy_override_in_snapshot() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id =
        seed_shell_agent_with_config(&db, r#"{"command":"echo","args":["profile-default"]}"#).await;
    let task = service
        .create_task(
            project_id,
            "Snapshot override",
            Some("unused".to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("task creates");

    let claimed = service
        .claim_task(
            task.id,
            Assignee::Agent(agent_id),
            Some(ExecutionOverrides {
                model_id: None,
                reasoning_effort: None,
                permission_policy: Some("auto".to_owned()),
            }),
        )
        .await
        .expect("task claims");
    let execution = ExecutionRepo::get_by_id(&*db, &claimed.execution.id)
        .await
        .expect("execution loads")
        .expect("execution exists");
    let snapshot: Value = serde_json::from_str(
        execution
            .executor_config_snapshot_json
            .as_deref()
            .expect("snapshot recorded"),
    )
    .expect("snapshot parses");

    assert_eq!(snapshot["config"]["permission_policy"], "auto");
    let execution_keys = snapshot["overrides_applied"]["execution"]
        .as_array()
        .expect("execution override keys are recorded");
    assert!(execution_keys
        .iter()
        .any(|key| key.as_str() == Some("permission_policy")));
}

#[tokio::test]
async fn claim_task_records_codex_overrides_in_normalized_snapshot() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(
        &db,
        "codex",
        r#"{"model":"agent-model","model_reasoning_effort":"medium","sandbox":"danger-full-access","permission_policy":"supervised"}"#,
    )
    .await;
    let task = service
        .create_task(
            project_id,
            "Snapshot codex overrides",
            Some("unused".to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("task creates");

    let claimed = service
        .claim_task(
            task.id,
            Assignee::Agent(agent_id),
            Some(ExecutionOverrides {
                model_id: Some("gpt-5-codex".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                permission_policy: Some("auto".to_owned()),
            }),
        )
        .await
        .expect("task claims");
    let snapshot: Value = serde_json::from_str(
        claimed
            .execution
            .executor_config_snapshot_json
            .as_deref()
            .expect("snapshot recorded"),
    )
    .expect("snapshot parses");

    assert_eq!(snapshot["executor_type"], "codex");
    assert_eq!(snapshot["config"]["model"], "gpt-5-codex");
    assert_eq!(snapshot["config"]["model_reasoning_effort"], "high");
    assert_eq!(snapshot["config"]["permission_policy"], "auto");
    assert!(snapshot["config"].get("effort").is_none());

    let agent_keys = snapshot["overrides_applied"]["agent"]
        .as_array()
        .expect("agent keys are recorded");
    assert!(agent_keys.iter().any(|key| key.as_str() == Some("model")));
    assert!(agent_keys
        .iter()
        .any(|key| key.as_str() == Some("model_reasoning_effort")));
    assert!(agent_keys
        .iter()
        .any(|key| key.as_str() == Some("permission_policy")));

    let execution_keys = snapshot["overrides_applied"]["execution"]
        .as_array()
        .expect("execution keys are recorded");
    assert!(execution_keys
        .iter()
        .any(|key| key.as_str() == Some("model")));
    assert!(execution_keys
        .iter()
        .any(|key| key.as_str() == Some("model_reasoning_effort")));
    assert!(execution_keys
        .iter()
        .any(|key| key.as_str() == Some("permission_policy")));
    assert!(!execution_keys
        .iter()
        .any(|key| key.as_str() == Some("effort")));
}
