use super::super::*;
use api_types::ActorRef;
use db::{CollaborationRepo, CoordinationMode, ProjectMemberRepo, TaskLifecycleRepo, TaskRoleRepo};
use serde_json::Value;

#[derive(Default)]
struct RecoveryDispatchExecutor {
    calls: std::sync::atomic::AtomicUsize,
    started: tokio::sync::Notify,
}

fn provider_start_receipt_for_test(
    execution: &db::Execution,
    event_type: &str,
) -> db::CreateDomainEvent {
    let actor = execution.actor_ref();
    db::CreateDomainEvent {
        id: db::new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: execution.id.clone(),
        actor_type: actor
            .as_ref()
            .map(|actor| actor.kind().to_string())
            .unwrap_or_else(|| "system".to_owned()),
        actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
        scope_type: "task".to_owned(),
        scope_id: execution.task_id.clone(),
        correlation_id: execution.id.clone(),
        causation_id: execution.parent_execution_id.clone(),
        causation_depth: i64::from(execution.parent_execution_id.is_some()),
        dedupe_key: Some(format!("{event_type}:{}", execution.id)),
        payload_json: json!({
            "execution_id": execution.id,
            "task_id": execution.task_id,
            "agent_id": execution.agent_id,
            "outcome": event_type.strip_prefix("execution.provider_start_").unwrap_or_default(),
        })
        .to_string(),
        created_at: now_rfc3339(),
    }
}

#[async_trait::async_trait]
impl executors::TaskExecutor for RecoveryDispatchExecutor {
    async fn execute(
        &self,
        _ctx: executors::ExecutionContext,
    ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.notify_one();
        Ok(executors::ExecutionResult {
            status: executors::ExecutionOutcome::Completed,
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

async fn execution_external_session_id(
    db: &db::SqliteDb,
    execution: &db::Execution,
) -> Option<String> {
    let Some(session_id) = execution.harness_session_id.as_deref() else {
        return None;
    };
    sqlx::query_scalar("SELECT external_session_id FROM harness_session WHERE id = ?")
        .bind(session_id)
        .fetch_optional(db.pool())
        .await
        .expect("exact HarnessSession loads")
        .flatten()
}

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
                "features": [
                    api_types::DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1,
                    api_types::DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1
                ]
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

async fn answer_uncertain_start_daemon(
    registry: Arc<crate::daemon_transport::DaemonConnectionRegistry>,
    daemon_id: String,
    mut outbound: tokio::sync::mpsc::Receiver<api_types::DaemonFrame>,
) -> Vec<String> {
    let mut methods = Vec::new();
    while let Ok(Some(frame)) =
        tokio::time::timeout(std::time::Duration::from_secs(10), outbound.recv()).await
    {
        let api_types::DaemonFrame::Request { id, method, .. } = frame else {
            continue;
        };
        methods.push(method.clone());
        match method.as_str() {
            api_types::METHOD_PROTOCOL_CAPABILITIES => registry.dispatch_incoming(
                &daemon_id,
                api_types::DaemonFrame::Response {
                    id,
                    result: serde_json::json!({
                        "schema_version": 1,
                        "features": [
                            api_types::DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1,
                            api_types::DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1
                        ]
                    }),
                },
            ),
            api_types::METHOD_EXECUTION_START => registry.dispatch_incoming(
                &daemon_id,
                api_types::DaemonFrame::Error {
                    id: Some(id),
                    error: api_types::DaemonErrorPayload {
                        code: "start_outcome_unknown".to_owned(),
                        message: "provider may have accepted Start before its response was lost"
                            .to_owned(),
                        details: None,
                    },
                },
            ),
            other => panic!("unexpected daemon request: {other}"),
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
            title: "Read-only exact Review fixture".to_owned(),
            description: Some("Review a dirty Workspace snapshot".to_owned()),
            task_type: "review".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Active,
            is_automation: false,
            priority: 0,
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

    let repo = db::RepoRepo::get_by_id(db, repo_id)
        .await
        .expect("review Repo loads")
        .expect("review Repo exists");
    let repo_path = std::path::PathBuf::from(
        repo.local_path
            .as_deref()
            .expect("review Repo has a local path"),
    );
    if !repo_path.join(".git").exists() {
        run_workspace_git(&repo_path, &["init", "-b", "main"]);
        run_workspace_git(&repo_path, &["config", "user.email", "test@forge.dev"]);
        run_workspace_git(&repo_path, &["config", "user.name", "Forge Test"]);
        std::fs::write(repo_path.join("README.md"), "review fixture base\n")
            .expect("review Repo base file writes");
        run_workspace_git(&repo_path, &["add", "README.md"]);
        run_workspace_git(&repo_path, &["commit", "-m", "review fixture base"]);
    }

    let human_id = seed_human_user(db).await;
    let now = now_rfc3339();
    ProjectMemberRepo::add_member(
        db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: project_id.to_owned(),
            user_id: human_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Workspace preparer joins the Project");
    let work_unit_service = crate::work_unit_service::WorkUnitService::new(
        Arc::new(db.clone()),
        Arc::new(EventBus::new(16)),
        workspace_root.to_path_buf(),
        Arc::new(::workspace::RepoCacheLockManager::new()),
        Arc::new(crate::WorkspaceExecutionLockManager::new()),
    );
    let work_unit = work_unit_service
        .create(
            crate::collaboration_service::CollaborationActorSource::Human(human_id.clone()),
            crate::work_unit_service::CreateWorkUnitInput {
                task_id: task.id.clone(),
                title: "Exact Review workspace".to_owned(),
                scope: "Read-only Review fixture".to_owned(),
                role: "reviewer".to_owned(),
                parent_work_unit_id: None,
                assigned_actor: Some(db::ActorRef::Agent(agent_id.to_owned())),
                requires_integration: true,
                provenance: None,
            },
        )
        .await
        .expect("Review WorkUnit creates");
    let workspace = work_unit_service
        .prepare_workspace(
            crate::collaboration_service::CollaborationActorSource::Human(human_id),
            &work_unit.id,
        )
        .await
        .expect("exact WorkUnit Review workspace prepares");
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
        .create_running_work_unit_execution_with_artifact_inputs(
            &work_unit.id,
            work_unit.version,
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
            Vec::new(),
        )
        .await
        .expect("running Review Execution binds to its exact WorkUnit and Workspace");
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
            crate::task_service::memberships::active_agent_membership(&memberships, agent_id)
                .is_none()
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
    let repo = db::RepoRepo::get_by_id(db, task.repo_id.as_deref().expect("Task Repo binds"))
        .await
        .expect("Execution Repo loads")
        .expect("Execution Repo exists");
    let repo_path = std::path::PathBuf::from(
        repo.local_path
            .as_deref()
            .expect("Execution Repo has a local path"),
    );
    if !repo_path.join(".git").exists() {
        run_workspace_git(&repo_path, &["init", "-b", "main"]);
        run_workspace_git(&repo_path, &["config", "user.email", "test@forge.dev"]);
        run_workspace_git(&repo_path, &["config", "user.name", "Forge Test"]);
        std::fs::write(repo_path.join("README.md"), "execution fixture base\n")
            .expect("Execution Repo base file writes");
        run_workspace_git(&repo_path, &["add", "README.md"]);
        run_workspace_git(&repo_path, &["commit", "-m", "execution fixture base"]);
    }
    let work_unit_service = crate::work_unit_service::WorkUnitService::new(
        Arc::new(db.clone()),
        Arc::new(EventBus::new(16)),
        workspace_root.to_path_buf(),
        Arc::new(::workspace::RepoCacheLockManager::new()),
        Arc::new(crate::WorkspaceExecutionLockManager::new()),
    );
    // The exact WorkUnit is created by a Project member, then assigned to the
    // Agent whose active TaskRole membership was established above.
    let human_id = seed_human_user(db).await;
    let now = now_rfc3339();
    db::ProjectMemberRepo::add_member(
        db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: task.project_id.clone(),
            user_id: human_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Workspace preparer joins the Project");
    let work_unit = work_unit_service
        .create(
            crate::collaboration_service::CollaborationActorSource::Human(human_id.clone()),
            crate::work_unit_service::CreateWorkUnitInput {
                task_id: task.id.clone(),
                title: "Exact repository Execution".to_owned(),
                scope: "Execution test fixture".to_owned(),
                role: role.to_owned(),
                parent_work_unit_id: None,
                assigned_actor: Some(db::ActorRef::Agent(agent_id.to_owned())),
                requires_integration: true,
                provenance: None,
            },
        )
        .await
        .expect("Execution WorkUnit creates");
    let workspace = work_unit_service
        .prepare_workspace(
            crate::collaboration_service::CollaborationActorSource::Human(human_id),
            &work_unit.id,
        )
        .await
        .expect("exact Execution WorkUnit workspace prepares");
    let now = now_rfc3339();
    let execution = service
        .create_running_work_unit_execution_with_artifact_inputs(
            &work_unit.id,
            work_unit.version,
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
            Vec::new(),
        )
        .await
        .expect("Running Execution receives its exact WorkUnit WorkspaceLease");
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn pr8_human_review_uses_only_plural_active_memberships() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let (human_a, membership_a) = add_human_reviewer(&db, &service, &task).await;
    let (human_b, membership_b) = add_human_reviewer(&db, &service, &task).await;

    assert_eq!(membership_a.status, db::RoleMembershipStatus::Active);
    assert_eq!(membership_b.status, db::RoleMembershipStatus::Active);
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn pr8_human_review_requires_an_exact_active_membership() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let user_id = seed_human_user(&db).await;
    let replacement_task =
        seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    service
        .create_task_role(
            &replacement_task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("replacement reviewer TaskRole exists with no members");
    let denied = service
        .start_human_review_execution(&replacement_task.id, &user_id, None)
        .await
        .expect_err("contradictory singleton cannot grant replacement-role authority");
    assert!(matches!(denied, ServiceError::AuthorizationDenied { .. }));

    let unassigned_task =
        seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let denied = service
        .start_human_review_execution(&unassigned_task.id, &user_id, None)
        .await
        .expect_err("missing TaskRole membership does not infer Human authority");
    assert!(matches!(denied, ServiceError::AuthorizationDenied { .. }));
}

#[tokio::test]
async fn pr8_completed_human_report_retry_survives_later_membership_change() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn pr8_human_review_report_rejects_a_different_human_actor() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let (assigned_human, _) = add_human_reviewer(&db, &service, &task).await;
    let execution = service
        .start_human_review_execution(&task.id, &assigned_human, None)
        .await
        .expect("active Human starts exact Review Execution");

    let result = service
        .submit_human_review_report(
            &execution.id,
            "different-human",
            human_review_request("Unauthorized report", Vec::new(), Vec::new()),
        )
        .await;
    assert!(matches!(
        result,
        Err(ServiceError::AuthorizationDenied { .. })
    ));
    assert!(db::CollaborationRepo::get_execution_artifact_output(
        &*db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await
    .expect("ReviewReport lookup")
    .is_none());
}

#[tokio::test]
async fn pr8_running_human_report_reconciles_after_membership_and_workspace_change() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)))
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn task_scoped_execution_entrypoints_fail_closed_without_storage() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = service
        .create_task(
            project_id,
            "Task-scoped launch rejection",
            Some("unused".to_owned()),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Task creates");

    let launch_error = service
        .launch_execution(task.id.clone(), agent_id.clone(), None, None)
        .await
        .err()
        .expect("Task identity cannot select a Workspace or Execution target");
    assert!(launch_error
        .to_string()
        .contains("Task-scoped launch is retired"));
    let dispatch_error = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            "implementer",
            db::ExecutionPurpose::Implement,
            "legacy workflow dispatch".to_owned(),
        )
        .await
        .expect_err("workflow role dispatch cannot infer a WorkUnit");
    assert!(dispatch_error
        .to_string()
        .contains("workflow dispatch is retired"));

    let execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Execution count reads");
    let workspace_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Workspace count reads");
    assert_eq!(execution_count, 0);
    assert_eq!(workspace_count, 0);
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
    let task =
        seed_task_with_status(&first_db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task =
        seed_task_with_status(&first_db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn role_dispatch_ignores_legacy_state_hook_settings() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let workspace_root = TempDir::new().expect("workspace temp dir creates");
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let daemon_id = agent.daemon_id.expect("Agent daemon binding");
    let registry = routing_test_registry();
    let outbound = register_routing_test_daemon(&registry, &daemon_id);
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_daemon_connections(Arc::clone(&registry));
    let marker_path = workspace_root.path().join("legacy-state-hook-ran");
    let settings = json!({
        "lifecycle_hooks": {
            "before_work": [{
                "type": "script",
                "command": format!("printf legacy-hook-ran > '{}'; exit 9", marker_path.display()),
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
        .expect("historical Project settings update");
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("implementer TaskRole creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("Agent Actor joins the exact TaskRole");

    let responder = tokio::spawn(answer_routing_test_daemon(
        Arc::clone(&registry),
        daemon_id,
        outbound,
        std::time::Duration::from_secs(2),
    ));
    let execution = service
        .dispatch_initial_role_execution(
            &task.id,
            &agent_id,
            "implementer",
            db::ExecutionPurpose::Implement,
            "start scoped work".to_owned(),
        )
        .await
        .expect("legacy state hook does not block Execution admission");

    assert_eq!(
        responder.await.expect("daemon responder joins"),
        vec![
            api_types::METHOD_PROTOCOL_CAPABILITIES,
            api_types::METHOD_EXECUTION_START
        ]
    );
    assert!(!marker_path.exists(), "historical script was not executed");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle lookup succeeds")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Active);
    let stored_execution = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("Execution lookup succeeds")
        .expect("Execution was admitted");
    assert_eq!(
        stored_execution.actor_ref(),
        Some(db::ActorRef::Agent(agent_id))
    );
    assert_eq!(stored_execution.role, "implementer");
    assert_eq!(
        stored_execution.purpose,
        Some(db::ExecutionPurpose::Implement)
    );
    assert!(stored_execution.harness_session_id.is_none());
}

#[tokio::test]
async fn follow_up_execution_reuses_explicit_harness_session() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "claude_code", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let message = "Please continue with the remaining edge cases".to_owned();
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        Some("test-session"),
        Some(
            r#"{"executor_type":"claude_code","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
        ),
    )
    .await;

    let result = service
        .follow_up_execution(
            parent_execution.id.clone(),
            message.clone(),
            None,
            None,
            "follow-up-session-reuse".to_owned(),
        )
        .await
        .expect("follow-up succeeds");

    assert_eq!(
        result.execution.parent_execution_id.as_deref(),
        Some(parent_execution.id.as_str())
    );
    assert_eq!(result.execution.summary.as_deref(), Some(message.as_str()));
    assert_eq!(result.execution.role, "implementer".to_owned());
    assert_eq!(
        result.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        execution_external_session_id(&db, &result.execution)
            .await
            .as_deref(),
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
async fn follow_up_without_exact_work_unit_fails_closed() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
            role: "implementer".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
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
            updated_at: now,
        },
    )
    .await
    .expect("historical-shaped Execution creates");

    let error = service
        .follow_up_execution(
            execution.id,
            "continue without an exact WorkUnit".to_owned(),
            Some(agent_id),
            None,
            "follow-up-no-work-unit".to_owned(),
        )
        .await
        .err()
        .expect("Task and session identity cannot supply a missing WorkUnit target");
    assert!(error
        .to_string()
        .contains("exact open WorkUnit and Workspace"));

    let execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Execution count reads");
    assert_eq!(
        execution_count, 1,
        "no synthetic follow-up Execution is created"
    );
}

#[tokio::test]
async fn follow_up_uses_its_exact_work_unit_while_a_sibling_work_unit_runs() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let parent = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    let sibling_agent_id = seed_agent_with_executor_type(&db, "shell", "{}").await;
    let sibling_work_unit =
        create_simple_execution_work_unit(&db, &service, &task, &sibling_agent_id).await;
    let sibling = service
        .launch_work_unit_execution(
            &sibling_work_unit.id,
            sibling_work_unit.version,
            db::ExecutionPurpose::Implement,
            "Sibling WorkUnit remains in progress".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("sibling WorkUnit launches independently");
    let result = service
        .follow_up_execution(
            parent.id.clone(),
            "continue".to_owned(),
            None,
            None,
            "follow-up-running-parent".to_owned(),
        )
        .await
        .expect("follow-up admits against its exact parent WorkUnit");
    assert_eq!(
        result.execution.parent_execution_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(result.execution.work_unit_id, parent.work_unit_id);
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
    assert_eq!(executions.items.len(), 3);
    assert!(executions
        .items
        .iter()
        .any(|execution| execution.id == sibling.execution.id));
}

#[tokio::test]
async fn follow_up_execution_codex_resumes_explicit_harness_session() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let message = "Please continue with the remaining edge cases".to_owned();
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        Some("codex-thread"),
        Some(
            r#"{"executor_type":"codex","config":{"resume_fallback_prompt":"do not send this full prompt"},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
        ),
    )
    .await;

    let result = service
        .follow_up_execution(
            parent_execution.id.clone(),
            message.clone(),
            None,
            None,
            "follow-up-codex-session".to_owned(),
        )
        .await
        .expect("follow-up succeeds");

    assert_eq!(result.execution.summary.as_deref(), Some(message.as_str()));
    assert_eq!(
        result.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        execution_external_session_id(&db, &result.execution)
            .await
            .as_deref(),
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            None,
            None,
            "follow-up-running-parent-reject".to_owned(),
        )
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Cancelled,
        Some("test-session"),
        Some(
            r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
        ),
    )
    .await;

    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            None,
            None,
            "follow-up-cancelled-session".to_owned(),
        )
        .await;

    let child = result.expect("follow-up succeeds");
    assert_eq!(
        child.execution.harness_session_id,
        parent_execution.harness_session_id
    );
    assert_eq!(
        execution_external_session_id(&db, &child.execution)
            .await
            .as_deref(),
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Cancelled,
        None,
        None,
    )
    .await;

    let parent_id = parent_execution.id.clone();
    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            None,
            None,
            "follow-up-cancelled-no-session".to_owned(),
        )
        .await;

    let child = result.expect("follow-up starts a fresh execution");
    assert_eq!(
        child.execution.parent_execution_id.as_deref(),
        Some(parent_id.as_str())
    );
    assert!(child.execution.harness_session_id.is_none());
    assert!(execution_external_session_id(&db, &child.execution)
        .await
        .is_none());
}

#[tokio::test]
async fn follow_up_without_exact_harness_session_starts_fresh_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        None,
        None,
    )
    .await;
    let parent_id = parent_execution.id.clone();
    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            None,
            None,
            "follow-up-no-session".to_owned(),
        )
        .await;

    let child = result.expect("follow-up starts a fresh execution");
    assert_eq!(
        child.execution.parent_execution_id.as_deref(),
        Some(parent_id.as_str())
    );
    assert!(child.execution.harness_session_id.is_none());
    assert!(execution_external_session_id(&db, &child.execution)
        .await
        .is_none());
    assert_eq!(
        crate::task_service::execution::harness_invocation_for_execution(
            &db,
            &child.execution,
            None,
        )
        .await
        .expect("missing external identity does not change Start into Resume"),
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Done).await;
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
            None,
            None,
            "follow-up-terminal-task".to_owned(),
        )
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
            parent_task_id: None,
            updated_at: db::now_rfc3339(),
        },
    )
    .await
    .expect("set blocked_json");
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        Some("test-session"),
        Some(
            r#"{"executor_type":"codex","config":{},"harness_capabilities":{"schema_version":1,"capabilities":{"resume":"native"}}}"#.to_owned(),
        ),
    )
    .await;

    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            None,
            None,
            "follow-up-blocked-task".to_owned(),
        )
        .await
        .expect("follow-up succeeds");

    assert_eq!(result.execution.role, "implementer".to_owned());
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &result.task.id)
        .await
        .expect("TaskLifecycle loads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Active);
}

