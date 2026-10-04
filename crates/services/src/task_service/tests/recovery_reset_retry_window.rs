use super::helpers::*;
use super::*;

#[tokio::test]
async fn retired_workflow_recovery_actions_cannot_change_task_lifecycle() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let task = seed_task_with_status(&db, &project_id, &repo_id, "todo").await;
    let before = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle loads")
        .expect("lifecycle exists");

    let retired = [
        api_types::RecoveryAction::MarkReviewed,
        api_types::RecoveryAction::RetryHook,
        api_types::RecoveryAction::ProceedOnce,
        api_types::RecoveryAction::ResetRetryWindow,
        api_types::RecoveryAction::ResumeProcess,
        api_types::RecoveryAction::UpdateWorkspaceAndRetryHook,
        api_types::RecoveryAction::SkipHookOnce,
    ];
    for action in retired {
        let error = service
            .recover_task(
                task.id.clone(),
                action,
                Some("legacy request".to_owned()),
                None,
            )
            .await
            .expect_err("legacy workflow recovery is retired");
        assert!(error
            .to_string()
            .contains("legacy workflow recovery actions are retired"));
    }

    let current = TaskRepo::get_by_id(&*db, &task.id, false)
        .await
        .expect("Task reloads")
        .expect("Task exists");
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*db, &task.id)
        .await
        .expect("lifecycle reloads")
        .expect("lifecycle exists");
    assert_eq!(current.status, task.status);
    assert_eq!(lifecycle.state, before.state);
    assert_eq!(lifecycle.version, before.version);
    assert!(ExecutionRepo::list_by_task(
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
    .expect("execution list loads")
    .items
    .is_empty());
}
