use super::super::*;

#[tokio::test]
async fn subtask_helpers_resolve_root_and_subtask() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let subtask = seed_subtask_with_status(&db, &root, "child", TaskLifecycleState::Ready, 0).await;
    let root_id = root.id.clone();

    assert!(is_root_task(&db, &root.id).await.expect("root checks"));
    assert!(!is_subtask(&db, &root.id).await.expect("subtask checks"));
    assert!(!is_root_task(&db, &subtask.id).await.expect("root checks"));
    assert!(is_subtask(&db, &subtask.id).await.expect("subtask checks"));
    let resolved_root = root_for(&db, &subtask.id).await.expect("root resolves");
    assert_eq!(resolved_root.id.as_str(), root_id.as_str());
    let resolved_self = root_for(&db, &root.id).await.expect("root resolves");
    assert_eq!(resolved_self.id.as_str(), root_id.as_str());
}

#[tokio::test]
async fn create_task_assigns_subtask_order_and_rejects_nested_subtask() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, _repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = service
        .create_task(
            project_id.clone(),
            "Root",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("root task creates");

    let subtask = service
        .create_task(
            project_id.clone(),
            "Child",
            None,
            Some(root.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("subtask creates");

    assert_eq!(subtask.parent_task_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(subtask.subtask_order, Some(0));

    let before_rejected_create: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("Task count before nested create");
    let result = service
        .create_task(
            project_id.clone(),
            "Grandchild",
            None,
            Some(subtask.id),
            None,
            None,
            None,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(ServiceError::NestedSubtaskUnsupported)
    ));
    let after_rejected_create: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("Task count after nested create");
    assert_eq!(after_rejected_create, before_rejected_create);
}

#[tokio::test]
async fn create_task_rejects_foreign_and_missing_parents_before_writing() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_a, _repo_a, _) = seed_project_repo(&db).await;
    let (project_b, _repo_b, _) = seed_project_repo(&db).await;
    let parent = service
        .create_task(
            project_a,
            "Foreign parent",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("parent creates");
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project_b)
        .fetch_one(db.pool())
        .await
        .expect("Project B Task count");

    for parent_id in [parent.id.as_str(), "missing-parent"] {
        let result = service
            .create_task(
                project_b.clone(),
                "Rejected child",
                None,
                Some(parent_id.to_owned()),
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(matches!(
            result,
            Err(ServiceError::NotFound { entity: "task", .. })
        ));
    }

    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project_b)
        .fetch_one(db.pool())
        .await
        .expect("Project B Task count");
    assert_eq!(
        after, before,
        "failed parent validation leaves no child row"
    );
}