#[tokio::test]
async fn follow_up_execution_rejects_agent_outside_exact_work_unit_assignment() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let shell_agent_id = seed_agent(&db).await;
    let codex_agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let parent_execution = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &shell_agent_id,
        ExecutionStatus::Completed,
        None,
        None,
    )
    .await;

    let result = service
        .follow_up_execution(
            parent_execution.id,
            "continue".to_owned(),
            Some(codex_agent_id),
            None,
            "follow-up-executor-mismatch".to_owned(),
        )
        .await;

    assert!(matches!(
        result,
        Err(ServiceError::AuthorizationDenied { .. })
    ));
}

#[tokio::test]
async fn re_execute_rejects_running_parent() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
async fn recover_reexecute_without_exact_blocked_execution_fails_closed() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let annotation = json!({
        "type": "recovery_required",
        "blocking_reason": "crash_recovery",
        "blocked_by": "system:crash_recovery",
        "blocked_at": now_rfc3339(),
        "message": "Recovered after server restart",
        "recovery_actions": ["reexecute", "cancel_task"],
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
            error_annotation: Some(Some(annotation.clone())),
            blocked_json: None,
            failed_json: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task recovery annotation saved");

    let failure = service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::Reexecute,
            Some("test".to_owned()),
            Some("resume current work".to_owned()),
        )
        .await
        .expect_err("Task recovery cannot infer a blocked Execution");
    assert!(failure
        .to_string()
        .contains("no blocking recovery annotation"));

    let recovered_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle loads")
        .expect("recovered TaskLifecycle exists");
    assert_eq!(recovered_lifecycle.state, db::TaskLifecycleState::Active);
    let unchanged = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    assert_eq!(unchanged.error_annotation, Some(annotation));
    assert_eq!(unchanged.version, task.version + 1);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("Execution count loads");
    assert_eq!(count, 0);
}

struct BlockedRecoveryFixture {
    db: Arc<SqliteDb>,
    service: TaskService,
    task: db::Task,
    lifecycle: db::TaskLifecycle,
    parent: db::Execution,
    work_unit: db::WorkUnit,
    workspace: Option<db::Workspace>,
    _repo_dir: TempDir,
    _workspace_root: TempDir,
}

async fn blocked_recovery_fixture(requires_workspace: bool) -> BlockedRecoveryFixture {
    blocked_recovery_fixture_for_executor(requires_workspace, "shell").await
}

async fn blocked_recovery_fixture_for_executor(
    requires_workspace: bool,
    executor_type: &str,
) -> BlockedRecoveryFixture {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let workspace_root = TempDir::new().expect("recovery workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, executor_type, "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;

    let (parent, workspace) = if requires_workspace {
        let (execution, workspace) = create_running_repository_execution(
            &db,
            &service,
            &task,
            &agent_id,
            "implementer",
            db::ExecutionPurpose::Implement,
            workspace_root.path(),
            Some("exact blocked parent".to_owned()),
            None,
        )
        .await;
        (execution, Some(workspace))
    } else {
        service
            .create_task_role(
                &task.id,
                "implementer",
                CoordinationMode::Collaborative,
                "{}".to_owned(),
            )
            .await
            .expect("implementer TaskRole creates");
        service
            .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_id.clone()))
            .await
            .expect("Agent joins the exact implementer TaskRole");
        let now = now_rfc3339();
        let work_unit_id = db::new_uuid_v4();
        let work_unit = db::WorkUnitRepo::create(
            &*db,
            db::CreateWorkUnit {
                id: work_unit_id.clone(),
                task_id: task.id.clone(),
                parent_work_unit_id: None,
                title: "Exact recovery target".to_owned(),
                scope: "Recovery admission test target".to_owned(),
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Agent(agent_id.clone())),
                requires_integration: false,
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
                actor_id: Some(agent_id),
                scope_type: "task".to_owned(),
                scope_id: task.id.clone(),
                correlation_id: work_unit_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("work-unit-created:{work_unit_id}")),
                payload_json: json!({"work_unit_id": work_unit_id, "task_id": task.id}).to_string(),
                created_at: now,
            },
        )
        .await
        .expect("exact WorkUnit creates")
        .record;
        let execution = service
            .launch_work_unit_execution(
                &work_unit.id,
                work_unit.version,
                db::ExecutionPurpose::Implement,
                "exact blocked parent".to_owned(),
                Vec::new(),
                None,
            )
            .await
            .expect("parent Execution binds to exact WorkUnit")
            .execution;
        (execution, None)
    };

    let work_unit_id = parent
        .work_unit_id
        .clone()
        .expect("blocked parent has exact WorkUnit identity");
    let work_unit = db::WorkUnitRepo::get_by_id(&*db, &work_unit_id)
        .await
        .expect("WorkUnit lookup succeeds")
        .expect("WorkUnit exists");
    let now = now_rfc3339();
    db::ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: parent.id.clone(),
            status: Some(ExecutionStatus::Failed),
            stop_reason: Some(Some(db::StopReason::ExecutorFailed)),
            stopped_by: Some(Some("system:executor".to_owned())),
            resume_policy: Some(Some(db::ResumePolicy::Manual)),
            stopped_at: Some(Some(now.clone())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some(Some("blocked fixture execution".to_owned())),
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("parent Execution becomes terminal");
    service
        .revoke_active_workspace_lease_for_execution(&task.id, &parent.id)
        .await;

    let blocked =
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .block(
                &task.id,
                crate::task_lifecycle::LifecycleCause::Execution(parent.id.clone()),
                "executor_failed",
                parent.id.clone(),
                format!("test-block:{}", parent.id),
            )
            .await
            .expect("TaskLifecycle enters Blocked");
    let annotation = api_types::TaskBlockingAnnotation {
        annotation_type: api_types::FailureKind::ExecutorFailed,
        blocking_reason: "executor_failed".to_owned(),
        blocked_by: Some("system:executor".to_owned()),
        blocked_at: Some(now.clone()),
        blocked_execution_id: Some(parent.id.clone()),
        artifact: Some(api_types::BlockingArtifact {
            kind: "execution".to_owned(),
            id: Some(parent.id.clone()),
            log_path: None,
        }),
        message: Some("exact blocked recovery fixture".to_owned()),
        hook: None,
        recovery_actions: vec![
            api_types::RecoveryAction::Reexecute,
            api_types::RecoveryAction::CancelTask,
        ],
    };
    let error_annotation = serde_json::to_string(&annotation).expect("blocking annotation JSON");
    let blocked_json = json!({
        "reason": "exact blocked recovery fixture",
        "kind": "executor_failed",
        "execution_id": parent.id.clone(),
        "created_at": now,
    })
    .to_string();
    let task = TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: blocked.task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(error_annotation)),
            blocked_json: Some(Some(blocked_json)),
            failed_json: Some(None),
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("exact recovery annotation persists");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle loads")
        .expect("TaskLifecycle exists");

    BlockedRecoveryFixture {
        db,
        service,
        task,
        lifecycle,
        parent,
        work_unit,
        workspace,
        _repo_dir: repo_dir,
        _workspace_root: workspace_root,
    }
}

