use super::super::*;

#[tokio::test]
async fn subtask_helpers_resolve_root_and_subtask() {
    let db = Arc::new(sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let subtask = seed_subtask_with_status(&db, &root, "child", "todo".to_owned(), 0).await;
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
            None,
        )
        .await
        .expect("subtask creates");

    assert_eq!(subtask.parent_task_id.as_deref(), Some(root.id.as_str()));
    assert_eq!(subtask.subtask_order, Some(0));

    let result = service
        .create_task(
            project_id,
            "Grandchild",
            None,
            Some(subtask.id),
            None,
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
            None,
        )
        .await
        .expect("Child A creates");
    let child_a = service
        .update_task(task_update(&child_a, None))
        .await
        .expect("ordinary update revalidates the existing parent link");
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
            None,
        )
        .await
        .expect("Root C creates");
    let child_c = service
        .create_task(
            project_id,
            "Child C",
            None,
            Some(root_c.id.clone()),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("Child C creates");
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
            None,
        )
        .await
        .expect("Foreign root creates");

    let valid_reparent = service
        .update_task(task_update(&root_a, Some(Some(root_b.id.clone()))))
        .await
        .expect("root Task reparents under a same-Project root");
    assert_eq!(
        valid_reparent.parent_task_id.as_deref(),
        Some(root_b.id.as_str())
    );

    let cases = [
        (
            &child_a,
            foreign_parent.id.clone(),
            "foreign Project parent",
        ),
        (&root_b, "missing-parent".to_owned(), "missing parent"),
        (&root_b, root_b.id.clone(), "self parent"),
        (&root_b, root_a.id.clone(), "cycle through a descendant"),
        (&root_b, child_c.id.clone(), "nested subtask parent"),
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
        task_state_config: None,
        parent_task_id,
        updated_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn create_subtasks_preserves_input_order_and_rejects_different_assignee_atomically() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let agent_a = seed_agent(&db).await;
    let _agent_b = seed_agent_with_executor_type(&db, "codex", "{}").await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    service
        .reassign_role(
            role_assignment_input(&root.id, "coder", Some(agent_a.clone()), None),
            false,
            false,
        )
        .await
        .expect("coder assignment succeeds");

    let subtasks = service
        .create_subtasks(
            root.id.clone(),
            vec![
                NewSubtaskInput {
                    title: "First".to_owned(),
                    description: None,
                    assignee_id: None,
                },
                NewSubtaskInput {
                    title: "Second".to_owned(),
                    description: Some("details".to_owned()),
                    assignee_id: None,
                },
            ],
        )
        .await
        .expect("subtasks create");

    assert_eq!(subtasks.len(), 2);
    assert_eq!(subtasks[0].subtask_order, Some(0));
    assert_eq!(subtasks[1].subtask_order, Some(1));
    assert_eq!(subtasks[0].project_id.as_str(), root.project_id.as_str());
    assert_eq!(
        subtasks[0].repo_id.as_deref().unwrap(),
        root.repo_id.as_deref().unwrap()
    );
    assert_eq!(
        subtasks[0].parent_task_id.as_deref(),
        Some(root.id.as_str())
    );
    let subtask_roles = TaskRoleAssignmentRepo::list_by_task(&*db, &subtasks[0].id)
        .await
        .expect("subtask roles load");
    assert!(subtask_roles.is_empty());

    let after = TaskRepo::list_subtasks_ordered(&*db, &root.id)
        .await
        .expect("subtasks load");
    assert_eq!(after.len(), 2);
}

#[tokio::test]
async fn reorder_subtasks_updates_order() {
    let db = Arc::new(sqlite_db().await);
    let event_bus = Arc::new(EventBus::new(16));
    let service = TaskService::new(Arc::clone(&db), event_bus);
    let (project_id, repo_id, _repo_dir) = seed_project_repo(&db).await;
    let root = seed_task_with_status(&db, &project_id, &repo_id, "todo".to_owned()).await;
    let subtasks = service
        .create_subtasks(
            root.id.clone(),
            vec![
                NewSubtaskInput {
                    title: "A".to_owned(),
                    description: None,
                    assignee_id: None,
                },
                NewSubtaskInput {
                    title: "B".to_owned(),
                    description: None,
                    assignee_id: None,
                },
                NewSubtaskInput {
                    title: "C".to_owned(),
                    description: None,
                    assignee_id: None,
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
