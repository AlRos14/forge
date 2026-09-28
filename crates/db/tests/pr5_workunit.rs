use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, ActorRef,
    AddWorkUnitDependency, CoordinationMode, CreateDomainEvent, CreateExecution, CreateProject,
    CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole, CreateWorkUnit,
    CreateWorkUnitExecution, CreateWorkUnitIntegration, CreateWorkUnitWorkspace, CreateWorkspace,
    CreateWorkspaceLease, DbError, ExecutionPurpose, ExecutionRepo, ExecutionStatus, ProjectRepo,
    RecordWorkUnitIntegration, RepoRepo, RoleMembershipRepo, RoleMembershipStatus, SqliteDb,
    TaskRepo, TaskRoleRepo, UpdateExecution, WorkMode, WorkUnitExecutionRepo,
    WorkUnitIntegrationOutcome, WorkUnitRepo, WorkUnitStatus, WorkUnitWorkspaceRepo,
    WorkspaceLeaseRepo, WorkspaceRepo, WorkspaceStatus,
};

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

fn running_execution(
    task_id: &str,
    work_unit_workspace_id: &str,
    agent_id: &str,
) -> CreateExecution {
    let now = now_rfc3339();
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
        agent_session_id: None,
        harness_session_id: None,
        agent_message_id: None,
        last_activity_at: None,
        summary: None,
        logs_path: None,
        before_sha: Some("base-sha".to_owned()),
        after_sha: None,
        error: None,
        executor_config_snapshot_json: None,
        workspace_id: Some(work_unit_workspace_id.to_owned()),
        created_at: now.clone(),
        updated_at: now,
    }
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
            workflow_definition: "{}".into(),
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
            executor_type: "codex".into(),
            provider: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".into(),
            tool_policy_json: "{}".into(),
            config_json: "{}".into(),
            credential_ref: None,
            daemon_id: None,
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
                assignee_type: None,
                assignee_id: None,
                title: "Task".into(),
                description: None,
                task_type: "implementation".into(),
                status: "todo".into(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                plan: None,
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

    let mut task_scoped_execution = running_execution(&task_id, "unused", &agent_id);
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
            agent_session_id: None,
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
            provenance_kind: None,
            provenance_id: None,
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
    let mut legacy_execution = running_execution(&other_task_id, "unused", &agent_id);
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
            agent_session_id: None,
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
    let legacy_writer = running_execution(&task_id, &integration_workspace.id, &agent_id);
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
        let execution = running_execution(&task_id, &workspace_id, &agent_id);
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
    let mut over_capacity_execution = running_execution(&task_id, "unused", &agent_id);
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
    let blocked_retry = running_execution(&task_id, workspace_d, &agent_id);
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
            agent_session_id: None,
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
}