fn recovery_admission_input(fixture: &BlockedRecoveryFixture, id: String) -> db::CreateExecution {
    let now = now_rfc3339();
    db::CreateExecution {
        id,
        task_id: fixture.task.id.clone(),
        agent_id: fixture.parent.agent_id.clone(),
        actor_ref: fixture.parent.actor_ref(),
        role: fixture.work_unit.role.clone(),
        purpose: fixture.parent.purpose.clone(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: Some(fixture.parent.id.clone()),
        harness_session_id: None,
        agent_message_id: None,
        last_activity_at: None,
        summary: Some("recovery child".to_owned()),
        logs_path: None,
        before_sha: fixture.parent.before_sha.clone(),
        after_sha: None,
        error: None,
        executor_config_snapshot_json: fixture.parent.executor_config_snapshot_json.clone(),
        workspace_id: fixture.parent.workspace_id.clone(),
        created_at: now.clone(),
        updated_at: now,
    }
}

async fn try_recovery_admission(
    fixture: &BlockedRecoveryFixture,
    input: db::CreateExecution,
    artifact_input_ids: Vec<String>,
) -> crate::Result<(db::Task, db::Execution)> {
    fixture
        .service
        .admit_recovery_work_unit_execution_with_artifact_inputs(
            &fixture.task,
            &fixture.lifecycle,
            &fixture.parent.id,
            &fixture.work_unit.id,
            fixture.work_unit.version,
            input,
            artifact_input_ids,
        )
        .await
}

async fn assert_recovery_admission_rolled_back(
    fixture: &BlockedRecoveryFixture,
    expected_workspace_status: db::WorkspaceStatus,
) {
    let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reload succeeds")
        .expect("Task remains present");
    assert_eq!(task.lifecycle_state, db::TaskLifecycleState::Blocked);
    assert_eq!(task.version, fixture.task.version);
    assert_eq!(task.error_annotation, fixture.task.error_annotation);
    assert_eq!(task.blocked_json, fixture.task.blocked_json);
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
        .await
        .expect("TaskLifecycle reload succeeds")
        .expect("TaskLifecycle remains present");
    assert_eq!(lifecycle, fixture.lifecycle);
    let work_unit = db::WorkUnitRepo::get_by_id(&*fixture.db, &fixture.work_unit.id)
        .await
        .expect("WorkUnit reload succeeds")
        .expect("WorkUnit remains present");
    assert_eq!(work_unit, fixture.work_unit);
    let parent = ExecutionRepo::get_by_id(&*fixture.db, &fixture.parent.id)
        .await
        .expect("parent reload succeeds")
        .expect("parent remains present");
    assert_eq!(parent.status, ExecutionStatus::Failed);
    assert_eq!(parent.parent_execution_id, None);
    let children: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution WHERE task_id = ? AND parent_execution_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("child count loads");
    assert_eq!(children, 0);
    assert!(!TaskLifecycleRepo::has_task_lifecycle_transition(
        &*fixture.db,
        &fixture.task.id,
        &format!("recovery:{}:active", fixture.parent.id),
    )
    .await
    .expect("recovery transition receipt lookup succeeds"));
    let lifecycle_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'task.lifecycle_changed' AND scope_id = ?
           AND correlation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(format!("recovery:{}:active", fixture.parent.id))
    .fetch_one(fixture.db.pool())
    .await
    .expect("recovery lifecycle event count loads");
    assert_eq!(
        lifecycle_events, 0,
        "rejected admission writes no lifecycle event"
    );
    let execution_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND scope_id = ? AND causation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("recovery Execution event count loads");
    assert_eq!(
        execution_events, 0,
        "rejected admission writes no start event"
    );
    let recovery_leases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease
         WHERE work_unit_id = ? AND execution_id IN (
             SELECT id FROM execution WHERE parent_execution_id = ?
         )",
    )
    .bind(&fixture.work_unit.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("recovery lease count loads");
    assert_eq!(
        recovery_leases, 0,
        "rejected admission creates no WorkspaceLease"
    );
    let artifact_bindings: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_artifact_input
         WHERE execution_id IN (
             SELECT id FROM execution WHERE task_id = ? AND parent_execution_id = ?
         )",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("ArtifactInput count loads");
    assert_eq!(artifact_bindings, 0);
    if let Some(workspace) = fixture.workspace.as_ref() {
        let workspace = db::WorkspaceRepo::get_by_id(&*fixture.db, &workspace.id)
            .await
            .expect("Workspace reload succeeds")
            .expect("Workspace remains present");
        assert_eq!(workspace.status, expected_workspace_status);
        let original_workspace = fixture.workspace.as_ref().expect("fixture Workspace");
        assert_eq!(workspace.id, original_workspace.id);
        assert_eq!(workspace.task_id, original_workspace.task_id);
        assert_eq!(workspace.repo_id, original_workspace.repo_id);
        assert_eq!(workspace.worktree_path, original_workspace.worktree_path);
        assert_eq!(workspace.branch, original_workspace.branch);
        assert_eq!(workspace.before_sha, original_workspace.before_sha);
        assert!(db::WorkspaceLeaseRepo::get_active_for_work_unit(
            &*fixture.db,
            &fixture.work_unit.id
        )
        .await
        .expect("active lease lookup succeeds")
        .is_none());
    }
}

#[tokio::test]
async fn recovery_admission_artifact_binding_rejection_rolls_back_lifecycle_and_annotation() {
    let fixture = blocked_recovery_fixture(false).await;
    let result = try_recovery_admission(
        &fixture,
        recovery_admission_input(&fixture, db::new_uuid_v4()),
        vec!["missing-artifact-input".to_owned()],
    )
    .await;
    assert!(
        result.is_err(),
        "missing ArtifactInput must reject admission"
    );
    assert_recovery_admission_rolled_back(&fixture, db::WorkspaceStatus::Ready).await;
}

#[tokio::test]
async fn recovery_admission_rechecks_work_unit_version_inside_lifecycle_transaction() {
    let fixture = blocked_recovery_fixture(false).await;
    sqlx::query(&format!(
        "CREATE TRIGGER recovery_test_bump_work_unit
         AFTER UPDATE OF state ON task_lifecycle
         WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE work_unit SET version = version + 1, updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        fixture.task.id, fixture.work_unit.id
    ))
    .execute(fixture.db.pool())
    .await
    .expect("concurrent WorkUnit revision trigger installs");
    let result = try_recovery_admission(
        &fixture,
        recovery_admission_input(&fixture, db::new_uuid_v4()),
        Vec::new(),
    )
    .await;
    assert!(
        result.is_err(),
        "stale WorkUnit version must reject admission"
    );
    assert_recovery_admission_rolled_back(&fixture, db::WorkspaceStatus::Ready).await;
}

#[tokio::test]
async fn recovery_admission_rechecks_role_membership_inside_lifecycle_transaction() {
    let fixture = blocked_recovery_fixture(false).await;
    sqlx::query(&format!(
        "CREATE TRIGGER recovery_test_suspend_membership
         AFTER UPDATE OF state ON task_lifecycle
         WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE role_membership
             SET status = 'suspended', version = version + 1, updated_at = NEW.updated_at
             WHERE task_role_id = (
                 SELECT id FROM task_role WHERE task_id = NEW.task_id AND role = 'implementer'
             ) AND actor_kind = 'agent' AND actor_id = '{}';
         END",
        fixture.task.id,
        fixture
            .work_unit
            .assigned_actor
            .as_ref()
            .expect("WorkUnit actor is assigned")
            .id()
    ))
    .execute(fixture.db.pool())
    .await
    .expect("concurrent membership loss trigger installs");
    let result = try_recovery_admission(
        &fixture,
        recovery_admission_input(&fixture, db::new_uuid_v4()),
        Vec::new(),
    )
    .await;
    assert!(
        result.is_err(),
        "inactive exact membership must reject admission"
    );
    assert_recovery_admission_rolled_back(&fixture, db::WorkspaceStatus::Ready).await;
    let membership_status: String = sqlx::query_scalar(
        "SELECT rm.status FROM role_membership rm
         JOIN task_role tr ON tr.id = rm.task_role_id
         WHERE tr.task_id = ? AND tr.role = 'implementer' AND rm.actor_kind = 'agent'
           AND rm.actor_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(
        fixture
            .work_unit
            .assigned_actor
            .as_ref()
            .expect("WorkUnit actor is assigned")
            .id(),
    )
    .fetch_one(fixture.db.pool())
    .await
    .expect("membership reload succeeds");
    assert_eq!(membership_status, "active");
}

#[tokio::test]
async fn recovery_rejects_an_unusable_exact_workspace_without_mutating_recovery_state() {
    let fixture = blocked_recovery_fixture(true).await;
    let workspace = fixture.workspace.as_ref().expect("fixture has Workspace");
    db::WorkspaceRepo::update_status(
        &*fixture.db,
        &workspace.id,
        db::WorkspaceStatus::Error,
        Some("test workspace is unusable".to_owned()),
        &now_rfc3339(),
    )
    .await
    .expect("exact Workspace becomes unusable");
    let result = fixture
        .service
        .recover_task(
            fixture.task.id.clone(),
            api_types::RecoveryAction::Reexecute,
            Some("test".to_owned()),
            None,
        )
        .await;
    assert!(result.is_err(), "unusable exact Workspace must fail closed");
    assert_recovery_admission_rolled_back(&fixture, db::WorkspaceStatus::Error).await;
}

#[tokio::test]
async fn recovery_uses_the_parent_work_units_workspace_when_task_has_siblings() {
    let fixture = blocked_recovery_fixture(true).await;
    let integration = fixture.workspace.as_ref().expect("fixture has Workspace");
    let sibling_service = crate::work_unit_service::WorkUnitService::new(
        Arc::clone(&fixture.db),
        Arc::new(EventBus::new(16)),
        fixture._workspace_root.path().to_path_buf(),
        Arc::new(::workspace::RepoCacheLockManager::new()),
        Arc::new(crate::WorkspaceExecutionLockManager::new()),
    );
    let human_id = seed_human_user(&fixture.db).await;
    let now = now_rfc3339();
    ProjectMemberRepo::add_member(
        &*fixture.db,
        db::CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: fixture.task.project_id.clone(),
            user_id: human_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("second workspace preparer joins the Project");
    let sibling = sibling_service
        .create(
            crate::collaboration_service::CollaborationActorSource::Human(human_id.clone()),
            crate::work_unit_service::CreateWorkUnitInput {
                task_id: fixture.task.id.clone(),
                title: "Sibling recovery target".to_owned(),
                scope: "Must not replace the blocked parent's exact Workspace".to_owned(),
                role: fixture.work_unit.role.clone(),
                parent_work_unit_id: None,
                assigned_actor: fixture.work_unit.assigned_actor.clone(),
                requires_integration: true,
                provenance: None,
            },
        )
        .await
        .expect("sibling WorkUnit creates");
    let sibling_workspace = sibling_service
        .prepare_workspace(
            crate::collaboration_service::CollaborationActorSource::Human(human_id),
            &sibling.id,
        )
        .await
        .expect("sibling WorkUnit gets its own Workspace");
    assert_ne!(integration.id, sibling_workspace.id);
    assert!(crate::task_service::execution::has_exact_work_unit_target(
        &fixture.db,
        &fixture.task.id,
        &fixture.parent
    )
    .await
    .expect("parent target check succeeds"));

    let mut sibling_bound_parent = fixture.parent.clone();
    sibling_bound_parent.workspace_id = Some(sibling_workspace.id.clone());
    assert!(!crate::task_service::execution::has_exact_work_unit_target(
        &fixture.db,
        &fixture.task.id,
        &sibling_bound_parent
    )
    .await
    .expect("misbound parent target check succeeds"));
    assert_eq!(
        db::WorkUnitWorkspaceRepo::get_scope_by_id(&*fixture.db, &sibling_workspace.id)
            .await
            .expect("sibling scope loads")
            .expect("sibling scope exists")
            .work_unit_id
            .as_deref(),
        Some(sibling.id.as_str())
    );
}

#[tokio::test]
async fn explicit_multi_work_unit_recovery_keeps_repository_workspaces_and_leases_separate() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf());
    let (project_id, repo_id, repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let agent_a = seed_agent_with_executor_type(&db, "shell", "{}").await;
    let agent_b = seed_agent_with_executor_type(&db, "shell", "{}").await;
    let (parent_a, workspace_a) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_a,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("repository parent A".to_owned()),
        None,
    )
    .await;
    let (parent_b, workspace_b) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_b,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("repository parent B".to_owned()),
        None,
    )
    .await;
    assert_ne!(parent_a.work_unit_id, parent_b.work_unit_id);
    assert_ne!(workspace_a.id, workspace_b.id);

    crate::CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
        .run_recovery()
        .await
        .expect("CrashRecovery stops both exact repository Executions");
    let task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task remains");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle reloads")
        .expect("TaskLifecycle remains");
    assert_eq!(
        lifecycle.reason_kind.as_deref(),
        Some("recovery_target_ambiguous")
    );
    let work_unit_a = db::WorkUnitRepo::get_by_id(
        &*db,
        parent_a
            .work_unit_id
            .as_deref()
            .expect("A WorkUnit identity"),
    )
    .await
    .expect("A WorkUnit loads")
    .expect("A WorkUnit exists");
    let child_a = service
        .recover_work_unit_execution(
            task.id.clone(),
            parent_a.id.clone(),
            api_types::RecoverWorkUnitExecutionRequest {
                work_unit_id: work_unit_a.id.clone(),
                expected_task_version: task.version,
                expected_lifecycle_version: lifecycle.version,
                expected_work_unit_version: work_unit_a.version,
                idempotency_key: "repository-recovery-a".to_owned(),
                context: None,
            },
            "repository-recovery-operator",
        )
        .await
        .expect("A admits with its own WorkspaceLease");
    assert_eq!(
        child_a.execution.workspace_id.as_deref(),
        Some(workspace_a.id.as_str())
    );
    let lease_a: Option<(String, String, String)> = sqlx::query_as(
        "SELECT work_unit_id, workspace_id, status FROM workspace_lease
         WHERE execution_id = ? AND status = 'active'",
    )
    .bind(&child_a.execution.id)
    .fetch_optional(db.pool())
    .await
    .expect("A lease loads");
    assert_eq!(
        lease_a,
        Some((
            work_unit_a.id.clone(),
            workspace_a.id.clone(),
            "active".to_owned()
        ))
    );
    let sibling_active_leases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease
         WHERE execution_id = ? AND status = 'active'",
    )
    .bind(&parent_b.id)
    .fetch_one(db.pool())
    .await
    .expect("B parent lease status loads");
    assert_eq!(sibling_active_leases, 0);

    let task_after_a = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads after A")
        .expect("Task remains");
    let lifecycle_after_a = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads after A")
        .expect("lifecycle remains");
    let work_unit_b = db::WorkUnitRepo::get_by_id(
        &*db,
        parent_b
            .work_unit_id
            .as_deref()
            .expect("B WorkUnit identity"),
    )
    .await
    .expect("B WorkUnit loads")
    .expect("B WorkUnit exists");
    let child_b = service
        .recover_work_unit_execution(
            task.id.clone(),
            parent_b.id.clone(),
            api_types::RecoverWorkUnitExecutionRequest {
                work_unit_id: work_unit_b.id.clone(),
                expected_task_version: task_after_a.version,
                expected_lifecycle_version: lifecycle_after_a.version,
                expected_work_unit_version: work_unit_b.version,
                idempotency_key: "repository-recovery-b".to_owned(),
                context: None,
            },
            "repository-recovery-operator",
        )
        .await
        .expect("B admits against its own Workspace while Task is Active");
    assert_eq!(
        child_b.execution.workspace_id.as_deref(),
        Some(workspace_b.id.as_str())
    );
    let lease_b: Option<(String, String, String)> = sqlx::query_as(
        "SELECT work_unit_id, workspace_id, status FROM workspace_lease
         WHERE execution_id = ? AND status = 'active'",
    )
    .bind(&child_b.execution.id)
    .fetch_optional(db.pool())
    .await
    .expect("B lease loads");
    assert_eq!(
        lease_b,
        Some((
            work_unit_b.id.clone(),
            workspace_b.id.clone(),
            "active".to_owned()
        ))
    );
    assert_ne!(work_unit_a.id, work_unit_b.id);
    assert_ne!(workspace_a.id, workspace_b.id);
    let old_lease_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease
         WHERE execution_id IN (?, ?) AND status = 'active'",
    )
    .bind(&parent_a.id)
    .bind(&parent_b.id)
    .fetch_one(db.pool())
    .await
    .expect("old lease count loads");
    assert_eq!(old_lease_count, 0);
    drop(repo_dir);
}