#[tokio::test]
async fn update_task_parent_rejects_foreign_self_nested_and_cyclic_links_atomically() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let (other_project_id, _, _) = seed_project_repo(&db).await;
    let root_a = service
        .create_task(
            project_id.clone(),
            "Root A",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Root A creates");
    let root_b = service
        .create_task(
            project_id.clone(),
            "Root B",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Root B creates");
    let child_a = service
        .create_task(
            project_id.clone(),
            "Child A",
            None,
            Some(root_a.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Child A creates");
    let child_a = service
        .update_task(task_update(&child_a, None))
        .await
        .expect("ordinary update preserves the existing parent link");
    assert_eq!(child_a.parent_task_id.as_deref(), Some(root_a.id.as_str()));
    assert_eq!(child_a.subtask_order, Some(0));
    let root_c = service
        .create_task(
            project_id.clone(),
            "Root C",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Root C creates");
    let foreign_parent = service
        .create_task(
            other_project_id,
            "Foreign root",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Foreign root creates");

    let valid_reparent = service
        .update_task(task_update(&root_c, Some(Some(root_b.id.clone()))))
        .await
        .expect("childless root reparents under a same-Project root");
    assert_eq!(
        valid_reparent.parent_task_id.as_deref(),
        Some(root_b.id.as_str())
    );
    assert_eq!(valid_reparent.subtask_order, Some(0));

    let cases = [
        (&root_b, foreign_parent.id.clone(), "foreign Project parent"),
        (&root_b, "missing-parent".to_owned(), "missing parent"),
        (&root_a, root_a.id.clone(), "self parent"),
        (
            &root_b,
            valid_reparent.id.clone(),
            "cycle through a descendant",
        ),
        (&root_b, child_a.id.clone(), "nested subtask parent"),
        (
            &root_a,
            root_b.id.clone(),
            "Task with children cannot become a subtask",
        ),
    ];
    for (task, parent_id, description) in cases {
        let before = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task read")
            .expect("Task exists");
        let result = service
            .update_task(task_update(&before, Some(Some(parent_id))))
            .await;
        assert!(result.is_err(), "{description} must be rejected");
        let after = TaskRepo::get_by_id(&*db, &task.id, false)
            .await
            .expect("Task read")
            .expect("Task remains");
        assert_eq!(after.project_id, before.project_id, "{description}");
        assert_eq!(after.repo_id, before.repo_id, "{description}");
        assert_eq!(after.parent_task_id, before.parent_task_id, "{description}");
        assert_eq!(after.subtask_order, before.subtask_order, "{description}");
        assert_eq!(after.version, before.version, "{description}");
    }

    let cleared = service
        .update_task(task_update(&valid_reparent, Some(None)))
        .await
        .expect("clearing an existing parent is supported");
    assert!(cleared.parent_task_id.is_none());
    assert!(cleared.subtask_order.is_none());
}

fn task_update(task: &Task, parent_task_id: Option<Option<String>>) -> db::UpdateTask {
    db::UpdateTask {
        id: task.id.clone(),
        expected_version: task.version,
        title: None,
        description: None,
        priority: None,
        merge_config: None,
        error_annotation: None,
        blocked_json: None,
        failed_json: None,
        parent_task_id,
        updated_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn parent_mutations_keep_parent_and_subtask_order_coherent() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent_a = create_root(&service, &project_id, "Parent A").await;
    let parent_b = create_root(&service, &project_id, "Parent B").await;
    let children_a = service
        .create_subtasks(
            parent_a.id.clone(),
            vec![subtask_input("A1"), subtask_input("A2")],
        )
        .await
        .expect("Parent A children create");
    let children_b = service
        .create_subtasks(
            parent_b.id.clone(),
            vec![subtask_input("B1"), subtask_input("B2")],
        )
        .await
        .expect("Parent B children create");
    assert_eq!(children_a[0].subtask_order, Some(0));
    assert_eq!(children_a[1].subtask_order, Some(1));
    assert_eq!(children_b[0].subtask_order, Some(0));
    assert_eq!(children_b[1].subtask_order, Some(1));
    for child in children_a.iter().chain(&children_b) {
        assert!(
            db::TaskLifecycleRepo::get_task_lifecycle(&*db, &child.id)
                .await
                .expect("subtask lifecycle lookup succeeds")
                .is_some(),
            "canonical subtask creation commits TaskLifecycle"
        );
    }

    let root_x = create_root(&service, &project_id, "Root X").await;
    let attached = service
        .update_task(task_update(&root_x, Some(Some(parent_a.id.clone()))))
        .await
        .expect("Root X attaches to Parent A");
    assert_eq!(
        attached.parent_task_id.as_deref(),
        Some(parent_a.id.as_str())
    );
    assert_eq!(attached.subtask_order, Some(2));

    let reparented = service
        .update_task(task_update(&attached, Some(Some(parent_b.id.clone()))))
        .await
        .expect("Root X reparents to Parent B");
    assert_eq!(
        reparented.parent_task_id.as_deref(),
        Some(parent_b.id.as_str())
    );
    assert_eq!(reparented.subtask_order, Some(2));

    let detached = service
        .update_task(task_update(&reparented, Some(None)))
        .await
        .expect("Root X detaches");
    assert!(detached.parent_task_id.is_none());
    assert!(detached.subtask_order.is_none());
}

#[tokio::test]
async fn create_subtasks_batch_rolls_back_all_rows_after_a_mid_batch_failure() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_id, _) = seed_project_repo(&db).await;
    let parent = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    sqlx::query(
        "CREATE TRIGGER reject_selected_subtask BEFORE INSERT ON task WHEN NEW.title = 'Reject middle child' BEGIN SELECT RAISE(ABORT, 'injected batch failure'); END",
    )
    .execute(db.pool())
    .await
    .expect("test trigger creates");

    let result = service
        .create_subtasks(
            parent.id.clone(),
            vec![
                subtask_input("First child"),
                subtask_input("Reject middle child"),
                subtask_input("Last child"),
            ],
        )
        .await;
    assert!(result.is_err(), "the injected middle insert must fail");
    let child_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
    )
    .bind(&parent.id)
    .fetch_one(db.pool())
    .await
    .expect("child count loads");
    assert_eq!(child_count, 0, "the batch leaves no partial children");
}

#[tokio::test]
async fn ordinary_task_update_does_not_rewrite_parent_structure() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "Parent").await;
    let child = service
        .create_task(
            project_id,
            "Child",
            None,
            Some(parent.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("child creates");

    let mut update = task_update(&child, None);
    update.title = Some("Changed title".to_owned());
    let updated = service
        .update_task(update)
        .await
        .expect("ordinary update succeeds");
    assert_eq!(updated.parent_task_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(updated.subtask_order, Some(0));
}

#[tokio::test]
async fn stale_and_direct_repository_parent_updates_are_rejected_atomically() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent_a = create_root(&service, &project_id, "Parent A").await;
    let parent_b = create_root(&service, &project_id, "Parent B").await;
    let child = service
        .create_task(
            project_id,
            "Child",
            None,
            Some(parent_a.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("child creates");
    let stale_input = task_update(&child, Some(Some(parent_b.id.clone())));
    let mut ordinary_update = task_update(&child, None);
    ordinary_update.title = Some("Version two".to_owned());
    let current = service
        .update_task(ordinary_update)
        .await
        .expect("ordinary update advances version");

    let stale = service.update_task(stale_input).await;
    assert!(matches!(
        stale,
        Err(ServiceError::Db(db::DbError::VersionConflict))
    ));
    let direct = TaskRepo::update(&*db, task_update(&current, Some(None))).await;
    assert!(matches!(direct, Err(db::DbError::InvalidTransition)));

    let after = TaskRepo::get_by_id(&*db, &child.id, false)
        .await
        .expect("child loads")
        .expect("child remains");
    assert_eq!(after.project_id, current.project_id);
    assert_eq!(after.repo_id, current.repo_id);
    assert_eq!(after.parent_task_id.as_deref(), Some(parent_a.id.as_str()));
    assert_eq!(after.subtask_order, Some(0));
    assert_eq!(after.version, current.version);
}

#[tokio::test]
async fn reciprocal_reparents_serialize_without_creating_a_cycle() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _repo_dir) = seed_project_repo(&db).await;
    let task_a = create_root(&service, &project_id, "A").await;
    let task_b = create_root(&service, &project_id, "B").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let service_a = service.clone();
    let task_a_for_update = task_a.clone();
    let task_b_id = task_b.id.clone();
    let barrier_a = Arc::clone(&barrier);
    let update_a = tokio::spawn(async move {
        barrier_a.wait().await;
        service_a
            .update_task(task_update(&task_a_for_update, Some(Some(task_b_id))))
            .await
    });

    let service_b = service.clone();
    let task_b_for_update = task_b.clone();
    let task_a_id = task_a.id.clone();
    let barrier_b = Arc::clone(&barrier);
    let update_b = tokio::spawn(async move {
        barrier_b.wait().await;
        service_b
            .update_task(task_update(&task_b_for_update, Some(Some(task_a_id))))
            .await
    });
    barrier.wait().await;
    let result_a = update_a.await.expect("A update task joins");
    let result_b = update_b.await.expect("B update task joins");
    assert_ne!(result_a.is_ok(), result_b.is_ok());

    let final_a = TaskRepo::get_by_id(&*db, &task_a.id, false)
        .await
        .expect("A loads")
        .expect("A remains");
    let final_b = TaskRepo::get_by_id(&*db, &task_b.id, false)
        .await
        .expect("B loads")
        .expect("B remains");
    assert!(
        !(final_a.parent_task_id.as_deref() == Some(task_b.id.as_str())
            && final_b.parent_task_id.as_deref() == Some(task_a.id.as_str()))
    );
}

#[tokio::test]
async fn create_child_serializes_with_parent_reparent() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _repo_dir) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "P").await;
    let destination = create_root(&service, &project_id, "Q").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let create_service = service.clone();
    let create_project_id = project_id.clone();
    let create_parent_id = parent.id.clone();
    let create_barrier = Arc::clone(&barrier);
    let create_child = tokio::spawn(async move {
        create_barrier.wait().await;
        create_service
            .create_task(
                create_project_id,
                "C",
                None,
                Some(create_parent_id),
                None,
                None,
                None,
                None,
            )
            .await
    });

    let move_service = service.clone();
    let move_parent = parent.clone();
    let destination_id = destination.id.clone();
    let move_barrier = Arc::clone(&barrier);
    let move_parent_task = tokio::spawn(async move {
        move_barrier.wait().await;
        move_service
            .update_task(task_update(&move_parent, Some(Some(destination_id))))
            .await
    });
    barrier.wait().await;
    let create_result = create_child.await.expect("child create joins");
    let move_result = move_parent_task.await.expect("parent move joins");
    assert_ne!(create_result.is_ok(), move_result.is_ok());

    let final_parent = TaskRepo::get_by_id(&*db, &parent.id, false)
        .await
        .expect("P loads")
        .expect("P remains");
    let child_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
    )
    .bind(&parent.id)
    .fetch_one(db.pool())
    .await
    .expect("P child count loads");
    assert!(!(final_parent.parent_task_id.is_some() && child_count != 0));
}

