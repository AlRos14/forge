use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    CreateAgent, CreateExecution, CreateProject, CreateRepo, CreateTask, CreateTaskRoleAssignment,
    CreateWorkspace, DaemonRepo, DaemonStatus, ExecutionRepo, ExecutionStatus, PageRequest,
    ProjectRepo, RepoRepo, ReviewRepo, SortBy, SortOrder, SqliteDb, TaskRepo,
    TaskRoleAssignmentRepo, UpdateDaemonReport, UpdateProject, UpsertDaemon, WorkspaceRepo,
    WorkspaceStatus,
};
use events::{EventBus, EventContext};
use executors::{ExecutionContext, ExecutionResult, ExecutorError, TaskExecutor};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;
use workspace::RepoCacheLockManager;

use super::{
    AutoCascadeOnReviewPass, CheckRetryBudget, DependencyGate, DispatchRoleAgent, NotifyRoleHolder,
    RequireUpstreamRolesCompleted, RunCiSteps,
};
use crate::workflow::{
    default_roles, default_states, default_workflow, HookAction, HookContext, HookResult,
};

async fn sqlite_db() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    SqliteDb::new(pool)
}

async fn seed_project_repo_and_task(db: &SqliteDb, task_id: &str, status: &str) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();

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
            remote_url: "https://example.com/forge.git".to_owned(),
            local_path: None,
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
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

    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.to_owned(),
            project_id: project_id.clone(),
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "test task".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: status.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task creates");
    project_id
}

async fn seed_project_without_repo_and_task(db: &SqliteDb, task_id: &str, status: &str) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();

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

    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.to_owned(),
            project_id: project_id.clone(),
            repo_id: None,
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "test task".to_owned(),
            description: Some("echo test".to_owned()),
            task_type: "implementation".to_owned(),
            status: status.to_owned(),
            priority: 0,
            is_automation: false,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task creates");
    project_id
}

fn setup_git_repo(path: &std::path::Path) -> String {
    run_git(path, &["init"]);
    run_git(path, &["config", "user.email", "test@forge.dev"]);
    run_git(path, &["config", "user.name", "Forge Test"]);
    std::fs::write(path.join("README.md"), "# Forge\n").expect("README writes");
    run_git(path, &["add", "-A"]);
    run_git(path, &["commit", "-m", "initial commit"]);
    run_git(path, &["symbolic-ref", "--short", "HEAD"])
}