#[tokio::test]
async fn concurrent_dispatcher_ticks_recover_post_commit_auto_start_after_unpause() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let workspace_root = TempDir::new().expect("recovery workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let agent = db::AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let mut snapshot =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("exact Agent routing snapshot builds")
            .expect("Agent routing snapshot exists");
    let mut snapshot_value: Value = serde_json::from_str(&snapshot).expect("snapshot parses");
    snapshot_value["harness_capabilities"] = json!({
        "schema_version": 1,
        "capabilities": { "resume": "native" }
    });
    snapshot = snapshot_value.to_string();

    let (parent, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("automatic recovery dispatcher race".to_owned()),
        Some(snapshot),
    )
    .await;
    ExecutionRepo::record_harness_session_result(
        &*db,
        &parent.id,
        "codex-session-for-dispatch-race",
        &now_rfc3339(),
    )
    .await
    .expect("parent receives an exact resumable HarnessSession");
    assert_eq!(
        ExecutionRepo::claim_provider_start(
            &*db,
            &parent.id,
            provider_start_receipt_for_test(&parent, "execution.provider_start_attempted",),
        )
        .await
        .expect("fixture records that the existing parent crossed provider Start"),
        db::ProviderStartClaim::Claimed
    );
    ExecutionRepo::record_provider_start_outcome(
        &*db,
        &parent.id,
        provider_start_receipt_for_test(&parent, "execution.provider_start_accepted"),
    )
    .await
    .expect("fixture records the parent's accepted provider Start");

    let daemon_id = agent.daemon_id.as_deref().expect("Agent has its daemon");
    let changed = sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
        .bind(crate::embedded_daemon::embedded_machine_id())
        .bind(daemon_id)
        .execute(db.pool())
        .await
        .expect("fixture daemon selects the embedded provider");
    assert_eq!(changed.rows_affected(), 1);

    crate::CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
        .run_recovery()
        .await
        .expect("CrashRecovery marks the exact resumable WorkUnit Auto");
    let stopped_parent = ExecutionRepo::get_by_id(&*db, &parent.id)
        .await
        .expect("Auto parent reloads")
        .expect("Auto parent remains historical");
    assert_eq!(stopped_parent.status, ExecutionStatus::Cancelled);
    assert_eq!(stopped_parent.resume_policy, Some(db::ResumePolicy::Auto));
    assert_eq!(
        stopped_parent.workspace_id.as_deref(),
        Some(workspace.id.as_str())
    );
    assert_eq!(
        crate::task_service::resumable_external_session(
            &db,
            &stopped_parent,
            Some(&agent_id),
            Some(&workspace.id),
        )
        .await
        .expect("HarnessSession resumability preflight succeeds")
        .as_deref(),
        Some("codex-session-for-dispatch-race")
    );

    let admission_barrier = Arc::new(tokio::sync::Barrier::new(3));
    let post_commit_barrier = Arc::new(tokio::sync::Barrier::new(2));
    let executor = Arc::new(RecoveryDispatchExecutor::default());
    let task_service = Arc::new(
        service
            .clone()
            .with_task_executor(executor.clone())
            .with_automatic_recovery_precommit_barrier(Arc::clone(&admission_barrier)),
    );
    let first = crate::TaskDispatcher::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        Arc::clone(&task_service),
    )
    .with_after_auto_admission_barrier(Arc::clone(&post_commit_barrier));
    let second = crate::TaskDispatcher::new(Arc::clone(&db), Arc::clone(&event_bus), task_service)
        .with_after_auto_admission_barrier(Arc::clone(&post_commit_barrier));
    let first_tick = tokio::spawn(async move { first.check_once().await });
    let second_tick = tokio::spawn(async move { second.check_once().await });
    admission_barrier.wait().await;
    admission_barrier.wait().await;

    // The fresh child and its execution.started receipt are committed, but
    // neither dispatcher has crossed start_execution() yet.
    post_commit_barrier.wait().await;
    let committed_children: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&parent.id)
            .fetch_one(db.pool())
            .await
            .expect("post-commit child count loads before provider dispatch");
    assert_eq!(committed_children, 1);
    db::AgentRepo::set_paused(&*db, &agent_id, true)
        .await
        .expect("Agent pauses after the child commit");
    post_commit_barrier.wait().await;

    let first_count = first_tick
        .await
        .expect("first dispatcher tick joins")
        .expect("first dispatcher tick succeeds");
    let second_count = second_tick
        .await
        .expect("second dispatcher tick joins")
        .expect("second dispatcher tick succeeds");
    assert_eq!(first_count + second_count, 0);
    assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let child_id: String =
        sqlx::query_scalar("SELECT id FROM execution WHERE parent_execution_id = ?")
            .bind(&parent.id)
            .fetch_one(db.pool())
            .await
            .expect("committed Auto child identity loads");
    let child = ExecutionRepo::get_by_id(&*db, &child_id)
        .await
        .expect("child reload succeeds")
        .expect("committed Auto child remains");
    assert_eq!(child.status, ExecutionStatus::Running);
    assert_eq!(
        child.parent_execution_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(child.work_unit_id, parent.work_unit_id);
    assert_eq!(child.workspace_id, parent.workspace_id);
    assert_eq!(
        child.actor_ref(),
        Some(db::ActorRef::Agent(agent_id.clone()))
    );
    assert_eq!(
        execution_external_session_id(&db, &child).await.as_deref(),
        Some("codex-session-for-dispatch-race"),
        "recovery retains the exact reusable HarnessSession"
    );
    let child_workspace_lease = WorkspaceLeaseRepo::get_active_for_work_unit(
        &*db,
        child
            .work_unit_id
            .as_deref()
            .expect("child WorkUnit identity"),
    )
    .await
    .expect("child WorkspaceLease loads")
    .expect("child keeps one active WorkspaceLease");
    assert_eq!(child_workspace_lease.execution_id, child.id);
    assert_eq!(child_workspace_lease.workspace_id, child.workspace_id);
    let child_start_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&child.id)
    .fetch_one(db.pool())
    .await
    .expect("child admission event count loads");
    let child_pending_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_pending' AND entity_id = ?",
    )
    .bind(&child.id)
    .fetch_one(db.pool())
    .await
    .expect("child pending receipt count loads");
    let child_attempts_while_paused: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_attempted' AND entity_id = ?",
    )
    .bind(&child.id)
    .fetch_one(db.pool())
    .await
    .expect("no-attempt receipt count loads while Agent paused");
    assert_eq!(child_start_events, 1);
    assert_eq!(child_pending_events, 1);
    assert_eq!(child_attempts_while_paused, 0);
    assert!(
        ExecutionRepo::list_work_unit_auto_recovery_candidates(&*db)
            .await
            .expect("Auto parent candidates reload")
            .is_empty(),
        "the committed child has consumed its Auto parent candidate"
    );

    sqlx::query("UPDATE execution SET last_activity_at = '1970-01-01T00:00:00+00:00' WHERE id = ?")
        .bind(&child.id)
        .execute(db.pool())
        .await
        .expect("pending provider Start is made old enough for the stall monitor");
    let monitor = crate::HeartbeatMonitor::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_execution_stall_timeout(std::time::Duration::from_secs(1));
    assert_eq!(
        monitor
            .check_once()
            .await
            .expect("heartbeat monitor checks pending Start"),
        0,
        "an unattempted provider Start is queued work, not a stalled Execution"
    );

    let shutdown_service = Arc::new(
        TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(executor.clone()),
    );
    crate::GracefulShutdown::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_task_service(shutdown_service)
        .shutdown()
        .await
        .expect("graceful server shutdown preserves the queued provider Start");

    assert_eq!(
        crate::CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
            .run_recovery()
            .await
            .expect("server startup CrashRecovery runs"),
        0,
        "server restart preserves the admitted child for dispatcher recovery"
    );
    assert_eq!(
        ExecutionRepo::get_by_id(&*db, &child.id)
            .await
            .expect("child reloads after restart reconciliation")
            .expect("child remains present")
            .status,
        ExecutionStatus::Running,
        "the exact child stays Running until its provider attempt is claimed"
    );

    db::AgentRepo::set_paused(&*db, &agent_id, false)
        .await
        .expect("Agent unpauses");

    // A fresh service and dispatcher over the durable DB simulate restart in
    // the admitted-before-provider interval.
    let restarted_service = Arc::new(
        TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(executor.clone())
            .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
            .with_workspace_root(workspace_root.path().to_path_buf())
            .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new())),
    );
    let provider_start_barrier = Arc::new(tokio::sync::Barrier::new(2));
    let restarted_first = crate::TaskDispatcher::new(
        Arc::clone(&db),
        Arc::clone(&event_bus),
        Arc::clone(&restarted_service),
    )
    .with_after_pending_snapshot_barrier(Arc::clone(&provider_start_barrier));
    let restarted_second =
        crate::TaskDispatcher::new(Arc::clone(&db), Arc::clone(&event_bus), restarted_service)
            .with_after_pending_snapshot_barrier(provider_start_barrier);
    let (first_dispatch, second_dispatch) =
        tokio::join!(restarted_first.check_once(), restarted_second.check_once());
    let accepted_first = first_dispatch.expect("first restarted dispatcher resolves pending start");
    let accepted_second =
        second_dispatch.expect("second restarted dispatcher resolves pending start");
    assert!(accepted_first + accepted_second > 0);
    assert!(accepted_first + accepted_second <= 2);

    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        executor.started.notified(),
    )
    .await
    .expect("one accepted provider start runs the exact child");
    assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let child_attempt_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_attempted' AND entity_id = ?",
    )
    .bind(&child.id)
    .fetch_one(db.pool())
    .await
    .expect("concurrent dispatchers share one attempt receipt");
    assert_eq!(child_attempt_receipts, 1);
    let child_accepted_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_accepted' AND entity_id = ?",
    )
    .bind(&child.id)
    .fetch_one(db.pool())
    .await
    .expect("accepted provider receipt count loads");
    assert_eq!(child_accepted_receipts, 1);
    let children: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&parent.id)
            .fetch_one(db.pool())
            .await
            .expect("auto child count loads after concurrent ticks");
    assert_eq!(children, 1);
    let parent_after = ExecutionRepo::get_by_id(&*db, &parent.id)
        .await
        .expect("parent reloads after concurrent recovery")
        .expect("parent remains historical");
    assert_eq!(parent_after.status, ExecutionStatus::Cancelled);
    assert_eq!(parent_after.resume_policy, Some(db::ResumePolicy::Auto));
}

#[tokio::test]
async fn concurrent_recovery_admissions_create_one_child_for_one_blocked_execution() {
    let fixture = blocked_recovery_fixture(false).await;
    let service_one = fixture.service.clone();
    let service_two = fixture.service.clone();
    let input_one = recovery_admission_input(&fixture, db::new_uuid_v4());
    let input_two = recovery_admission_input(&fixture, db::new_uuid_v4());
    let one = service_one.admit_recovery_work_unit_execution_with_artifact_inputs(
        &fixture.task,
        &fixture.lifecycle,
        &fixture.parent.id,
        &fixture.work_unit.id,
        fixture.work_unit.version,
        input_one,
        Vec::new(),
    );
    let two = service_two.admit_recovery_work_unit_execution_with_artifact_inputs(
        &fixture.task,
        &fixture.lifecycle,
        &fixture.parent.id,
        &fixture.work_unit.id,
        fixture.work_unit.version,
        input_two,
        Vec::new(),
    );
    let (one, two) = tokio::join!(one, two);
    assert_ne!(one.is_ok(), two.is_ok(), "exactly one admission commits");
    let children: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT id, parent_execution_id FROM execution
         WHERE task_id = ? AND parent_execution_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_all(fixture.db.pool())
    .await
    .expect("recovery children load");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].1.as_deref(), Some(fixture.parent.id.as_str()));
    assert_eq!(
        db::WorkUnitRepo::get_by_id(&*fixture.db, &fixture.work_unit.id)
            .await
            .expect("WorkUnit reload succeeds")
            .expect("WorkUnit remains present"),
        fixture.work_unit
    );
    let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reload succeeds")
        .expect("Task remains present");
    assert_eq!(task.lifecycle_state, db::TaskLifecycleState::Active);
    assert_eq!(task.version, fixture.task.version + 1);
    assert_eq!(task.error_annotation, None);
    assert_eq!(task.blocked_json, None);
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
            .await
            .expect("TaskLifecycle reload succeeds")
            .expect("TaskLifecycle remains present")
            .state,
        db::TaskLifecycleState::Active
    );
    assert!(TaskLifecycleRepo::has_task_lifecycle_transition(
        &*fixture.db,
        &task.id,
        &format!("recovery:{}:active", fixture.parent.id),
    )
    .await
    .expect("transition receipt lookup succeeds"));
    let lifecycle_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'task.lifecycle_changed' AND scope_id = ?
           AND correlation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(format!("recovery:{}:active", fixture.parent.id))
    .fetch_one(fixture.db.pool())
    .await
    .expect("committed recovery lifecycle event count loads");
    assert_eq!(lifecycle_events, 1);
    let execution_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND scope_id = ? AND causation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("committed recovery Execution event count loads");
    assert_eq!(execution_events, 1);
}

#[tokio::test]
async fn failed_external_start_after_durable_recovery_does_not_replay_old_attempt() {
    let fixture = blocked_recovery_fixture(false).await;
    let original_input = recovery_admission_input(&fixture, db::new_uuid_v4());
    let original_id = original_input.id.clone();
    let (_recovered_task, child) = try_recovery_admission(&fixture, original_input, Vec::new())
        .await
        .expect("atomic recovery admission commits");
    assert_eq!(
        child.parent_execution_id.as_deref(),
        Some(fixture.parent.id.as_str())
    );

    // Simulate the external provider failing after the durable local
    // admission. The admitted Execution is terminalized and annotated; retrying
    // the stale admission snapshot still cannot mint another child.
    assert!(fixture
        .service
        .start_execution(child.id.clone())
        .await
        .is_err());
    let stale_replay = try_recovery_admission(
        &fixture,
        recovery_admission_input(&fixture, db::new_uuid_v4()),
        Vec::new(),
    )
    .await;
    assert!(stale_replay.is_err());
    let children: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, parent_execution_id, status FROM execution
         WHERE task_id = ? AND (parent_execution_id = ? OR id = ?)",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .bind(&original_id)
    .fetch_all(fixture.db.pool())
    .await
    .expect("durable recovery execution lineage loads");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].0, child.id);
    assert_eq!(children[0].1.as_deref(), Some(fixture.parent.id.as_str()));
    assert!(matches!(children[0].2.as_str(), "failed" | "running"));
    let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reload succeeds")
        .expect("Task remains present");
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &task.id)
        .await
        .expect("TaskLifecycle reload succeeds")
        .expect("TaskLifecycle remains present");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
    assert_eq!(task.lifecycle_state, db::TaskLifecycleState::Blocked);
    let annotation: api_types::TaskBlockingAnnotation = serde_json::from_str(
        task.error_annotation
            .as_deref()
            .expect("new child has a recovery annotation"),
    )
    .expect("new child annotation parses");
    assert_eq!(
        annotation.blocked_execution_id.as_deref(),
        Some(child.id.as_str())
    );
    assert!(TaskLifecycleRepo::has_task_lifecycle_transition(
        &*fixture.db,
        &fixture.task.id,
        &format!("recovery:{}:active", fixture.parent.id),
    )
    .await
    .expect("durable transition receipt lookup succeeds"));
    let execution_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND entity_id = ? AND causation_id = ?",
    )
    .bind(&child.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("durable recovery start event count loads");
    assert_eq!(
        execution_events, 1,
        "provider failure does not duplicate admission event"
    );
}

async fn launch_work_unit_for_override_snapshot(
    db: &SqliteDb,
    service: &TaskService,
    task: &Task,
    agent_id: &str,
    overrides: Option<ExecutionOverrides>,
) -> db::Execution {
    let now = now_rfc3339();
    let work_unit_id = db::new_uuid_v4();
    let work_unit = db::WorkUnitRepo::create(
        db,
        db::CreateWorkUnit {
            id: work_unit_id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Snapshot test WorkUnit".to_owned(),
            scope: "Exact assigned execution target".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(db::ActorRef::Agent(agent_id.to_owned())),
            requires_integration: false,
            provenance: None,
            created_by: db::ActorRef::Agent(agent_id.to_owned()),
            created_at: now.clone(),
        },
        db::CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: work_unit_id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: work_unit_id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("work-unit-created:{work_unit_id}")),
            payload_json: json!({
                "work_unit_id": work_unit_id,
                "task_id": task.id,
                "version": 1,
                "status": "open"
            })
            .to_string(),
            created_at: now,
        },
    )
    .await
    .expect("exact WorkUnit creates")
    .record;
    service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "snapshot override test".to_owned(),
            Vec::new(),
            overrides,
        )
        .await
        .expect("exact WorkUnit Execution launches")
        .execution
}

async fn create_simple_execution_work_unit(
    db: &SqliteDb,
    service: &TaskService,
    task: &Task,
    agent_id: &str,
) -> db::WorkUnit {
    if TaskRoleRepo::get_by_task_and_role(db, &task.id, "implementer")
        .await
        .expect("implementer TaskRole lookup succeeds")
        .is_none()
    {
        service
            .create_task_role(
                &task.id,
                "implementer",
                CoordinationMode::Independent,
                "{}".to_owned(),
            )
            .await
            .expect("implementer TaskRole creates");
    }
    if crate::task_service::current_role_memberships_authoritative(db, &task.id, "implementer")
        .await
        .expect("TaskRole memberships load")
        .is_none_or(|memberships| {
            crate::task_service::memberships::active_agent_membership(&memberships, agent_id)
                .is_none()
        })
    {
        service
            .add_task_role_member(
                &task.id,
                "implementer",
                ActorRef::Agent(agent_id.to_owned()),
            )
            .await
            .expect("Agent joins the exact implementer TaskRole");
    }
    let now = now_rfc3339();
    let id = db::new_uuid_v4();
    db::WorkUnitRepo::create(
        db,
        db::CreateWorkUnit {
            id: id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Atomic launch target".to_owned(),
            scope: "Exact launch admission test target".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(db::ActorRef::Agent(agent_id.to_owned())),
            requires_integration: false,
            provenance: None,
            created_by: db::ActorRef::Agent(agent_id.to_owned()),
            created_at: now.clone(),
        },
        db::CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("work-unit-created:{id}")),
            payload_json: json!({"work_unit_id": id, "task_id": task.id}).to_string(),
            created_at: now,
        },
    )
    .await
    .expect("exact WorkUnit creates")
    .record
}

