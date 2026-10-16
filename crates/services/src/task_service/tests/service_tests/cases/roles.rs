use super::super::*;
use crate::ProjectMemberService;
use api_types::ActorRef;
use db::{
    CoordinationMode, CreateProjectMember, ProjectMemberRepo, RoleMembershipRepo,
    TaskLifecycleRepo, TaskRoleRepo,
};

struct RecordingExecutionProvider {
    executor: Arc<RecordingCancelExecutor>,
}

#[async_trait::async_trait]
impl crate::daemon_transport::ExecutionProvider for RecordingExecutionProvider {
    async fn start(
        &self,
        params: api_types::ExecutionStartParams,
    ) -> std::result::Result<api_types::ExecutionStartResult, crate::ServiceError> {
        Ok(api_types::ExecutionStartResult {
            execution_id: params.execution_id,
            accepted: true,
        })
    }

    async fn cancel(
        &self,
        params: api_types::ExecutionCancelParams,
    ) -> std::result::Result<api_types::ExecutionCancelResult, crate::ServiceError> {
        let _ = self.executor.cancel(&params.execution_id).await;
        Ok(api_types::ExecutionCancelResult {
            execution_id: params.execution_id,
            cancelled: true,
        })
    }
}

async fn running_non_repository_execution_for_cancel(
    db: &db::SqliteDb,
    service: &TaskService,
    task: &db::Task,
    agent_id: &str,
) -> db::Execution {
    let agent = AgentRepo::get_by_id(db, agent_id)
        .await
        .expect("Agent loads")
        .expect("Agent exists");
    let daemon_id = agent.daemon_id.expect("Agent has its exact daemon");
    let changed = sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
        .bind(crate::embedded_daemon::embedded_machine_id())
        .bind(&daemon_id)
        .execute(db.pool())
        .await
        .expect("fixture routes the Agent to the embedded host");
    assert_eq!(changed.rows_affected(), 1);
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
        .add_task_role_member(
            &task.id,
            "implementer",
            ActorRef::Agent(agent_id.to_owned()),
        )
        .await
        .expect("Agent joins implementer TaskRole");
    let work_unit_id = db::new_uuid_v4();
    let now = db::now_rfc3339();
    let work_unit = db::WorkUnitRepo::create(
        db,
        db::CreateWorkUnit {
            id: work_unit_id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Task cancellation WorkUnit".to_owned(),
            scope: "Cancel one exact Running Execution".to_owned(),
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
            correlation_id: work_unit_id,
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: "{}".to_owned(),
            created_at: now,
        },
    )
    .await
    .expect("exact non-repository WorkUnit creates")
    .record;
    let execution = service
        .launch_work_unit_execution(
            &work_unit.id,
            work_unit.version,
            db::ExecutionPurpose::Implement,
            "running execution for cancellation".to_owned(),
            Vec::new(),
            None,
        )
        .await
        .expect("exact no-repository WorkUnit Execution admits")
        .execution;
    assert!(
        service
            .start_execution(execution.id.clone())
            .await
            .expect("provider accepts the exact Start")
            .accepted
    );
    execution
}

#[tokio::test]
async fn cancel_execution_invokes_task_executor_cancel() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let executor = Arc::new(RecordingCancelExecutor::default());
    let provider = Arc::new(RecordingExecutionProvider {
        executor: Arc::clone(&executor),
    });
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_task_executor(executor.clone())
        .with_execution_provider_override(provider);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let execution =
        running_non_repository_execution_for_cancel(&db, &service, &task, &agent_id).await;

    service
        .cancel_execution(execution.id.clone(), "cancelled by test".to_owned())
        .await
        .expect("execution cancels");

    let cancelled = executor.cancelled.lock().expect("cancel log lock").clone();
    assert_eq!(cancelled, vec![execution.id]);
}

