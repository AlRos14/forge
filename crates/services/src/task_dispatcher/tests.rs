use std::{future::pending, path::Path, sync::Arc};

use async_trait::async_trait;
use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    CreateAgent, CreateProject, CreateRepo, CreateTask, CreateTaskRoleAssignment, DaemonRepo,
    DaemonStatus, ExecutionRepo, ExecutionStatus, RepoRepo, ResumePolicy, StopReason, TaskRepo,
    TaskRoleAssignmentRepo, UpdateDaemonReport, UpdateProject, UpdateTask, UpsertDaemon,
};
use executors::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutor};
use tempfile::TempDir;
use tokio::sync::mpsc;
use workspace::RepoCacheLockManager;

use crate::deferred_dispatch;

use super::*;

struct RecordingExecutor {
    sender: mpsc::UnboundedSender<ExecutionContext>,
}

#[async_trait]
impl TaskExecutor for RecordingExecutor {
    async fn execute(
        &self,
        ctx: ExecutionContext,
    ) -> std::result::Result<ExecutionResult, ExecutorError> {
        let _ = self.sender.send(ctx);
        pending::<()>().await;
        unreachable!()
    }

    async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
        Ok(())
    }
}

async fn sqlite_db() -> db::SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    db::SqliteDb::new(pool)
}

fn setup_git_repo(path: &Path) -> String {
    run_git(path, &["init"]);
    run_git(path, &["config", "user.email", "test@forge.dev"]);
    run_git(path, &["config", "user.name", "Forge Test"]);
    std::fs::write(path.join("README.md"), "# Forge\n").expect("README writes");
    run_git(path, &["add", "-A"]);
    run_git(path, &["commit", "-m", "initial commit"]);
    run_git(path, &["symbolic-ref", "--short", "HEAD"])
}

fn run_git(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("stdout utf8")
        .trim()
        .to_owned()
}