async fn terminal_work_unit_execution(
    db: &SqliteDb,
    service: &TaskService,
    task: &Task,
    agent_id: &str,
    terminal_status: ExecutionStatus,
    external_session_id: Option<&str>,
    snapshot_override: Option<String>,
) -> db::Execution {
    let work_unit = create_simple_execution_work_unit(db, service, task, agent_id).await;
    let mut execution = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "exact WorkUnit parent Execution".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("parent Execution admits through its exact WorkUnit")
        .execution;
    if let Some(snapshot) = snapshot_override {
        execution = ExecutionRepo::update(
            db,
            db::UpdateExecution {
                id: execution.id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: Some(Some(snapshot)),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("parent snapshot override stores");
    }
    if let Some(external_session_id) = external_session_id {
        execution = ExecutionRepo::record_harness_session_result(
            db,
            &execution.id,
            external_session_id,
            &now_rfc3339(),
        )
        .await
        .expect("exact parent HarnessSession records");
    }
    execution = ExecutionRepo::update(
        db,
        db::UpdateExecution {
            id: execution.id.clone(),
            status: Some(terminal_status),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now_rfc3339())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("parent Execution becomes terminal");
    execution
}

struct AdmissionSideEffectSnapshot {
    task: db::Task,
    lifecycle: db::TaskLifecycle,
    work_unit: db::WorkUnit,
    lifecycle_transitions: i64,
    admission_events: i64,
    executions: i64,
    workspace_leases: i64,
    artifact_inputs: i64,
}

async fn admission_side_effect_snapshot(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
) -> AdmissionSideEffectSnapshot {
    AdmissionSideEffectSnapshot {
        task: TaskRepo::get_by_id(db, task_id, false)
            .await
            .expect("Task loads")
            .expect("Task exists"),
        lifecycle: TaskLifecycleRepo::get_task_lifecycle(db, task_id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists"),
        work_unit: db::WorkUnitRepo::get_by_id(db, work_unit_id)
            .await
            .expect("WorkUnit loads")
            .expect("WorkUnit exists"),
        lifecycle_transitions: sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_one(db.pool())
        .await
        .expect("lifecycle transition count loads"),
        admission_events: sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE scope_type = 'task' AND scope_id = ?
               AND event_type IN ('task.lifecycle_changed', 'execution.started')",
        )
        .bind(task_id)
        .fetch_one(db.pool())
        .await
        .expect("admission event count loads"),
        executions: sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(task_id)
            .fetch_one(db.pool())
            .await
            .expect("Execution count loads"),
        workspace_leases: sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_lease WHERE task_id = ? AND work_unit_id = ?",
        )
        .bind(task_id)
        .bind(work_unit_id)
        .fetch_one(db.pool())
        .await
        .expect("WorkspaceLease count loads"),
        artifact_inputs: sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution_artifact_input WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_one(db.pool())
        .await
        .expect("ArtifactInput count loads"),
    }
}

async fn assert_admission_side_effects_unchanged(
    db: &SqliteDb,
    task_id: &str,
    work_unit_id: &str,
    before: &AdmissionSideEffectSnapshot,
) {
    assert_eq!(
        TaskRepo::get_by_id(db, task_id, false)
            .await
            .expect("Task reload succeeds")
            .expect("Task remains present"),
        before.task
    );
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(db, task_id)
            .await
            .expect("TaskLifecycle reload succeeds")
            .expect("TaskLifecycle remains present"),
        before.lifecycle
    );
    assert_eq!(
        db::WorkUnitRepo::get_by_id(db, work_unit_id)
            .await
            .expect("WorkUnit reload succeeds")
            .expect("WorkUnit remains present"),
        before.work_unit
    );
    let after = admission_side_effect_snapshot(db, task_id, work_unit_id).await;
    assert_eq!(after.lifecycle_transitions, before.lifecycle_transitions);
    assert_eq!(after.admission_events, before.admission_events);
    assert_eq!(after.executions, before.executions);
    assert_eq!(after.workspace_leases, before.workspace_leases);
    assert_eq!(after.artifact_inputs, before.artifact_inputs);
}

#[tokio::test]
async fn ordinary_launch_snapshot_failure_does_not_activate_task() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id =
        seed_agent_with_executor_type(&db, "shell", r#"{"fallbacks":"this must be an array"}"#)
            .await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;

    let error = match service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "snapshot failure must not activate lifecycle".to_owned(),
            Vec::new(),
            None,
        )
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("invalid fallback configuration rejects before admission"),
    };
    assert!(error.to_string().contains("fallbacks"));
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
}

#[tokio::test]
async fn ordinary_launch_artifact_rejection_rolls_back_activation_receipt_and_events() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;

    let result = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "invalid artifact admission".to_owned(),
            vec!["missing-launch-artifact".to_owned()],
            None,
        )
        .await;
    assert!(result.is_err(), "unknown ArtifactInput rejects admission");
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
}

#[tokio::test]
async fn ordinary_provider_start_failure_keeps_one_admitted_execution_and_event() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let admitted = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "provider start occurs after durable admission".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("Execution admission commits before provider start")
        .execution;

    let provider_result = service.start_execution(admitted.id.clone()).await;
    assert!(
        provider_result.is_err(),
        "unavailable local provider rejects start"
    );
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM execution WHERE task_id = ? AND work_unit_id = ?")
            .bind(&task.id)
            .bind(&work_unit.id)
            .fetch_all(db.pool())
            .await
            .expect("admitted Execution rows load");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, admitted.id);
    let start_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND entity_id = ?",
    )
    .bind(&admitted.id)
    .fetch_one(db.pool())
    .await
    .expect("Execution start event count loads");
    assert_eq!(start_events, 1);
    let transitions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_lifecycle_transition
         WHERE task_id = ? AND idempotency_key = ?",
    )
    .bind(&task.id)
    .bind(format!("execution-start:{}:active:1", task.id))
    .fetch_one(db.pool())
    .await
    .expect("activation receipt count loads");
    assert_eq!(transitions, 1);
}

#[derive(Clone, Copy, Debug)]
enum ProviderStartClaimAuthorityMutation {
    SuspendMembership,
    ReassignWorkUnit,
    CloseWorkUnit,
    ChangeWorkUnitVersion,
    RevokeLease,
    ExpireLease,
}

async fn assert_provider_start_claim_rejects_authority_race(
    mutation: ProviderStartClaimAuthorityMutation,
) {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let snapshot =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("exact Agent routing snapshot builds")
            .expect("Agent snapshot exists");
    let workspace_root = TempDir::new().expect("workspace root creates");
    let executor = Arc::new(RecoveryDispatchExecutor::default());
    let claim_barrier = Arc::new(tokio::sync::Barrier::new(2));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_task_executor(executor.clone())
        .with_provider_start_claim_barrier(Arc::clone(&claim_barrier));
    let (execution, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("provider Start authority race fixture".to_owned()),
        Some(snapshot),
    )
    .await;
    let daemon_id = agent.daemon_id.as_deref().expect("Agent has its daemon");
    let changed = sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
        .bind(crate::embedded_daemon::embedded_machine_id())
        .bind(daemon_id)
        .execute(db.pool())
        .await
        .expect("fixture routes provider Start to the embedded executor");
    assert_eq!(changed.rows_affected(), 1);
    let work_unit_id = execution.work_unit_id.as_deref().expect("WorkUnit id");
    let lease = WorkspaceLeaseRepo::get_active_for_work_unit(&*db, work_unit_id)
        .await
        .expect("exact WorkUnit lease loads")
        .expect("admission creates the exact WorkUnit lease");

    let start_service = service.clone();
    let start_execution_id = execution.id.clone();
    let mut start =
        tokio::spawn(async move { start_service.start_execution(start_execution_id).await });
    // The service has completed all ordinary preflight and provider parameter
    // preparation, and is stopped immediately before the durable claim.
    if tokio::time::timeout(std::time::Duration::from_secs(20), claim_barrier.wait())
        .await
        .is_err()
    {
        let early_result =
            tokio::time::timeout(std::time::Duration::from_secs(1), &mut start).await;
        panic!("provider start did not reach the claim barrier; early result: {early_result:?}");
    }
    match mutation {
        ProviderStartClaimAuthorityMutation::SuspendMembership => {
            let (_, membership) = db::RoleMembershipRepo::list_by_task(&*db, &task.id, false)
                .await
                .expect("TaskRole memberships load")
                .into_iter()
                .find(|(_, membership)| membership.actor_id == agent_id)
                .expect("Agent membership exists");
            db::RoleMembershipRepo::update(
                &*db,
                db::UpdateRoleMembership {
                    id: membership.id,
                    expected_version: membership.version,
                    status: db::RoleMembershipStatus::Suspended,
                    updated_at: now_rfc3339(),
                    ended_at: None,
                },
            )
            .await
            .expect("RoleMembership suspends after preflight");
        }
        ProviderStartClaimAuthorityMutation::ReassignWorkUnit => {
            let replacement_agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
            service
                .add_task_role_member(
                    &task.id,
                    "implementer",
                    ActorRef::Agent(replacement_agent_id.clone()),
                )
                .await
                .expect("replacement Agent joins the same TaskRole");
            sqlx::query("DROP TRIGGER work_unit_allocation_running_guard")
                .execute(db.pool())
                .await
                .expect("isolated fixture removes the running allocation guard");
            let changed = sqlx::query(
                "UPDATE work_unit SET assigned_actor_kind = 'agent', assigned_actor_id = ?,
                     version = version + 1, updated_at = ? WHERE id = ?",
            )
            .bind(replacement_agent_id)
            .bind(now_rfc3339())
            .bind(work_unit_id)
            .execute(db.pool())
            .await
            .expect("fixture reassigns the WorkUnit after preflight");
            assert_eq!(changed.rows_affected(), 1);
        }
        ProviderStartClaimAuthorityMutation::CloseWorkUnit => {
            sqlx::query("DROP TRIGGER work_unit_completion_running_guard")
                .execute(db.pool())
                .await
                .expect("isolated fixture removes the running completion guard");
            let changed = sqlx::query(
                "UPDATE work_unit SET status = 'cancelled', version = version + 1,
                     updated_at = ? WHERE id = ?",
            )
            .bind(now_rfc3339())
            .bind(work_unit_id)
            .execute(db.pool())
            .await
            .expect("fixture closes the WorkUnit after preflight");
            assert_eq!(changed.rows_affected(), 1);
        }
        ProviderStartClaimAuthorityMutation::ChangeWorkUnitVersion => {
            let changed = sqlx::query(
                "UPDATE work_unit SET version = version + 1, updated_at = ? WHERE id = ?",
            )
            .bind(now_rfc3339())
            .bind(work_unit_id)
            .execute(db.pool())
            .await
            .expect("fixture advances the WorkUnit version after preflight");
            assert_eq!(changed.rows_affected(), 1);
        }
        ProviderStartClaimAuthorityMutation::RevokeLease => {
            WorkspaceLeaseRepo::revoke(&*db, &lease.id, lease.version, &now_rfc3339())
                .await
                .expect("exact WorkspaceLease revokes after preflight");
        }
        ProviderStartClaimAuthorityMutation::ExpireLease => {
            let expired = WorkspaceLeaseRepo::expire(&*db, "9999-12-31T00:00:00Z", 500)
                .await
                .expect("exact WorkspaceLease expires after preflight");
            assert!(expired.iter().any(|expired| expired.id == lease.id));
        }
    }
    claim_barrier.wait().await;

    let result = tokio::time::timeout(std::time::Duration::from_secs(10), &mut start)
        .await
        .expect("claim resolves without waiting for an external provider")
        .expect("start task joins");
    assert!(
        matches!(result, Err(ServiceError::InvalidOperation { ref message })
            if message.contains("authority changed before provider Start was claimed")),
        "{mutation:?} must be rejected by the durable claim, got {result:?}"
    );
    assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 0);

    let stored = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("Execution reloads after claim rejection")
        .expect("Execution remains present");
    assert_eq!(stored.status, ExecutionStatus::Failed, "{mutation:?}");
    assert_eq!(stored.work_unit_id.as_deref(), Some(work_unit_id));
    assert_eq!(stored.workspace_id.as_deref(), Some(workspace.id.as_str()));
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle remains present")
            .state,
        db::TaskLifecycleState::Blocked
    );
    let lease_after = WorkspaceLeaseRepo::get_by_id(&*db, &lease.id)
        .await
        .expect("lease reloads")
        .expect("lease remains present");
    match mutation {
        ProviderStartClaimAuthorityMutation::ExpireLease => {
            assert_eq!(lease_after.status, "expired");
        }
        ProviderStartClaimAuthorityMutation::RevokeLease => {
            assert_eq!(lease_after.status, "revoked");
        }
        _ => assert_eq!(lease_after.status, "revoked"),
    }
    for (event_type, expected_count) in [
        ("execution.started", 1_i64),
        ("execution.provider_start_pending", 1),
        ("execution.provider_start_attempted", 0),
        ("execution.failed", 1),
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE entity_id = ? AND event_type = ?",
        )
        .bind(&execution.id)
        .bind(event_type)
        .fetch_one(db.pool())
        .await
        .expect("Execution receipt count loads");
        assert_eq!(count, expected_count, "{mutation:?}: {event_type}");
    }
}

#[tokio::test]
async fn provider_start_claim_revalidates_authority_after_preflight() {
    for mutation in [
        ProviderStartClaimAuthorityMutation::SuspendMembership,
        ProviderStartClaimAuthorityMutation::ReassignWorkUnit,
        ProviderStartClaimAuthorityMutation::CloseWorkUnit,
        ProviderStartClaimAuthorityMutation::ChangeWorkUnitVersion,
        ProviderStartClaimAuthorityMutation::RevokeLease,
        ProviderStartClaimAuthorityMutation::ExpireLease,
    ] {
        assert_provider_start_claim_rejects_authority_race(mutation).await;
    }
}

#[tokio::test]
async fn concurrent_provider_start_claims_dispatch_one_external_start() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let snapshot =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("exact Agent routing snapshot builds")
            .expect("Agent snapshot exists");
    let workspace_root = TempDir::new().expect("workspace root creates");
    let executor = Arc::new(RecoveryDispatchExecutor::default());
    let claim_barrier = Arc::new(tokio::sync::Barrier::new(3));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_task_executor(executor.clone())
        .with_provider_start_claim_barrier(Arc::clone(&claim_barrier));
    let (execution, _) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("concurrent provider claim fixture".to_owned()),
        Some(snapshot),
    )
    .await;
    let daemon_id = agent.daemon_id.as_deref().expect("Agent has its daemon");
    let changed = sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
        .bind(crate::embedded_daemon::embedded_machine_id())
        .bind(daemon_id)
        .execute(db.pool())
        .await
        .expect("fixture routes provider Start to the embedded executor");
    assert_eq!(changed.rows_affected(), 1);

    let first_service = service.clone();
    let first_id = execution.id.clone();
    let first = tokio::spawn(async move { first_service.start_execution(first_id).await });
    let second_service = service.clone();
    let second_id = execution.id.clone();
    let second = tokio::spawn(async move { second_service.start_execution(second_id).await });
    // Release both calls from the preclaim boundary in the same barrier cycle.
    tokio::time::timeout(std::time::Duration::from_secs(20), claim_barrier.wait())
        .await
        .expect("both starts finish preflight");
    tokio::time::timeout(std::time::Duration::from_secs(20), claim_barrier.wait())
        .await
        .expect("both starts are released toward the claim");
    let (first_result, second_result) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            (
                first.await.expect("first start joins"),
                second.await.expect("second start joins"),
            )
        })
        .await
        .expect("both provider claims resolve");
    assert!(first_result.is_ok() || second_result.is_ok());
    assert!(
        matches!(&first_result, Ok(result) if result.accepted)
            || matches!(
                &first_result,
                Err(ServiceError::ExecutionStartUncertain { .. })
            )
    );
    assert!(
        matches!(&second_result, Ok(result) if result.accepted)
            || matches!(
                &second_result,
                Err(ServiceError::ExecutionStartUncertain { .. })
            )
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        executor.started.notified(),
    )
    .await
    .expect("the claimed provider Start invokes the executor");
    assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    for (event_type, expected_count) in [
        ("execution.provider_start_attempted", 1_i64),
        ("execution.provider_start_accepted", 1),
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE entity_id = ? AND event_type = ?",
        )
        .bind(&execution.id)
        .bind(event_type)
        .fetch_one(db.pool())
        .await
        .expect("provider receipt count loads");
        assert_eq!(count, expected_count, "{event_type}");
    }
}