fn run_git(path: &std::path::Path, args: &[&str]) -> String {
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

async fn seed_local_project_repo_and_task(
    db: &SqliteDb,
    repo_path: &std::path::Path,
    task_id: &str,
    status: &str,
) -> String {
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
            updated_at: now.clone(),
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

    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.to_owned(),
            project_id: project_id.clone(),
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "test task".to_owned(),
            description: Some("echo test".to_owned()),
            task_type: "implementation".to_owned(),
            status: status.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task creates");
    project_id
}

async fn seed_agent(db: &SqliteDb, agent_id: &str) {
    seed_agent_with_max(db, agent_id, 1).await;
}

async fn seed_agent_with_max(db: &SqliteDb, agent_id: &str, max_concurrent_tasks: i64) {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();

    DaemonRepo::upsert_by_machine_id(
        db,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("machine-{daemon_id}"),
            hostname: "test-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: DaemonStatus::Online,
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
            status: DaemonStatus::Online,
            last_report_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon report updates");

    AgentRepo::create(
        db,
        CreateAgent {
            id: agent_id.to_owned(),
            name: "test-agent".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            max_concurrent_tasks,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("agent creates");
}

struct PendingExecutor {
    sender: mpsc::UnboundedSender<ExecutionContext>,
}

#[async_trait]
impl TaskExecutor for PendingExecutor {
    async fn execute(
        &self,
        ctx: ExecutionContext,
    ) -> std::result::Result<ExecutionResult, ExecutorError> {
        let _ = self.sender.send(ctx);
        std::future::pending::<()>().await;
        unreachable!()
    }

    async fn cancel(&self, _execution_id: &str) -> std::result::Result<(), ExecutorError> {
        Ok(())
    }
}

struct DispatchHarness {
    ctx: HookContext,
    rx: mpsc::UnboundedReceiver<ExecutionContext>,
    _repo_dir: TempDir,
    _workspace_root: TempDir,
}

async fn build_role_dispatch_harness(
    task_id: &str,
    from_state: &str,
    to_state: &str,
    role: &str,
    agent_id: &str,
    max_concurrent_tasks: i64,
) -> DispatchHarness {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_root = TempDir::new().expect("workspace dir creates");
    let project_id =
        seed_local_project_repo_and_task(&db, repo_dir.path(), task_id, to_state).await;
    seed_agent_with_max(&db, agent_id, max_concurrent_tasks).await;
    assign_agent_role(&db, task_id, role, agent_id).await;

    let workflow = Arc::new(default_workflow::default_workflow());
    let gate_config = workflow
        .states
        .iter()
        .find(|state| state.name == to_state)
        .and_then(|state| state.gate_config.clone());
    let (tx, rx) = mpsc::unbounded_channel();

    DispatchHarness {
        ctx: HookContext {
            task_id: task_id.to_owned(),
            project_id,
            from_state: from_state.to_owned(),
            to_state: to_state.to_owned(),
            db,
            event_bus: Arc::new(EventBus::new(16)),
            gate_config,
            workflow,
            triggered_by: api_types::Actor::system(api_types::SystemComponent::Test),
            review_runner: None,
            merge_service: None,
            cleanup_scheduler: None,
            task_executor: Some(Arc::new(PendingExecutor { sender: tx })),
            adapter_registry: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            workspace_root: workspace_root.path().to_path_buf(),
            repo_cache_locks: Some(Arc::new(RepoCacheLockManager::default())),
            workspace_id: None,
            agent_id: None,
            execution_id: None,
            state_config: json!({}),
        },
        rx,
        _repo_dir: repo_dir,
        _workspace_root: workspace_root,
    }
}

async fn build_non_review_gate_dispatch_harness(
    task_id: &str,
    agent_id: &str,
    max_concurrent_tasks: i64,
) -> DispatchHarness {
    let gate_name = "approval_gate";
    let role = "approver";
    let mut harness = build_role_dispatch_harness(
        task_id,
        default_states::TODO,
        gate_name,
        role,
        agent_id,
        max_concurrent_tasks,
    )
    .await;
    let mut workflow = (*harness.ctx.workflow).clone();
    let mut gate = workflow
        .states
        .iter()
        .find(|state| state.name == default_states::IN_PROGRESS)
        .expect("active state exists")
        .clone();
    gate.name = gate_name.to_owned();
    gate.kind = api_types::StateKind::Gate;
    gate.role = Some(role.to_owned());
    gate.gate_config = Some(api_types::GateConfig {
        reject_target: Some(default_states::IN_PROGRESS.to_owned()),
        max_rejections: Some(2),
        approve_label: None,
        reject_label: None,
        requires_user_approval: Some(true),
        optional_when_unassigned: Some(false),
    });
    gate.triggers.clear();
    gate.triggers.insert(
        api_types::WorkflowTrigger::Accept,
        api_types::WorkflowTriggerDefinition {
            to: default_states::IN_PROGRESS.to_owned(),
            dispatch: None,
        },
    );
    workflow.states.push(gate.clone());
    harness.ctx.to_state = gate_name.to_owned();
    harness.ctx.gate_config = gate.gate_config;
    harness.ctx.workflow = Arc::new(workflow);
    harness
}

async fn build_no_repo_dispatch_harness(
    task_id: &str,
    agent_id: &str,
    max_concurrent_tasks: i64,
) -> DispatchHarness {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_root = TempDir::new().expect("workspace dir creates");
    let project_id =
        seed_project_without_repo_and_task(&db, task_id, default_states::IN_PROGRESS).await;
    seed_agent_with_max(&db, agent_id, max_concurrent_tasks).await;
    assign_agent_role(&db, task_id, default_roles::CODER, agent_id).await;

    let workflow = Arc::new(default_workflow::default_workflow());
    let gate_config = workflow
        .states
        .iter()
        .find(|state| state.name == default_states::IN_PROGRESS)
        .and_then(|state| state.gate_config.clone());
    let (tx, rx) = mpsc::unbounded_channel();

    DispatchHarness {
        ctx: HookContext {
            task_id: task_id.to_owned(),
            project_id,
            from_state: default_states::TODO.to_owned(),
            to_state: default_states::IN_PROGRESS.to_owned(),
            db,
            event_bus: Arc::new(EventBus::new(16)),
            gate_config,
            workflow,
            triggered_by: api_types::Actor::system(api_types::SystemComponent::Test),
            review_runner: None,
            merge_service: None,
            cleanup_scheduler: None,
            task_executor: Some(Arc::new(PendingExecutor { sender: tx })),
            adapter_registry: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            workspace_root: workspace_root.path().to_path_buf(),
            repo_cache_locks: Some(Arc::new(RepoCacheLockManager::default())),
            workspace_id: None,
            agent_id: None,
            execution_id: None,
            state_config: json!({}),
        },
        rx,
        _repo_dir: repo_dir,
        _workspace_root: workspace_root,
    }
}

async fn build_initial_dispatch_harness(
    task_id: &str,
    agent_id: &str,
    max_concurrent_tasks: i64,
) -> DispatchHarness {
    build_role_dispatch_harness(
        task_id,
        default_states::TODO,
        default_states::IN_PROGRESS,
        default_roles::CODER,
        agent_id,
        max_concurrent_tasks,
    )
    .await
}

async fn assign_agent_role(db: &SqliteDb, task_id: &str, role: &str, agent_id: &str) {
    let now = now_rfc3339();
    TaskRoleAssignmentRepo::assign(
        db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            role_name: role.to_owned(),
            assignee_type: Some(db::AssigneeKind::Agent),
            assignee_id: Some(agent_id.to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("role assignment creates");
}

async fn build_test_ctx(
    task_id: &str,
    from_state: &str,
    to_state: &str,
    role_agent_setup: Option<(&str, &str)>,
) -> HookContext {
    let db = Arc::new(sqlite_db().await);
    let project_id = seed_project_repo_and_task(&db, task_id, from_state).await;

    if let Some((role, agent_id)) = role_agent_setup {
        seed_agent(&db, agent_id).await;
        assign_agent_role(&db, task_id, role, agent_id).await;
    }

    let workflow = Arc::new(default_workflow::default_workflow());
    let gate_config = workflow
        .states
        .iter()
        .find(|state| state.name == to_state)
        .and_then(|state| state.gate_config.clone());

    HookContext {
        task_id: task_id.to_owned(),
        project_id,
        from_state: from_state.to_owned(),
        to_state: to_state.to_owned(),
        db,
        event_bus: Arc::new(EventBus::new(16)),
        gate_config,
        workflow,
        triggered_by: api_types::Actor::system(api_types::SystemComponent::Test),
        review_runner: None,
        merge_service: None,
        cleanup_scheduler: None,
        task_executor: None,
        adapter_registry: None,
        daemon_connections: None,
        workspace_exec_locks: None,
        terminal_activity: None,
        workspace_root: PathBuf::new(),
        repo_cache_locks: None,
        workspace_id: None,
        agent_id: None,
        execution_id: None,
        state_config: json!({}),
    }
}

fn add_upstream_gate(ctx: &mut HookContext, gate_name: &str, role: &str) {
    let mut workflow = (*ctx.workflow).clone();
    let mut gate = workflow
        .states
        .iter()
        .find(|state| state.name == default_states::REVIEW)
        .expect("review template exists")
        .clone();
    gate.name = gate_name.to_owned();
    gate.role = Some(role.to_owned());
    gate.triggers.clear();
    gate.triggers.insert(
        api_types::WorkflowTrigger::Accept,
        api_types::WorkflowTriggerDefinition {
            to: ctx.to_state.clone(),
            dispatch: None,
        },
    );
    workflow.states.push(gate);
    ctx.workflow = Arc::new(workflow);
}

#[tokio::test]
async fn dependency_gate_skips_user_managed_transitions() {
    let mut ctx = build_test_ctx(
        "task-dependency-user-skip",
        default_states::TODO,
        default_states::PLANNING,
        None,
    )
    .await;
    ctx.triggered_by = api_types::Actor::user(api_types::UserActionSource::Api);

    let result = DependencyGate.execute(&ctx).await;

    assert!(matches!(
        result,
        HookResult::Skipped { reason } if reason.contains("user-managed transition")
    ));
}

async fn seed_transition_log(
    db: &SqliteDb,
    task_id: &str,
    from_state: &str,
    to_state: &str,
    rejection: bool,
) {
    seed_transition_log_at(db, task_id, from_state, to_state, rejection, &now_rfc3339()).await;
}

async fn seed_transition_log_at(
    db: &SqliteDb,
    task_id: &str,
    from_state: &str,
    to_state: &str,
    rejection: bool,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO transition_log (id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, hook_results_json, rejection, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(task_id)
    .bind(from_state)
    .bind(to_state)
    .bind::<Option<String>>(None)
    .bind("system")
    .bind("test")
    .bind(Option::<&str>::None)
    .bind(if rejection { 1_i64 } else { 0_i64 })
    .bind(created_at)
    .execute(db.pool())
    .await
    .expect("transition log inserts");
}

async fn seed_completed_executor_execution(ctx: &mut HookContext) -> String {
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let now = now_rfc3339();
    let workspace_id = new_uuid_v4();
    let worktree_path = std::env::temp_dir().join(format!("forge-workflow-action-{workspace_id}"));
    std::fs::create_dir_all(&worktree_path).expect("worktree path creates");
    setup_git_repo(&worktree_path);
    let before_sha = run_git(&worktree_path, &["rev-parse", "HEAD"]);
    WorkspaceRepo::create(
        &*ctx.db,
        CreateWorkspace {
            id: workspace_id.clone(),
            task_id: ctx.task_id.clone(),
            repo_id: task.repo_id.clone().unwrap(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            branch: ::workspace::task_branch_name(&ctx.task_id),
            status: WorkspaceStatus::Ready,
            before_sha: Some(before_sha.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("workspace creates");
    ctx.workspace_id = Some(workspace_id.clone());

    let execution_id = new_uuid_v4();
    ExecutionRepo::create(
        &*ctx.db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: ctx.task_id.clone(),
            agent_id: None,
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
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("executor summary".to_owned()),
            logs_path: None,
            before_sha: Some(before_sha.clone()),
            after_sha: Some(before_sha),
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: Some(workspace_id),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("execution creates");
    execution_id
}

async fn seed_running_execution_for_task(db: &SqliteDb, task_id: &str, agent_id: &str, role: &str) {
    let now = now_rfc3339();
    ExecutionRepo::create(
        db,
        CreateExecution {
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
    .expect("running execution creates");
}

#[tokio::test]
async fn run_ci_steps_empty_reason_contains_ci_steps() {
    let mut ctx = build_test_ctx(
        "task-run-review-ci-steps",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.state_config = json!({ "ci_steps": [] });

    let result = RunCiSteps.execute(&ctx).await;

    match result {
        HookResult::Skipped { reason } => assert!(reason.contains("ci steps")),
        other => panic!("expected skipped result, got {other:?}"),
    }
}

#[tokio::test]
async fn run_ci_steps_skips_when_ci_steps_empty() {
    let mut ctx = build_test_ctx(
        "task-run-review-empty",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.state_config = json!({ "ci_steps": [] });

    let result = RunCiSteps.execute(&ctx).await;

    match result {
        HookResult::Skipped { reason } => assert!(reason.contains("ci steps")),
        other => panic!("expected skipped result, got {other:?}"),
    }
}

#[tokio::test]
async fn run_ci_steps_fails_closed_when_workspace_missing() {
    let mut ctx = build_test_ctx(
        "task-run-review-no-workspace",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.state_config = json!({ "ci_steps": ["cargo test"] });

    let result = RunCiSteps.execute(&ctx).await;

    assert!(matches!(result, HookResult::Failed { reason } if reason.contains("exact Workspace")));
}

#[tokio::test]
async fn run_ci_steps_does_not_require_an_actor_execution() {
    let mut ctx = build_test_ctx(
        "task-run-review-runner-missing",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    let _unused_execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(result, HookResult::Ok), "{result:?}");
    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(runs[0].caused_by_execution_id.is_none());
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());
}

#[tokio::test]
async fn run_ci_steps_creates_validation_without_review_authority() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let result = RunCiSteps.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());
}

#[tokio::test]
async fn run_ci_steps_pass_then_dispatches_reviewer() {
    let agent_id = "agent-reviewer-dispatch";
    let mut harness = build_role_dispatch_harness(
        "task-run-ci-reviewer-dispatch",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        default_roles::REVIEWER,
        agent_id,
        1,
    )
    .await;
    let mut ctx = harness.ctx.clone();
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    assert!(
        matches!(dispatch_result, HookResult::Ok),
        "{dispatch_result:?}"
    );

    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("reviewer executor spawned in time")
        .expect("reviewer execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);
    assert!(execution_ctx.description.contains("FORGE_RESULT:"));
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::REVIEWER)
            .await
            .expect("reviewer execution count loads"),
        1
    );
}

#[tokio::test]
async fn legacy_review_pass_timestamp_does_not_skip_new_review_execution() {
    let agent_id = "agent-reviewer-ci-only";
    let mut harness = build_role_dispatch_harness(
        "task-run-ci-reviewer-ci-only",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        default_roles::REVIEWER,
        agent_id,
        1,
    )
    .await;
    let mut ctx = harness.ctx.clone();
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });
    TaskRepo::set_review_passed_at(&*ctx.db, &ctx.task_id, Some(now_rfc3339()), &now_rfc3339())
        .await
        .expect("review_passed_at seeds");

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    assert!(
        matches!(dispatch_result, HookResult::Ok),
        "{dispatch_result:?}"
    );
    let _reviewer_context =
        tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
            .await
            .expect("reviewer dispatch is not inferred from review_passed_at")
            .expect("reviewer Execution context is produced");
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::REVIEWER)
            .await
            .expect("reviewer execution count loads"),
        1
    );
}

#[tokio::test]
async fn run_ci_steps_failure_prevents_reviewer_dispatch() {
    let agent_id = "agent-reviewer-ci-fail";
    let harness = build_role_dispatch_harness(
        "task-run-ci-reviewer-fail",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        default_roles::REVIEWER,
        agent_id,
        1,
    )
    .await;
    let mut ctx = harness.ctx.clone();
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["false"] });

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(
        matches!(ci_result, HookResult::Failed { .. }),
        "{ci_result:?}"
    );

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Failed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::REVIEWER)
            .await
            .expect("reviewer executions load"),
        0
    );
}

#[tokio::test]
async fn validation_is_independent_when_reviewer_is_at_capacity() {
    let agent_id = "agent-reviewer-capacity";
    let mut harness = build_role_dispatch_harness(
        "task-run-ci-reviewer-capacity",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        default_roles::REVIEWER,
        agent_id,
        1,
    )
    .await;
    let mut ctx = harness.ctx.clone();
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let current_task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let other_task_id = "task-run-ci-reviewer-capacity-other";
    TaskRepo::create(
        &*ctx.db,
        CreateTask {
            id: other_task_id.to_owned(),
            project_id: current_task.project_id.clone(),
            repo_id: current_task.repo_id.clone(),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "other review task".to_owned(),
            description: Some("echo other".to_owned()),
            task_type: "implementation".to_owned(),
            status: default_states::REVIEW.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("other task creates");
    assign_agent_role(&ctx.db, other_task_id, default_roles::REVIEWER, agent_id).await;
    seed_running_execution_for_task(&ctx.db, other_task_id, agent_id, default_roles::REVIEWER)
        .await;

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    match dispatch_result {
        HookResult::Skipped { reason } => assert_eq!(reason, "agent at capacity"),
        other => panic!("expected capacity skip, got {other:?}"),
    }

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.rx.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn validation_pass_without_review_execution_does_not_authorize_merge() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    assert!(matches!(
        AutoCascadeOnReviewPass.execute(&ctx).await,
        HookResult::Skipped { .. }
    ));
    assert_eq!(
        TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
            .await
            .expect("Task lookup succeeds")
            .expect("Task exists")
            .status,
        default_states::REVIEW
    );
}

#[tokio::test]
async fn validation_pass_and_review_request_changes_remain_independent() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    let executor_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(executor_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });
    assert!(matches!(RunCiSteps.execute(&ctx).await, HookResult::Ok));
    let validation = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load")
        .into_iter()
        .next()
        .expect("ValidationRun exists");
    assert_eq!(validation.status, db::ValidationRunStatus::Passed);

    let agent_id = new_uuid_v4();
    seed_agent_with_max(&ctx.db, &agent_id, 1).await;
    let now = now_rfc3339();
    let reviewer = ExecutionRepo::create(
        &*ctx.db,
        CreateExecution {
            id: new_uuid_v4(),
            task_id: ctx.task_id.clone(),
            agent_id: Some(agent_id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent_id)),
            purpose: Some(db::ExecutionPurpose::Review),
            harness_session_id: None,
            role: default_roles::REVIEWER.to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("requests a correction".to_owned()),
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
    .expect("Review Execution creates");
    crate::CollaborationService::new(Arc::clone(&ctx.db), Arc::clone(&ctx.event_bus))
        .create_review_report_from_execution(
            &reviewer.id,
            "FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"fail\",\"summary\":\"A correction is required.\",\"criteria\":[\"correctness\"],\"findings\":[\"Fix the boundary case.\"],\"questions\":[],\"evidence_considered\":[]}",
        )
        .await
        .expect("request-changes ReviewReport creates");

    ctx.execution_id = Some(reviewer.id.clone());
    assert!(matches!(
        AutoCascadeOnReviewPass.execute(&ctx).await,
        HookResult::Cascade { to, .. } if to == default_states::IN_PROGRESS
    ));
    assert_eq!(
        db::ValidationRunRepo::get_validation_run(&*ctx.db, &validation.id)
            .await
            .expect("exact ValidationRun rereads")
            .expect("ValidationRun remains")
            .status,
        db::ValidationRunStatus::Passed
    );
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Review rows load")
        .is_empty());
}

#[tokio::test]
async fn run_ci_steps_with_user_approval_gate_waits_for_human() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.gate_config
        .as_mut()
        .expect("review gate config")
        .requires_user_approval = Some(true);
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);
    ctx.state_config = json!({ "ci_steps": ["test -d ."] });

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let reviews = ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("reviews load");
    assert!(
        reviews.is_empty(),
        "Human review is an exact Human Execution"
    );

    let cascade_result = AutoCascadeOnReviewPass.execute(&ctx).await;
    assert!(matches!(cascade_result, HookResult::Skipped { .. }));
}

#[tokio::test]
async fn unconfigured_review_with_user_approval_gate_waits_for_human() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.gate_config
        .as_mut()
        .expect("review gate config")
        .requires_user_approval = Some(true);
    let execution_id = seed_completed_executor_execution(&mut ctx).await;
    ctx.execution_id = Some(execution_id);

    let result = super::AutoCascadeOnUnconfiguredReview.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok), "{result:?}");
    let reviews = ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("reviews load");
    assert!(
        reviews.is_empty(),
        "no synthetic awaiting_human Review exists"
    );
}