async fn seed_project_repo(db: &db::SqliteDb, repo_path: &Path) -> (String, String) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let default_branch = setup_git_repo(repo_path);

    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "forge".to_owned(),
            remote_url: repo_path.to_string_lossy().into_owned(),
            local_path: Some(repo_path.to_string_lossy().into_owned()),
            work_mode: db::WorkMode::DirectMerge,
            default_branch,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("repo creates");
    ProjectRepo::update(
        db,
        UpdateProject {
            id: project_id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project primary repo updates");

    (project_id, repo_id)
}

async fn seed_agent(
    db: &db::SqliteDb,
    max_concurrent_tasks: i64,
    daemon_status: DaemonStatus,
    agent_status: AgentStatus,
) -> String {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    DaemonRepo::upsert_by_machine_id(
        db,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("machine-{daemon_id}"),
            hostname: "host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: daemon_status.clone(),
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon creates");
    DaemonRepo::update_report(
        db,
        UpdateDaemonReport {
            id: daemon_id.clone(),
            detected_clis_json: r#"[{"kind":"shell","availability":"authenticated"}]"#.to_owned(),
            labels_json: None,
            status: daemon_status,
            last_report_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon report updates");

    let agent_id = new_uuid_v4();
    AgentRepo::create(
        db,
        CreateAgent {
            id: agent_id.clone(),
            name: "shell".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            max_concurrent_tasks,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: agent_status,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            prompt_template: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("agent creates");
    agent_id
}

async fn seed_task(
    db: &db::SqliteDb,
    project_id: &str,
    repo_id: &str,
    title: &str,
    status: &str,
    priority: i64,
) -> Task {
    let now = now_rfc3339();
    TaskRepo::create(
        db,
        CreateTask {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: title.to_owned(),
            description: Some("echo test".to_owned()),
            task_type: "implementation".to_owned(),
            status: status.to_owned(),
            is_automation: false,
            priority,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task creates")
}

async fn assign_role(db: &db::SqliteDb, task_id: &str, role_name: &str, agent_id: &str) {
    let now = now_rfc3339();
    TaskRoleAssignmentRepo::assign(
        db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            role_name: role_name.to_owned(),
            assignee_type: Some(db::AssigneeKind::Agent),
            assignee_id: Some(agent_id.to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("role assignment creates");
}

async fn assign_authoritative_role(
    db: &db::SqliteDb,
    task_id: &str,
    role_name: &str,
    agent_id: &str,
) {
    let role_name = db::canonical_task_role_name(role_name).expect("TaskRole name is valid");
    let role = match db::TaskRoleRepo::get_by_task_and_role(db, task_id, &role_name)
        .await
        .expect("TaskRole lookup succeeds")
    {
        Some(role) => role,
        None => {
            let now = now_rfc3339();
            db::TaskRoleRepo::create(
                db,
                db::CreateTaskRole {
                    id: new_uuid_v4(),
                    task_id: task_id.to_owned(),
                    role: role_name,
                    coordination_mode: Some(db::CoordinationMode::Collaborative),
                    policy_json: "{}".to_owned(),
                    created_at: now.clone(),
                    updated_at: now,
                },
            )
            .await
            .expect("TaskRole creates")
        }
    };
    let now = now_rfc3339();
    db::RoleMembershipRepo::add(
        db,
        db::CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role.id,
            actor_kind: db::ActorKind::Agent,
            actor_id: agent_id.to_owned(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("authoritative RoleMembership creates");
}

async fn seed_running_execution(db: &db::SqliteDb, task_id: &str, agent_id: &str, role: &str) {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            actor_ref: None,
            purpose: None,
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
            summary: Some("running".to_owned()),
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
}

async fn seed_cancelled_execution(
    db: &db::SqliteDb,
    task_id: &str,
    agent_id: &str,
    role: &str,
    stop_reason: Option<StopReason>,
    resume_policy: Option<ResumePolicy>,
) -> db::Execution {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: role.to_owned(),
            status: ExecutionStatus::Cancelled,
            stop_reason,
            stopped_by: Some("system:test".to_owned()),
            resume_policy,
            stopped_at: Some(now.clone()),
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some("intentional stop".to_owned()),
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("execution creates")
}

async fn seed_failed_execution(
    db: &db::SqliteDb,
    task_id: &str,
    agent_id: &str,
    role: &str,
    resume_policy: Option<ResumePolicy>,
) -> db::Execution {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            actor_ref: None,
            purpose: None,
            role: role.to_owned(),
            status: ExecutionStatus::Failed,
            stop_reason: Some(StopReason::ExecutorFailed),
            stopped_by: Some("system:test".to_owned()),
            resume_policy,
            stopped_at: Some(now.clone()),
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: Some("deterministic failure".to_owned()),
            executor_config_snapshot_json: Some(
                r#"{"executor_type":"shell","config":{}}"#.to_owned(),
            ),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("failed execution creates")
}

async fn build_dispatcher(
    db: Arc<db::SqliteDb>,
    workspace_root: &Path,
) -> (TaskDispatcher, mpsc::UnboundedReceiver<ExecutionContext>) {
    let event_bus = Arc::new(EventBus::new(64));
    let (tx, rx) = mpsc::unbounded_channel();
    let task_executor: Arc<dyn TaskExecutor> = Arc::new(RecordingExecutor { sender: tx });
    let task_service = Arc::new(
        TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
            .with_task_executor(task_executor)
            .with_repo_cache_locks(Arc::new(RepoCacheLockManager::default()))
            .with_workspace_root(workspace_root.to_path_buf()),
    );
    (
        TaskDispatcher::with_check_interval(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            task_service,
            Duration::from_millis(10),
        ),
        rx,
    )
}

#[tokio::test]
async fn dispatcher_uses_aggregate_lifecycle_and_task_roles_not_workflow_states() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let mut workflow = crate::workflow::default_workflow::default_workflow();
    for state in &mut workflow.states {
        if state.name == crate::workflow::default_states::PLANNING
            || state.name == crate::workflow::default_states::IN_PROGRESS
        {
            state.role = Some("orchestrator".to_owned());
        }
    }
    let mut legacy_worker = workflow
        .states
        .iter()
        .find(|state| state.name == crate::workflow::default_states::IN_PROGRESS)
        .expect("active state exists")
        .clone();
    legacy_worker.name = "legacy_worker".to_owned();
    legacy_worker.display_name = "Legacy worker".to_owned();
    legacy_worker.role = Some(crate::workflow::default_roles::CODER.to_owned());
    workflow.states.push(legacy_worker);
    let default_branch = setup_git_repo(repo_dir.path());
    ProjectRepo::create(
        &*db,
        CreateProject {
            id: project_id.clone(),
            name: "PR6 TaskDispatcher coexistence".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: serde_json::to_string(&workflow).expect("workflow serializes"),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project creates with PR6 test workflow");
    RepoRepo::create(
        &*db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "forge".to_owned(),
            remote_url: repo_dir.path().to_string_lossy().into_owned(),
            local_path: Some(repo_dir.path().to_string_lossy().into_owned()),
            work_mode: db::WorkMode::DirectMerge,
            default_branch,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("repository creates");
    ProjectRepo::update(
        &*db,
        UpdateProject {
            id: project_id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Project primary repository is set");
    let agent_id = seed_agent(&*db, 2, DaemonStatus::Online, AgentStatus::Idle).await;
    let initial_orchestrator = seed_task(&*db, &project_id, &repo_id, "high", "todo", 1).await;
    let active_orchestrator =
        seed_task(&*db, &project_id, &repo_id, "active", "in_progress", 0).await;
    let legacy_worker = seed_task(&*db, &project_id, &repo_id, "active", "legacy_worker", 0).await;
    let legacy_reviewer = seed_task(&*db, &project_id, &repo_id, "review", "review", 0).await;
    assign_authoritative_role(
        &*db,
        &initial_orchestrator.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    assert_eq!(dispatcher.check_once().await.expect("dispatcher runs"), 1);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*db, &initial_orchestrator.id, "orchestrator")
            .await
            .expect("orchestrator execution count loads"),
        0,
    );
    assert_eq!(
        TaskRepo::get_by_id(&*db, &initial_orchestrator.id, false)
            .await
            .expect("initial Task loads")
            .expect("initial Task exists")
            .status,
        "in_progress",
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("initial implementer execution starts in time")
            .expect("initial execution context arrives")
            .role,
        "implementer",
    );
    for task in [&active_orchestrator, &legacy_worker, &legacy_reviewer] {
        assert_eq!(
            ExecutionRepo::list_by_task(
                &*db,
                &task.id,
                db::PageRequest {
                    cursor: None,
                    limit: 10,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            )
            .await
            .expect("Task execution rows load")
            .items
            .len(),
            0,
        );
    }
}

#[tokio::test]
async fn dispatcher_check_once_does_not_dispatch_after_stop() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "high", "todo", 1).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;
    dispatcher.stop();

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, "todo");
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_schedules_ready_task_from_authoritative_role_membership() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "high", "todo", 1).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");
    assert_eq!(dispatched, 1);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, "in_progress");
    let execution_ctx = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("execution spawned in time")
        .expect("execution context received");
    assert_eq!(execution_ctx.task_id, task.id);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("execution count loads"),
        1
    );
}

#[tokio::test]
async fn dispatcher_ignores_legacy_deferred_metadata_for_active_task() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(
        &db,
        &project_id,
        &repo_id,
        "deferred",
        crate::workflow::default_states::IN_PROGRESS,
        1,
    )
    .await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    deferred_dispatch::set(
        &db,
        &task,
        crate::workflow::default_states::IN_PROGRESS,
        &(chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        "test cooldown",
    )
    .await
    .expect("deferred dispatch metadata writes");

    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;
    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert!(rx.try_recv().is_err());
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::CODER
        )
        .await
        .expect("execution count loads"),
        0
    );

    let task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task reloads")
        .expect("task exists");
    deferred_dispatch::set(
        &db,
        &task,
        crate::workflow::default_states::IN_PROGRESS,
        &(chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
        "test cooldown expired",
    )
    .await
    .expect("deferred dispatch metadata updates");
    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert!(rx.try_recv().is_err());
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("execution count loads"),
        0
    );
    let task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task reloads")
        .expect("task exists");
    // The old metadata remains inert compatibility data; active lifecycle
    // work is never scheduled or cleared from that projection.
    assert!(deferred_dispatch::pending_until(&task).is_some());
}

#[tokio::test]
async fn dispatcher_does_not_dispatch_active_task_from_legacy_planning_projection() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(
        &db,
        &project_id,
        &repo_id,
        "legacy planning projection",
        crate::workflow::default_states::PLANNING,
        1,
    )
    .await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, crate::workflow::default_states::IN_PROGRESS);
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("Task lifecycle lookup succeeds")
        .expect("Task lifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Active);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("execution count loads"),
        0
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn dispatcher_skips_task_when_agent_at_capacity() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let blocked = seed_task(&db, &project_id, &repo_id, "blocked", "in_progress", 0).await;
    assign_role(
        &db,
        &blocked.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    seed_running_execution(
        &db,
        &blocked.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
    )
    .await;
    let task = seed_task(&db, &project_id, &repo_id, "todo", "todo", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, "todo");
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_skips_task_when_agent_offline() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Offline, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "todo", "todo", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, "todo");
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_skips_paused_project() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "todo", "todo", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    sqlx::query("UPDATE project SET paused_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project paused");
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let updated = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(updated.status, "todo");
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_never_infers_review_gate_from_legacy_review_status() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "review", "review", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::REVIEWER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("Task lifecycle lookup succeeds")
        .expect("Task lifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::REVIEWER
        )
        .await
        .expect("reviewer execution count loads"),
        0
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn dispatcher_respects_priority_ordering() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let low = seed_task(&db, &project_id, &repo_id, "low", "todo", 1).await;
    assign_authoritative_role(
        &db,
        &low.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let high = seed_task(
        &db,
        &project_id,
        &repo_id,
        "high",
        "todo",
        "10".parse().unwrap(),
    )
    .await;
    assign_authoritative_role(
        &db,
        &high.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 1);
    let ctx = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("execution spawned in time")
        .expect("execution context received");
    assert_eq!(ctx.task_id, high.id);

    let high_task = TaskRepo::get_by_id(&*db, &high.id, false)
        .await
        .expect("high task loads")
        .expect("high task exists");
    let low_task = TaskRepo::get_by_id(&*db, &low.id, false)
        .await
        .expect("low task loads")
        .expect("low task exists");
    assert_eq!(high_task.status, "in_progress");
    assert_eq!(low_task.status, "todo");
}

#[tokio::test]
async fn dispatcher_skips_auto_restart_for_user_cancelled_execution() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "cancelled", "in_progress", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let manual_stop = serde_json::json!({
        "type": "manual_stop",
        "blocking_reason": "user_cancelled",
        "blocked_by": "user:test",
        "blocked_at": now_rfc3339(),
        "message": "user stop",
        "recovery_actions": ["reexecute", "reset_to_initial", "cancel_task"],
    })
    .to_string();
    let before = now_rfc3339();
    TaskRepo::update(
        &*db,
        UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(manual_stop)),
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: before.clone(),
        },
    )
    .await
    .expect("task update creates");
    seed_cancelled_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        Some(StopReason::UserCancelled),
        Some(ResumePolicy::Manual),
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::CODER
        )
        .await
        .expect("execution count loads"),
        1
    );
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_skips_auto_restart_for_task_cancelled_execution() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "cancelled", "in_progress", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    seed_cancelled_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        Some(StopReason::TaskCancelled),
        None,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::CODER
        )
        .await
        .expect("execution count loads"),
        1
    );
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_dispatches_when_graceful_shutdown_stop_is_auto() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "cancelled", "in_progress", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let cancelled_execution = seed_cancelled_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        Some(StopReason::GracefulShutdown),
        Some(ResumePolicy::Auto),
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 1);
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        db::PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: db::SortBy::CreatedAt,
            sort_order: db::SortOrder::Desc,
        },
    )
    .await
    .expect("execution rows load");
    assert_eq!(executions.items.len(), 2);
    assert_eq!(executions.items[0].role, cancelled_execution.role);
    assert_eq!(
        executions.items[0].parent_execution_id.as_deref(),
        Some(cancelled_execution.id.as_str())
    );
    let ctx = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("execution spawned in time")
        .expect("execution context received");
    assert_eq!(ctx.task_id, task.id);
    assert_eq!(
        dispatcher.check_once().await.expect("replay check runs"),
        0,
        "the new running Execution fences duplicate recovery"
    );
}