#[tokio::test]
async fn uncertain_provider_start_retry_never_sends_a_second_external_start() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let daemon_id = add_remote_executor_daemon(&db, "uncertain-start-provider").await;
    change_agent_daemon(&db, &agent_id, Some(&daemon_id)).await;
    let workspace_root = TempDir::new().expect("workspace root creates");
    let registry = routing_test_registry();
    let outbound = register_routing_test_daemon(&registry, &daemon_id);
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()))
        .with_daemon_connections(Arc::clone(&registry));
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let routing_snapshot =
        crate::task_service::config::build_executor_config_snapshot(&db, &task, &agent, None, None)
            .await
            .expect("exact remote routing snapshot builds")
            .expect("Agent snapshot exists");
    let (execution, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("external Start response may be uncertain".to_owned()),
        Some(routing_snapshot),
    )
    .await;
    let responder = tokio::spawn(answer_uncertain_start_daemon(
        Arc::clone(&registry),
        daemon_id.clone(),
        outbound,
    ));

    let first_start = service
        .start_execution(execution.id.clone())
        .await
        .expect_err("uncertain remote response is not reported as a clean acceptance");
    assert!(
        matches!(
            &first_start,
            ServiceError::ExecutionStartUncertain { execution_id: id } if id == &execution.id
        ),
        "first provider result: {first_start:?}"
    );
    let stored = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("Execution reloads after uncertain response")
        .expect("Execution remains present");
    assert_eq!(
        stored.status,
        ExecutionStatus::Running,
        "uncertainty remains distinct from a confirmed failed Execution"
    );
    let attempt_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_attempted' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("provider attempt receipt count");
    let uncertain_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_uncertain' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("uncertain outcome receipt count");
    assert_eq!(attempt_receipts, 1);
    assert_eq!(uncertain_receipts, 1);
    assert_eq!(
        WorkspaceLeaseRepo::get_active_for_work_unit(
            &*db,
            execution.work_unit_id.as_deref().unwrap()
        )
        .await
        .expect("exact WorkUnit lease reloads")
        .expect("active lease remains while provider outcome is uncertain")
        .workspace_id,
        Some(workspace.id)
    );

    service
        .fail_execution_before_dispatch(
            &execution.id,
            "late authority reconciliation must preserve an attempted provider Start".to_owned(),
        )
        .await
        .expect("late pre-dispatch reconciliation preserves claimed external work");
    let after_late_reconciliation = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("Execution reloads after late reconciliation")
        .expect("Execution remains present");
    assert_eq!(after_late_reconciliation.status, ExecutionStatus::Running);
    let failed_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.failed' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("late reconciliation emits no failed event");
    assert_eq!(failed_events, 0);

    let retry = service
        .start_execution(execution.id.clone())
        .await
        .expect_err("retry surfaces uncertainty without replaying Start");
    assert!(matches!(
        retry,
        ServiceError::ExecutionStartUncertain { execution_id: id } if id == execution.id
    ));
    registry.unregister(&daemon_id);
    let methods = responder.await.expect("fake daemon responder joins");
    assert_eq!(
        methods
            .iter()
            .filter(|method| method.as_str() == api_types::METHOD_EXECUTION_START)
            .count(),
        1,
        "only the first request crosses the external Start boundary"
    );
    assert_eq!(
        methods
            .iter()
            .filter(|method| method.as_str() == api_types::METHOD_PROTOCOL_CAPABILITIES)
            .count(),
        1,
        "uncertain receipt short-circuits before provider capability negotiation"
    );
    let final_uncertain_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_uncertain' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("uncertain outcome remains singular");
    assert_eq!(final_uncertain_receipts, 1);

    AgentRepo::set_paused(&*db, &agent_id, true)
        .await
        .expect("Agent pauses before simulated server restart");
    crate::CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
        .run_recovery()
        .await
        .expect("restart recovery handles the attempted uncertain Execution");
    let recovered = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("uncertain Execution reloads after restart")
        .expect("uncertain Execution remains present");
    assert_eq!(recovered.status, ExecutionStatus::Cancelled);
    assert_eq!(recovered.stop_reason, Some(db::StopReason::CrashRecovery));
    assert_eq!(
        WorkspaceLeaseRepo::get_active_for_work_unit(
            &*db,
            execution.work_unit_id.as_deref().expect("WorkUnit id"),
        )
        .await
        .expect("WorkspaceLease state reloads after restart"),
        None
    );
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle reloads after restart")
            .expect("TaskLifecycle exists")
            .state,
        TaskLifecycleState::Blocked
    );
    let restarted_service = Arc::new(TaskService::new(Arc::clone(&db), Arc::clone(&event_bus)));
    assert_eq!(
        crate::TaskDispatcher::new(Arc::clone(&db), Arc::clone(&event_bus), restarted_service)
            .check_once()
            .await
            .expect("restarted dispatcher ignores attempted uncertain Start"),
        0
    );
    let attempts_after_restart: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.provider_start_attempted' AND entity_id = ?",
    )
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("attempt receipt remains singular after restart");
    assert_eq!(attempts_after_restart, 1);
    assert_eq!(
        methods
            .iter()
            .filter(|method| method.as_str() == api_types::METHOD_EXECUTION_START)
            .count(),
        1,
        "pausing and restarting does not repeat the uncertain external Start"
    );
}

#[tokio::test]
async fn crash_recovery_resolves_pending_start_after_its_workspace_lease_expired() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let workspace_root = TempDir::new().expect("workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent_with_executor_type(&db, "shell", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let (execution, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("provider Start is still pending".to_owned()),
        None,
    )
    .await;
    let unsolicited_terminal = service
        .complete_remote_execution(completed_remote_notification(&execution.id, None), None)
        .await
        .expect_err("a remote terminal report cannot complete an unattempted provider Start");
    assert!(unsolicited_terminal
        .to_string()
        .contains("provider Start was never attempted"));
    assert_eq!(
        ExecutionRepo::get_by_id(&*db, &execution.id)
            .await
            .expect("pending Execution reloads")
            .expect("pending Execution exists")
            .status,
        ExecutionStatus::Running
    );
    AgentRepo::set_paused(&*db, &agent_id, true)
        .await
        .expect("Agent pauses before any provider attempt");
    let lease = WorkspaceLeaseRepo::get_active_for_work_unit(
        &*db,
        execution.work_unit_id.as_deref().expect("WorkUnit id"),
    )
    .await
    .expect("WorkspaceLease loads")
    .expect("admission committed its WorkspaceLease");
    assert_eq!(lease.execution_id, execution.id);
    assert_eq!(lease.workspace_id.as_deref(), Some(workspace.id.as_str()));
    let work_unit_before = db::WorkUnitRepo::get_by_id(
        &*db,
        execution.work_unit_id.as_deref().expect("WorkUnit id"),
    )
    .await
    .expect("WorkUnit loads before lease expiry")
    .expect("WorkUnit exists before lease expiry");

    // Simulate a prior process expiring the lease before it was able to
    // terminalize the pending admission.
    let expired = WorkspaceLeaseRepo::expire(&*db, "9999-01-01T00:00:00Z", 500)
        .await
        .expect("long restart expires the stale grant");
    assert!(expired.iter().any(|expired| expired.id == lease.id));
    AgentRepo::set_paused(&*db, &agent_id, false)
        .await
        .expect("fixture enables direct admission-boundary check");
    assert_eq!(
        ExecutionRepo::claim_provider_start(
            &*db,
            &execution.id,
            provider_start_receipt_for_test(&execution, "execution.provider_start_attempted",),
        )
        .await
        .expect("expired WorkspaceLease is rejected by the durable claim boundary"),
        db::ProviderStartClaim::AuthorityLost
    );
    AgentRepo::set_paused(&*db, &agent_id, true)
        .await
        .expect("Agent remains paused for restart recovery");

    crate::CrashRecovery::new(Arc::clone(&db), Arc::clone(&event_bus))
        .run_recovery()
        .await
        .expect("startup recovery resolves pending work without its lease");

    let resolved = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("Execution reloads")
        .expect("Execution exists");
    assert_eq!(resolved.status, ExecutionStatus::Failed);
    assert_eq!(resolved.work_unit_id, execution.work_unit_id);
    assert_eq!(resolved.workspace_id, execution.workspace_id);
    assert_eq!(resolved.parent_execution_id, execution.parent_execution_id);
    assert_eq!(resolved.stop_reason, Some(db::StopReason::ExecutorFailed));
    assert!(resolved
        .error
        .as_deref()
        .is_some_and(|error| error.contains("WorkspaceLease")));
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle loads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, TaskLifecycleState::Blocked);
    let blocked_task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task exists");
    let annotation: Value = serde_json::from_str(
        blocked_task
            .error_annotation
            .as_deref()
            .expect("failure block"),
    )
    .expect("failure annotation parses");
    assert_eq!(annotation["blocked_execution_id"], execution.id);
    assert_eq!(
        WorkspaceLeaseRepo::get_active_for_work_unit(
            &*db,
            execution.work_unit_id.as_deref().expect("WorkUnit id"),
        )
        .await
        .expect("active WorkspaceLease query succeeds"),
        None,
        "recovery never revives the expired grant"
    );
    let lease_status: String =
        sqlx::query_scalar("SELECT status FROM workspace_lease WHERE id = ?")
            .bind(&lease.id)
            .fetch_one(db.pool())
            .await
            .expect("expired lease status loads");
    assert_eq!(lease_status, "expired");
    let work_unit_after = db::WorkUnitRepo::get_by_id(
        &*db,
        execution.work_unit_id.as_deref().expect("WorkUnit id"),
    )
    .await
    .expect("WorkUnit reloads after lease expiry")
    .expect("WorkUnit remains present");
    assert_eq!(work_unit_after.id, work_unit_before.id);
    assert_eq!(work_unit_after.status, work_unit_before.status);
    assert_eq!(work_unit_after.version, work_unit_before.version);

    for (event_type, expected) in [
        ("execution.started", 1_i64),
        ("execution.provider_start_pending", 1),
        ("execution.provider_start_attempted", 0),
        ("execution.failed", 1),
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE entity_id = ? AND event_type = ?",
        )
        .bind(&execution.id)
        .bind(event_type)
        .fetch_one(db.pool())
        .await
        .expect("Execution event count loads");
        assert_eq!(count, expected, "unexpected {event_type} receipt count");
    }

    let executor = Arc::new(RecoveryDispatchExecutor::default());
    let dispatcher_service = Arc::new(service.with_task_executor(executor.clone()));
    crate::TaskDispatcher::new(Arc::clone(&db), Arc::clone(&event_bus), dispatcher_service)
        .check_once()
        .await
        .expect("post-restart dispatcher sees no pending provider Start");
    assert_eq!(executor.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ordinary_launch_work_unit_version_race_rolls_back_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_launch_work_unit_version_race
         AFTER UPDATE OF state ON task_lifecycle WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE work_unit SET version = version + 1, updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        task.id, work_unit.id
    );
    sqlx::query(&trigger)
        .execute(db.pool())
        .await
        .expect("deterministic WorkUnit race trigger creates");

    let result = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "version race must roll back activation".to_owned(),
            Vec::new(),
            None,
        )
        .await;
    assert!(result.is_err(), "stale WorkUnit version rejects admission");
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
}

#[tokio::test]
async fn ordinary_repository_launch_workspace_readiness_race_rolls_back_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let workspace_root = TempDir::new().expect("Workspace root creates");
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .with_workspace_root(workspace_root.path().to_path_buf())
        .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
        .with_workspace_exec_locks(Arc::new(crate::WorkspaceExecutionLockManager::new()));
    let (parent, workspace) = create_running_repository_execution(
        &db,
        &service,
        &task,
        &agent_id,
        "implementer",
        db::ExecutionPurpose::Implement,
        workspace_root.path(),
        Some("workspace readiness race parent".to_owned()),
        None,
    )
    .await;
    ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: parent.id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now_rfc3339())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("first Execution becomes terminal");
    service
        .revoke_active_workspace_lease_for_execution(&task.id, &parent.id)
        .await;
    let work_unit = db::WorkUnitRepo::get_by_id(
        &*db,
        parent
            .work_unit_id
            .as_deref()
            .expect("parent binds WorkUnit"),
    )
    .await
    .expect("WorkUnit loads")
    .expect("WorkUnit exists");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_launch_workspace_readiness_race
         AFTER UPDATE OF state ON task_lifecycle WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE workspace SET status = 'error', updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        task.id, workspace.id
    );
    sqlx::query(&trigger)
        .execute(db.pool())
        .await
        .expect("deterministic Workspace readiness trigger creates");

    let result = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "workspace readiness race must roll back activation".to_owned(),
            Vec::new(),
            None,
        )
        .await;
    assert!(
        result.is_err(),
        "Workspace that stopped being Ready rejects admission"
    );
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
    assert_eq!(
        db::WorkspaceRepo::get_by_id(&*db, &workspace.id)
            .await
            .expect("Workspace reload succeeds")
            .expect("Workspace remains present")
            .status,
        db::WorkspaceStatus::Ready,
        "the race trigger's mutation rolls back with rejected admission"
    );
}