#[tokio::test]
async fn cancel_task_cancels_running_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let executor = Arc::new(RecordingCancelExecutor::default());
    let provider = Arc::new(RecordingExecutionProvider {
        executor: Arc::clone(&executor),
    });
    let service = TaskService::new(Arc::clone(&db), event_bus)
        .with_task_executor(executor.clone())
        .with_execution_provider_override(provider);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
    let execution =
        running_non_repository_execution_for_cancel(&db, &service, &task, &agent_id).await;

    let cancelled_task = service
        .cancel_task(task.id.clone())
        .await
        .expect("task cancels");

    let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*db, &cancelled_task.id)
        .await
        .expect("TaskLifecycle loads")
        .expect("TaskLifecycle exists");
    assert_eq!(lifecycle.state, db::TaskLifecycleState::Cancelled);
    let cancelled = executor.cancelled.lock().expect("cancel log lock").clone();
    assert_eq!(cancelled, vec![execution.id.clone()]);
    let execution_after = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("execution loads")
        .expect("execution exists");
    assert_eq!(execution_after.status, ExecutionStatus::Cancelled);
}

#[tokio::test]
async fn authoritative_memberships_preserve_multi_actor_cross_role_and_human_parity() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let human_id = seed_human_user(&db).await;
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&human_id)
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project owner updates");
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;

    for role in ["planner", "orchestrator", "reviewer"] {
        service
            .create_task_role(
                &task.id,
                role,
                CoordinationMode::Collaborative,
                "{}".to_owned(),
            )
            .await
            .expect("TaskRole creates");
    }
    service
        .add_task_role_member(&task.id, "planner", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("planner Agent membership creates");
    service
        .add_task_role_member(&task.id, "orchestrator", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("orchestrator Agent membership creates");
    service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human(human_id.clone()))
        .await
        .expect("reviewer Human membership creates");
    service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("reviewer Agent membership creates");
    let reviewer_role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "reviewer")
        .await
        .expect("reviewer TaskRole loads")
        .expect("reviewer TaskRole exists");
    let reviewer_memberships = RoleMembershipRepo::list_by_role(&*db, &reviewer_role.id, false)
        .await
        .expect("reviewer memberships load");
    assert_eq!(
        crate::task_service::select_usable_agent_id(&db, &reviewer_memberships)
            .await
            .expect("reviewer Agent selection succeeds")
            .as_deref(),
        Some(agent_id.as_str()),
        "an Agent execution path must not select the Human reviewer member"
    );

    let memberships = RoleMembershipRepo::list_by_task(&*db, &task.id, false)
        .await
        .expect("TaskRole memberships load");
    assert_eq!(
        memberships
            .iter()
            .filter(|(role, _)| role.role == "planner" || role.role == "orchestrator")
            .count(),
        2
    );
    assert_eq!(
        memberships
            .iter()
            .filter(|(role, _)| role.role == "reviewer")
            .count(),
        2,
        "Human and Agent reviewer memberships must coexist"
    );
    assert_eq!(
        memberships
            .iter()
            .filter(|(_, member)| member.actor_kind == db::ActorKind::Agent)
            .count(),
        3,
        "one Agent may participate in several roles"
    );
}

#[tokio::test]
async fn membership_lifecycle_ends_one_member_without_removing_another() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    service
        .create_task_role(
            &task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer TaskRole creates");
    let membership_a = service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(agent_a))
        .await
        .expect("first reviewer membership creates");
    let membership_b = service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("second reviewer membership creates");

    let suspended = service
        .update_task_role_member(
            &task.id,
            &membership_a.id,
            membership_a.version,
            db::RoleMembershipStatus::Suspended,
        )
        .await
        .expect("membership suspends");
    let current_memberships =
        RoleMembershipRepo::list_by_role(&*db, &membership_b.task_role_id, false)
            .await
            .expect("current memberships load");
    let selected = crate::task_service::select_usable_agent_id(&db, &current_memberships)
        .await
        .expect("usable member selection succeeds");
    assert_eq!(selected.as_deref(), Some(membership_b.actor_id.as_str()));

    let ended = service
        .update_task_role_member(
            &task.id,
            &membership_a.id,
            suspended.version,
            db::RoleMembershipStatus::Ended,
        )
        .await
        .expect("membership ends");
    assert_eq!(ended.status, db::RoleMembershipStatus::Ended);
    let current = RoleMembershipRepo::list_by_role(&*db, &membership_b.task_role_id, false)
        .await
        .expect("current memberships load");
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].actor_id, agent_b);
    let history = RoleMembershipRepo::list_by_role(&*db, &membership_b.task_role_id, true)
        .await
        .expect("membership history loads");
    assert_eq!(history.len(), 2);
    assert!(history
        .iter()
        .any(|member| member.id == membership_a.id
            && member.status == db::RoleMembershipStatus::Ended));
}