#[tokio::test]
async fn check_retry_budget_allows_review_entry_when_rejections_reach_max() {
    let ctx = build_test_ctx(
        "task-retry-budget-cascade",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    seed_transition_log(
        &ctx.db,
        &ctx.task_id,
        default_states::REVIEW,
        default_states::IN_PROGRESS,
        true,
    )
    .await;
    seed_transition_log(
        &ctx.db,
        &ctx.task_id,
        default_states::REVIEW,
        default_states::IN_PROGRESS,
        true,
    )
    .await;
    let result = CheckRetryBudget.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok), "{result:?}");
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(task.blocked_json, None);
}

#[tokio::test]
async fn check_retry_budget_ok_when_below_max() {
    let ctx = build_test_ctx(
        "task-retry-budget-ok",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    seed_transition_log(
        &ctx.db,
        &ctx.task_id,
        default_states::REVIEW,
        default_states::IN_PROGRESS,
        true,
    )
    .await;

    let result = CheckRetryBudget.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
}

#[tokio::test]
async fn check_retry_budget_ok_without_gate_config() {
    let mut ctx = build_test_ctx(
        "task-retry-budget-no-config",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.gate_config = None;

    let result = CheckRetryBudget.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
}

#[tokio::test]
async fn require_upstream_roles_completed_fails_when_upstream_gate_was_not_entered() {
    let mut ctx = build_test_ctx(
        "task-upstream-missing-gate",
        "approval_gate",
        default_states::IN_PROGRESS,
        Some(("approver", "agent-approver-missing-gate")),
    )
    .await;
    add_upstream_gate(&mut ctx, "approval_gate", "approver");

    let result = RequireUpstreamRolesCompleted.execute(&ctx).await;

    match result {
        HookResult::Failed { reason } => {
            assert!(reason.contains("approver"));
            assert!(reason.contains("approval_gate"));
        }
        other => panic!("expected failed result, got {other:?}"),
    }
}

#[tokio::test]
async fn require_upstream_roles_completed_ok_when_upstream_gate_has_no_assignee() {
    let mut ctx = build_test_ctx(
        "task-upstream-no-gate-assignee",
        "approval_gate",
        default_states::IN_PROGRESS,
        None,
    )
    .await;
    add_upstream_gate(&mut ctx, "approval_gate", "approver");

    let result = RequireUpstreamRolesCompleted.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
}

#[tokio::test]
async fn require_upstream_roles_completed_ok_when_upstream_gate_entry_is_logged() {
    let mut ctx = build_test_ctx(
        "task-upstream-gate-log",
        "approval_gate",
        default_states::IN_PROGRESS,
        Some(("approver", "agent-approver-with-gate-log")),
    )
    .await;
    add_upstream_gate(&mut ctx, "approval_gate", "approver");
    seed_transition_log(
        &ctx.db,
        &ctx.task_id,
        default_states::TODO,
        "approval_gate",
        false,
    )
    .await;

    let result = RequireUpstreamRolesCompleted.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
}

#[tokio::test]
async fn dispatch_role_agent_skips_without_coder_assignment() {
    let ctx = build_test_ctx(
        "task-dispatch-no-coder",
        default_states::TODO,
        default_states::IN_PROGRESS,
        None,
    )
    .await;

    let result = DispatchRoleAgent.execute(&ctx).await;

    assert!(matches!(result, HookResult::Skipped { .. }));
}

#[tokio::test]
async fn dispatch_role_agent_emits_event_for_coder_assignment() {
    let agent_id = "agent-coder-dispatch";
    let harness = build_initial_dispatch_harness("task-dispatch-coder", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    let mut rx = ctx.event_bus.subscribe();

    let result = DispatchRoleAgent.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
    let event = rx.try_recv().expect("dispatch event emits");
    assert_eq!(event.event_type, "task.role_agent_dispatched");
    assert_eq!(event.entity_id, ctx.task_id);
    match event.context {
        EventContext::TaskRoleAgentDispatched {
            task_id,
            role,
            agent_id: dispatched_agent_id,
            state,
            prompt_system,
            prompt_user,
            parent_execution_id: _,
        } => {
            assert_eq!(task_id, ctx.task_id);
            assert_eq!(role, default_roles::CODER);
            assert_eq!(dispatched_agent_id, agent_id);
            assert_eq!(state, default_states::IN_PROGRESS);
            assert!(prompt_system.contains("coder"));
            assert!(prompt_user.contains("test task"));
        }
        other => panic!("unexpected event context: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_role_agent_initial_dispatch_creates_execution_with_capacity() {
    let agent_id = "agent-coder-initial";
    let mut harness = build_initial_dispatch_harness("task-dispatch-initial", agent_id, 1).await;
    let ctx = harness.ctx.clone();

    let result = DispatchRoleAgent.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok), "{result:?}");
    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("executor spawned in time")
        .expect("execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);

    let executions = ExecutionRepo::list_by_task_and_role(
        &*ctx.db,
        &ctx.task_id,
        default_roles::CODER,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("executions load");
    assert_eq!(executions.items.len(), 1);
    assert_eq!(executions.items[0].status, ExecutionStatus::Running);
    assert_eq!(executions.items[0].agent_id.as_deref(), Some(agent_id));
}

#[tokio::test]
async fn non_review_gate_rejection_budget_allows_dispatch_at_configured_limit() {
    let agent_id = "agent-approver-budget-limit";
    let mut harness =
        build_non_review_gate_dispatch_harness("task-approver-budget-limit", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    for _ in 0..2 {
        seed_transition_log(
            &ctx.db,
            &ctx.task_id,
            "approval_gate",
            "approval_gate",
            true,
        )
        .await;
    }

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    let budget_result = CheckRetryBudget.execute(&ctx).await;

    assert!(matches!(dispatch_result, HookResult::Ok));
    assert!(matches!(budget_result, HookResult::Ok));
    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("planner executor spawned in time")
        .expect("planner execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert!(task.blocked_json.is_none());
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, "approver")
            .await
            .expect("execution count loads"),
        1
    );
}

#[tokio::test]
async fn exhausted_non_review_gate_budget_blocks_without_dispatching() {
    let agent_id = "agent-approver-budget-exhausted";
    let mut harness =
        build_non_review_gate_dispatch_harness("task-approver-budget-exhausted", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    for _ in 0..3 {
        seed_transition_log(
            &ctx.db,
            &ctx.task_id,
            "approval_gate",
            "approval_gate",
            true,
        )
        .await;
    }

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    let budget_result = CheckRetryBudget.execute(&ctx).await;

    assert!(matches!(
        dispatch_result,
        HookResult::Skipped { reason }
            if reason == "retry budget exhausted (3/2); task will be blocked before dispatch"
    ));
    assert!(matches!(budget_result, HookResult::Ok));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.rx.recv())
            .await
            .is_err()
    );
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, "approver")
            .await
            .expect("execution count loads"),
        0
    );
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let blocked: serde_json::Value = serde_json::from_str(
        task.blocked_json
            .as_deref()
            .expect("retry exhaustion blocks the task"),
    )
    .expect("blocking annotation parses");
    assert_eq!(blocked["kind"], "retry_exhausted");
    assert_eq!(blocked["reason"], "gate rejection budget exhausted: 3/2");
}

#[tokio::test]
async fn dispatch_role_agent_skips_initial_dispatch_without_repo() {
    let agent_id = "agent-coder-no-repo";
    let mut harness = build_no_repo_dispatch_harness("task-dispatch-no-repo", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    let mut event_rx = ctx.event_bus.subscribe();

    let result = DispatchRoleAgent.execute(&ctx).await;

    match result {
        HookResult::Skipped { reason } => assert_eq!(reason, "task has no associated repo"),
        other => panic!("expected skipped result, got {other:?}"),
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.rx.recv())
            .await
            .is_err()
    );
    assert!(
        event_rx.try_recv().is_err(),
        "dispatch event should not emit for repo-less task"
    );
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::CODER)
            .await
            .expect("execution count loads"),
        0
    );
}

