use super::helpers::*;
use super::*;

#[tokio::test]
async fn test_derive_workflow_exception_review_failed_no_annotation() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::REVIEW,
    )
    .await;
    assert_eq!(task.error_annotation, None);
    assert_eq!(task.blocked_json, None);
    let execution = seed_execution(
        &db,
        &task.id,
        None,
        crate::workflow::default_roles::REVIEWER,
        ExecutionStatus::Completed,
        Some("review-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let review = seed_failed_review(
        &db,
        &task.id,
        &execution.id,
        1,
        json!({
            "ci_steps": [{
                "command": "cargo test --workspace",
                "exit_code": 101,
                "output_tail": "test failure tail",
                "stderr_tail": "stderr failure tail"
            }]
        }),
    )
    .await;
    let mut remaining_retries = std::collections::HashMap::new();
    remaining_retries.insert(crate::workflow::default_states::REVIEW.to_owned(), 2);

    let exception = crate::task_diagnostics::derive_workflow_exception(
        &task,
        &crate::workflow::default_workflow::default_workflow(),
        Some(&review),
        Some(&execution),
        &remaining_retries,
    )
    .expect("workflow exception derives");

    assert_eq!(exception.exception_type, "review_failed");
    let failing_step = exception.failing_step.expect("failing step exists");
    assert_eq!(
        failing_step.command.as_deref(),
        Some("cargo test --workspace")
    );
    assert_eq!(failing_step.exit_code, Some(101));
    assert_eq!(
        failing_step.output_tail.as_deref(),
        Some("test failure tail")
    );
    assert_eq!(
        failing_step.stderr_tail.as_deref(),
        Some("stderr failure tail")
    );

    let action_kinds = exception
        .actions
        .iter()
        .map(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind serializes as string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert!(action_kinds.iter().any(|kind| kind == "retry_hook"));
    assert!(action_kinds.iter().any(|kind| kind == "resume_process"));
    assert!(action_kinds.iter().any(|kind| kind == "proceed_once"));
    assert!(action_kinds.iter().any(|kind| kind == "open_interactive"));

    let retry_hook = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("retry_hook")
        })
        .expect("retry_hook action exists");
    assert!(
        retry_hook.enabled,
        "retry_hook should be enabled for review gate with failed review and retries remaining"
    );

    let resume_process = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("resume_process")
        })
        .expect("resume_process action exists");
    assert!(resume_process.enabled);
    assert_eq!(
        resume_process.target_state.as_deref(),
        Some(crate::workflow::default_states::IN_PROGRESS)
    );
    assert_eq!(
        resume_process.target_role.as_deref(),
        Some(crate::workflow::default_roles::CODER)
    );

    let proceed_once = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("proceed_once")
        })
        .expect("proceed_once action exists");
    assert!(
        !proceed_once.enabled,
        "proceed_once should be disabled when retry budget is not exhausted"
    );
}

#[tokio::test]
async fn test_derive_workflow_exception_infers_actions_for_empty_exhausted_annotation() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::REVIEW,
    )
    .await;
    let execution = seed_execution(
        &db,
        &task.id,
        None,
        crate::workflow::default_roles::REVIEWER,
        ExecutionStatus::Completed,
        Some("review-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let review =
        seed_failed_review(&db, &task.id, &execution.id, 2, json!({ "ci_steps": [] })).await;
    let annotation = api_types::TaskAnnotation::Blocking(api_types::TaskBlockingAnnotation {
        annotation_type: "review_budget_exhausted".to_owned(),
        blocking_reason: "review retry budget exhausted".to_owned(),
        blocked_by: Some("system".to_owned()),
        blocked_at: Some(now_rfc3339()),
        blocked_execution_id: None,
        artifact: None,
        message: Some("review retry budget exhausted".to_owned()),
        hook: None,
        recovery_actions: Vec::new(),
    });
    let task = db::TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            plan: None,
            error_annotation: Some(Some(
                serde_json::to_string(&annotation).expect("annotation serializes"),
            )),
            blocked_json: Some(Some(
                json!({
                    "reason": "review retry budget exhausted",
                    "created_at": now_rfc3339(),
                    "kind": "review_gate_failed"
                })
                .to_string(),
            )),
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task updates");

    let exception = crate::task_diagnostics::derive_workflow_exception(
        &task,
        &crate::workflow::default_workflow::default_workflow(),
        Some(&review),
        Some(&execution),
        &std::collections::HashMap::new(),
    )
    .expect("workflow exception derives");

    let action_kinds = exception
        .actions
        .iter()
        .map(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind serializes as string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert!(action_kinds.iter().any(|kind| kind == "retry_hook"));
    assert!(action_kinds.iter().any(|kind| kind == "resume_process"));
    assert!(action_kinds.iter().any(|kind| kind == "reset_retry_window"));
    assert!(action_kinds.iter().any(|kind| kind == "proceed_once"));
    assert!(action_kinds.iter().any(|kind| kind == "open_interactive"));
}

#[tokio::test]
async fn test_retry_exhausted_blocked_metadata_takes_precedence_over_stale_error_annotation() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::MERGING,
    )
    .await;
    let execution = seed_execution(
        &db,
        &task.id,
        None,
        crate::workflow::default_roles::CODER,
        ExecutionStatus::Completed,
        Some("coder-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let stale_annotation = api_types::TaskAnnotation::Blocking(api_types::TaskBlockingAnnotation {
        annotation_type: "target_repo_dirty".to_owned(),
        blocking_reason: String::new(),
        blocked_by: None,
        blocked_at: None,
        blocked_execution_id: None,
        artifact: None,
        message: Some("target repository has uncommitted changes".to_owned()),
        hook: None,
        recovery_actions: Vec::new(),
    });
    let task = db::TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            plan: None,
            error_annotation: Some(Some(
                serde_json::to_string(&stale_annotation).expect("annotation serializes"),
            )),
            blocked_json: Some(Some(
                json!({
                    "reason": "gate rejection budget exhausted: 1/1",
                    "created_at": now_rfc3339(),
                    "kind": "retry_exhausted",
                    "execution_id": execution.id.clone()
                })
                .to_string(),
            )),
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task updates");

    let exception = crate::task_diagnostics::derive_workflow_exception(
        &task,
        &crate::workflow::default_workflow::default_workflow(),
        None,
        Some(&execution),
        &std::collections::HashMap::new(),
    )
    .expect("workflow exception derives");

    assert_eq!(exception.exception_type, "retry_exhausted");
    let action_kinds = exception
        .actions
        .iter()
        .map(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind serializes as string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let retry_hook = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("retry_hook")
        })
        .expect("retry_hook action exists");
    assert_eq!(retry_hook.label, "Retry Merge");
    assert!(
        retry_hook.enabled,
        "retry merge should reset the retry window and resume merge-fix work in one action"
    );
    assert_eq!(
        retry_hook.target_state.as_deref(),
        Some(crate::workflow::default_states::MERGE_FAILED)
    );
    assert_eq!(
        retry_hook.target_role.as_deref(),
        Some(crate::workflow::default_roles::CODER)
    );
    assert!(
        action_kinds.iter().any(|kind| kind == "resume_process"),
        "resume_process should be visible for exhausted merge gates"
    );
    let reset_retry_window = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("reset_retry_window")
        })
        .expect("reset_retry_window action exists");
    assert!(
        reset_retry_window.propagates,
        "reset_retry_window should indicate that it resumes merge-fix work"
    );
    assert!(
        action_kinds.iter().any(|kind| kind == "reset_retry_window"),
        "reset_retry_window should be offered instead of falling back to cancel only"
    );
    assert!(
        action_kinds.iter().all(|kind| kind != "cancel_task"),
        "retry exhaustion actions should not collapse to cancel_task"
    );
}