#[tokio::test]
async fn unusable_first_agent_does_not_hide_later_usable_membership() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
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
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_a.clone()))
        .await
        .expect("first Agent membership creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("second Agent membership creates");
    sqlx::query("UPDATE agent_identity SET paused = 1 WHERE id = ?")
        .bind(&agent_a)
        .execute(db.pool())
        .await
        .expect("first Agent pauses");

    let memberships =
        crate::task_service::current_role_memberships_authoritative(&db, &task.id, "coder")
            .await
            .expect("authoritative memberships load")
            .expect("TaskRole exists");
    let selected = crate::task_service::select_usable_agent_id(&db, &memberships)
        .await
        .expect("usable Agent selection succeeds");
    assert_eq!(selected.as_deref(), Some(agent_b.as_str()));
    sqlx::query("UPDATE agent_identity SET paused = 0 WHERE id = ?")
        .bind(&agent_a)
        .execute(db.pool())
        .await
        .expect("orchestration Agent resumes");
    let generic_selected = crate::task_service::select_usable_agent_id(&db, &memberships)
        .await
        .expect("generic Agent selection succeeds");
    assert_eq!(generic_selected.as_deref(), Some(agent_a.as_str()));
    let repository_selected =
        crate::task_service::select_usable_repository_agent_id(&db, &project_id, &memberships)
            .await
            .expect("repository Agent selection succeeds");
    assert_eq!(repository_selected.as_deref(), Some(agent_b.as_str()));
}