#[tokio::test]
async fn dispatcher_does_not_dispatch_when_graceful_shutdown_stop_is_manual() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "cancelled", "in_progress", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    seed_cancelled_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        Some(StopReason::GracefulShutdown),
        Some(ResumePolicy::Manual),
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::CODER
        )
        .await
        .expect("execution count loads"),
        1
    );
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_skips_legacy_stopped_execution_without_resume_policy() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "legacy", "in_progress", 0).await;
    assign_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    seed_cancelled_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        None,
        None,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(
            &*db,
            &task.id,
            crate::workflow::default_roles::CODER
        )
        .await
        .expect("execution count loads"),
        1
    );
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_leaves_exact_retry_direction_to_orchestrator() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir");
    let workspace_dir = TempDir::new().expect("workspace dir");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "exact retry", "in_progress", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let execution = seed_failed_execution(
        &db,
        &task.id,
        &agent_id,
        crate::workflow::default_roles::CODER,
        Some(ResumePolicy::Manual),
    )
    .await;
    let failed_at = now_rfc3339();
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let failure_event = db::DomainEventRepo::append_event(
        &*db,
        db::CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "execution.failed".to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: execution.id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent_id),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: execution.id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("test:execution-failed:{}", execution.id)),
            payload_json: serde_json::json!({
                "execution_id": execution.id,
                "task_id": task.id,
            })
            .to_string(),
            created_at: failed_at,
        },
    )
    .await
    .expect("durable source failure event");
    assert_eq!(
        crate::task_failure_retry::TaskFailureRetryService::new(
            Arc::clone(&db),
            Arc::clone(&dispatcher.event_bus),
        )
        .process_domain_event(&failure_event)
        .await
        .expect("exact retry policy consumes the failure"),
        1
    );
    let rework_event_id: String = sqlx::query_scalar(
        "SELECT id FROM domain_event
         WHERE event_type = 'task.rework_requested' AND scope_id = ?
           AND json_extract(payload_json, '$.failure_ref') = ?",
    )
    .bind(&task.id)
    .bind(&execution.id)
    .fetch_one(db.pool())
    .await
    .expect("durable rework event is recorded");
    let rework_event = db::DomainEventRepo::get_event(&*db, &rework_event_id)
        .await
        .expect("rework event lookup succeeds")
        .expect("rework event exists");
    assert!(
        crate::task_failure_retry::TaskFailureRetryService::is_rework_request_event(
            &db,
            &rework_event,
        )
        .await
        .expect("exact receipt validates rework event")
    );

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        db::PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: db::SortBy::CreatedAt,
            sort_order: db::SortOrder::Desc,
        },
    )
    .await
    .expect("execution rows load");
    assert_eq!(executions.items.len(), 1);
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_does_not_dispatch_active_task_work_without_orchestrator_direction() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir");
    let workspace_dir = TempDir::new().expect("workspace dir");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "orchestrated", "in_progress", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    let dispatched = dispatcher.check_once().await.expect("dispatcher runs");

    assert_eq!(dispatched, 0);
    assert_eq!(
        ExecutionRepo::list_by_task(
            &*db,
            &task.id,
            db::PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: db::SortBy::CreatedAt,
                sort_order: db::SortOrder::Desc,
            },
        )
        .await
        .expect("execution rows load")
        .items
        .len(),
        0
    );
    assert!(tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .is_err());
}

