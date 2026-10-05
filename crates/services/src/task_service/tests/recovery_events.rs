use super::helpers::*;
use super::*;

#[tokio::test]
async fn clearing_blocked_metadata_does_not_publish_unblocked_while_lifecycle_is_blocked() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "blocked").await;
    sqlx::query("UPDATE task SET blocked_json = ? WHERE id = ?")
        .bind(format!(
            r#"{{"kind":"retry_exhausted","reason":"test exhaustion","created_at":"{}"}}"#,
            now_rfc3339()
        ))
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("legacy blocking metadata persists for the regression");
    let mut events = event_bus.subscribe();

    service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::CancelTask,
            Some("close the blocked Task".to_owned()),
            None,
        )
        .await
        .expect("the authorized lifecycle cancellation completes");

    assert_eq!(
        db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
            .await
            .expect("lifecycle loads")
            .expect("lifecycle exists")
            .state,
        db::TaskLifecycleState::Cancelled
    );
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(20), events.recv()).await {
            Ok(Ok(event)) => assert_ne!(
                event.event_type, "task.unblocked",
                "clearing legacy metadata cannot claim a blocked lifecycle was reopened"
            ),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }
}

async fn seed_assigned_task(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    agent_id: &str,
) -> Task {
    let now = now_rfc3339();
    TaskRepo::create(
        db,
        db::CreateTask {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: Some("agent".to_owned()),
            assignee_id: Some(agent_id.to_owned()),
            title: "assigned task".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            status: crate::workflow::default_states::IN_PROGRESS.to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task creates")
}

#[tokio::test]
async fn test_reset_to_initial_clears_assignee_after_workspace_failure() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_assigned_task(&db, &project_id, &repo_id, &agent_id).await;
    assert_eq!(task.assignee_id.as_deref(), Some(agent_id.as_str()));

    service
        .fail_task(
            task.id.clone(),
            "workspace reset required: task branch no longer exists",
            Some(api_types::FailureKind::WorkspaceFailed),
            None,
        )
        .await
        .expect("task fails");

    let recovered = service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::ResetToInitial,
            None,
            None,
        )
        .await
        .expect("task resets");
    assert_eq!(
        recovered.assignee_id, None,
        "workspace failure reset must clear the assignee"
    );
    assert!(recovered.failed_json.is_none());
}

#[tokio::test]
async fn test_reset_to_initial_keeps_assignee_for_non_workspace_failure() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_id = seed_agent(&db).await;
    let task = seed_assigned_task(&db, &project_id, &repo_id, &agent_id).await;
    assert_eq!(task.assignee_id.as_deref(), Some(agent_id.as_str()));

    service
        .fail_task(
            task.id.clone(),
            "executor crashed",
            Some(api_types::FailureKind::ExecutorFailed),
            None,
        )
        .await
        .expect("task fails");

    let recovered = service
        .recover_task(
            task.id.clone(),
            api_types::RecoveryAction::ResetToInitial,
            None,
            None,
        )
        .await
        .expect("task resets");
    assert_eq!(
        recovered.assignee_id.as_deref(),
        Some(agent_id.as_str()),
        "non-workspace failure reset keeps the assignee"
    );
}