#[tokio::test]
async fn test_merge_gate_stale_error_annotation_offers_retry_merge_when_window_available() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::MERGING,
    )
    .await;
    let execution = seed_execution(
        &db,
        &task.id,
        None,
        crate::workflow::default_roles::CODER,
        ExecutionStatus::Completed,
        Some("coder-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let annotation = api_types::TaskAnnotation::Blocking(api_types::TaskBlockingAnnotation {
        annotation_type: "target_repo_dirty".to_owned(),
        blocking_reason: String::new(),
        blocked_by: None,
        blocked_at: None,
        blocked_execution_id: None,
        artifact: None,
        message: Some("target repository has uncommitted changes".to_owned()),
        hook: None,
        recovery_actions: Vec::new(),
    });
    let task = db::TaskRepo::update(
        &*db,
        db::UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            plan: None,
            error_annotation: Some(Some(
                serde_json::to_string(&annotation).expect("annotation serializes"),
            )),
            blocked_json: Some(None),
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task updates");

    let exception = crate::task_diagnostics::derive_workflow_exception(
        &task,
        &crate::workflow::default_workflow::default_workflow(),
        None,
        Some(&execution),
        &std::collections::HashMap::new(),
    )
    .expect("workflow exception derives");

    let retry_hook = exception
        .actions
        .iter()
        .find(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                == Some("retry_hook")
        })
        .expect("retry_hook action exists");
    assert_eq!(retry_hook.label, "Retry Merge");
    assert!(retry_hook.enabled);
    assert_eq!(
        retry_hook.target_state.as_deref(),
        Some(crate::workflow::default_states::MERGING)
    );

    assert!(
        exception.actions.iter().all(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                != Some("resume_process")
        }),
        "target-repo dirty recovery should retry the merge gate, not dispatch merge-fix work"
    );
}

#[tokio::test]
async fn test_reviewer_execution_failure_only_offers_retry_or_pass() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(
        &db,
        &project_id,
        &repo_id,
        crate::workflow::default_states::REVIEW,
    )
    .await;
    let execution = seed_execution(
        &db,
        &task.id,
        None,
        crate::workflow::default_roles::REVIEWER,
        ExecutionStatus::Failed,
        Some("review-session"),
        "2026-05-02T10:00:00Z",
    )
    .await;
    let review = seed_failed_review(
        &db,
        &task.id,
        &execution.id,
        1,
        json!({
            "ci_steps": [],
            "execution": {
                "id": execution.id,
                "status": "failed",
                "error": "reviewer exited before verdict"
            }
        }),
    )
    .await;
    let mut remaining_retries = std::collections::HashMap::new();
    remaining_retries.insert(crate::workflow::default_states::REVIEW.to_owned(), 2);

    let exception = crate::task_diagnostics::derive_workflow_exception(
        &task,
        &crate::workflow::default_workflow::default_workflow(),
        Some(&review),
        Some(&execution),
        &remaining_retries,
    )
    .expect("workflow exception derives");

    let action_kinds = exception
        .actions
        .iter()
        .map(|action| {
            serde_json::to_value(action.kind)
                .expect("kind serializes")
                .as_str()
                .expect("kind serializes as string")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(action_kinds, vec!["retry_hook", "mark_reviewed"]);
    assert_eq!(exception.actions[1].label, "Pass Review");
}