#[tokio::test]
async fn dispatcher_keeps_permanent_executor_unavailability_blocked() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let repo_dir = TempDir::new().expect("repo dir");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let task = seed_task(&db, &project_id, &repo_id, "unavailable", "in_progress", 0).await;
    let result = crate::task_lifecycle::TaskLifecycleService::new(Arc::clone(&db), event_bus)
        .block(
            &task.id,
            crate::task_lifecycle::LifecycleCause::System(
                api_types::SystemComponent::TaskDispatcher,
            ),
            "executor_unavailable",
            "execution:unavailable",
            "test:executor-unavailable",
        )
        .await
        .expect("permanent executor unavailability blocks aggregate lifecycle");
    assert_eq!(result.lifecycle.state, db::TaskLifecycleState::Blocked);
}

#[tokio::test]
async fn dispatcher_does_not_treat_a_legacy_annotation_as_lifecycle_authority() {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_dir = TempDir::new().expect("workspace dir creates");
    let (project_id, repo_id) = seed_project_repo(&db, repo_dir.path()).await;
    let agent_id = seed_agent(&db, 1, DaemonStatus::Online, AgentStatus::Idle).await;
    let task = seed_task(&db, &project_id, &repo_id, "ready", "todo", 0).await;
    assign_authoritative_role(
        &db,
        &task.id,
        crate::workflow::default_roles::CODER,
        &agent_id,
    )
    .await;
    let blocked = serde_json::json!({
        "type": "manual_stop",
        "blocking_reason": "user_cancelled",
        "blocked_by": "user:test",
        "blocked_at": now_rfc3339(),
        "message": "blocked for review",
        "recovery_actions": ["resume_session", "reexecute", "reset_to_initial", "cancel_task"],
    })
    .to_string();
    TaskRepo::update(
        &*db,
        UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(blocked)),
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task update creates");
    let (dispatcher, mut rx) = build_dispatcher(Arc::clone(&db), workspace_dir.path()).await;

    assert_eq!(dispatcher.check_once().await.expect("dispatcher runs"), 1);
    let executions = ExecutionRepo::list_by_task(
        &*db,
        &task.id,
        db::PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: db::SortBy::CreatedAt,
            sort_order: db::SortOrder::Desc,
        },
    )
    .await
    .expect("execution rows load");
    assert_eq!(executions.items.len(), 1);
    assert!(tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("ready lifecycle dispatches despite stale annotation")
        .is_some());
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*db, &task.id, "implementer")
            .await
            .expect("execution count loads"),
        1
    );
}
