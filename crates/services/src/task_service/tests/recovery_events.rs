use super::helpers::*;
use super::*;

#[tokio::test]
async fn test_reset_retry_window_publishes_recovery_and_resume_events() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
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
    seed_failed_review(
        &db,
        &task.id,
        &execution.id,
        1,
        json!({ "ci_steps": [{"command": "cargo test", "exit_code": 1}] }),
    )
    .await;
    seed_review_rejection_log(&db, &task.id, "review failed once").await;
    seed_review_rejection_log(&db, &task.id, "review failed twice").await;
    let task = set_retry_exhausted_metadata(&db, &task).await;
    let mut rx = event_bus.subscribe();

    service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::ResetRetryWindow,
            Some("reason".to_owned()),
            None,
        )
        .await
        .expect("reset retry window succeeds");

    let mut events = Vec::new();
    while let Ok(Ok(event)) =
        tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
    {
        events.push(event);
    }

    let recovery_events = events
        .iter()
        .filter(|event| event.event_type == "task.recovery_applied")
        .collect::<Vec<_>>();
    assert_eq!(recovery_events.len(), 2);
    let event = recovery_events
        .iter()
        .find(|event| {
            matches!(
                &event.context,
                EventContext::RecoveryApplied { action, .. } if action == "reset_retry_window"
            )
        })
        .expect("reset retry window recovery event");
    assert_eq!(event.entity_id, task.id);
    match &event.context {
        EventContext::RecoveryApplied {
            project_id: event_project_id,
            task_id,
            action,
            state,
            transition_log_id,
        } => {
            assert_eq!(event_project_id, &project_id);
            assert_eq!(task_id, &task.id);
            assert_eq!(action, "reset_retry_window");
            assert_eq!(
                state.as_deref(),
                Some(crate::workflow::default_states::REVIEW)
            );
            assert!(transition_log_id.is_some());
        }
        other => panic!("unexpected event context: {other:?}"),
    }

    assert!(
        events.iter().any(|event| {
            matches!(
                &event.context,
                EventContext::RecoveryApplied { action, .. } if action == "resume_process"
            )
        }),
        "reset_retry_window should resume process and publish resume_process recovery event"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "task.status_changed"),
        "reset_retry_window should resume work and publish task.status_changed"
    );
}
