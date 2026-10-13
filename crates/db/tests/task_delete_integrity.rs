use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, CreateProject, CreateRepo,
    CreateTask, DbError, ProjectRepo, RepoRepo, SqliteDb, TaskRepo, UpdateProject, WorkMode,
};
use sqlx::SqlitePool;
use tempfile::TempDir;

#[tokio::test]
async fn repo_delete_rejection_preserves_a_same_repo_parent_and_child() {
    let (db, _database_dir) = file_db().await;
    assert_foreign_keys_enabled_on_pool_connections(db.pool()).await;
    let (project_id, repo_id) = create_project_and_repo(&db).await;
    let parent = create_task(&db, &project_id, &repo_id, None, None, "P").await;
    let child = create_task(&db, &project_id, &repo_id, Some(&parent.id), Some(0), "C").await;

    assert!(matches!(
        RepoRepo::delete(&db, &repo_id).await,
        Err(DbError::RepoInUse)
    ));
    assert!(RepoRepo::get_by_id(&db, &repo_id)
        .await
        .expect("Repo query succeeds")
        .is_some());
    let selected_repo: Option<String> =
        sqlx::query_scalar("SELECT primary_repo_id FROM project WHERE id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("Project primary Repo loads");
    assert_eq!(selected_repo.as_deref(), Some(repo_id.as_str()));

    let live_parent = TaskRepo::get_by_id(&db, &parent.id, false)
        .await
        .expect("parent query succeeds")
        .expect("parent remains live");
    let live_child = TaskRepo::get_by_id(&db, &child.id, false)
        .await
        .expect("child query succeeds")
        .expect("child remains live");
    assert_eq!(
        live_child.parent_task_id.as_deref(),
        Some(live_parent.id.as_str())
    );
    assert_eq!(live_child.subtask_order, Some(0));
    assert_no_live_parent_or_order_violations(db.pool()).await;
}

#[tokio::test]
async fn project_delete_removes_a_parent_and_child_together() {
    let (db, _database_dir) = file_db().await;
    assert_foreign_keys_enabled_on_pool_connections(db.pool()).await;
    let (project_id, repo_id) = create_project_and_repo(&db).await;
    let parent = create_task(&db, &project_id, &repo_id, None, None, "P").await;
    let child = create_task(&db, &project_id, &repo_id, Some(&parent.id), Some(0), "C").await;

    ProjectRepo::delete(&db, &project_id)
        .await
        .expect("Project delete succeeds");

    let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
        .bind(&project_id)
        .fetch_one(db.pool())
        .await
        .expect("project Task count loads");
    assert_eq!(task_count, 0);
    assert!(TaskRepo::get_by_id(&db, &parent.id, true)
        .await
        .expect("parent query succeeds")
        .is_none());
    assert!(TaskRepo::get_by_id(&db, &child.id, true)
        .await
        .expect("child query succeeds")
        .is_none());
    assert_no_live_parent_or_order_violations(db.pool()).await;
}

async fn file_db() -> (SqliteDb, TempDir) {
    let database_dir = TempDir::new().expect("database temp directory creates");
    let database_url = format!(
        "sqlite://{}",
        database_dir
            .path()
            .join("task-delete-integrity.db")
            .display()
    );
    let pool = create_sqlite_pool(&database_url)
        .await
        .expect("file-backed SQLite pool creates");
    run_migrations(&pool).await.expect("migrations run");
    (SqliteDb::new(pool), database_dir)
}

async fn assert_foreign_keys_enabled_on_pool_connections(pool: &SqlitePool) {
    let mut connections = Vec::new();
    for _ in 0..5 {
        connections.push(pool.acquire().await.expect("pool connection acquires"));
    }
    for connection in &mut connections {
        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .expect("foreign_keys pragma loads");
        assert_eq!(enabled, 1, "foreign keys are enabled per connection");
    }
}

async fn create_project_and_repo(db: &SqliteDb) -> (String, String) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "Task delete integrity".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project creates");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "repo".to_owned(),
            remote_url: "https://example.invalid/repo".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Repo creates");
    ProjectRepo::update(
        db,
        UpdateProject {
            id: project_id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("primary Repo is selected");
    (project_id, repo_id)
}

async fn create_task(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    parent_task_id: Option<&str>,
    subtask_order: Option<i64>,
    title: &str,
) -> db::Task {
    let now = now_rfc3339();
    TaskRepo::create(
        db,
        CreateTask {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: parent_task_id.map(str::to_owned),
            subtask_order,
            title: title.to_owned(),
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
    .expect("Task fixture creates")
}

async fn assert_no_live_parent_or_order_violations(pool: &SqlitePool) {
    let live_orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM task child
         LEFT JOIN task parent ON parent.id = child.parent_task_id
         WHERE child.deleted_at IS NULL
           AND child.parent_task_id IS NOT NULL
           AND (parent.id IS NULL OR parent.deleted_at IS NOT NULL)",
    )
    .fetch_one(pool)
    .await
    .expect("live parent invariant query succeeds");
    let live_roots_with_order: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task
         WHERE deleted_at IS NULL
           AND parent_task_id IS NULL
           AND subtask_order IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("root/order invariant query succeeds");
    assert_eq!(live_orphans, 0, "all live children have live parents");
    assert_eq!(live_roots_with_order, 0, "live roots have no stale order");
}