#[tokio::test]
async fn dispatch_role_agent_initial_dispatch_skips_at_capacity() {
    let agent_id = "agent-coder-capacity";
    let mut harness = build_initial_dispatch_harness("task-dispatch-capacity", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    let other_task_id = "task-dispatch-capacity-other";
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    TaskRepo::create(
        &*ctx.db,
        CreateTask {
            id: other_task_id.to_owned(),
            project_id: task.project_id.clone(),
            repo_id: task.repo_id.clone(),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "other task".to_owned(),
            description: Some("echo other".to_owned()),
            task_type: "implementation".to_owned(),
            status: default_states::IN_PROGRESS.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("other task creates");
    assign_agent_role(&ctx.db, other_task_id, default_roles::CODER, agent_id).await;
    seed_running_execution_for_task(&ctx.db, other_task_id, agent_id, default_roles::CODER).await;

    let result = DispatchRoleAgent.execute(&ctx).await;

    match result {
        HookResult::Skipped { reason } => assert_eq!(reason, "agent at capacity"),
        other => panic!("expected skipped result, got {other:?}"),
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.rx.recv())
            .await
            .is_err()
    );
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::CODER)
            .await
            .expect("execution count loads"),
        0
    );
}

#[tokio::test]
async fn dispatch_role_agent_initial_dispatch_skips_when_execution_already_running() {
    let agent_id = "agent-coder-running";
    let mut harness = build_initial_dispatch_harness("task-dispatch-running", agent_id, 1).await;
    let ctx = harness.ctx.clone();
    seed_running_execution_for_task(&ctx.db, &ctx.task_id, agent_id, "executor").await;

    let result = DispatchRoleAgent.execute(&ctx).await;

    match result {
        HookResult::Skipped { reason } => assert_eq!(reason, "execution already running"),
        other => panic!("expected skipped result, got {other:?}"),
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), harness.rx.recv())
            .await
            .is_err()
    );
    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::CODER)
            .await
            .expect("coder execution count loads"),
        0
    );
}

