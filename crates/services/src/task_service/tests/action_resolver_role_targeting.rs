use super::helpers::*;
use super::*;

#[tokio::test]
async fn test_resolve_execution_actions_targets_current_role() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::IN_PROGRESS,
    )
    .await;
    let agent_id = seed_agent(&db).await;
    let coder_execution = seed_execution(
        &db,
        &task.id,
        Some(&agent_id),
        crate::workflow::default_roles::CODER,
        ExecutionStatus::Completed,
        Some("coder-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let reviewer_execution = seed_execution(
        &db,
        &task.id,
        Some(&agent_id),
        crate::workflow::default_roles::REVIEWER,
        ExecutionStatus::Completed,
        Some("reviewer-session"),
        "2026-05-02T10:05:00Z",
    )
    .await;
    let workflow = crate::workflow::default_workflow::default_workflow();
    let executions = vec![coder_execution.clone(), reviewer_execution.clone()];
    let annotation = api_types::TaskBlockingAnnotation {
        annotation_type: api_types::FailureKind::ExecutorFailed,
        blocking_reason: "executor failed".to_owned(),
        blocked_by: Some("system".to_owned()),
        blocked_at: Some(now_rfc3339()),
        blocked_execution_id: Some(coder_execution.id.clone()),
        artifact: None,
        message: None,
        hook: None,
        recovery_actions: vec![api_types::RecoveryAction::ResumeSession],
    };

    let resumable_execution_ids = std::collections::HashSet::from([
        coder_execution.id.clone(),
        reviewer_execution.id.clone(),
    ]);
    let actions =
        crate::task_service::action_resolver::resolve_execution_actions_with_session_state(
            &task,
            &workflow,
            &executions,
            Some(&annotation),
            Some(&resumable_execution_ids),
        );

    let workflow_resume = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::WorkflowResume)
        .expect("workflow resume action exists");
    assert_eq!(
        workflow_resume.target_execution_id.as_deref(),
        Some(coder_execution.id.as_str())
    );
    assert_ne!(
        workflow_resume.target_execution_id.as_deref(),
        Some(reviewer_execution.id.as_str())
    );

    let reexecute = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::ReExecute)
        .expect("re-execute action exists");
    assert_eq!(
        reexecute.target_execution_id.as_deref(),
        Some(coder_execution.id.as_str())
    );
    assert_ne!(
        reexecute.target_execution_id.as_deref(),
        Some(reviewer_execution.id.as_str())
    );

    let follow_up = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::SessionFollowUp)
        .expect("session follow-up action exists");
    assert!(!follow_up.propagates);

    let coder_without_session = seed_execution(
        &db,
        &task.id,
        Some(&agent_id),
        crate::workflow::default_roles::CODER,
        ExecutionStatus::Completed,
        None,
        "2026-05-02T10:10:00Z",
    )
    .await;
    let actions =
        crate::task_service::action_resolver::resolve_execution_actions_with_session_state(
            &task,
            &workflow,
            &[coder_without_session],
            Some(&api_types::TaskBlockingAnnotation {
                blocked_execution_id: Some("missing".to_owned()),
                ..annotation
            }),
            Some(&std::collections::HashSet::new()),
        );
    let workflow_resume = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::WorkflowResume)
        .expect("workflow resume action exists");
    assert!(!workflow_resume.enabled);
    let reason = workflow_resume
        .disabled_reason
        .as_deref()
        .expect("disabled reason is present")
        .to_ascii_lowercase();
    assert!(reason.contains("no") && reason.contains("session"));
}

#[tokio::test]
async fn task_execution_actions_ignore_work_unit_executions() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::IN_PROGRESS,
    )
    .await;
    let agent_id = seed_agent(&db).await;
    let task_execution = seed_execution(
        &db,
        &task.id,
        Some(&agent_id),
        crate::workflow::default_roles::CODER,
        ExecutionStatus::Completed,
        None,
        "2026-05-02T10:00:00Z",
    )
    .await;
    let mut work_unit_execution = task_execution.clone();
    work_unit_execution.id = new_uuid_v4();
    work_unit_execution.work_unit_id = Some(new_uuid_v4());
    work_unit_execution.work_unit_version = Some(1);
    work_unit_execution.status = ExecutionStatus::Running;
    work_unit_execution.created_at = "2026-05-02T10:10:00Z".to_owned();
    let workflow = crate::workflow::default_workflow::default_workflow();

    let actions = crate::task_service::action_resolver::resolve_execution_actions(
        &task,
        &workflow,
        &[task_execution.clone(), work_unit_execution],
        None,
    );
    let stop = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::StopExecution)
        .expect("stop action exists");
    assert!(
        !stop.enabled,
        "Task action must not stop an arbitrary WorkUnit"
    );
    let reexecute = actions
        .iter()
        .find(|action| action.action == api_types::ExecutionActionKind::ReExecute)
        .expect("re-execute action exists");
    assert_eq!(
        reexecute.target_execution_id.as_deref(),
        Some(task_execution.id.as_str()),
        "Task retry targets its own historical Execution"
    );
}
