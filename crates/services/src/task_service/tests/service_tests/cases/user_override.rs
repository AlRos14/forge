use super::super::*;

#[tokio::test]
async fn user_subtask_in_progress_to_review_succeeds() {
    // Delta: User routes a subtask into review
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let subtask =
        seed_subtask_with_status(&db, &root, "child", "in_progress".to_owned(), 0).await;

    let result = service
        .transition(
            subtask.id.clone(),
            crate::workflow::default_states::REVIEW.to_owned(),
            (subtask.version, None),
        )
        .await
        .expect("user subtask in_progress -> review succeeds");

    assert_eq!(result.task.status, crate::workflow::default_states::REVIEW);
}

#[tokio::test]
async fn root_task_resolution_unchanged() {
    // Delta: Root task resolution is unchanged
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let valid = service
        .transition(
            task.id.clone(),
            crate::workflow::default_states::IN_PROGRESS.to_owned(),
            (task.version, None),
        )
        .await
        .expect("valid root transition along project workflow succeeds");
    assert_eq!(
        valid.task.status,
        crate::workflow::default_states::IN_PROGRESS
    );

    let invalid = service
        .transition(
            task.id,
            "nonexistent".to_owned(),
            (valid.task.version, None),
        )
        .await;
    match invalid {
        Err(ServiceError::InvalidOperation { message }) => {
            assert!(
                message.contains("not defined in workflow"),
                "root task still validates against project workflow: {message}"
            );
        }
        other => panic!("expected undefined-state rejection for root task, got {other:?}"),
    }
}

#[tokio::test]
async fn system_subtask_transition_still_uses_subtask_workflow() {
    // Delta: Automatic subtask lifecycle is unchanged for subtask-workflow states
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let subtask =
        seed_subtask_with_status(&db, &root, "child", "in_progress".to_owned(), 0).await;

    let result = service
        .transition(
            subtask.id.clone(),
            crate::workflow::default_states::REVIEW.to_owned(),
            TransitionOptions {
                version: subtask.version,
                reason: Some("system cascade attempt".to_owned()),
                triggered_by: "system".to_owned(),
                rejection: false,
                defer_dispatch_seconds: None,
            },
        )
        .await;

    match result {
        Err(ServiceError::InvalidOperation { message }) => {
            assert!(
                message.contains("not defined in workflow"),
                "system subtask transition should use subtask workflow without review: {message}"
            );
        }
        other => panic!(
            "system subtask in_progress -> review should be rejected, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn no_agent_override_move_writes_log_and_no_executor() {
    // Delta: User move with no agent assigned succeeds without execution
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;

    let workflow = WorkflowDefinition {
        roles: Vec::new(),
        states: vec![
            {
                let mut working =
                    workflow_state("working", StateKind::Active, None, StateHooks::default());
                working.triggers.insert(
                    WorkflowTrigger::Accept,
                    WorkflowTriggerDefinition {
                        to: "paused".to_owned(),
                        dispatch: None,
                    },
                );
                working
            },
            workflow_state("paused", StateKind::Custom, None, StateHooks::default()),
            workflow_state(
                "coding",
                StateKind::Active,
                Some(default_roles::CODER),
                StateHooks {
                    on_enter: vec![hook("dispatch_role_agent")],
                    ..StateHooks::default()
                },
            ),
            workflow_state("done", StateKind::Terminal, None, StateHooks::default()),
        ],
        configuration: Vec::new(),
        cancellation_state: None,
    };
    update_project_workflow(&db, &project_id, &workflow).await;

    let task = seed_task_with_status(&db, &project_id, &repo_id, "working".to_owned()).await;
    let reason = "override into dispatchable state without agent";

    let result = service
        .transition(
            task.id.clone(),
            "coding".to_owned(),
            TransitionOptions {
                version: task.version,
                reason: Some(reason.to_owned()),
                triggered_by: "user:api".to_owned(),
                rejection: false,
                defer_dispatch_seconds: None,
            },
        )
        .await
        .expect("user override into dispatchable state succeeds without agent");

    assert_eq!(result.task.status, "coding");

    let logs = TransitionLogRepo::list_by_task(&*db, &task.id)
        .await
        .expect("transition logs load");
    let latest = logs
        .iter()
        .find(|entry| entry.to_state == "coding" && !entry.rejection)
        .expect("transition log written for override move");
    assert_eq!(latest.triggered_by, "user:override:api");
    assert_eq!(latest.trigger_reason, reason);

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
        "no executor should launch when no agent is assigned"
    );
}

#[tokio::test]
async fn override_move_out_of_active_state_cancels_running_execution() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;

    let workflow = WorkflowDefinition {
        roles: Vec::new(),
        states: vec![
            {
                let mut working =
                    workflow_state("working", StateKind::Active, None, StateHooks::default());
                working.triggers.insert(
                    WorkflowTrigger::Accept,
                    WorkflowTriggerDefinition {
                        to: "paused".to_owned(),
                        dispatch: None,
                    },
                );
                working
            },
            workflow_state("paused", StateKind::Custom, None, StateHooks::default()),
            workflow_state("done", StateKind::Terminal, None, StateHooks::default()),
        ],
        configuration: Vec::new(),
        cancellation_state: None,
    };
    update_project_workflow(&db, &project_id, &workflow).await;

    let task = seed_task_with_status(&db, &project_id, &repo_id, "working".to_owned()).await;
    let execution = seed_running_coder_execution(&db, &task.id, Some(agent_id), None).await;

    let result = service
        .transition(
            task.id.clone(),
            "done".to_owned(),
            TransitionOptions {
                version: task.version,
                reason: Some("override across missing edge".to_owned()),
                triggered_by: "user:api".to_owned(),
                rejection: false,
                defer_dispatch_seconds: None,
            },
        )
        .await
        .expect("user override across missing edge succeeds");

    assert_eq!(result.task.status, "done");
    let execution_after = ExecutionRepo::get_by_id(&*db, &execution.id)
        .await
        .expect("execution loads")
        .expect("execution exists");
    assert_eq!(execution_after.status, ExecutionStatus::Cancelled);
    assert_eq!(
        execution_after.error.as_deref(),
        Some("cancelled by user transition")
    );
}

#[tokio::test]
async fn subtask_in_project_only_state_cannot_be_deleted() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "in_progress".to_owned()).await;
    let subtask =
        seed_subtask_with_status(&db, &root, "child", "in_progress".to_owned(), 0).await;

    let routed = service
        .transition(
            subtask.id.clone(),
            crate::workflow::default_states::REVIEW.to_owned(),
            (subtask.version, None),
        )
        .await
        .expect("user routes subtask into review");

    assert_eq!(routed.task.status, crate::workflow::default_states::REVIEW);

    let delete_result = service.soft_delete(routed.task.id).await;
    match delete_result {
        Err(ServiceError::InvalidOperation { message }) => {
            assert!(
                message.contains("tasks can only be deleted from inactive states"),
                "subtask in review gate must not be deletable: {message}"
            );
        }
        other => panic!("expected soft_delete rejection for subtask in review, got {other:?}"),
    }
}