#[tokio::test]
async fn work_unit_leases_bind_exact_execution_workspace_actor_and_revoke_independently() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let agent_b = seed_agent(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Active).await;
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
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_a.clone()))
        .await
        .expect("Agent A joins implementer");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_b.clone()))
        .await
        .expect("Agent B joins implementer");

    let task = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    let now = now_rfc3339();
    fn make_work_unit_test_event(
        task_id: &str,
        now: &str,
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        actor_id: &str,
    ) -> db::CreateDomainEvent {
        db::CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
            actor_type: "agent".to_owned(),
            actor_id: Some(actor_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: serde_json::json!({
                "task_id": task_id,
                "entity_id": entity_id,
                "work_unit_id": entity_id,
            })
            .to_string(),
            created_at: now.to_owned(),
        }
    }

    async fn create_running_work_unit(
        database: &db::SqliteDb,
        service: &TaskService,
        task: &db::Task,
        repo_id: &str,
        actor_id: &str,
        now: &str,
    ) -> (db::WorkUnit, db::Workspace, String) {
        let work_unit_id = new_uuid_v4();
        let work_unit = db::WorkUnitRepo::create(
            database,
            db::CreateWorkUnit {
                id: work_unit_id.clone(),
                task_id: task.id.clone(),
                parent_work_unit_id: None,
                title: "isolated execution".to_owned(),
                scope: "test scope".to_owned(),
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Agent(actor_id.to_owned())),
                requires_integration: true,
                provenance: None,
                created_by: db::ActorRef::Agent(actor_id.to_owned()),
                created_at: now.to_owned(),
            },
            make_work_unit_test_event(
                &task.id,
                now,
                "work_unit.created",
                "work_unit",
                &work_unit_id,
                actor_id,
            ),
        )
        .await
        .expect("WorkUnit creates")
        .record;
        let workspace_id = new_uuid_v4();
        let workspace = db::WorkUnitWorkspaceRepo::create_for_work_unit(
            database,
            db::CreateWorkUnitWorkspace {
                workspace: db::CreateWorkspace {
                    id: workspace_id.clone(),
                    task_id: task.id.clone(),
                    repo_id: repo_id.to_owned(),
                    worktree_path: format!("/tmp/pr5/{workspace_id}"),
                    branch: format!("work-unit/{workspace_id}"),
                    status: db::WorkspaceStatus::Ready,
                    before_sha: Some("base-sha".to_owned()),
                    created_at: now.to_owned(),
                    updated_at: now.to_owned(),
                },
                work_unit_id: work_unit_id.clone(),
            },
        )
        .await
        .expect("WorkUnit Workspace creates");
        let execution_id = new_uuid_v4();
        let execution = db::CreateExecution {
            id: execution_id.clone(),
            task_id: task.id.clone(),
            agent_id: Some(actor_id.to_owned()),
            actor_ref: Some(db::ActorRef::Agent(actor_id.to_owned())),
            role: "implementer".to_owned(),
            purpose: Some(db::ExecutionPurpose::Implement),
            status: db::ExecutionStatus::Running,
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
            executor_config_snapshot_json: None,
            workspace_id: Some(workspace.id.clone()),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        };
        let lease = service
            .prepare_work_unit_workspace_lease(
                task,
                &work_unit,
                &workspace,
                &execution,
                &db::ActorRef::Agent(actor_id.to_owned()),
            )
            .await
            .expect("exact WorkUnit lease prepares");
        db::WorkUnitExecutionRepo::create_for_work_unit(
            database,
            db::CreateWorkUnitExecution {
                execution,
                work_unit_id: work_unit.id.clone(),
                work_unit_version: work_unit.version,
                workspace_lease: Some(lease),
            },
            make_work_unit_test_event(
                &task.id,
                now,
                "execution.started",
                "execution",
                &execution_id,
                actor_id,
            ),
        )
        .await
        .expect("WorkUnit Execution and Lease bind");
        (work_unit, workspace, execution_id)
    }

    let (unit_a, workspace_a, execution_a) =
        create_running_work_unit(&db, &service, &task, &repo_id, &agent_a, &now).await;
    let (unit_b, workspace_b, execution_b) =
        create_running_work_unit(&db, &service, &task, &repo_id, &agent_b, &now).await;
    assert_ne!(workspace_a.id, workspace_b.id);
    let execution_count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Task Execution count loads");
    let recovery = service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::ResumeSession,
            None,
            None,
        )
        .await
        .expect_err("Task recovery cannot infer one target from multiple WorkUnits");
    assert!(recovery
        .to_string()
        .contains("no blocking recovery annotation"));
    let execution_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .expect("Task Execution count reloads");
    assert_eq!(execution_count_after, execution_count_before);

    let lease_a = db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &unit_a.id)
        .await
        .expect("A lease loads")
        .expect("A lease is active");
    let lease_b = db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &unit_b.id)
        .await
        .expect("B lease loads")
        .expect("B lease is active");
    assert_ne!(lease_a.id, lease_b.id);
    assert_eq!(
        lease_a.workspace_id.as_deref(),
        Some(workspace_a.id.as_str())
    );
    assert_eq!(
        lease_b.workspace_id.as_deref(),
        Some(workspace_b.id.as_str())
    );

    assert!(service
        .verify_active_workspace_lease(
            &task,
            &workspace_a,
            "implementer",
            Some(&agent_a),
            &execution_a,
        )
        .await
        .is_ok());
    assert!(service
        .verify_active_workspace_lease(
            &task,
            &workspace_a,
            "implementer",
            Some(&agent_b),
            &execution_a,
        )
        .await
        .is_err());
    assert!(service
        .verify_active_workspace_lease(
            &task,
            &workspace_b,
            "implementer",
            Some(&agent_b),
            &execution_a,
        )
        .await
        .is_err());

    let competing_execution_id = new_uuid_v4();
    let competing_execution = db::CreateExecution {
        id: competing_execution_id.clone(),
        task_id: task.id.clone(),
        agent_id: Some(agent_a.clone()),
        actor_ref: Some(db::ActorRef::Agent(agent_a.clone())),
        role: "implementer".to_owned(),
        purpose: Some(db::ExecutionPurpose::Implement),
        status: db::ExecutionStatus::Running,
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
        executor_config_snapshot_json: None,
        workspace_id: Some(workspace_a.id.clone()),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let competing_lease = service
        .prepare_work_unit_workspace_lease(
            &task,
            &unit_a,
            &workspace_a,
            &competing_execution,
            &db::ActorRef::Agent(agent_a.clone()),
        )
        .await
        .expect("competing exact lease can be prepared before admission");
    let competing_admission = db::WorkUnitExecutionRepo::create_for_work_unit(
        &*db,
        db::CreateWorkUnitExecution {
            execution: competing_execution,
            work_unit_id: unit_a.id.clone(),
            work_unit_version: unit_a.version,
            workspace_lease: Some(competing_lease),
        },
        make_work_unit_test_event(
            &task.id,
            &now,
            "execution.started",
            "execution",
            &competing_execution_id,
            &agent_a,
        ),
    )
    .await;
    assert!(
        competing_admission.is_err(),
        "one WorkUnit cannot admit two mutable Executions at once"
    );

    db::ExecutionRepo::update(
        &*db,
        db::UpdateExecution {
            id: execution_a.clone(),
            status: Some(db::ExecutionStatus::Completed),
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
            after_sha: Some(Some("result-a".to_owned())),
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("A Execution completes");
    service
        .revoke_active_workspace_lease_for_execution(&task.id, &execution_a)
        .await;
    assert!(
        db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &unit_a.id)
            .await
            .expect("A lease reloads")
            .is_none()
    );
    assert_eq!(
        db::WorkspaceLeaseRepo::get_active_for_work_unit(&*db, &unit_b.id)
            .await
            .expect("B lease reloads")
            .expect("B lease remains active")
            .execution_id,
        execution_b
    );
}

#[tokio::test]
async fn actor_validation_requires_project_human_and_rejects_sentinel() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let human_id = seed_human_user(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    service
        .create_task_role(
            &task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer TaskRole creates");

    assert!(service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human("human".to_owned()))
        .await
        .is_err());
    assert!(service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human(human_id.clone()))
        .await
        .is_err());

    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&human_id)
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project owner updates");
    service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human(human_id))
        .await
        .expect("project Human membership succeeds");
}

