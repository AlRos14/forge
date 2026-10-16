use super::helpers::*;
use super::*;

#[tokio::test]
async fn clearing_blocked_metadata_does_not_publish_unblocked_while_lifecycle_is_blocked() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task =
        seed_task_with_status(&db, &project_id, &repo_id, db::TaskLifecycleState::Blocked).await;
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