#[tokio::test]
async fn follow_up_rejects_ended_harness_session_without_clearing_recovery_annotation() {
    let fixture = blocked_recovery_fixture_for_executor(false, "codex").await;
    let parent = ExecutionRepo::record_harness_session_result(
        &*fixture.db,
        &fixture.parent.id,
        "historical-follow-up-session",
        &now_rfc3339(),
    )
    .await
    .expect("explicit HarnessSession attaches to exact parent");
    let session_id = parent
        .harness_session_id
        .clone()
        .expect("parent carries explicit HarnessSession id");
    db::HarnessSessionRepo::update(
        &*fixture.db,
        db::UpdateHarnessSession {
            id: session_id,
            status: Some(db::HarnessSessionStatus::Ended),
            last_activity_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("historical HarnessSession becomes nonreusable");
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;

    let result = fixture
        .service
        .follow_up_execution(
            parent.id,
            "do not resume an ended session".to_owned(),
            None,
            None,
            "follow-up-ended-session".to_owned(),
        )
        .await;
    assert!(result.is_err(), "ended explicit session rejects follow-up");
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    assert_eq!(before.task.error_annotation, fixture.task.error_annotation);
}

#[tokio::test]
async fn rejected_recovery_cancel_preserves_blocking_metadata() {
    let fixture = blocked_recovery_fixture(false).await;
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_recovery_cancel_transition_rejection
         BEFORE INSERT ON task_lifecycle_transition
         WHEN NEW.task_id = '{}' AND NEW.to_state = 'cancelled'
         BEGIN SELECT RAISE(ABORT, 'deterministic cancellation transition rejection'); END",
        fixture.task.id
    );
    sqlx::query(&trigger)
        .execute(fixture.db.pool())
        .await
        .expect("cancellation rejection trigger creates");

    let result = fixture
        .service
        .recover_task(
            fixture.task.id.clone(),
            api_types::RecoveryAction::CancelTask,
            Some("cancel the blocked Task".to_owned()),
            None,
        )
        .await;
    assert!(
        result.is_err(),
        "failed lifecycle transition rejects cancellation"
    );
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    assert_eq!(before.task.error_annotation, fixture.task.error_annotation);
    assert_eq!(before.task.blocked_json, fixture.task.blocked_json);

    sqlx::query("DROP TRIGGER test_recovery_cancel_transition_rejection")
        .execute(fixture.db.pool())
        .await
        .expect("cancellation rejection trigger drops");
    let cancelled = fixture
        .service
        .recover_task(
            fixture.task.id.clone(),
            api_types::RecoveryAction::CancelTask,
            Some("retry cancellation after policy rejection".to_owned()),
            None,
        )
        .await
        .expect("authorized recovery cancellation succeeds");
    assert_eq!(cancelled.lifecycle_state, TaskLifecycleState::Cancelled);
    assert!(cancelled.error_annotation.is_none());
    assert!(cancelled.blocked_json.is_none());
}

#[tokio::test]
async fn follow_up_membership_change_rejects_child_and_preserves_recovery_annotation() {
    let fixture = blocked_recovery_fixture(false).await;
    let membership_id: String = sqlx::query_scalar(
        "SELECT rm.id FROM role_membership rm
         JOIN task_role tr ON tr.id = rm.task_role_id
         WHERE tr.task_id = ? AND tr.role = ? AND rm.actor_id = ? AND rm.status = 'active'",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.work_unit.role)
    .bind(
        fixture
            .parent
            .agent_id
            .as_deref()
            .expect("parent Actor Agent"),
    )
    .fetch_one(fixture.db.pool())
    .await
    .expect("exact active RoleMembership loads");
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_follow_up_membership_change
         AFTER UPDATE OF state ON task_lifecycle WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE role_membership
             SET status = 'suspended', version = version + 1, updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        fixture.task.id, membership_id
    );
    sqlx::query(&trigger)
        .execute(fixture.db.pool())
        .await
        .expect("deterministic membership change trigger creates");

    let result = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            "membership change must reject child".to_owned(),
            None,
            None,
            "follow-up-membership-change".to_owned(),
        )
        .await;
    assert!(
        result.is_err(),
        "suspended RoleMembership rejects admission"
    );
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    let membership_status: String =
        sqlx::query_scalar("SELECT status FROM role_membership WHERE id = ?")
            .bind(&membership_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("RoleMembership status reloads");
    assert_eq!(membership_status, "active");
    assert_eq!(before.task.error_annotation, fixture.task.error_annotation);
}

#[tokio::test]
async fn follow_up_work_unit_version_change_rolls_back_lifecycle_and_metadata() {
    let fixture = blocked_recovery_fixture(false).await;
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_follow_up_work_unit_version_change
         AFTER UPDATE OF state ON task_lifecycle WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE work_unit
             SET version = version + 1, updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        fixture.task.id, fixture.work_unit.id
    );
    sqlx::query(&trigger)
        .execute(fixture.db.pool())
        .await
        .expect("deterministic WorkUnit version race trigger creates");

    let result = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            "stale WorkUnit version must reject follow-up".to_owned(),
            None,
            None,
            "follow-up-work-unit-version-change".to_owned(),
        )
        .await;
    assert!(
        result.is_err(),
        "changed WorkUnit revision rejects admission"
    );
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    assert_eq!(before.task.error_annotation, fixture.task.error_annotation);
    assert_eq!(before.task.blocked_json, fixture.task.blocked_json);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_follow_up_receipt WHERE idempotency_key = ?",
    )
    .bind("follow-up-work-unit-version-change")
    .fetch_one(fixture.db.pool())
    .await
    .expect("follow-up receipt count loads");
    assert_eq!(
        receipts, 0,
        "rejected admission writes no follow-up receipt"
    );
}

#[tokio::test]
async fn follow_up_on_sibling_work_unit_preserves_current_block_owner_and_evidence() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let parent_a = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        None,
        None,
    )
    .await;
    let parent_b = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    service
        .annotate_executor_failure_block(&parent_b)
        .await
        .expect("Execution B becomes the exact current Task block");
    let work_unit_a =
        db::WorkUnitRepo::get_by_id(&*db, parent_a.work_unit_id.as_deref().expect("A WorkUnit"))
            .await
            .expect("A WorkUnit loads")
            .expect("A WorkUnit exists");
    let work_unit_b =
        db::WorkUnitRepo::get_by_id(&*db, parent_b.work_unit_id.as_deref().expect("B WorkUnit"))
            .await
            .expect("B WorkUnit loads")
            .expect("B WorkUnit exists");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit_a.id).await;
    let lifecycle_before = before.lifecycle.clone();
    let task_before = before.task.clone();
    let transition_count_before = before.lifecycle_transitions;
    let admission_events_before = before.admission_events;
    let executions_before = before.executions;

    let blocking_annotation: api_types::TaskBlockingAnnotation = serde_json::from_str(
        task_before
            .error_annotation
            .as_deref()
            .expect("B blocking annotation is present"),
    )
    .expect("blocking annotation parses");
    assert_eq!(
        blocking_annotation.blocked_execution_id.as_deref(),
        Some(parent_b.id.as_str())
    );
    assert!(blocking_annotation
        .recovery_actions
        .contains(&api_types::RecoveryAction::Reexecute));
    assert_eq!(
        serde_json::from_str::<Value>(task_before.blocked_json.as_deref().unwrap())
            .expect("B blocked metadata parses")["execution_id"],
        parent_b.id
    );
    assert_eq!(
        lifecycle_before.reason_ref.as_deref(),
        Some(parent_b.id.as_str())
    );

    let request_key = "sibling-follow-up-must-not-clear-current-block";
    let result = service
        .follow_up_execution(
            parent_a.id.clone(),
            "continue completed sibling A".to_owned(),
            None,
            None,
            request_key.to_owned(),
        )
        .await;
    assert!(result.is_err(), "A cannot resolve B's aggregate block");

    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit_a.id, &before).await;
    assert_eq!(
        db::WorkUnitRepo::get_by_id(&*db, &work_unit_b.id)
            .await
            .expect("B WorkUnit reloads")
            .expect("B WorkUnit remains present"),
        work_unit_b
    );
    let current_task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task remains present");
    assert_eq!(current_task, task_before);
    let current_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("TaskLifecycle reloads")
        .expect("TaskLifecycle remains present");
    assert_eq!(current_lifecycle, lifecycle_before);
    let (transition_count_after, admission_events_after, executions_after): (i64, i64, i64) =
        sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM task_lifecycle_transition WHERE task_id = ?),
                 (SELECT COUNT(*) FROM domain_event WHERE scope_type = 'task' AND scope_id = ?
                    AND event_type IN ('task.lifecycle_changed', 'execution.started')),
                 (SELECT COUNT(*) FROM execution WHERE task_id = ?)",
        )
        .bind(&task.id)
        .bind(&task.id)
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("sibling admission receipts and events reload");
    assert_eq!(transition_count_after, transition_count_before);
    assert_eq!(admission_events_after, admission_events_before);
    assert_eq!(executions_after, executions_before);
    let children_a: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&parent_a.id)
            .fetch_one(db.pool())
            .await
            .expect("A lineage count loads");
    assert_eq!(children_a, 0);
    let children_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&parent_b.id)
            .fetch_one(db.pool())
            .await
            .expect("B lineage count loads");
    assert_eq!(children_b, 0);
    let operation_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_follow_up_receipt WHERE idempotency_key = ?",
    )
    .bind(request_key)
    .fetch_one(db.pool())
    .await
    .expect("sibling follow-up receipt count loads");
    assert_eq!(operation_receipts, 0);
}

#[tokio::test]
async fn durable_admission_rejects_block_owner_changed_after_preflight() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let parent_a = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Completed,
        None,
        None,
    )
    .await;
    let parent_b = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    service
        .annotate_executor_failure_block(&parent_b)
        .await
        .expect("Execution B installs the current block");

    // Model a request that prepared A's identity before the blocker changed
    // to B. Present the repository with current Task/lifecycle CAS versions so
    // this assertion exercises its exact-owner check inside BEGIN IMMEDIATE.
    TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: TaskRepo::get_by_id(&*db, &task.id, false)
                .await
                .expect("Task loads before metadata fixture update")
                .expect("Task exists")
                .version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(None),
            blocked_json: Some(None),
            failed_json: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("remove metadata to isolate lifecycle owner validation");
    let current_task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("current Task loads")
        .expect("Task exists");
    let current_lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("current TaskLifecycle loads")
        .expect("TaskLifecycle exists");
    let work_unit_a = db::WorkUnitRepo::get_by_id(
        &*db,
        parent_a
            .work_unit_id
            .as_deref()
            .expect("A WorkUnit identity"),
    )
    .await
    .expect("A WorkUnit loads")
    .expect("A WorkUnit exists");
    let work_unit_b = db::WorkUnitRepo::get_by_id(
        &*db,
        parent_b
            .work_unit_id
            .as_deref()
            .expect("B WorkUnit identity"),
    )
    .await
    .expect("B WorkUnit loads")
    .expect("B WorkUnit exists");
    let lifecycle_transitions =
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .plan_execution_activation(&current_task, &current_lifecycle)
            .await
            .expect("current B activation transition prepares");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit_a.id).await;
    let now = now_rfc3339();
    let input = db::CreateExecution {
        id: db::new_uuid_v4(),
        task_id: task.id.clone(),
        agent_id: parent_a.agent_id.clone(),
        actor_ref: parent_a.actor_ref(),
        purpose: parent_a.purpose.clone(),
        harness_session_id: None,
        role: work_unit_a.role.clone(),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: Some(parent_a.id.clone()),
        agent_message_id: None,
        last_activity_at: None,
        summary: Some("stale A follow-up plan".to_owned()),
        logs_path: None,
        before_sha: parent_a.before_sha.clone(),
        after_sha: None,
        error: None,
        executor_config_snapshot_json: parent_a.executor_config_snapshot_json.clone(),
        workspace_id: parent_a.workspace_id.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    let event = crate::task_service::execution_domain_event(&input, "execution.started");
    let admission = db::WorkUnitExecutionRepo::admit_with_lifecycle_and_artifact_inputs(
        &*db,
        db::AdmitWorkUnitExecutionWithLifecycle {
            admission: db::CreateWorkUnitExecution {
                execution: input,
                work_unit_id: work_unit_a.id.clone(),
                work_unit_version: work_unit_a.version,
                workspace_lease: None,
            },
            expected_task_version: current_task.version,
            expected_lifecycle_state: current_lifecycle.state,
            expected_lifecycle_version: current_lifecycle.version,
            lifecycle_transitions,
            clear_blocking_metadata: Some(db::ClearTaskBlockingMetadata {
                blocking_execution_id: parent_a.id.clone(),
                blocking_work_unit_id: work_unit_a.id.clone(),
                expected_lifecycle_reason_kind: current_lifecycle
                    .reason_kind
                    .clone()
                    .expect("current block reason kind exists"),
                expected_error_annotation: None,
                expected_blocked_json: None,
                expected_metadata_json: current_task.metadata_json.clone(),
                replacement_metadata_json: current_task.metadata_json.clone(),
            }),
            follow_up_receipt: None,
            automatic_recovery_parent_id: None,
        },
        Vec::new(),
        event,
    )
    .await;
    assert!(
        matches!(admission, Err(db::DbError::VersionConflict)),
        "durable admission rejects a stale blocker identity even with current CAS versions"
    );
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit_a.id, &before).await;
    assert_eq!(
        db::WorkUnitRepo::get_by_id(&*db, &work_unit_b.id)
            .await
            .expect("B WorkUnit reloads")
            .expect("B WorkUnit remains present"),
        work_unit_b
    );
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle remains present")
            .reason_ref
            .as_deref(),
        Some(parent_b.id.as_str())
    );
}

#[tokio::test]
async fn secondary_work_unit_failure_does_not_replace_current_block_metadata() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let parent_a = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    let parent_b = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    service
        .annotate_executor_failure_block(&parent_a)
        .await
        .expect("A installs the current Task block");
    let before_second_failure = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task loads after A failure")
        .expect("Task exists");
    service
        .annotate_executor_failure_block(&parent_b)
        .await
        .expect("B failure is recorded without stealing A's aggregate block");
    let after_second_failure = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads after B failure")
        .expect("Task exists");
    assert_eq!(after_second_failure, before_second_failure);
    let annotation: api_types::TaskBlockingAnnotation = serde_json::from_str(
        after_second_failure
            .error_annotation
            .as_deref()
            .expect("A annotation remains present"),
    )
    .expect("A blocking annotation parses");
    assert_eq!(
        annotation.blocked_execution_id.as_deref(),
        Some(parent_a.id.as_str())
    );
    assert_eq!(
        serde_json::from_str::<Value>(after_second_failure.blocked_json.as_deref().unwrap())
            .expect("A blocked metadata parses")["execution_id"],
        parent_a.id
    );
    assert_eq!(
        TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("TaskLifecycle reloads")
            .expect("TaskLifecycle exists")
            .reason_ref
            .as_deref(),
        Some(parent_a.id.as_str())
    );
    let b = ExecutionRepo::get_by_id(&*db, &parent_b.id)
        .await
        .expect("B Execution reloads")
        .expect("B Execution remains present");
    assert_eq!(b.status, ExecutionStatus::Failed);
    assert_eq!(b.work_unit_id, parent_b.work_unit_id);
}

#[tokio::test]
async fn manual_lifecycle_unblock_cannot_leave_execution_metadata_stale() {
    let fixture = blocked_recovery_fixture(false).await;
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;
    let result = crate::task_lifecycle::TaskLifecycleService::new(
        Arc::clone(&fixture.db),
        Arc::new(EventBus::new(8)),
    )
    .transition_at_lifecycle_version(
        crate::task_lifecycle::TransitionLifecycleInput {
            task_id: fixture.task.id.clone(),
            expected_task_version: fixture.task.version,
            to_state: TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::Actor(api_types::Actor::user(
                api_types::UserActionSource::Api,
            )),
            reason_kind: Some("manual_unblock".to_owned()),
            reason_ref: Some("manual-unblock-request".to_owned()),
            idempotency_key: "manual-unblock-with-pending-execution-metadata".to_owned(),
        },
        fixture.lifecycle.version,
    )
    .await;
    assert!(
        result.is_err(),
        "manual lifecycle transition cannot strand execution recovery evidence"
    );
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    let current = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task remains present");
    assert_eq!(current.lifecycle_state, TaskLifecycleState::Blocked);
    assert_eq!(current.error_annotation, fixture.task.error_annotation);
    assert_eq!(current.blocked_json, fixture.task.blocked_json);
}

#[tokio::test]
async fn task_failure_metadata_cannot_replace_an_exact_execution_block() {
    let fixture = blocked_recovery_fixture(false).await;
    let before =
        admission_side_effect_snapshot(&fixture.db, &fixture.task.id, &fixture.work_unit.id).await;
    let result = fixture
        .service
        .fail_task(
            fixture.task.id.clone(),
            "unrelated Task-level failure".to_owned(),
            None,
            Some("different-execution".to_owned()),
        )
        .await;
    assert!(
        result.is_err(),
        "Task failure cannot replace the exact block"
    );
    assert_admission_side_effects_unchanged(
        &fixture.db,
        &fixture.task.id,
        &fixture.work_unit.id,
        &before,
    )
    .await;
    let current = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task remains present");
    assert_eq!(current.failed_json, None);
    assert_eq!(current.error_annotation, fixture.task.error_annotation);
    assert_eq!(current.blocked_json, fixture.task.blocked_json);
}