#[tokio::test]
async fn agent_validation_requires_project_scope_but_not_runtime_status() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(32));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let project_owner = seed_human_user(&db).await;
    let project_member = seed_human_user(&db).await;
    let outside_owner = seed_human_user(&db).await;
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&project_owner)
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project owner updates");
    ProjectMemberRepo::add_member(
        &*db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: project_member.clone(),
            role: "member".to_owned(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project member creates");

    let outside_agent = seed_agent(&db).await;
    sqlx::query(
        "UPDATE agent_identity
         SET visibility = 'account', owner_id = ?
         WHERE id = ?",
    )
    .bind(&outside_owner)
    .bind(&outside_agent)
    .execute(db.pool())
    .await
    .expect("outside Agent scope updates");

    let member_agent = seed_agent(&db).await;
    sqlx::query(
        "UPDATE agent_identity
         SET visibility = 'account', owner_id = ?
         WHERE id = ?",
    )
    .bind(&project_member)
    .bind(&member_agent)
    .execute(db.pool())
    .await
    .expect("project member Agent scope updates");

    let out_of_scope_agent = seed_agent(&db).await;
    sqlx::query(
        "UPDATE agent_identity
         SET visibility = 'account', owner_id = ?
         WHERE id = ?",
    )
    .bind(&outside_owner)
    .bind(&out_of_scope_agent)
    .execute(db.pool())
    .await
    .expect("out-of-scope Agent identity updates");
    let global_agent = seed_agent(&db).await;
    let paused_agent = seed_agent(&db).await;
    sqlx::query("UPDATE agent_identity SET paused = 1 WHERE id = ?")
        .bind(&paused_agent)
        .execute(db.pool())
        .await
        .expect("Agent pauses");

    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    for role in ["planner", "implementer", "reviewer", "orchestrator"] {
        service
            .create_task_role(
                &task.id,
                role,
                CoordinationMode::Collaborative,
                "{}".to_owned(),
            )
            .await
            .expect("TaskRole creates");
    }

    assert!(service
        .add_task_role_member(&task.id, "planner", ActorRef::Agent(outside_agent),)
        .await
        .is_err());
    service
        .add_task_role_member(&task.id, "planner", ActorRef::Agent(member_agent))
        .await
        .expect("Project member account Agent membership succeeds");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(global_agent))
        .await
        .expect("global Agent membership succeeds");
    assert!(
        service
            .add_task_role_member(&task.id, "reviewer", ActorRef::Agent(out_of_scope_agent))
            .await
            .is_err(),
        "an account Agent owned outside the Project cannot join its TaskRole"
    );
    service
        .add_task_role_member(&task.id, "orchestrator", ActorRef::Agent(paused_agent))
        .await
        .expect("paused but valid Agent membership succeeds");
}