#[tokio::test]
async fn create_subtasks_serializes_with_parent_reparent() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _repo_dir) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "P").await;
    let destination = create_root(&service, &project_id, "Q").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let batch_service = service.clone();
    let batch_parent_id = parent.id.clone();
    let batch_barrier = Arc::clone(&barrier);
    let create_batch = tokio::spawn(async move {
        batch_barrier.wait().await;
        batch_service
            .create_subtasks(
                batch_parent_id,
                vec![
                    subtask_input("Batch child 1"),
                    subtask_input("Batch child 2"),
                ],
            )
            .await
    });

    let move_service = service.clone();
    let move_parent = parent.clone();
    let destination_id = destination.id.clone();
    let move_barrier = Arc::clone(&barrier);
    let move_parent_task = tokio::spawn(async move {
        move_barrier.wait().await;
        move_service
            .update_task(task_update(&move_parent, Some(Some(destination_id))))
            .await
    });
    barrier.wait().await;
    let batch_result = create_batch.await.expect("batch create joins");
    let move_result = move_parent_task.await.expect("parent move joins");
    assert_ne!(batch_result.is_ok(), move_result.is_ok());

    let final_parent = TaskRepo::get_by_id(&*db, &parent.id, false)
        .await
        .expect("P loads")
        .expect("P remains");
    let child_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task WHERE parent_task_id = ? AND deleted_at IS NULL",
    )
    .bind(&parent.id)
    .fetch_one(db.pool())
    .await
    .expect("P child count loads");
    assert!(!(final_parent.parent_task_id.is_some() && child_count != 0));
    if batch_result.is_err() {
        assert_eq!(child_count, 0, "a rejected batch leaves no partial rows");
    } else {
        assert_eq!(child_count, 2, "the accepted batch is complete");
    }
}

