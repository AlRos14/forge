use super::super::*;

#[tokio::test]
async fn reorder_task_sets_midpoint_between_two_tasks() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let before = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let after = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let mut rx = event_bus.subscribe();

    let updated = service
        .reorder_task(
            target.id.clone(),
            Some(before.id.clone()),
            Some(after.id.clone()),
        )
        .await
        .expect("task reorders");

    assert_position(updated.board_position, 1.5);
    let no_event = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
    assert!(no_event.is_err(), "reorder_task must not publish an event");
}

#[tokio::test]
async fn reorder_task_with_only_before_appends_after_before() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let before = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let updated = service
        .reorder_task(target.id, Some(before.id.clone()), None)
        .await
        .expect("task reorders");

    assert_position(updated.board_position, before.board_position + 1.0);
}

#[tokio::test]
async fn duplicate_task_appends_after_existing_board_tasks() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let source = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let existing_last = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let duplicated = service
        .duplicate_task(&source.id)
        .await
        .expect("task duplicates");

    assert!(duplicated.board_position > existing_last.board_position);
    let ordered_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM task WHERE project_id = ? AND deleted_at IS NULL ORDER BY board_position ASC, created_at ASC, id ASC",
    )
    .bind(&project_id)
    .fetch_all(db.pool())
    .await
    .expect("task order loads");
    assert_eq!(ordered_ids.last(), Some(&duplicated.id));
}

#[tokio::test]
async fn reorder_task_with_only_after_moves_before_after() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let after = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let updated = service
        .reorder_task(target.id, None, Some(after.id.clone()))
        .await
        .expect("task reorders");

    assert_position(updated.board_position, after.board_position - 1.0);
}

#[tokio::test]
async fn reorder_task_with_only_after_at_zero_moves_to_distinct_lower_position() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let after = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    set_board_position(&db, &after.id, 0.0).await;
    set_board_position(&db, &target.id, 2.0).await;

    let updated = service
        .reorder_task(target.id, None, Some(after.id.clone()))
        .await
        .expect("task reorders before zero-position task");

    assert_position(updated.board_position, -1.0);
    let positions = board_positions(&db, &project_id).await;
    assert_eq!(positions, vec![-1.0, 0.0]);
}

#[tokio::test]
async fn reorder_task_rejects_both_null() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let error = service
        .reorder_task(target.id.clone(), None, None)
        .await
        .expect_err("both null rejected");

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
    let loaded = TaskRepo::get_by_id(&*db, &target.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_position(loaded.board_position, target.board_position);
}

#[tokio::test]
async fn reorder_task_rejects_neighbour_in_different_project() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let (other_project_id, other_repo_id, _other_repo_dir) = seed_project_repo(&db).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let neighbour =
        seed_task_with_status(&db, &other_project_id, &other_repo_id, "todo".to_owned()).await;

    let error = service
        .reorder_task(target.id, Some(neighbour.id), None)
        .await
        .expect_err("different project neighbour rejected");

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
}

#[tokio::test]
async fn reorder_task_rejects_same_before_and_after() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let neighbour = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let error = service
        .reorder_task(
            target.id,
            Some(neighbour.id.clone()),
            Some(neighbour.id.clone()),
        )
        .await
        .expect_err("same neighbours rejected");

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
}

#[tokio::test]
async fn reorder_task_rejects_neighbour_equal_to_task_id() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;

    let error = service
        .reorder_task(target.id.clone(), Some(target.id.clone()), None)
        .await
        .expect_err("self neighbour rejected");

    assert!(matches!(error, ServiceError::InvalidOperation { .. }));
}

#[tokio::test]
async fn reorder_task_renormalizes_when_gap_is_too_small() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let before = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let after = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let target = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    set_board_position(&db, &before.id, 1.0).await;
    set_board_position(&db, &after.id, 1.0 + 1e-12).await;
    set_board_position(&db, &target.id, 3.0).await;

    let updated = service
        .reorder_task(target.id, Some(before.id), Some(after.id))
        .await
        .expect("task reorders after renormalisation");

    assert!(updated.board_position.is_finite());
    assert_ne!(updated.board_position, 0.0);
    let positions = board_positions(&db, &project_id).await;
    assert!(positions.iter().all(|position| position.is_finite()));
    assert!(positions.iter().all(|position| *position != 0.0));
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}