#[tokio::test]
async fn invalid_actor_cannot_join_a_target_task_role() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let project_owner = seed_human_user(&db).await;
    let outsider = seed_human_user(&db).await;
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&project_owner)
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project owner updates");
    let invalid_agent = seed_agent(&db).await;
    sqlx::query(
        "UPDATE agent_identity
         SET visibility = 'account', owner_id = ?
         WHERE id = ?",
    )
    .bind(&outsider)
    .bind(&invalid_agent)
    .execute(db.pool())
    .await
    .expect("outsider Agent scope updates");

    let task = service
        .create_task(
            project_id.clone(),
            "explicit membership validation",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Task is independent of membership changes");
    service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Independent,
            "{}".to_owned(),
        )
        .await
        .expect("target TaskRole creates");
    assert!(service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(invalid_agent))
        .await
        .is_err());
    let role = TaskRoleRepo::get_by_task_and_role(&*db, &task.id, "implementer")
        .await
        .expect("TaskRole lookup succeeds")
        .expect("TaskRole is present");
    assert!(RoleMembershipRepo::list_by_role(&*db, &role.id, false)
        .await
        .expect("membership list loads")
        .is_empty());
}

#[tokio::test]
async fn task_creation_ignores_legacy_project_role_defaults() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let settings = serde_json::json!({
        "default_role_assignments": [{
            "role_name": "implementer",
            "assignee_type": "agent",
            "assignee_id": agent_id
        }]
    });
    sqlx::query("UPDATE project SET settings = ?, updated_at = ? WHERE id = ?")
        .bind(settings.to_string())
        .bind(now_rfc3339())
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("project defaults update");

    let task = service
        .create_task(
            project_id.clone(),
            "ordinary Task ignores old defaults",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
    let task = task.expect("ordinary Task creates");
    let role_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_role WHERE task_id = ?")
        .bind(&task.id)
        .fetch_one(db.pool())
        .await
        .expect("TaskRole count loads");
    let membership_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_membership
         WHERE task_role_id IN (SELECT id FROM task_role WHERE task_id = ?)",
    )
    .bind(&task.id)
    .fetch_one(db.pool())
    .await
    .expect("RoleMembership count loads");
    assert_eq!(role_count, 0, "old Project settings create no TaskRole");
    assert_eq!(
        membership_count, 0,
        "old Project settings create no membership"
    );

    let retained_settings: String = sqlx::query_scalar("SELECT settings FROM project WHERE id = ?")
        .bind(&project_id)
        .fetch_one(db.pool())
        .await
        .expect("historical Project settings remain stored");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&retained_settings).expect("settings parse"),
        settings,
        "retiring the reader preserves the historical Project setting"
    );
}