async fn create_root(service: &TaskService, project_id: &str, title: &str) -> Task {
    service
        .create_task(
            project_id.to_owned(),
            title.to_owned(),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("root creates")
}

fn subtask_input(title: &str) -> NewSubtaskInput {
    NewSubtaskInput {
        title: title.to_owned(),
        description: None,
    }
}

async fn sqlite_file_db() -> (Arc<SqliteDb>, TempDir) {
    let database_dir = TempDir::new().expect("database temp dir creates");
    let database_url = format!(
        "sqlite://{}",
        database_dir.path().join("tasks.db").display()
    );
    let pool = create_sqlite_pool(&database_url)
        .await
        .expect("file-backed SQLite pool creates");
    run_migrations(&pool).await.expect("migrations run");
    let db = SqliteDb::new(pool);
    seed_test_account(&db).await;
    (Arc::new(db), database_dir)
}

#[tokio::test]
async fn soft_delete_rejects_parent_with_live_children_without_partial_writes() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "Parent").await;
    let child = service
        .create_task(
            project_id,
            "Child",
            None,
            Some(parent.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("child creates");

    let result = service.soft_delete(parent.id.clone()).await;
    assert!(
        matches!(result, Err(ServiceError::InvalidOperation { .. })),
        "a parent with live children is rejected: {result:?}"
    );

    let after_parent = TaskRepo::get_by_id(&*db, &parent.id, true)
        .await
        .expect("parent query succeeds")
        .expect("parent remains");
    let after_child = TaskRepo::get_by_id(&*db, &child.id, true)
        .await
        .expect("child query succeeds")
        .expect("child remains");
    assert_eq!(after_parent.deleted_at, parent.deleted_at);
    assert_eq!(after_parent.version, parent.version);
    assert_eq!(after_parent.updated_at, parent.updated_at);
    assert_eq!(after_child, child, "rejected delete leaves child unchanged");
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn soft_delete_leaf_keeps_its_parent_live() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "Parent").await;
    let child = service
        .create_task(
            project_id,
            "Child",
            None,
            Some(parent.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("child creates");

    let deleted_child = service
        .soft_delete(child.id.clone())
        .await
        .expect("leaf deletion succeeds");
    assert!(deleted_child.deleted_at.is_some());
    assert_eq!(
        deleted_child.parent_task_id.as_deref(),
        Some(parent.id.as_str())
    );
    assert_eq!(deleted_child.subtask_order, Some(0));
    let live_parent = TaskRepo::get_by_id(&*db, &parent.id, false)
        .await
        .expect("parent query succeeds")
        .expect("parent remains live");
    assert_eq!(live_parent.version, parent.version);
    assert_eq!(live_parent.deleted_at, None);
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn create_child_serializes_with_parent_soft_delete() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "P").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let create_service = service.clone();
    let create_barrier = Arc::clone(&barrier);
    let create_parent_id = parent.id.clone();
    let create_project_id = project_id.clone();
    let create_child = tokio::spawn(async move {
        create_barrier.wait().await;
        create_service
            .create_task(
                create_project_id,
                "C",
                None,
                Some(create_parent_id),
                None,
                None,
                None,
                None,
            )
            .await
    });

    let delete_service = service.clone();
    let delete_barrier = Arc::clone(&barrier);
    let delete_parent_id = parent.id.clone();
    let delete_parent = tokio::spawn(async move {
        delete_barrier.wait().await;
        delete_service.soft_delete(delete_parent_id).await
    });
    barrier.wait().await;

    let create_result = create_child.await.expect("child create joins");
    let delete_result = delete_parent.await.expect("parent delete joins");
    match create_result {
        Ok(child) => {
            eprintln!("observed race outcome: child create committed; parent delete was rejected");
            assert!(
                delete_result.is_err(),
                "parent delete loses to child create"
            );
            let live_parent = TaskRepo::get_by_id(&*db, &parent.id, false)
                .await
                .expect("parent query succeeds")
                .expect("parent remains live");
            assert_eq!(child.parent_task_id.as_deref(), Some(parent.id.as_str()));
            assert!(live_parent.deleted_at.is_none());
            assert_eq!(live_parent.version, parent.version);
        }
        Err(create_error) => {
            eprintln!("observed race outcome: parent delete committed; child create was rejected");
            assert!(
                matches!(&create_error, ServiceError::NotFound { entity: "task", .. }),
                "create loses only because parent is no longer live: {create_error:?}"
            );
            let deleted_parent = delete_result.expect("parent delete wins");
            assert!(deleted_parent.deleted_at.is_some());
            let child_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE parent_task_id = ?")
                    .bind(&parent.id)
                    .fetch_one(db.pool())
                    .await
                    .expect("child count loads");
            assert_eq!(child_count, 0, "losing create leaves no child row");
        }
    }
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn create_subtasks_batch_serializes_with_parent_soft_delete() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "P").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let batch_service = service.clone();
    let batch_barrier = Arc::clone(&barrier);
    let batch_parent_id = parent.id.clone();
    let create_batch = tokio::spawn(async move {
        batch_barrier.wait().await;
        batch_service
            .create_subtasks(
                batch_parent_id,
                vec![subtask_input("C1"), subtask_input("C2")],
            )
            .await
    });

    let delete_service = service.clone();
    let delete_barrier = Arc::clone(&barrier);
    let delete_parent_id = parent.id.clone();
    let delete_parent = tokio::spawn(async move {
        delete_barrier.wait().await;
        delete_service.soft_delete(delete_parent_id).await
    });
    barrier.wait().await;

    let batch_result = create_batch.await.expect("batch create joins");
    let delete_result = delete_parent.await.expect("parent delete joins");
    match batch_result {
        Ok(children) => {
            eprintln!("observed race outcome: batch committed; parent delete was rejected");
            assert_eq!(children.len(), 2, "the batch is complete");
            assert!(delete_result.is_err(), "parent delete loses to the batch");
            assert_eq!(children[0].subtask_order, Some(0));
            assert_eq!(children[1].subtask_order, Some(1));
            let live_parent = TaskRepo::get_by_id(&*db, &parent.id, false)
                .await
                .expect("parent query succeeds")
                .expect("parent remains live");
            assert!(live_parent.deleted_at.is_none());
            assert_eq!(live_parent.version, parent.version);
        }
        Err(batch_error) => {
            eprintln!("observed race outcome: parent delete committed; batch was rejected");
            assert!(
                matches!(&batch_error, ServiceError::NotFound { entity: "task", .. }),
                "batch loses only because parent is no longer live: {batch_error:?}"
            );
            let deleted_parent = delete_result.expect("parent delete wins");
            assert!(deleted_parent.deleted_at.is_some());
            let child_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE parent_task_id = ?")
                    .bind(&parent.id)
                    .fetch_one(db.pool())
                    .await
                    .expect("child count loads");
            assert_eq!(child_count, 0, "losing batch leaves no partial children");
        }
    }
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn reparent_serializes_with_destination_parent_soft_delete() {
    let (db, _database_dir) = sqlite_file_db().await;
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let child = create_root(&service, &project_id, "C").await;
    let parent = create_root(&service, &project_id, "P").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(3));

    let move_service = service.clone();
    let move_child = child.clone();
    let move_parent_id = parent.id.clone();
    let move_barrier = Arc::clone(&barrier);
    let reparent = tokio::spawn(async move {
        move_barrier.wait().await;
        move_service
            .update_task(task_update(&move_child, Some(Some(move_parent_id))))
            .await
    });

    let delete_service = service.clone();
    let delete_parent_id = parent.id.clone();
    let delete_barrier = Arc::clone(&barrier);
    let delete_parent = tokio::spawn(async move {
        delete_barrier.wait().await;
        delete_service.soft_delete(delete_parent_id).await
    });
    barrier.wait().await;

    let reparent_result = reparent.await.expect("reparent joins");
    let delete_result = delete_parent.await.expect("parent delete joins");
    let final_child = TaskRepo::get_by_id(&*db, &child.id, false)
        .await
        .expect("child query succeeds")
        .expect("child remains live");
    let final_parent = TaskRepo::get_by_id(&*db, &parent.id, true)
        .await
        .expect("parent query succeeds")
        .expect("parent row remains");
    match reparent_result {
        Ok(moved) => {
            eprintln!("observed race outcome: reparent committed; destination delete was rejected");
            assert!(delete_result.is_err(), "delete loses to reparent");
            assert_eq!(moved.parent_task_id.as_deref(), Some(parent.id.as_str()));
            assert!(final_parent.deleted_at.is_none());
            assert_eq!(final_parent.version, parent.version);
        }
        Err(reparent_error) => {
            eprintln!("observed race outcome: destination delete committed; reparent was rejected");
            assert!(delete_result.is_ok(), "parent delete wins the race");
            assert!(
                matches!(
                    &reparent_error,
                    ServiceError::NotFound { entity: "task", .. }
                ),
                "reparent loses because the destination is no longer live: {reparent_error:?}"
            );
            assert!(final_parent.deleted_at.is_some());
            assert!(final_child.parent_task_id.is_none());
            assert!(final_child.subtask_order.is_none());
            assert_eq!(final_child.version, child.version);
        }
    }
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn reparent_requires_the_same_exact_repo() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, repo_a_id, _) = seed_project_repo(&db).await;
    let now = now_rfc3339();
    let repo_b = db::RepoRepo::create(
        &*db,
        db::CreateRepo {
            id: db::new_uuid_v4(),
            project_id: project_id.clone(),
            name: "repo-b".to_owned(),
            remote_url: "https://example.invalid/repo-b".to_owned(),
            local_path: None,
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("second Repo creates");
    let child = create_root(&service, &project_id, "A in Repo A").await;
    assert_eq!(child.repo_id.as_deref(), Some(repo_a_id.as_str()));
    let parent_b = TaskRepo::create(
        &*db,
        CreateTask {
            id: db::new_uuid_v4(),
            project_id: project_id.clone(),
            repo_id: Some(repo_b.id.clone()),
            parent_task_id: None,
            subtask_order: None,
            title: "B in Repo B".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Repo B parent creates");

    let rejected = service
        .update_task(task_update(&child, Some(Some(parent_b.id.clone()))))
        .await;
    assert!(
        matches!(rejected, Err(ServiceError::InvalidOperation { .. })),
        "cross-Repo reparent is rejected: {rejected:?}"
    );
    let after_rejected = TaskRepo::get_by_id(&*db, &child.id, false)
        .await
        .expect("child query succeeds")
        .expect("child remains");
    assert_eq!(after_rejected.repo_id, child.repo_id);
    assert_eq!(after_rejected.parent_task_id, child.parent_task_id);
    assert_eq!(after_rejected.subtask_order, child.subtask_order);
    assert_eq!(after_rejected.version, child.version);

    let parent_a = create_root(&service, &project_id, "Parent in Repo A").await;
    let attached = service
        .update_task(task_update(
            &after_rejected,
            Some(Some(parent_a.id.clone())),
        ))
        .await
        .expect("same-Repo reparent succeeds");
    assert_eq!(attached.repo_id, child.repo_id);
    assert_eq!(
        attached.parent_task_id.as_deref(),
        Some(parent_a.id.as_str())
    );
    assert_eq!(attached.subtask_order, Some(0));
    assert_parent_structure_is_clean(&db).await;
}

#[tokio::test]
async fn deleting_parent_after_detach_preserves_a_clean_root() {
    let db = Arc::new(sqlite_db().await);
    let service = TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(16)));
    let (project_id, _, _) = seed_project_repo(&db).await;
    let parent = create_root(&service, &project_id, "Parent").await;
    let child = service
        .create_task(
            project_id,
            "Child",
            None,
            Some(parent.id.clone()),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("child creates");
    let detached = service
        .update_task(task_update(&child, Some(None)))
        .await
        .expect("child detaches");
    let deleted_parent = service
        .soft_delete(parent.id.clone())
        .await
        .expect("parent without live children deletes");
    assert!(deleted_parent.deleted_at.is_some());
    assert!(detached.parent_task_id.is_none());
    assert!(detached.subtask_order.is_none());
    assert_parent_structure_is_clean(&db).await;
}

async fn assert_parent_structure_is_clean(db: &SqliteDb) {
    let live_orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM task child
         LEFT JOIN task parent ON parent.id = child.parent_task_id
         WHERE child.deleted_at IS NULL
           AND child.parent_task_id IS NOT NULL
           AND (parent.id IS NULL OR parent.deleted_at IS NOT NULL)",
    )
    .fetch_one(db.pool())
    .await
    .expect("live parent invariant query succeeds");
    let live_roots_with_order: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task
         WHERE deleted_at IS NULL
           AND parent_task_id IS NULL
           AND subtask_order IS NOT NULL",
    )
    .fetch_one(db.pool())
    .await
    .expect("root/order invariant query succeeds");
    assert_eq!(live_orphans, 0, "all live children have a live parent");
    assert_eq!(live_roots_with_order, 0, "live roots have no stale order");
}

#[tokio::test]
async fn reorder_subtasks_updates_order() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let subtasks = service
        .create_subtasks(
            root.id.clone(),
            vec![
                NewSubtaskInput {
                    title: "A".to_owned(),
                    description: None,
                },
                NewSubtaskInput {
                    title: "B".to_owned(),
                    description: None,
                },
                NewSubtaskInput {
                    title: "C".to_owned(),
                    description: None,
                },
            ],
        )
        .await
        .expect("subtasks create");
    let reordered_ids = subtasks
        .iter()
        .rev()
        .map(|subtask| subtask.id.clone())
        .collect::<Vec<_>>();

    service
        .reorder_subtasks(root.id.clone(), reordered_ids)
        .await
        .expect("reorder succeeds");

    let reordered = TaskRepo::list_subtasks_ordered(&*db, &root.id)
        .await
        .expect("subtasks load");
    assert_eq!(reordered[0].title, "C");
    assert_eq!(reordered[1].title, "B");
    assert_eq!(reordered[2].title, "A");
}