#[tokio::test]
async fn notify_role_holder_skips_without_coder_assignment() {
    let ctx = build_test_ctx(
        "task-notify-no-coder",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;

    let result = NotifyRoleHolder.execute(&ctx).await;

    assert!(matches!(result, HookResult::Skipped { .. }));
}

#[tokio::test]
async fn notify_role_holder_emits_event_for_coder_assignment() {
    let agent_id = "agent-coder-notify";
    let ctx = build_test_ctx(
        "task-notify-coder",
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        Some((default_roles::CODER, agent_id)),
    )
    .await;
    let mut rx = ctx.event_bus.subscribe();

    let result = NotifyRoleHolder.execute(&ctx).await;

    assert!(matches!(result, HookResult::Ok));
    let event = rx.try_recv().expect("notification event emits");
    assert_eq!(event.event_type, "task.role_notified");
    assert_eq!(event.entity_id, ctx.task_id);
    match event.context {
        EventContext::TaskRoleNotified {
            task_id,
            role,
            notified_agent_id,
            notified_user_handle,
            state,
            reason,
        } => {
            assert_eq!(task_id, ctx.task_id);
            assert_eq!(role, default_roles::CODER);
            assert_eq!(notified_agent_id.as_deref(), Some(agent_id));
            assert_eq!(notified_user_handle, None);
            assert_eq!(state, default_states::REVIEW);
            assert!(reason.contains(default_states::REVIEW));
        }
        other => panic!("unexpected event context: {other:?}"),
    }
}

// --- Reviewer dispatch harness for review-state tests (7.3–7.6) ---

async fn build_reviewer_dispatch_harness(
    task_id: &str,
    reviewer_agent_id: &str,
    max_concurrent_tasks: i64,
    ci_steps: Vec<&str>,
) -> DispatchHarness {
    let db = Arc::new(sqlite_db().await);
    let repo_dir = TempDir::new().expect("repo dir creates");
    let workspace_root = TempDir::new().expect("workspace dir creates");
    let project_id =
        seed_local_project_repo_and_task(&db, repo_dir.path(), task_id, default_states::REVIEW)
            .await;
    seed_agent_with_max(&db, reviewer_agent_id, max_concurrent_tasks).await;
    assign_agent_role(&db, task_id, default_roles::REVIEWER, reviewer_agent_id).await;

    let workflow = Arc::new(default_workflow::default_workflow());
    let gate_config = workflow
        .states
        .iter()
        .find(|state| state.name == default_states::REVIEW)
        .and_then(|state| state.gate_config.clone());
    let (tx, rx) = mpsc::unbounded_channel();

    let ci_steps_value: Vec<serde_json::Value> = ci_steps
        .into_iter()
        .map(|step| serde_json::Value::String(step.to_owned()))
        .collect();

    let mut harness = DispatchHarness {
        ctx: HookContext {
            task_id: task_id.to_owned(),
            project_id,
            from_state: default_states::IN_PROGRESS.to_owned(),
            to_state: default_states::REVIEW.to_owned(),
            db,
            event_bus: Arc::new(EventBus::new(16)),
            gate_config,
            workflow,
            triggered_by: api_types::Actor::system(api_types::SystemComponent::Test),
            review_runner: None,
            merge_service: None,
            cleanup_scheduler: None,
            task_executor: Some(Arc::new(PendingExecutor { sender: tx })),
            adapter_registry: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            workspace_root: workspace_root.path().to_path_buf(),
            repo_cache_locks: Some(Arc::new(RepoCacheLockManager::default())),
            workspace_id: None,
            agent_id: None,
            execution_id: None,
            state_config: json!({ "ci_steps": ci_steps_value }),
        },
        rx,
        _repo_dir: repo_dir,
        _workspace_root: workspace_root,
    };

    let execution_id = seed_completed_executor_execution(&mut harness.ctx).await;
    harness.ctx.execution_id = Some(execution_id);
    harness
}

#[tokio::test]
async fn ci_passes_then_reviewer_dispatched_via_dispatch_role_agent() {
    let task_id = new_uuid_v4();
    let reviewer_id = "agent-reviewer-dispatch";
    let mut harness =
        build_reviewer_dispatch_harness(&task_id, reviewer_id, 2, vec!["test -d ."]).await;
    let ctx = harness.ctx.clone();

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(
        matches!(ci_result, HookResult::Ok),
        "CI should pass: {ci_result:?}"
    );

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    assert!(
        matches!(dispatch_result, HookResult::Ok),
        "dispatch should succeed: {dispatch_result:?}"
    );

    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("executor spawned in time")
        .expect("execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);

    let executions = ExecutionRepo::list_by_task_and_role(
        &*ctx.db,
        &ctx.task_id,
        default_roles::REVIEWER,
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("reviewer executions load");
    assert_eq!(executions.items.len(), 1);
    assert_eq!(executions.items[0].status, db::ExecutionStatus::Running);
    assert_eq!(executions.items[0].agent_id.as_deref(), Some(reviewer_id));
    assert_eq!(executions.items[0].role, default_roles::REVIEWER);
}

#[tokio::test]
async fn subtask_root_still_dispatches_reviewer_after_coder_completion() {
    let task_id = new_uuid_v4();
    let reviewer_id = "agent-reviewer-subtask-root";
    let mut harness =
        build_reviewer_dispatch_harness(&task_id, reviewer_id, 2, vec!["test -d ."]).await;
    let ctx = harness.ctx.clone();
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let now = now_rfc3339();
    let subtask_id = new_uuid_v4();
    TaskRepo::create(
        &*ctx.db,
        CreateTask {
            id: subtask_id.clone(),
            project_id: task.project_id.clone(),
            repo_id: task.repo_id.clone(),
            parent_task_id: Some(task.id.clone()),
            subtask_order: Some(0),
            assignee_type: None,
            assignee_id: None,
            title: "ordered child".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: default_states::DONE.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("subtask creates");
    TaskRepo::set_metadata_json(
        &*ctx.db,
        &subtask_id,
        Some(r#"{"ordered_sequence_started":true}"#.to_owned()),
        &now,
    )
    .await
    .expect("subtask metadata writes");

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    assert!(
        matches!(dispatch_result, HookResult::Ok),
        "reviewer dispatch should not be skipped for subtask roots: {dispatch_result:?}"
    );

    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("executor spawned in time")
        .expect("execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);
}

#[tokio::test]
async fn reviewer_dispatch_ignores_waiting_review_tasks_without_running_execution() {
    let task_id = new_uuid_v4();
    let reviewer_id = "agent-reviewer-waiting-review";
    let mut harness =
        build_reviewer_dispatch_harness(&task_id, reviewer_id, 1, vec!["test -d ."]).await;
    let ctx = harness.ctx.clone();
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    let other_task_id = "task-waiting-review-no-execution";
    TaskRepo::create(
        &*ctx.db,
        CreateTask {
            id: other_task_id.to_owned(),
            project_id: task.project_id.clone(),
            repo_id: task.repo_id.clone(),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "other waiting review task".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: default_states::REVIEW.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("other task creates");
    assign_agent_role(&ctx.db, other_task_id, default_roles::REVIEWER, reviewer_id).await;

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(matches!(ci_result, HookResult::Ok), "{ci_result:?}");

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    assert!(
        matches!(dispatch_result, HookResult::Ok),
        "dispatch should not be blocked by waiting review tasks: {dispatch_result:?}"
    );

    let execution_ctx = tokio::time::timeout(std::time::Duration::from_secs(1), harness.rx.recv())
        .await
        .expect("executor spawned in time")
        .expect("execution context received");
    assert_eq!(execution_ctx.task_id, ctx.task_id);
}

#[tokio::test]
async fn validation_failure_is_not_a_review_verdict() {
    let task_id = new_uuid_v4();
    let reviewer_id = "agent-reviewer-ci-fail";
    let harness = build_reviewer_dispatch_harness(&task_id, reviewer_id, 2, vec!["exit 1"]).await;
    let ctx = harness.ctx.clone();

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(
        matches!(ci_result, HookResult::Failed { .. }),
        "CI hook returns Failed on CI failure: {ci_result:?}"
    );

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Failed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());

    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::REVIEWER)
            .await
            .expect("reviewer execution count"),
        0,
        "Validation failure does not fabricate a reviewer verdict"
    );
}

#[tokio::test]
async fn reviewer_at_capacity_ci_runs_dispatch_queues() {
    let task_id = new_uuid_v4();
    let reviewer_id = "agent-reviewer-capacity";
    let harness =
        build_reviewer_dispatch_harness(&task_id, reviewer_id, 1, vec!["test -d ."]).await;
    let ctx = harness.ctx.clone();

    let other_task_id = new_uuid_v4();
    let task = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    TaskRepo::create(
        &*ctx.db,
        CreateTask {
            id: other_task_id.clone(),
            project_id: task.project_id.clone(),
            repo_id: task.repo_id.clone(),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "other review task".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: default_states::REVIEW.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("other task creates");
    assign_agent_role(
        &ctx.db,
        &other_task_id,
        default_roles::REVIEWER,
        reviewer_id,
    )
    .await;
    seed_running_execution_for_task(
        &ctx.db,
        &other_task_id,
        reviewer_id,
        default_roles::REVIEWER,
    )
    .await;

    let ci_result = RunCiSteps.execute(&ctx).await;
    assert!(
        matches!(ci_result, HookResult::Ok),
        "CI should pass even when reviewer at capacity: {ci_result:?}"
    );

    let runs = db::ValidationRunRepo::list_validation_runs_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("ValidationRuns load");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, db::ValidationRunStatus::Passed);
    assert!(ReviewRepo::list_by_task(&*ctx.db, &ctx.task_id)
        .await
        .expect("legacy Reviews load")
        .is_empty());

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    match dispatch_result {
        HookResult::Skipped { reason } => assert_eq!(reason, "agent at capacity"),
        other => panic!("expected skipped at capacity, got {other:?}"),
    }

    assert_eq!(
        ExecutionRepo::count_by_task_and_role(&*ctx.db, &ctx.task_id, default_roles::REVIEWER)
            .await
            .expect("reviewer execution count"),
        0,
        "no reviewer execution when agent at capacity"
    );
}

#[tokio::test]
async fn no_reviewer_assigned_auto_cascade_to_merging() {
    let task_id = new_uuid_v4();
    let mut ctx = build_test_ctx(
        &task_id,
        default_states::IN_PROGRESS,
        default_states::REVIEW,
        None,
    )
    .await;
    ctx.state_config = json!({});

    let ci_result = RunCiSteps.execute(&ctx).await;
    match ci_result {
        HookResult::Skipped { reason } => assert!(reason.contains("ci steps")),
        other => panic!("expected skipped for empty ci, got {other:?}"),
    }

    let dispatch_result = DispatchRoleAgent.execute(&ctx).await;
    match dispatch_result {
        HookResult::Skipped { reason } => assert_eq!(reason, "no reviewer role assigned"),
        other => panic!("expected skipped for no reviewer, got {other:?}"),
    }

    let cascade_result = super::AutoCascadeOnUnconfiguredReview.execute(&ctx).await;
    match cascade_result {
        HookResult::Cascade { to, reason } => {
            assert_eq!(to, default_states::MERGING);
            assert!(reason.contains("no checks or reviewer"));
        }
        other => panic!("expected cascade to merging, got {other:?}"),
    }
}