#[tokio::test]
async fn follow_up_clears_exact_blocked_json_when_error_annotation_is_absent() {
    let fixture = blocked_recovery_fixture(false).await;
    TaskRepo::update(
        &*fixture.db,
        db::UpdateTask {
            id: fixture.task.id.clone(),
            expected_version: fixture.task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(None),
            blocked_json: None,
            failed_json: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("error_annotation is absent in the blocked_json-only case");
    let before = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task exists");
    assert!(before.error_annotation.is_none());
    assert!(before.blocked_json.is_some());

    let launch = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            "resolve the exact blocked_json owner".to_owned(),
            None,
            None,
            "follow-up-blocked-json-only".to_owned(),
        )
        .await
        .expect("exact parent follow-up admits with blocked_json-only evidence");
    assert_eq!(launch.task.lifecycle_state, TaskLifecycleState::Active);
    assert!(launch.task.error_annotation.is_none());
    assert!(launch.task.blocked_json.is_none());
    assert_eq!(
        launch.execution.parent_execution_id.as_deref(),
        Some(fixture.parent.id.as_str())
    );
    let task_version = launch.task.version;
    let task_after = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reloads after admission")
        .expect("Task exists after admission");
    assert_eq!(task_after.version, task_version);
    assert!(task_after.error_annotation.is_none());
    assert!(task_after.blocked_json.is_none());
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task.id)
        .await
        .expect("TaskLifecycle reloads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);
    assert_eq!(lifecycle.version, fixture.lifecycle.version + 1);
    let child_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&fixture.parent.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("exact child count loads");
    assert_eq!(child_count, 1);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_lifecycle_transition
         WHERE task_id = ? AND idempotency_key = ?",
    )
    .bind(&fixture.task.id)
    .bind(format!(
        "execution-start:{}:active:{}",
        fixture.task.id, fixture.lifecycle.version
    ))
    .fetch_one(fixture.db.pool())
    .await
    .expect("activation receipt count loads");
    assert_eq!(receipt_count, 1);
}

#[tokio::test]
async fn ordinary_reexecute_resolves_only_its_exact_current_block() {
    let fixture = blocked_recovery_fixture(false).await;
    let launch = fixture
        .service
        .re_execute_execution(fixture.parent.id.clone())
        .await
        .expect("ordinary re-execute admits the exact current block parent");
    assert_eq!(launch.task.lifecycle_state, TaskLifecycleState::Active);
    assert!(launch.task.error_annotation.is_none());
    assert!(launch.task.blocked_json.is_none());
    assert_eq!(
        launch.execution.parent_execution_id.as_deref(),
        Some(fixture.parent.id.as_str())
    );
    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task.id)
        .await
        .expect("TaskLifecycle reloads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, TaskLifecycleState::Active);
    assert_eq!(lifecycle.version, fixture.lifecycle.version + 1);
    let children: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&fixture.parent.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("re-execution lineage count loads");
    assert_eq!(children, 1);
    let start_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND scope_id = ? AND causation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("re-execution start event count loads");
    assert_eq!(start_events, 1);
}

#[tokio::test]
async fn initial_launch_on_sibling_work_unit_preserves_current_block() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit_a = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let parent_b = terminal_work_unit_execution(
        &db,
        &service,
        &task,
        &agent_id,
        ExecutionStatus::Failed,
        None,
        None,
    )
    .await;
    service
        .annotate_executor_failure_block(&parent_b)
        .await
        .expect("Execution B becomes the exact current Task block");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit_a.id).await;

    let result = service
        .launch_work_unit_execution(
            &work_unit_a.id,
            work_unit_a.version,
            db::ExecutionPurpose::Implement,
            "launch another WorkUnit while B remains blocked".to_owned(),
            Vec::new(),
            None,
        )
        .await;
    assert!(
        result.is_err(),
        "initial launch cannot reactivate a blocked Task"
    );
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit_a.id, &before).await;
    let blocking_annotation: api_types::TaskBlockingAnnotation = serde_json::from_str(
        before
            .task
            .error_annotation
            .as_deref()
            .expect("B annotation remains present"),
    )
    .expect("B annotation parses");
    assert_eq!(
        blocking_annotation.blocked_execution_id.as_deref(),
        Some(parent_b.id.as_str())
    );
    assert_eq!(
        before.lifecycle.reason_ref.as_deref(),
        Some(parent_b.id.as_str())
    );
}

#[tokio::test]
async fn follow_up_artifact_pin_rejection_preserves_blocking_annotation_and_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let parent = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Plan,
            "produce follow-up plan artifact".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("Plan Execution admits")
        .execution;
    let collaboration = crate::CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let artifact = collaboration
        .create_plan_artifact_from_execution(
            &parent.id,
            "# Exact plan\n\n- Preserve the transaction boundary.\n",
        )
        .await
        .expect("exact Plan Artifact persists");
    let now = now_rfc3339();
    ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: parent.id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now.clone())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Plan parent becomes terminal");
    let blocked =
        crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .block(
                &task.id,
                crate::task_lifecycle::LifecycleCause::Execution(parent.id.clone()),
                "test_block",
                parent.id.clone(),
                format!("test-follow-up-block:{}", parent.id),
            )
            .await
            .expect("TaskLifecycle enters Blocked");
    let annotation = api_types::TaskBlockingAnnotation {
        annotation_type: api_types::FailureKind::ExecutorFailed,
        blocking_reason: "test_block".to_owned(),
        blocked_by: Some("system:test".to_owned()),
        blocked_at: Some(now.clone()),
        blocked_execution_id: Some(parent.id.clone()),
        artifact: Some(api_types::BlockingArtifact {
            kind: "execution".to_owned(),
            id: Some(parent.id.clone()),
            log_path: None,
        }),
        message: Some("preserve until follow-up admission commits".to_owned()),
        hook: None,
        recovery_actions: vec![
            api_types::RecoveryAction::Reexecute,
            api_types::RecoveryAction::CancelTask,
        ],
    };
    let annotation_json = serde_json::to_string(&annotation).expect("annotation serializes");
    TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: blocked.task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(annotation_json.clone())),
            blocked_json: Some(Some(
                json!({"reason":"test_block", "execution_id":parent.id}).to_string(),
            )),
            failed_json: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("blocking annotation persists");
    let inherited = service
        .inherit_plan_artifact_inputs(&parent.id, &task.id)
        .await
        .expect("exact Plan ArtifactInputs prepare");
    assert_eq!(inherited, vec![artifact.id]);
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_follow_up_artifact_pin_failure
         BEFORE INSERT ON execution_artifact_input
         WHEN NEW.task_id = '{}'
         BEGIN SELECT RAISE(ABORT, 'deterministic ArtifactInput admission failure'); END",
        task.id
    );
    sqlx::query(&trigger)
        .execute(db.pool())
        .await
        .expect("ArtifactInput failure trigger creates");

    let result = service
        .follow_up_execution(
            parent.id.clone(),
            "continue with the exact plan".to_owned(),
            Some(agent_id),
            None,
            "follow-up-artifact-pin".to_owned(),
        )
        .await;
    assert!(result.is_err(), "ArtifactInput pin rejects child admission");
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
    assert_eq!(
        before.task.error_annotation.as_deref(),
        Some(annotation_json.as_str())
    );
    let children: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&parent.id)
            .fetch_one(db.pool())
            .await
            .expect("follow-up child count loads");
    assert_eq!(children, 0);
}

#[tokio::test]
async fn concurrent_follow_up_retries_replay_one_child_and_one_activation_receipt() {
    let fixture = blocked_recovery_fixture(false).await;
    let service_one = fixture.service.clone();
    let service_two = fixture.service.clone();
    let parent_id = fixture.parent.id.clone();
    let one = service_one.follow_up_execution(
        parent_id.clone(),
        "same concurrently submitted follow-up".to_owned(),
        None,
        None,
        "same-concurrent-follow-up".to_owned(),
    );
    let two = service_two.follow_up_execution(
        parent_id.clone(),
        "same concurrently submitted follow-up".to_owned(),
        None,
        None,
        "same-concurrent-follow-up".to_owned(),
    );
    let (one, two) = tokio::join!(one, two);
    let one = one.expect("first concurrent request admits or replays the child");
    let two = two.expect("same concurrent request replays the admitted child");
    assert_ne!(one.idempotent_replay, two.idempotent_replay);
    assert_eq!(one.execution.id, two.execution.id);
    let children: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM execution WHERE parent_execution_id = ?")
            .bind(&parent_id)
            .fetch_all(fixture.db.pool())
            .await
            .expect("follow-up children load");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].1, "running");
    let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task.id, false)
        .await
        .expect("Task reload succeeds")
        .expect("Task exists");
    assert_eq!(task.version, fixture.task.version + 2);
    assert_eq!(task.lifecycle_state, TaskLifecycleState::Active);
    assert!(task.error_annotation.is_none());
    assert!(task.blocked_json.is_none());
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_lifecycle_transition
         WHERE task_id = ? AND idempotency_key = ?",
    )
    .bind(&task.id)
    .bind(format!(
        "execution-start:{}:active:{}",
        task.id, fixture.lifecycle.version
    ))
    .fetch_one(fixture.db.pool())
    .await
    .expect("activation receipt count loads");
    assert_eq!(receipts, 1);
    let lifecycle_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'task.lifecycle_changed' AND scope_id = ?
           AND correlation_id = ?",
    )
    .bind(&task.id)
    .bind(format!(
        "execution-start:{}:active:{}",
        task.id, fixture.lifecycle.version
    ))
    .fetch_one(fixture.db.pool())
    .await
    .expect("activation event count loads");
    assert_eq!(lifecycle_events, 1);
    let start_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event
         WHERE event_type = 'execution.started' AND scope_id = ? AND causation_id = ?",
    )
    .bind(&task.id)
    .bind(&parent_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("child start event count loads");
    assert_eq!(start_events, 1);
    let operation_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_follow_up_receipt WHERE idempotency_key = ?",
    )
    .bind("same-concurrent-follow-up")
    .fetch_one(fixture.db.pool())
    .await
    .expect("follow-up operation receipt count loads");
    assert_eq!(operation_receipts, 1);
}

#[tokio::test]
async fn follow_up_retry_replays_exact_child_and_new_key_authorizes_next_child() {
    let fixture = blocked_recovery_fixture(false).await;
    let key = "follow-up-uncertain-response".to_owned();
    let request_message = "resume after uncertain response".to_owned();
    let first = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            request_message.clone(),
            None,
            None,
            key.clone(),
        )
        .await
        .expect("first follow-up commits");
    assert!(!first.idempotent_replay);
    let first_task_version = first.task.version;
    let first_child_id = first.execution.id.clone();
    let replay = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            request_message.clone(),
            None,
            None,
            key.clone(),
        )
        .await
        .expect("uncertain response retry replays exact child");
    assert!(replay.idempotent_replay);
    assert_eq!(replay.execution.id, first_child_id);
    assert_eq!(replay.task.version, first_task_version);

    let reused_key = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            "different request body".to_owned(),
            None,
            None,
            key.clone(),
        )
        .await;
    assert!(matches!(
        reused_key,
        Err(ServiceError::Db(db::DbError::IdempotencyConflict))
    ));

    db::ExecutionRepo::update(
        &*fixture.db,
        db::UpdateExecution {
            id: first_child_id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now_rfc3339())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("first child becomes terminal");
    let next = fixture
        .service
        .follow_up_execution(
            fixture.parent.id.clone(),
            "separately authorized follow-up".to_owned(),
            None,
            None,
            "follow-up-next-authorization".to_owned(),
        )
        .await
        .expect("new operation key authorizes another child after the first is terminal");
    assert!(!next.idempotent_replay);
    assert_ne!(next.execution.id, first_child_id);

    let child_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE parent_execution_id = ?")
            .bind(&fixture.parent.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("exact parent child count loads");
    assert_eq!(child_count, 2);
    let start_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'execution.started'
         AND entity_type = 'execution' AND scope_id = ? AND causation_id = ?",
    )
    .bind(&fixture.task.id)
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("start event count loads");
    assert_eq!(start_events, 2);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_follow_up_receipt WHERE parent_execution_id = ?",
    )
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("follow-up receipt count loads");
    assert_eq!(receipt_count, 2);
    db::ProjectRepo::delete(&*fixture.db, &fixture.task.project_id)
        .await
        .expect("guarded Project teardown removes immutable follow-up receipts");
    let teardown_receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_follow_up_receipt WHERE parent_execution_id = ?",
    )
    .bind(&fixture.parent.id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("Project teardown receipt count loads");
    assert_eq!(teardown_receipts, 0);
}

#[tokio::test]
async fn ordinary_reexecute_snapshot_failure_keeps_ready_lifecycle_without_child() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let parent = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "ordinary re-execution source".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("source Execution admits")
        .execution;
    ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: parent.id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now_rfc3339())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("source Execution becomes terminal");
    let active_task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::new(EventBus::new(16)))
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: task.id.clone(),
            expected_task_version: active_task.version,
            to_state: TaskLifecycleState::Ready,
            cause: crate::task_lifecycle::LifecycleCause::System(
                api_types::SystemComponent::Executor,
            ),
            reason_kind: Some("test_reexecute_retry".to_owned()),
            reason_ref: Some(parent.id.clone()),
            idempotency_key: format!("test-reexecute-ready:{}", parent.id),
        })
        .await
        .expect("Task returns to Ready before re-execution");
    let agent = AgentRepo::get_by_id(&*db, &agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    AgentRepo::update(
        &*db,
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
            config_json: Some(r#"{"fallbacks":"invalid"}"#.to_owned()),
            daemon_id: None,
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
    .expect("Agent snapshot source becomes invalid");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;

    let result = service
        .re_execute_execution_with_context(parent.id.clone(), None)
        .await;
    assert!(
        result.is_err(),
        "snapshot failure rejects ordinary re-execution"
    );
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
    assert_eq!(before.lifecycle.state, TaskLifecycleState::Ready);
}

#[tokio::test]
async fn ordinary_reexecute_work_unit_version_race_rolls_back_activation() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let work_unit = create_simple_execution_work_unit(&db, &service, &task, &agent_id).await;
    let parent = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "ordinary re-execution source".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("source Execution admits")
        .execution;
    ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: parent.id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(Some(now_rfc3339())),
            external_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("source Execution becomes terminal");
    let active_task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: task.id.clone(),
            expected_task_version: active_task.version,
            to_state: TaskLifecycleState::Ready,
            cause: crate::task_lifecycle::LifecycleCause::System(
                api_types::SystemComponent::Executor,
            ),
            reason_kind: Some("test_reexecute_retry".to_owned()),
            reason_ref: Some(parent.id.clone()),
            idempotency_key: format!("test-reexecute-ready:{}", parent.id),
        })
        .await
        .expect("Task returns to Ready before re-execution");
    let before = admission_side_effect_snapshot(&db, &task.id, &work_unit.id).await;
    let trigger = format!(
        "CREATE TRIGGER test_reexecute_work_unit_version_race
         AFTER UPDATE OF state ON task_lifecycle WHEN NEW.task_id = '{}'
         BEGIN
             UPDATE work_unit SET version = version + 1, updated_at = NEW.updated_at
             WHERE id = '{}';
         END",
        task.id, work_unit.id
    );
    sqlx::query(&trigger)
        .execute(db.pool())
        .await
        .expect("deterministic re-execution WorkUnit race trigger creates");

    let result = service
        .re_execute_execution_with_context(parent.id.clone(), None)
        .await;
    assert!(
        result.is_err(),
        "changed WorkUnit version rejects re-execution"
    );
    assert_admission_side_effects_unchanged(&db, &task.id, &work_unit.id, &before).await;
    assert_eq!(before.lifecycle.state, TaskLifecycleState::Ready);
}

#[tokio::test]
async fn work_unit_launch_records_execution_permission_policy_override_in_snapshot() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
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
        )
        .await
        .expect("task creates");
    let execution = launch_work_unit_for_override_snapshot(
        &db,
        &service,
        &task,
        &agent_id,
        Some(ExecutionOverrides {
            model_id: None,
            reasoning_effort: None,
            permission_policy: Some("auto".to_owned()),
        }),
    )
    .await;
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
async fn work_unit_launch_records_codex_overrides_in_normalized_snapshot() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
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
        )
        .await
        .expect("task creates");
    let execution = launch_work_unit_for_override_snapshot(
        &db,
        &service,
        &task,
        &agent_id,
        Some(ExecutionOverrides {
            model_id: Some("gpt-5-codex".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            permission_policy: Some("auto".to_owned()),
        }),
    )
    .await;
    let snapshot: Value = serde_json::from_str(
        execution
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
        .expect("execution override keys are recorded");
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