#[tokio::test]
async fn project_member_removal_ends_human_membership_and_preserves_history() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let owner_id = seed_human_user(&db).await;
    let member_id = seed_human_user(&db).await;
    configure_project_scope(&db, &project_id, &owner_id, &[&member_id]).await;

    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let role = service
        .create_task_role(
            &task.id,
            "reviewer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("reviewer role creates");
    let membership = service
        .add_task_role_member(&task.id, "reviewer", ActorRef::Human(member_id.clone()))
        .await
        .expect("Human membership creates");
    let mut events = event_bus.subscribe();

    ProjectMemberService::new(Arc::clone(&db), Arc::clone(&event_bus))
        .remove_member(&project_id, &owner_id, &member_id)
        .await
        .expect("Project member removal succeeds");

    let event = events.recv().await.expect("scope revocation event emits");
    assert_eq!(event.event_type, "task.updated");
    assert_eq!(event.entity_id, task.id);
    match event.context {
        EventContext::TaskUpdated {
            project_id: emitted,
        } => assert_eq!(emitted, project_id),
        other => panic!("unexpected scope revocation event context: {other:?}"),
    }
    assert!(events.try_recv().is_err());

    let history = RoleMembershipRepo::list_by_role(&*db, &role.id, true)
        .await
        .expect("membership history loads");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, membership.id);
    assert_eq!(history[0].status, db::RoleMembershipStatus::Ended);
    assert!(history[0].ended_at.is_some());
    assert!(ProjectMemberRepo::get_member(&*db, &project_id, &member_id)
        .await
        .expect("Project member lookup succeeds")
        .is_none());
}

#[tokio::test]
async fn project_member_removal_ends_owned_account_agent_membership() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let owner_id = seed_human_user(&db).await;
    let member_id = seed_human_user(&db).await;
    configure_project_scope(&db, &project_id, &owner_id, &[&member_id]).await;
    let agent_id = seed_agent(&db).await;
    set_account_agent_owner(&db, &agent_id, &member_id).await;

    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let role = service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("implementer role creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("account Agent membership creates");

    ProjectMemberService::new(Arc::clone(&db), Arc::new(EventBus::new(16)))
        .remove_member(&project_id, &owner_id, &member_id)
        .await
        .expect("Project member removal succeeds");

    let history = RoleMembershipRepo::list_by_role(&*db, &role.id, true)
        .await
        .expect("membership history loads");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].actor_id, agent_id);
    assert_eq!(history[0].status, db::RoleMembershipStatus::Ended);
}

#[tokio::test]
async fn project_member_removal_ends_agent_membership_without_project_eligibility() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let owner_id = seed_human_user(&db).await;
    let member_id = seed_human_user(&db).await;
    configure_project_scope(&db, &project_id, &owner_id, &[&member_id]).await;
    let agent_id = seed_agent(&db).await;
    set_account_agent_owner(&db, &agent_id, &member_id).await;

    let task = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let role = service
        .create_task_role(
            &task.id,
            "implementer",
            CoordinationMode::Collaborative,
            "{}".to_owned(),
        )
        .await
        .expect("implementer role creates");
    service
        .add_task_role_member(&task.id, "implementer", ActorRef::Agent(agent_id.clone()))
        .await
        .expect("account-member Agent membership creates");

    ProjectMemberService::new(Arc::clone(&db), Arc::new(EventBus::new(16)))
        .remove_member(&project_id, &owner_id, &member_id)
        .await
        .expect("Project member removal succeeds");

    let current = RoleMembershipRepo::list_by_role(&*db, &role.id, false)
        .await
        .expect("current memberships load");
    assert!(
        current.is_empty(),
        "removing the owning Project member revokes the Agent membership"
    );
}

async fn configure_project_scope(
    db: &db::SqliteDb,
    project_id: &str,
    owner_id: &str,
    member_ids: &[&str],
) {
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(owner_id)
        .bind(project_id)
        .execute(db.pool())
        .await
        .expect("Project owner updates");
    add_project_member(db, project_id, owner_id, "owner").await;
    for member_id in member_ids {
        add_project_member(db, project_id, member_id, "member").await;
    }
}

async fn add_project_member(db: &db::SqliteDb, project_id: &str, user_id: &str, role: &str) {
    let now = now_rfc3339();
    ProjectMemberRepo::add_member(
        db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            user_id: user_id.to_owned(),
            role: role.to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Project member creates");
}

async fn set_account_agent_owner(db: &db::SqliteDb, agent_id: &str, owner_id: &str) {
    sqlx::query(
        "UPDATE agent_identity
         SET visibility = 'account', owner_id = ?
         WHERE id = ?",
    )
    .bind(owner_id)
    .bind(agent_id)
    .execute(db.pool())
    .await
    .expect("account Agent scope updates");
}
