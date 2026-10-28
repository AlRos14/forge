use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, ActorRef,
    AddWorkUnitDependency, CoordinationMode, CreateDomainEvent, CreateExecution, CreateProject,
    CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole, CreateWorkUnit,
    CreateWorkUnitExecution, CreateWorkUnitIntegration, CreateWorkUnitWorkspace, CreateWorkspace,
    CreateWorkspaceLease, DbError, ExecutionPurpose, ExecutionRepo, ExecutionStatus, ProjectRepo,
    RecordWorkUnitIntegration, RepoRepo, RoleMembershipRepo, RoleMembershipStatus, SqliteDb,
    TaskIntegrationOperationRepo, TaskRepo, TaskRoleRepo, UpdateExecution, WorkMode,
    WorkUnitExecutionRepo, WorkUnitIntegrationOutcome, WorkUnitRepo, WorkUnitStatus,
    WorkUnitWorkspaceRepo, WorkspaceLeaseRepo, WorkspaceRepo, WorkspaceStatus,
};
use std::path::Path;
use tempfile::TempDir;

async fn database() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    SqliteDb::new(pool)
}

fn event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    actor_id: &str,
) -> CreateDomainEvent {
    let now = now_rfc3339();
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(actor_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: new_uuid_v4(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: serde_json::json!({"task_id": task_id, "entity_id": entity_id}).to_string(),
        created_at: now,
    }
}

fn execution_started_event(task_id: &str, execution_id: &str, agent_id: &str) -> CreateDomainEvent {
    let now = now_rfc3339();
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "execution.started".to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: execution_id.to_owned(),
        actor_type: "agent".to_owned(),
        actor_id: Some(agent_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: new_uuid_v4(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: serde_json::json!({
            "task_id": task_id,
            "execution_id": execution_id,
            "status": "running"
        })
        .to_string(),
        created_at: now,
    }
}

async fn running_execution(
    db: &SqliteDb,
    task_id: &str,
    work_unit_workspace_id: &str,
    agent_id: &str,
) -> CreateExecution {
    let now = now_rfc3339();
    let agent = db::AgentRepo::get_by_id(db, agent_id)
        .await
        .expect("Agent loads for execution authority")
        .expect("Agent exists for execution authority");
    let daemon_id = agent
        .daemon_id
        .as_deref()
        .expect("Agent has its authorized daemon")
        .to_owned();
    let daemon = db::DaemonRepo::get_by_id(db, &daemon_id)
        .await
        .expect("daemon loads for execution authority")
        .expect("daemon exists for execution authority");
    let snapshot = serde_json::json!({
        "agent_id": agent.id,
        "profile_id": agent.profile_id,
        "executor_type": agent.executor_type,
        "resolved_daemon_id": daemon_id,
        "daemon_authority": db::daemon_authority_snapshot(&agent, &daemon),
        "capabilities": serde_json::from_str::<serde_json::Value>(&agent.capabilities_json)
            .expect("Agent capabilities parse"),
        "config": serde_json::from_str::<serde_json::Value>(&agent.config_json)
            .expect("Agent config parses")
    });
    CreateExecution {
        id: new_uuid_v4(),
        task_id: task_id.to_owned(),
        agent_id: Some(agent_id.to_owned()),
        actor_ref: Some(ActorRef::Agent(agent_id.to_owned())),
        role: "implementer".to_owned(),
        purpose: Some(ExecutionPurpose::Implement),
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
        before_sha: Some("base-sha".to_owned()),
        after_sha: None,
        error: None,
        executor_config_snapshot_json: Some(snapshot.to_string()),
        workspace_id: Some(work_unit_workspace_id.to_owned()),
        created_at: now.clone(),
        updated_at: now,
    }
}

async fn seed_authorized_daemon(db: &SqliteDb, owner_id: &str, executor_type: &str) -> String {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    db::DaemonRepo::upsert_by_machine_id(
        db,
        db::UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("pr5-external-machine-{daemon_id}"),
            hostname: "pr5-external-daemon".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: Some("fixture".to_owned()),
            labels_json: "{}".to_owned(),
            status: db::DaemonStatus::Online,
            registration_token_hash: Some(format!("pr5-registration-token-{daemon_id}")),
            owner_id: Some(owner_id.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("authorized external daemon creates");
    db::DaemonRepo::update_report(
        db,
        db::UpdateDaemonReport {
            id: daemon_id.clone(),
            last_report_at: now.clone(),
            status: db::DaemonStatus::Online,
            detected_clis_json: serde_json::json!([{
                "kind": executor_type,
                "availability": "authenticated"
            }])
            .to_string(),
            labels_json: None,
            updated_at: now,
        },
    )
    .await
    .expect("external daemon reports authenticated CLI");
    daemon_id
}

struct WorkUnitLeaseBinding<'a> {
    task_id: &'a str,
    project_id: &'a str,
    repo_id: &'a str,
    work_unit_id: &'a str,
    workspace_id: &'a str,
    execution_id: &'a str,
    task_version: i64,
    agent_id: &'a str,
}

fn work_unit_lease(binding: WorkUnitLeaseBinding<'_>) -> CreateWorkspaceLease {
    let now = now_rfc3339();
    CreateWorkspaceLease {
        id: new_uuid_v4(),
        project_id: binding.project_id.to_owned(),
        task_id: binding.task_id.to_owned(),
        work_unit_id: Some(binding.work_unit_id.to_owned()),
        workspace_id: Some(binding.workspace_id.to_owned()),
        task_version: binding.task_version,
        execution_id: binding.execution_id.to_owned(),
        operation_idempotency_key: binding.execution_id.to_owned(),
        repository_binding_id: binding.repo_id.to_owned(),
        base_ref: "base-sha".to_owned(),
        role: "worker".to_owned(),
        capabilities_json: r#"["repository_write"]"#.to_owned(),
        assigned_principal_type: "agent".to_owned(),
        assigned_principal_id: binding.agent_id.to_owned(),
        capability_profile_revision: "forge.capability-profile/v1".to_owned(),
        capability_profile_digest:
            "sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8".to_owned(),
        issuing_principal_type: "system".to_owned(),
        issuing_principal_id: "task-service-scheduler".to_owned(),
        issued_at: now.clone(),
        expires_at: "2999-01-01T00:00:00Z".to_owned(),
        created_at: now.clone(),
        updated_at: now,
    }
}

struct CleanupAdmissionFixture {
    project_id: String,
    task_id: String,
    repo_id: String,
    human_id: String,
    agent_id: String,
    work_unit_id: String,
    workspace_id: String,
    task_version: i64,
}

async fn cleanup_admission_fixture(db: &SqliteDb, root: &Path) -> CleanupAdmissionFixture {
    let now = now_rfc3339();
    let fixture = CleanupAdmissionFixture {
        project_id: new_uuid_v4(),
        task_id: new_uuid_v4(),
        repo_id: new_uuid_v4(),
        human_id: new_uuid_v4(),
        agent_id: new_uuid_v4(),
        work_unit_id: new_uuid_v4(),
        workspace_id: new_uuid_v4(),
        task_version: 1,
    };
    db::UserRepo::create_user(
        db,
        &db::User {
            id: fixture.human_id.clone(),
            email: format!("{}@example.invalid", fixture.human_id),
            password_hash: "unused".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Human");
    let daemon_id = seed_authorized_daemon(db, &fixture.human_id, "shell").await;
    ProjectRepo::create(
        db,
        CreateProject {
            id: fixture.project_id.clone(),
            name: "WorkUnit cleanup admission".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(fixture.human_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");
    RepoRepo::create(
        db,
        CreateRepo {
            id: fixture.repo_id.clone(),
            project_id: fixture.project_id.clone(),
            name: "repo".to_owned(),
            remote_url: "https://example.invalid/repo.git".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Repository");
    db::AgentRepo::create_identity_with_profile(
        db,
        db::CreateAgentIdentity {
            id: fixture.agent_id.clone(),
            name: "Cleanup race worker".to_owned(),
            description: None,
            max_concurrent_tasks: 4,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: db::AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(fixture.human_id.clone()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        db::CreateAgentProfile {
            id: new_uuid_v4(),
            identity_id: fixture.agent_id.clone(),
            backend_kind: "cli".to_owned(),
            executor_type: "shell".to_owned(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent");
    TaskRepo::create(
        db,
        CreateTask {
            id: fixture.task_id.clone(),
            project_id: fixture.project_id.clone(),
            repo_id: Some(fixture.repo_id.clone()),
            parent_task_id: None,
            subtask_order: None,
            title: "Cleanup race".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task");
    let role_id = new_uuid_v4();
    TaskRoleRepo::create(
        db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: fixture.task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task role");
    RoleMembershipRepo::add(
        db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role_id,
            actor_kind: ActorKind::Agent,
            actor_id: fixture.agent_id.clone(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("role membership");
    WorkUnitRepo::create(
        db,
        CreateWorkUnit {
            id: fixture.work_unit_id.clone(),
            task_id: fixture.task_id.clone(),
            parent_work_unit_id: None,
            title: "Cleanup race".to_owned(),
            scope: "Repository work".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(ActorRef::Agent(fixture.agent_id.clone())),
            requires_integration: true,
            provenance: None,
            created_by: ActorRef::Human(fixture.human_id.clone()),
            created_at: now.clone(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &fixture.work_unit_id,
            &fixture.task_id,
            &fixture.human_id,
        ),
    )
    .await
    .expect("WorkUnit");
    WorkUnitWorkspaceRepo::create_for_work_unit(
        db,
        CreateWorkUnitWorkspace {
            workspace: CreateWorkspace {
                id: fixture.workspace_id.clone(),
                task_id: fixture.task_id.clone(),
                repo_id: fixture.repo_id.clone(),
                worktree_path: root
                    .join(format!("worktree-{}", fixture.work_unit_id))
                    .to_string_lossy()
                    .into_owned(),
                branch: format!(
                    "forge/work-unit/{}/{}/{}",
                    fixture.task_id, fixture.work_unit_id, fixture.workspace_id
                ),
                status: WorkspaceStatus::Ready,
                before_sha: Some("base-sha".to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
            work_unit_id: fixture.work_unit_id.clone(),
        },
    )
    .await
    .expect("WorkUnit Workspace");
    fixture
}

#[tokio::test]
async fn integration_lookup_stays_exact_with_multiple_work_unit_workspaces() {
    let db = database().await;
    let root = TempDir::new().expect("temporary directory");
    let fixture = cleanup_admission_fixture(&db, root.path()).await;
    let now = now_rfc3339();

    let integration_workspace = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: fixture.task_id.clone(),
            repo_id: fixture.repo_id.clone(),
            worktree_path: root
                .path()
                .join("integration")
                .to_string_lossy()
                .into_owned(),
            branch: format!("forge/task-integration/{}", fixture.task_id),
            status: WorkspaceStatus::Ready,
            before_sha: Some("integration-sha".to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("integration Workspace");

    let second_work_unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: second_work_unit_id.clone(),
            task_id: fixture.task_id.clone(),
            parent_work_unit_id: None,
            title: "Second isolated unit".to_owned(),
            scope: "Independent repository work".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(ActorRef::Agent(fixture.agent_id.clone())),
            requires_integration: true,
            provenance: None,
            created_by: ActorRef::Human(fixture.human_id.clone()),
            created_at: now.clone(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &second_work_unit_id,
            &fixture.task_id,
            &fixture.human_id,
        ),
    )
    .await
    .expect("second WorkUnit");
    let second_workspace = WorkUnitWorkspaceRepo::create_for_work_unit(
        &db,
        CreateWorkUnitWorkspace {
            workspace: CreateWorkspace {
                id: new_uuid_v4(),
                task_id: fixture.task_id.clone(),
                repo_id: fixture.repo_id.clone(),
                worktree_path: root
                    .path()
                    .join("second-work-unit")
                    .to_string_lossy()
                    .into_owned(),
                branch: format!(
                    "forge/work-unit/{}/{}/second",
                    fixture.task_id, second_work_unit_id
                ),
                status: WorkspaceStatus::Ready,
                before_sha: Some("second-unit-sha".to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
            work_unit_id: second_work_unit_id.clone(),
        },
    )
    .await
    .expect("second WorkUnit Workspace");

    let selected_integration = WorkspaceRepo::get_integration_by_task_id(&db, &fixture.task_id)
        .await
        .expect("integration scope lookup")
        .expect("canonical integration Workspace");
    assert_eq!(selected_integration.id, integration_workspace.id);

    let first_unit_workspace =
        WorkUnitWorkspaceRepo::get_by_work_unit_id(&db, &fixture.work_unit_id)
            .await
            .expect("first exact WorkUnit Workspace lookup")
            .expect("first WorkUnit Workspace");
    assert_eq!(first_unit_workspace.id, fixture.workspace_id);
    let selected_second_unit =
        WorkUnitWorkspaceRepo::get_by_work_unit_id(&db, &second_work_unit_id)
            .await
            .expect("second exact WorkUnit Workspace lookup")
            .expect("second WorkUnit Workspace");
    assert_eq!(selected_second_unit.id, second_workspace.id);
    assert_ne!(selected_second_unit.id, selected_integration.id);
    assert_ne!(selected_second_unit.id, first_unit_workspace.id);
}

async fn cleanup_admission_attempt(
    db: &SqliteDb,
    fixture: &CleanupAdmissionFixture,
) -> (CreateWorkUnitExecution, CreateDomainEvent) {
    let execution = running_execution(
        db,
        &fixture.task_id,
        &fixture.workspace_id,
        &fixture.agent_id,
    )
    .await;
    let execution_id = execution.id.clone();
    let task = fixture.task_id.as_str();
    let lease = work_unit_lease(WorkUnitLeaseBinding {
        task_id: task,
        project_id: &fixture.project_id,
        repo_id: &fixture.repo_id,
        work_unit_id: &fixture.work_unit_id,
        workspace_id: &fixture.workspace_id,
        execution_id: &execution_id,
        task_version: fixture.task_version,
        agent_id: &fixture.agent_id,
    });
    (
        CreateWorkUnitExecution {
            execution,
            work_unit_id: fixture.work_unit_id.clone(),
            work_unit_version: 1,
            workspace_lease: Some(lease),
        },
        execution_started_event(task, &execution_id, &fixture.agent_id),
    )
}

#[tokio::test]
async fn execution_admission_wins_over_cleanup_claim_across_sqlite_pools() {
    let temp = TempDir::new().expect("temporary directory");
    let database_path = temp.path().join("cleanup-admission.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("first pool");
    run_migrations(&pool).await.expect("migrations");
    let db = SqliteDb::new(pool);
    let fixture = cleanup_admission_fixture(&db, temp.path()).await;
    let competing_pool = create_sqlite_pool(&database_url)
        .await
        .expect("independent pool on the same database file");
    let competing_db = SqliteDb::new(competing_pool);
    let (attempt, event) = cleanup_admission_attempt(&db, &fixture).await;
    let execution_id = attempt.execution.id.clone();

    WorkUnitExecutionRepo::create_for_work_unit(&db, attempt, event)
        .await
        .expect("Execution and active lease commit together first");
    assert!(WorkspaceRepo::claim_work_unit_cleanup(
        &competing_db,
        &fixture.workspace_id,
        &fixture.task_id,
        &fixture.work_unit_id,
        &now_rfc3339(),
    )
    .await
    .expect("cleanup claim checks durable authority")
    .is_none());
    assert_eq!(
        WorkspaceRepo::get_by_id(&competing_db, &fixture.workspace_id)
            .await
            .expect("workspace lookup")
            .expect("workspace exists")
            .status,
        WorkspaceStatus::Ready
    );

    ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: execution_id,
            status: Some(ExecutionStatus::Failed),
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
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("execution stops and revokes its active lease");
    let cleanup = WorkspaceRepo::claim_work_unit_cleanup(
        &competing_db,
        &fixture.workspace_id,
        &fixture.task_id,
        &fixture.work_unit_id,
        &now_rfc3339(),
    )
    .await
    .expect("terminal execution releases workspace cleanup authority")
    .expect("terminal execution revoked its active lease");
    assert_eq!(cleanup.status, WorkspaceStatus::Cleaning);
}

#[tokio::test]
async fn cleanup_claim_wins_and_database_rejects_work_unit_execution_admission() {
    let temp = TempDir::new().expect("temporary directory");
    let database_path = temp.path().join("cleanup-claim.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("first pool");
    run_migrations(&pool).await.expect("migrations");
    let db = SqliteDb::new(pool);
    let fixture = cleanup_admission_fixture(&db, temp.path()).await;
    let competing_pool = create_sqlite_pool(&database_url)
        .await
        .expect("independent pool on the same database file");
    let competing_db = SqliteDb::new(competing_pool);

    let claimed = WorkspaceRepo::claim_work_unit_cleanup(
        &db,
        &fixture.workspace_id,
        &fixture.task_id,
        &fixture.work_unit_id,
        &now_rfc3339(),
    )
    .await
    .expect("cleanup claim commits")
    .expect("Ready Workspace has no competing execution or lease");
    assert_eq!(claimed.status, WorkspaceStatus::Cleaning);

    let (attempt, event) = cleanup_admission_attempt(&db, &fixture).await;
    let execution_id = attempt.execution.id.clone();
    assert!(
        WorkUnitExecutionRepo::create_for_work_unit(&competing_db, attempt, event)
            .await
            .is_err()
    );
    let execution_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE id = ?")
        .bind(execution_id)
        .fetch_one(competing_db.pool())
        .await
        .expect("rejected execution count");
    let lease_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease WHERE workspace_id = ? AND status = 'active'",
    )
    .bind(&fixture.workspace_id)
    .fetch_one(competing_db.pool())
    .await
    .expect("active lease count");
    assert_eq!(execution_count, 0);
    assert_eq!(lease_count, 0);
}

async fn assert_failed_admission_has_no_execution_lease_or_event(
    db: &SqliteDb,
    attempt: CreateWorkUnitExecution,
    event_input: CreateDomainEvent,
    expected: DbError,
) {
    let execution_id = attempt.execution.id.clone();
    let lease_id = attempt
        .workspace_lease
        .as_ref()
        .map(|lease| lease.id.clone());
    let result = WorkUnitExecutionRepo::create_for_work_unit(db, attempt, event_input).await;
    assert!(
        matches!(&result, Err(error) if std::mem::discriminant(error) == std::mem::discriminant(&expected)),
        "expected {expected:?}, received {result:?}"
    );
    let execution_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE id = ?")
        .bind(&execution_id)
        .fetch_one(db.pool())
        .await
        .expect("execution count");
    let lease_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_lease WHERE execution_id = ?")
            .bind(&execution_id)
            .fetch_one(db.pool())
            .await
            .expect("lease count");
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE entity_id = ?")
            .bind(&execution_id)
            .fetch_one(db.pool())
            .await
            .expect("event count");
    assert_eq!(execution_count, 0, "rejected execution was rolled back");
    assert_eq!(lease_count, 0, "rejected lease was rolled back");
    assert_eq!(event_count, 0, "rejected events were rolled back");
    if let Some(lease_id) = lease_id {
        let lease_by_id: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM workspace_lease WHERE id = ?")
                .bind(lease_id)
                .fetch_one(db.pool())
                .await
                .expect("lease id count");
        assert_eq!(lease_by_id, 0, "rejected lease id was rolled back");
    }
}

#[tokio::test]
async fn work_unit_admission_guard_classifies_each_live_rejection_without_partial_rows() {
    let db = database().await;
    let root = TempDir::new().expect("temporary directory");

    // Simulate a state change after the caller's preflight; each write still
    // reaches the SQLite guard and receives the guard's specific cause.
    let paused_membership = cleanup_admission_fixture(&db, root.path()).await;
    let membership = RoleMembershipRepo::list_by_task(&db, &paused_membership.task_id, true)
        .await
        .expect("current role membership")
        .into_iter()
        .find(|(_, membership)| membership.actor_id == paused_membership.agent_id)
        .expect("Agent membership")
        .1;
    RoleMembershipRepo::update(
        &db,
        db::UpdateRoleMembership {
            id: membership.id,
            expected_version: membership.version,
            status: RoleMembershipStatus::Suspended,
            updated_at: now_rfc3339(),
            ended_at: None,
        },
    )
    .await
    .expect("membership is suspended after preflight");
    let (attempt, event_input) = cleanup_admission_attempt(&db, &paused_membership).await;
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::WorkUnitRoleMembershipInactive,
    )
    .await;

    let closed_work_unit = cleanup_admission_fixture(&db, root.path()).await;
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: closed_work_unit.work_unit_id.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Cancelled,
            updated_at: now_rfc3339(),
        },
        event(
            "work_unit.cancelled",
            "work_unit",
            &closed_work_unit.work_unit_id,
            &closed_work_unit.task_id,
            &closed_work_unit.human_id,
        ),
    )
    .await
    .expect("WorkUnit closes after preflight");
    let (attempt, event_input) = cleanup_admission_attempt(&db, &closed_work_unit).await;
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::WorkUnitNotOpen,
    )
    .await;

    let unavailable_workspace = cleanup_admission_fixture(&db, root.path()).await;
    sqlx::query("UPDATE workspace SET status = 'error' WHERE id = ?")
        .bind(&unavailable_workspace.workspace_id)
        .execute(db.pool())
        .await
        .expect("workspace becomes unavailable after preflight");
    let (attempt, event_input) = cleanup_admission_attempt(&db, &unavailable_workspace).await;
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::WorkUnitWorkspaceUnavailable,
    )
    .await;

    let blocked_dependency = cleanup_admission_fixture(&db, root.path()).await;
    let dependent_work_unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: dependent_work_unit_id.clone(),
            task_id: blocked_dependency.task_id.clone(),
            parent_work_unit_id: None,
            title: "Dependency-gated scope".to_owned(),
            scope: "No workspace is required for this scope".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(ActorRef::Agent(blocked_dependency.agent_id.clone())),
            requires_integration: false,
            provenance: None,
            created_by: ActorRef::Human(blocked_dependency.human_id.clone()),
            created_at: now_rfc3339(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &dependent_work_unit_id,
            &blocked_dependency.task_id,
            &blocked_dependency.human_id,
        ),
    )
    .await
    .expect("dependency-gated WorkUnit");
    let prerequisite_id = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: prerequisite_id.clone(),
            task_id: blocked_dependency.task_id.clone(),
            parent_work_unit_id: None,
            title: "Unmet prerequisite".to_owned(),
            scope: "The prerequisite remains open".to_owned(),
            role: "implementer".to_owned(),
            assigned_actor: Some(ActorRef::Agent(blocked_dependency.agent_id.clone())),
            requires_integration: false,
            provenance: None,
            created_by: ActorRef::Human(blocked_dependency.human_id.clone()),
            created_at: now_rfc3339(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &prerequisite_id,
            &blocked_dependency.task_id,
            &blocked_dependency.human_id,
        ),
    )
    .await
    .expect("open prerequisite");
    WorkUnitRepo::add_dependency(
        &db,
        AddWorkUnitDependency {
            work_unit_id: dependent_work_unit_id.clone(),
            depends_on_work_unit_id: prerequisite_id,
            expected_version: 1,
            created_by: ActorRef::Human(blocked_dependency.human_id.clone()),
            created_at: now_rfc3339(),
        },
        event(
            "work_unit.dependency_added",
            "work_unit",
            &dependent_work_unit_id,
            &blocked_dependency.task_id,
            &blocked_dependency.human_id,
        ),
    )
    .await
    .expect("unsatisfied dependency added after preflight");
    let mut execution = running_execution(
        &db,
        &blocked_dependency.task_id,
        "unused",
        &blocked_dependency.agent_id,
    )
    .await;
    execution.workspace_id = None;
    let execution_id = execution.id.clone();
    let attempt = CreateWorkUnitExecution {
        execution,
        work_unit_id: dependent_work_unit_id.clone(),
        work_unit_version: 2,
        workspace_lease: None,
    };
    let event_input = execution_started_event(
        &blocked_dependency.task_id,
        &execution_id,
        &blocked_dependency.agent_id,
    );
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::DependencyGate,
    )
    .await;

    let integration_human = cleanup_admission_fixture(&db, root.path()).await;
    let (mut attempt, mut event_input) = cleanup_admission_attempt(&db, &integration_human).await;
    attempt.execution.actor_ref = Some(ActorRef::Human(integration_human.human_id.clone()));
    attempt.execution.agent_id = None;
    attempt.workspace_lease = None;
    event_input.actor_type = "human".to_owned();
    event_input.actor_id = Some(integration_human.human_id.clone());
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::Check("integration Execution must use its assigned Agent".to_owned()),
    )
    .await;

    let at_capacity = cleanup_admission_fixture(&db, root.path()).await;
    for _ in 0..4 {
        let mut running =
            running_execution(&db, &at_capacity.task_id, "unused", &at_capacity.agent_id).await;
        running.workspace_id = None;
        ExecutionRepo::create(&db, running)
            .await
            .expect("fill the real Agent concurrency limit");
    }
    let (attempt, event_input) = cleanup_admission_attempt(&db, &at_capacity).await;
    assert_failed_admission_has_no_execution_lease_or_event(
        &db,
        attempt,
        event_input,
        DbError::AgentAtCapacity,
    )
    .await;
}

#[tokio::test]
async fn concurrent_work_unit_execution_admission_returns_running_conflict_atomically() {
    let root = TempDir::new().expect("temporary directory");
    let database_path = root.path().join("work-unit-admission-race.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("first pool");
    run_migrations(&pool).await.expect("migrations");
    let db = SqliteDb::new(pool);
    let fixture = cleanup_admission_fixture(&db, root.path()).await;
    let competing_pool = create_sqlite_pool(&database_url)
        .await
        .expect("second pool for a competing writer");
    let competing_db = SqliteDb::new(competing_pool);
    let (attempt_a, event_a) = cleanup_admission_attempt(&db, &fixture).await;
    let (attempt_b, event_b) = cleanup_admission_attempt(&db, &fixture).await;
    let execution_a = attempt_a.execution.id.clone();
    let execution_b = attempt_b.execution.id.clone();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));

    let submit = |db: SqliteDb,
                  attempt: CreateWorkUnitExecution,
                  event_input: CreateDomainEvent,
                  barrier: std::sync::Arc<tokio::sync::Barrier>| async move {
        barrier.wait().await;
        WorkUnitExecutionRepo::create_for_work_unit(&db, attempt, event_input).await
    };
    let writer_a = tokio::spawn(submit(
        db.clone(),
        attempt_a,
        event_a,
        std::sync::Arc::clone(&barrier),
    ));
    let writer_b = tokio::spawn(submit(
        competing_db.clone(),
        attempt_b,
        event_b,
        std::sync::Arc::clone(&barrier),
    ));
    barrier.wait().await;
    let result_a = writer_a.await.expect("first writer joins");
    let result_b = writer_b.await.expect("second writer joins");
    let results = [result_a, result_b];
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "BEGIN IMMEDIATE allows exactly one admission"
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(DbError::WorkUnitExecutionAlreadyRunning)))
            .count(),
        1,
        "the losing write receives a stable WorkUnit-running conflict"
    );
    let (winner_id, loser_id) = if results[0].is_ok() {
        (&execution_a, &execution_b)
    } else {
        (&execution_b, &execution_a)
    };
    let winner_execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE id = ?")
            .bind(winner_id)
            .fetch_one(db.pool())
            .await
            .expect("winner execution count");
    let winner_event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE entity_id = ?")
            .bind(winner_id)
            .fetch_one(db.pool())
            .await
            .expect("winner event count");
    let loser_execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE id = ?")
            .bind(loser_id)
            .fetch_one(db.pool())
            .await
            .expect("loser execution count");
    let loser_lease_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM workspace_lease WHERE execution_id = ?")
            .bind(loser_id)
            .fetch_one(db.pool())
            .await
            .expect("loser lease count");
    let loser_event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE entity_id = ?")
            .bind(loser_id)
            .fetch_one(db.pool())
            .await
            .expect("loser event count");
    assert_eq!(winner_execution_count, 1);
    assert_eq!(
        winner_event_count, 2,
        "start and pending receipts are atomic"
    );
    assert_eq!(loser_execution_count, 0, "losing execution rolled back");
    assert_eq!(loser_lease_count, 0, "losing lease rolled back");
    assert_eq!(loser_event_count, 0, "losing events rolled back");
    let active_leases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_lease WHERE work_unit_id = ? AND status = 'active'",
    )
    .bind(&fixture.work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("active lease count");
    assert_eq!(active_leases, 1, "only the winning Execution has a lease");
}

#[tokio::test]
async fn active_task_operation_blocks_work_unit_cleanup_claim() {
    let temp = TempDir::new().expect("temporary directory");
    let db = database().await;
    let fixture = cleanup_admission_fixture(&db, temp.path()).await;
    let operation = TaskIntegrationOperationRepo::begin(
        &db,
        db::CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: fixture.task_id.clone(),
            kind: db::TaskIntegrationOperationKind::WorkUnitWorkspacePrepare,
            owner_id: fixture.work_unit_id.clone(),
            gate_evaluation_id: None,
            remote_waiting: false,
            parent_operation_id: None,
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("active Task operation");

    assert!(WorkspaceRepo::claim_work_unit_cleanup(
        &db,
        &fixture.workspace_id,
        &fixture.task_id,
        &fixture.work_unit_id,
        &now_rfc3339(),
    )
    .await
    .expect("Task operation is in the atomic cleanup admission predicate")
    .is_none());
    TaskIntegrationOperationRepo::finish(
        &db,
        db::FinishTaskIntegrationOperation {
            id: operation.id,
            expected_version: operation.version,
            status: db::TaskIntegrationOperationStatus::Failed,
            result_event_id: None,
            updated_at: now_rfc3339(),
            finished_at: now_rfc3339(),
        },
    )
    .await
    .expect("operation is closed");
    assert!(WorkspaceRepo::claim_work_unit_cleanup(
        &db,
        &fixture.workspace_id,
        &fixture.task_id,
        &fixture.work_unit_id,
        &now_rfc3339(),
    )
    .await
    .expect("closed Task operation no longer blocks cleanup")
    .is_some());
}

#[tokio::test]
async fn work_unit_task_binding_is_immutable_without_provenance() {
    let db = database().await;
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let other_task_id = new_uuid_v4();
    let human_id = new_uuid_v4();

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "PR5 WorkUnit Task binding".into(),
            settings: "{}".into(),
            primary_repo_id: None,
            owner_id: Some(human_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");
    db::UserRepo::create_user(
        &db,
        &db::User {
            id: human_id.clone(),
            email: "task-binding@example.invalid".into(),
            password_hash: "not-used".into(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Human");

    for id in [&task_id, &other_task_id] {
        TaskRepo::create(
            &db,
            CreateTask {
                id: id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                title: "Task".into(),
                description: None,
                task_type: "implementation".into(),
                lifecycle_state: db::TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                subtask_order: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");
        TaskRoleRepo::create(
            &db,
            CreateTaskRole {
                id: new_uuid_v4(),
                task_id: id.clone(),
                role: "implementer".into(),
                coordination_mode: Some(CoordinationMode::Independent),
                policy_json: "{}".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("TaskRole");
    }

    let work_unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: work_unit_id.clone(),
            task_id: task_id.clone(),
            parent_work_unit_id: None,
            title: "Immutable owner".into(),
            scope: "Task ownership is historical identity".into(),
            role: "implementer".into(),
            assigned_actor: None,
            requires_integration: false,
            provenance: None,
            created_by: ActorRef::Human(human_id.clone()),
            created_at: now.clone(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &work_unit_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("WorkUnit without provenance");

    let update = sqlx::query("UPDATE work_unit SET task_id = ? WHERE id = ?")
        .bind(&other_task_id)
        .bind(&work_unit_id)
        .execute(db.pool())
        .await;
    assert!(update.is_err(), "direct Task rebinding must fail");
    assert_eq!(
        WorkUnitRepo::get_by_id(&db, &work_unit_id)
            .await
            .expect("WorkUnit lookup")
            .expect("WorkUnit remains present")
            .task_id,
        task_id,
        "failed rebinding leaves original Task ownership intact",
    );
}

#[tokio::test]
async fn work_unit_dag_is_same_task_acyclic_versioned_and_teardown_safe() {
    let db = database().await;
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let other_task_id = new_uuid_v4();
    let human_id = new_uuid_v4();
    let agent_id = new_uuid_v4();

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "PR5 DAG".into(),
            settings: "{}".into(),
            primary_repo_id: None,
            owner_id: Some(human_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project");
    RepoRepo::create(
        &db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "repo".into(),
            remote_url: "https://example.invalid/repo.git".into(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo");
    db::UserRepo::create_user(
        &db,
        &db::User {
            id: human_id.clone(),
            email: "pr5@example.invalid".into(),
            password_hash: "not-used".into(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("human");
    let daemon_id = seed_authorized_daemon(&db, &human_id, "shell").await;
    db::AgentRepo::create_identity_with_profile(
        &db,
        db::CreateAgentIdentity {
            id: agent_id.clone(),
            name: "Worker".into(),
            description: None,
            max_concurrent_tasks: 2,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: db::AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(human_id.clone()),
            visibility: "account".into(),
            account_permission_ceiling: "{}".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        db::CreateAgentProfile {
            id: new_uuid_v4(),
            identity_id: agent_id.clone(),
            backend_kind: "cli".into(),
            executor_type: "shell".into(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".into(),
            tool_policy_json: "{}".into(),
            config_json: "{}".into(),
            credential_ref: None,
            daemon_id: Some(daemon_id),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent identity");
    for id in [&task_id, &other_task_id] {
        TaskRepo::create(
            &db,
            CreateTask {
                id: id.clone(),
                project_id: project_id.clone(),
                repo_id: Some(repo_id.clone()),
                parent_task_id: None,
                title: "Task".into(),
                description: None,
                task_type: "implementation".into(),
                lifecycle_state: db::TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                subtask_order: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task");
    }
    let role_id = new_uuid_v4();
    let other_role_id = new_uuid_v4();
    for (role_id, task_id) in [(&role_id, &task_id), (&other_role_id, &other_task_id)] {
        TaskRoleRepo::create(
            &db,
            CreateTaskRole {
                id: role_id.clone(),
                task_id: task_id.clone(),
                role: "implementer".into(),
                coordination_mode: Some(CoordinationMode::Independent),
                policy_json: "{}".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("TaskRole");
        RoleMembershipRepo::add(
            &db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: role_id.clone(),
                actor_kind: ActorKind::Agent,
                actor_id: agent_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("membership");
    }

    let mut task_scoped_execution = running_execution(&db, &task_id, "unused", &agent_id).await;
    task_scoped_execution.workspace_id = None;
    let task_scoped_execution_id = task_scoped_execution.id.clone();
    ExecutionRepo::create(&db, task_scoped_execution)
        .await
        .expect("legacy Task Execution");
    ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: task_scoped_execution_id.clone(),
            status: Some(ExecutionStatus::Completed),
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
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("legacy Task Execution completes before WorkUnits exist");

    let create =
        |id: String, task: &str, creator: &str, requires_integration: bool| CreateWorkUnit {
            id,
            task_id: task.to_owned(),
            parent_work_unit_id: None,
            title: "scope".into(),
            scope: "coordination context".into(),
            role: "implementer".into(),
            assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
            requires_integration,
            provenance: None,
            created_by: ActorRef::Human(creator.to_owned()),
            created_at: now.clone(),
        };
    let a = new_uuid_v4();
    let b = new_uuid_v4();
    let c = new_uuid_v4();
    let foreign = new_uuid_v4();
    for id in [&a, &b, &c] {
        WorkUnitRepo::create(
            &db,
            create(id.clone(), &task_id, &human_id, false),
            event("work_unit.created", "work_unit", id, &task_id, &human_id),
        )
        .await
        .expect("WorkUnit");
    }

    let provenance_id = new_uuid_v4();
    let provenance = WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: provenance_id.clone(),
            task_id: task_id.clone(),
            parent_work_unit_id: None,
            title: "typed provenance".into(),
            scope: "retain Actor identity".into(),
            role: "implementer".into(),
            assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
            requires_integration: false,
            provenance: Some(db::WorkUnitProvenance::Actor(ActorRef::Human(
                human_id.clone(),
            ))),
            created_by: ActorRef::Human(human_id.clone()),
            created_at: now.clone(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &provenance_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("typed Actor provenance creates")
    .record;
    assert_eq!(
        provenance.provenance,
        Some(db::WorkUnitProvenance::Actor(ActorRef::Human(
            human_id.clone()
        )))
    );
    let invalid_provenance_id = new_uuid_v4();
    assert!(WorkUnitRepo::create(
        &db,
        CreateWorkUnit {
            id: invalid_provenance_id.clone(),
            task_id: task_id.clone(),
            parent_work_unit_id: None,
            title: "invalid provenance".into(),
            scope: "must refer to an Actor".into(),
            role: "implementer".into(),
            assigned_actor: Some(ActorRef::Agent(agent_id.clone())),
            requires_integration: false,
            provenance: Some(db::WorkUnitProvenance::Actor(ActorRef::Agent(
                new_uuid_v4(),
            ))),
            created_by: ActorRef::Human(human_id.clone()),
            created_at: now.clone(),
        },
        event(
            "work_unit.created",
            "work_unit",
            &invalid_provenance_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .is_err());

    assert!(WorkUnitRepo::allocate(
        &db,
        db::AllocateWorkUnit {
            id: a.clone(),
            expected_version: 1,
            role: "implementer".into(),
            assigned_actor: Some(ActorRef::Human(human_id.clone())),
            updated_at: now.clone(),
        },
        event(
            "work_unit.allocation_changed",
            "work_unit",
            &a,
            &task_id,
            &human_id
        ),
    )
    .await
    .is_err());
    assert_eq!(
        WorkUnitRepo::get_by_id(&db, &a)
            .await
            .expect("WorkUnit lookup")
            .expect("WorkUnit remains allocated")
            .assigned_actor,
        Some(ActorRef::Agent(agent_id.clone())),
        "rejected non-member allocation leaves WorkUnit unchanged",
    );
    let legacy_execution_id = new_uuid_v4();
    let mut legacy_execution = running_execution(&db, &other_task_id, "unused", &agent_id).await;
    legacy_execution.id = legacy_execution_id.clone();
    legacy_execution.workspace_id = None;
    ExecutionRepo::create(&db, legacy_execution)
        .await
        .expect("legacy Task Execution starts before WorkUnits");
    assert!(WorkUnitRepo::create(
        &db,
        create(foreign.clone(), &other_task_id, &human_id, false),
        event(
            "work_unit.created",
            "work_unit",
            &foreign,
            &other_task_id,
            &human_id
        ),
    )
    .await
    .is_err());
    ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: legacy_execution_id,
            status: Some(ExecutionStatus::Completed),
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
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("legacy Execution finishes");
    WorkUnitRepo::create(
        &db,
        create(foreign.clone(), &other_task_id, &human_id, false),
        event(
            "work_unit.created",
            "work_unit",
            &foreign,
            &other_task_id,
            &human_id,
        ),
    )
    .await
    .expect("foreign WorkUnit");

    let edge = |from: &str, to: &str, expected_version| AddWorkUnitDependency {
        work_unit_id: from.to_owned(),
        depends_on_work_unit_id: to.to_owned(),
        expected_version,
        created_by: ActorRef::Human(human_id.clone()),
        created_at: now.clone(),
    };
    let add = |from: &str| {
        event(
            "work_unit.dependency_added",
            "work_unit",
            from,
            &task_id,
            &human_id,
        )
    };
    WorkUnitRepo::add_dependency(&db, edge(&a, &b, 1), add(&a))
        .await
        .expect("A depends on B");
    WorkUnitRepo::add_dependency(&db, edge(&b, &c, 1), add(&b))
        .await
        .expect("B depends on C");
    assert!(matches!(
        WorkUnitRepo::add_dependency(&db, edge(&c, &a, 1), add(&c)).await,
        Err(DbError::CycleDetected)
    ));
    assert!(WorkUnitRepo::add_dependency(&db, edge(&c, &c, 1), add(&c))
        .await
        .is_err());
    assert!(matches!(
        WorkUnitRepo::add_dependency(&db, edge(&a, &b, 2), add(&a)).await,
        Err(DbError::IdempotencyConflict)
    ));
    assert!(
        WorkUnitRepo::add_dependency(&db, edge(&a, &foreign, 2), add(&a))
            .await
            .is_err()
    );

    let blocked = WorkUnitRepo::list_dependencies(&db, &b)
        .await
        .expect("B dependencies");
    assert!(!blocked[0].satisfied);
    assert!(WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: b.clone(),
            expected_version: 2,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event("work_unit.completed", "work_unit", &b, &task_id, &human_id),
    )
    .await
    .is_err());
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: c.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event("work_unit.completed", "work_unit", &c, &task_id, &human_id),
    )
    .await
    .expect("C completes");
    let satisfied = WorkUnitRepo::list_dependencies(&db, &b)
        .await
        .expect("B dependencies");
    assert!(satisfied[0].satisfied);

    let diamond_base = new_uuid_v4();
    let diamond_left = new_uuid_v4();
    let diamond_right = new_uuid_v4();
    let diamond_final = new_uuid_v4();
    for id in [&diamond_base, &diamond_left, &diamond_right, &diamond_final] {
        WorkUnitRepo::create(
            &db,
            create(id.clone(), &task_id, &human_id, false),
            event("work_unit.created", "work_unit", id, &task_id, &human_id),
        )
        .await
        .expect("diamond WorkUnit");
    }
    WorkUnitRepo::add_dependency(
        &db,
        edge(&diamond_left, &diamond_base, 1),
        add(&diamond_left),
    )
    .await
    .expect("left depends on base");
    WorkUnitRepo::add_dependency(
        &db,
        edge(&diamond_right, &diamond_base, 1),
        add(&diamond_right),
    )
    .await
    .expect("right depends on base");
    WorkUnitRepo::add_dependency(
        &db,
        edge(&diamond_final, &diamond_left, 1),
        add(&diamond_final),
    )
    .await
    .expect("final depends on left");
    WorkUnitRepo::add_dependency(
        &db,
        edge(&diamond_final, &diamond_right, 2),
        add(&diamond_final),
    )
    .await
    .expect("final depends on right");
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: diamond_base.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event(
            "work_unit.completed",
            "work_unit",
            &diamond_base,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("diamond base completes");
    let final_dependencies = WorkUnitRepo::list_dependencies(&db, &diamond_final)
        .await
        .expect("diamond final prerequisites");
    assert_eq!(final_dependencies.len(), 2);
    assert!(final_dependencies
        .iter()
        .all(|dependency| !dependency.satisfied));
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: diamond_left.clone(),
            expected_version: 2,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event(
            "work_unit.completed",
            "work_unit",
            &diamond_left,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("diamond left completes");
    let final_dependencies = WorkUnitRepo::list_dependencies(&db, &diamond_final)
        .await
        .expect("diamond final after left");
    assert_eq!(
        final_dependencies
            .iter()
            .filter(|item| item.satisfied)
            .count(),
        1
    );
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: diamond_right.clone(),
            expected_version: 2,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event(
            "work_unit.completed",
            "work_unit",
            &diamond_right,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("diamond right completes");
    let final_dependencies = WorkUnitRepo::list_dependencies(&db, &diamond_final)
        .await
        .expect("diamond final after both branches");
    assert!(final_dependencies
        .iter()
        .all(|dependency| dependency.satisfied));

    let d = new_uuid_v4();
    let e = new_uuid_v4();
    for id in [&d, &e] {
        WorkUnitRepo::create(
            &db,
            create(id.clone(), &task_id, &human_id, true),
            event("work_unit.created", "work_unit", id, &task_id, &human_id),
        )
        .await
        .expect("repository WorkUnit");
    }
    WorkUnitRepo::add_dependency(
        &db,
        edge(&a, &d, 2),
        event(
            "work_unit.dependency_added",
            "work_unit",
            &a,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("A depends on repository WorkUnit D");
    assert!(WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: d.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event("work_unit.completed", "work_unit", &d, &task_id, &human_id),
    )
    .await
    .is_err());
    let d_dependency = WorkUnitRepo::list_dependencies(&db, &a)
        .await
        .expect("A dependencies")
        .into_iter()
        .find(|dependency| dependency.depends_on_work_unit_id == d)
        .expect("D dependency exists");
    assert!(!d_dependency.satisfied);
    let integration_workspace = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            repo_id: repo_id.clone(),
            worktree_path: "/tmp/pr5/task-integration".into(),
            branch: format!("task/{task_id}"),
            status: WorkspaceStatus::Ready,
            before_sha: Some("base-sha".into()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task integration Workspace");
    let pinned_unit = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        create(pinned_unit.clone(), &task_id, &human_id, true),
        event(
            "work_unit.created",
            "work_unit",
            &pinned_unit,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("WorkUnit with a fixed prerequisite");
    WorkUnitRepo::add_dependency(
        &db,
        edge(&pinned_unit, &c, 1),
        event(
            "work_unit.dependency_added",
            "work_unit",
            &pinned_unit,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("prerequisite is fixed before Workspace preparation");
    WorkUnitWorkspaceRepo::create_for_work_unit(
        &db,
        CreateWorkUnitWorkspace {
            workspace: CreateWorkspace {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                repo_id: repo_id.clone(),
                worktree_path: "/tmp/pr5/workunit-pinned".into(),
                branch: "work-unit/pinned".into(),
                status: WorkspaceStatus::Ready,
                before_sha: Some("base-sha".into()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            work_unit_id: pinned_unit.clone(),
        },
    )
    .await
    .expect("WorkUnit workspace pins its dependency base");
    assert!(WorkUnitRepo::add_dependency(
        &db,
        edge(&pinned_unit, &d, 2),
        event(
            "work_unit.dependency_added",
            "work_unit",
            &pinned_unit,
            &task_id,
            &human_id,
        ),
    )
    .await
    .is_err());
    assert!(WorkUnitRepo::remove_dependency(
        &db,
        db::RemoveWorkUnitDependency {
            work_unit_id: pinned_unit.clone(),
            depends_on_work_unit_id: c.clone(),
            expected_version: 2,
            updated_at: now.clone(),
        },
        event(
            "work_unit.dependency_removed",
            "work_unit",
            &pinned_unit,
            &task_id,
            &human_id,
        ),
    )
    .await
    .is_err());
    assert!(WorkUnitRepo::update(
        &db,
        db::UpdateWorkUnit {
            id: pinned_unit.clone(),
            expected_version: 2,
            title: None,
            scope: None,
            parent_work_unit_id: None,
            requires_integration: Some(false),
            updated_at: now.clone(),
        },
        event(
            "work_unit.updated",
            "work_unit",
            &pinned_unit,
            &task_id,
            &human_id,
        ),
    )
    .await
    .is_err());
    let legacy_writer =
        running_execution(&db, &task_id, &integration_workspace.id, &agent_id).await;
    assert!(ExecutionRepo::create(&db, legacy_writer).await.is_err());
    let task = TaskRepo::get_by_id(&db, &task_id, false)
        .await
        .expect("Task lookup")
        .expect("Task exists");

    let mut unit_workspaces = Vec::new();
    let mut execution_ids = Vec::new();
    for (unit_id, suffix) in [(&d, "d"), (&e, "e")] {
        let workspace_id = new_uuid_v4();
        WorkUnitWorkspaceRepo::create_for_work_unit(
            &db,
            CreateWorkUnitWorkspace {
                workspace: CreateWorkspace {
                    id: workspace_id.clone(),
                    task_id: task_id.clone(),
                    repo_id: repo_id.clone(),
                    worktree_path: format!("/tmp/pr5/workunit-{suffix}"),
                    branch: format!("work-unit/{suffix}"),
                    status: WorkspaceStatus::Ready,
                    before_sha: Some("base-sha".into()),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                work_unit_id: unit_id.clone(),
            },
        )
        .await
        .expect("WorkUnit Workspace");
        let execution = running_execution(&db, &task_id, &workspace_id, &agent_id).await;
        let execution_id = execution.id.clone();
        let lease = work_unit_lease(WorkUnitLeaseBinding {
            task_id: &task_id,
            project_id: &project_id,
            repo_id: &repo_id,
            work_unit_id: unit_id,
            workspace_id: &workspace_id,
            execution_id: &execution_id,
            task_version: task.version,
            agent_id: &agent_id,
        });
        WorkUnitExecutionRepo::create_for_work_unit(
            &db,
            CreateWorkUnitExecution {
                execution,
                work_unit_id: unit_id.clone(),
                work_unit_version: 1,
                workspace_lease: Some(lease),
            },
            execution_started_event(&task_id, &execution_id, &agent_id),
        )
        .await
        .expect("WorkUnit Execution and exact lease commit together");
        unit_workspaces.push((unit_id.clone(), workspace_id));
        execution_ids.push(execution_id);
    }
    let latest_task_execution = ExecutionRepo::list_latest_executions_for_tasks(&db, &[&task_id])
        .await
        .expect("latest Task-scoped Execution query");
    assert_eq!(latest_task_execution.len(), 1);
    assert_eq!(latest_task_execution[0].id, task_scoped_execution_id);
    let lease_d = WorkspaceLeaseRepo::get_active_for_work_unit(&db, &d)
        .await
        .expect("lease query")
        .expect("D lease active");
    let lease_e = WorkspaceLeaseRepo::get_active_for_work_unit(&db, &e)
        .await
        .expect("lease query")
        .expect("E lease active");
    assert_ne!(lease_d.id, lease_e.id);
    assert_ne!(lease_d.workspace_id, lease_e.workspace_id);
    assert_eq!(lease_d.execution_id, execution_ids[0]);
    assert_eq!(lease_e.execution_id, execution_ids[1]);

    let over_capacity_work_unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        &db,
        create(
            over_capacity_work_unit_id.clone(),
            &task_id,
            &human_id,
            false,
        ),
        event(
            "work_unit.created",
            "work_unit",
            &over_capacity_work_unit_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("WorkUnit exists for capacity check");
    let mut over_capacity_execution = running_execution(&db, &task_id, "unused", &agent_id).await;
    over_capacity_execution.workspace_id = None;
    let over_capacity_execution_id = over_capacity_execution.id.clone();
    let capacity_result = WorkUnitExecutionRepo::create_for_work_unit(
        &db,
        CreateWorkUnitExecution {
            execution: over_capacity_execution,
            work_unit_id: over_capacity_work_unit_id,
            work_unit_version: 1,
            workspace_lease: None,
        },
        execution_started_event(&task_id, &over_capacity_execution_id, &agent_id),
    )
    .await;
    assert!(
        matches!(capacity_result, Err(DbError::AgentAtCapacity)),
        "capacity admission result: {capacity_result:?}"
    );
    let rejected_execution_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE id = ?")
            .bind(&over_capacity_execution_id)
            .fetch_one(db.pool())
            .await
            .expect("rejected execution count");
    let rejected_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE entity_id = ? AND event_type = 'execution.started'",
    )
    .bind(&over_capacity_execution_id)
    .fetch_one(db.pool())
    .await
    .expect("rejected event count");
    assert_eq!(rejected_execution_count, 0);
    assert_eq!(rejected_event_count, 0);

    assert!(WorkUnitRepo::update(
        &db,
        db::UpdateWorkUnit {
            id: d.clone(),
            expected_version: 1,
            title: None,
            scope: Some("mutated during execution".into()),
            parent_work_unit_id: None,
            requires_integration: None,
            updated_at: now.clone(),
        },
        event("work_unit.updated", "work_unit", &d, &task_id, &human_id),
    )
    .await
    .is_err());
    assert!(WorkUnitRepo::add_dependency(
        &db,
        edge(&d, &c, 1),
        event(
            "work_unit.dependency_added",
            "work_unit",
            &d,
            &task_id,
            &human_id
        ),
    )
    .await
    .is_err());

    let (unit_d, workspace_d) = &unit_workspaces[0];
    let blocked_retry = running_execution(&db, &task_id, workspace_d, &agent_id).await;
    assert!(WorkUnitExecutionRepo::create_for_work_unit(
        &db,
        CreateWorkUnitExecution {
            workspace_lease: Some(work_unit_lease(WorkUnitLeaseBinding {
                task_id: &task_id,
                project_id: &project_id,
                repo_id: &repo_id,
                work_unit_id: unit_d,
                workspace_id: workspace_d,
                execution_id: &blocked_retry.id,
                task_version: task.version,
                agent_id: &agent_id,
            })),
            work_unit_id: unit_d.clone(),
            work_unit_version: 1,
            execution: blocked_retry.clone(),
        },
        execution_started_event(&task_id, &blocked_retry.id, &agent_id),
    )
    .await
    .is_err());

    ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: execution_ids[0].clone(),
            status: Some(ExecutionStatus::Completed),
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
            after_sha: Some(Some("result-sha".into())),
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Execution completes");
    assert!(WorkspaceLeaseRepo::get_active_for_work_unit(&db, &d)
        .await
        .expect("D lease query")
        .is_none());
    assert!(WorkspaceLeaseRepo::get_active_for_work_unit(&db, &e)
        .await
        .expect("E lease query")
        .is_some());

    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: d.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now.clone(),
        },
        event("work_unit.completed", "work_unit", &d, &task_id, &human_id),
    )
    .await
    .expect("D WorkUnit completes");
    assert!(
        !WorkUnitRepo::list_dependencies(&db, &a)
            .await
            .expect("A dependencies before integration")
            .into_iter()
            .find(|dependency| dependency.depends_on_work_unit_id == d)
            .expect("D dependency remains")
            .satisfied
    );

    let source_workspace_id = unit_workspaces[0].1.clone();
    let integration_id = new_uuid_v4();
    let integration_now = now_rfc3339();
    let started = WorkUnitRepo::begin_integration(
        &db,
        CreateWorkUnitIntegration {
            id: integration_id.clone(),
            task_id: task_id.clone(),
            work_unit_id: d.clone(),
            execution_id: execution_ids[0].clone(),
            source_workspace_id: source_workspace_id.clone(),
            source_branch: "work-unit/d".into(),
            source_sha: "result-sha".into(),
            target_workspace_id: integration_workspace.id.clone(),
            target_branch: integration_workspace.branch.clone(),
            target_before_sha: "base-sha".into(),
            operation_idempotency_key: "integrate-d-once".into(),
            started_at: integration_now.clone(),
            created_at: integration_now.clone(),
        },
        event(
            "work_unit.integration_started",
            "work_unit_integration",
            &integration_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("durable integration attempt starts")
    .record;
    WorkUnitRepo::record_integration(
        &db,
        RecordWorkUnitIntegration {
            id: started.id,
            expected_version: started.version,
            outcome: WorkUnitIntegrationOutcome::Success,
            target_after_sha: Some("integrated-sha".into()),
            conflict_metadata_json: None,
            finished_at: now.clone(),
            updated_at: now.clone(),
        },
        event(
            "work_unit.integration_succeeded",
            "work_unit_integration",
            &integration_id,
            &task_id,
            &human_id,
        ),
    )
    .await
    .expect("integration success is durable");
    assert!(
        WorkUnitRepo::list_dependencies(&db, &a)
            .await
            .expect("A dependencies after integration")
            .into_iter()
            .find(|dependency| dependency.depends_on_work_unit_id == d)
            .expect("D dependency remains")
            .satisfied
    );

    let a_before = WorkUnitRepo::get_by_id(&db, &a)
        .await
        .expect("A loads before independent WorkUnit transition")
        .expect("A exists");
    let b_before = WorkUnitRepo::get_by_id(&db, &b)
        .await
        .expect("B loads before independent WorkUnit transition")
        .expect("B exists");
    let c_before = WorkUnitRepo::get_by_id(&db, &c)
        .await
        .expect("C loads before independent WorkUnit transition")
        .expect("C exists");
    let dependency_snapshot = |dependencies: Vec<db::WorkUnitDependency>| {
        dependencies
            .into_iter()
            .map(|dependency| (dependency.depends_on_work_unit_id, dependency.satisfied))
            .collect::<Vec<_>>()
    };
    let a_dependencies_before = dependency_snapshot(
        WorkUnitRepo::list_dependencies(&db, &a)
            .await
            .expect("A dependencies load before independent transition"),
    );
    let b_dependencies_before = dependency_snapshot(
        WorkUnitRepo::list_dependencies(&db, &b)
            .await
            .expect("B dependencies load before independent transition"),
    );
    let c_dependencies_before = dependency_snapshot(
        WorkUnitRepo::list_dependencies(&db, &c)
            .await
            .expect("C dependencies load before independent transition"),
    );
    ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: execution_ids[1].clone(),
            status: Some(ExecutionStatus::Completed),
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
            after_sha: Some(Some("independent-e-result".into())),
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("independent E Execution completes");
    WorkUnitRepo::transition(
        &db,
        db::TransitionWorkUnit {
            id: e.clone(),
            expected_version: 1,
            status: WorkUnitStatus::Completed,
            updated_at: now_rfc3339(),
        },
        event("work_unit.completed", "work_unit", &e, &task_id, &human_id),
    )
    .await
    .expect("independent E WorkUnit completes");
    assert_eq!(
        WorkUnitRepo::get_by_id(&db, &a)
            .await
            .expect("A reloads")
            .expect("A remains")
            .status,
        a_before.status
    );
    assert_eq!(
        WorkUnitRepo::get_by_id(&db, &b)
            .await
            .expect("B reloads")
            .expect("B remains")
            .status,
        b_before.status
    );
    assert_eq!(
        WorkUnitRepo::get_by_id(&db, &c)
            .await
            .expect("C reloads")
            .expect("C remains")
            .status,
        c_before.status
    );
    assert_eq!(
        dependency_snapshot(
            WorkUnitRepo::list_dependencies(&db, &a)
                .await
                .expect("A dependencies reload"),
        ),
        a_dependencies_before
    );
    assert_eq!(
        dependency_snapshot(
            WorkUnitRepo::list_dependencies(&db, &b)
                .await
                .expect("B dependencies reload"),
        ),
        b_dependencies_before
    );
    assert_eq!(
        dependency_snapshot(
            WorkUnitRepo::list_dependencies(&db, &c)
                .await
                .expect("C dependencies reload"),
        ),
        c_dependencies_before
    );

    let active_operation = TaskIntegrationOperationRepo::begin(
        &db,
        db::CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            kind: db::TaskIntegrationOperationKind::IntegrationWorkspaceCleanup,
            owner_id: "project-teardown-guard".into(),
            gate_evaluation_id: None,
            remote_waiting: false,
            parent_operation_id: None,
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("active Task operation exists");
    assert!(ProjectRepo::delete(&db, &project_id).await.is_err());
    let finished_at = now_rfc3339();
    TaskIntegrationOperationRepo::finish(
        &db,
        db::FinishTaskIntegrationOperation {
            id: active_operation.id,
            expected_version: active_operation.version,
            status: db::TaskIntegrationOperationStatus::Abandoned,
            result_event_id: None,
            updated_at: finished_at.clone(),
            finished_at,
        },
    )
    .await
    .expect("abandon operation before teardown");

    ProjectRepo::delete(&db, &project_id)
        .await
        .expect("guarded teardown");
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE task_id IN (?, ?)")
            .bind(&task_id)
            .bind(&other_task_id)
            .fetch_one(db.pool())
            .await
            .expect("count");
    assert_eq!(remaining, 0);
    let remaining_operations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_integration_operation WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(db.pool())
            .await
            .expect("count operation history");
    assert_eq!(remaining_operations, 0);
}
