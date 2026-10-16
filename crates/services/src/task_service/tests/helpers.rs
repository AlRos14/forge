use super::*;

use db::{
    create_sqlite_pool, run_migrations, CreateProject, CreateRepo, CreateTask, ProjectRepo,
    RepoRepo, TaskLifecycleState, TaskRepo, UpdateProject,
};
use tempfile::TempDir;

pub(super) async fn sqlite_db() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    SqliteDb::new(pool)
}

pub(super) async fn seed_project_repo(db: &SqliteDb) -> (String, String, TempDir) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let repo_dir = TempDir::new().expect("repo temp dir creates");
    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "forge".to_owned(),
            remote_url: repo_dir.path().to_string_lossy().into_owned(),
            local_path: Some(repo_dir.path().to_string_lossy().into_owned()),
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
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
    .expect("project primary repo updates");
    (project_id, repo_id, repo_dir)
}

pub(super) async fn seed_task_with_status(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    lifecycle_state: TaskLifecycleState,
) -> Task {
    TaskRepo::create(
        db,
        CreateTask {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            subtask_order: None,
            title: format!("task-{lifecycle_state:?}"),
            description: Some("seeded task".to_owned()),
            task_type: "implementation".to_owned(),
            lifecycle_state,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("task creates")
}
