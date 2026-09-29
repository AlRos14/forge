use crate::task_integration_operation::TaskIntegrationOperationManager;
use crate::{Result, ServiceError};
use async_trait::async_trait;
use db::{
    now_rfc3339, RepoRepo, SqliteDb, WorkUnitRepo, WorkUnitWorkspaceRepo, WorkspaceRepo,
    WorkspaceScopeKind,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{interval, timeout},
};
use tracing::info;
use workspace::RepoCacheLockManager;
use workspace::{WorkspaceError, WorkspaceManager};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const TICK_INTERVAL: Duration = Duration::from_secs(60);

pub struct WorkspaceCleanupScheduler {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    workspace_root: PathBuf,
    repo_cache_locks: RwLock<Option<Arc<RepoCacheLockManager>>>,
    workspace_exec_locks:
        RwLock<Option<Arc<crate::workspace_execution_lock::WorkspaceExecutionLockManager>>>,
    terminal_cleanup: RwLock<Option<Arc<dyn WorkspaceCleanupObserver>>>,
}

#[async_trait]
pub trait WorkspaceCleanupObserver: Send + Sync {
    async fn cleanup_workspace_terminals(&self, workspace_id: &str) -> Result<()>;
}

impl WorkspaceCleanupScheduler {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, workspace_root: PathBuf) -> Self {
        Self {
            db,
            event_bus,
            workspace_root,
            repo_cache_locks: RwLock::new(None),
            workspace_exec_locks: RwLock::new(None),
            terminal_cleanup: RwLock::new(None),
        }
    }

    pub fn with_repo_cache_locks(self, locks: Arc<RepoCacheLockManager>) -> Self {
        if let Ok(mut configured) = self.repo_cache_locks.write() {
            *configured = Some(locks);
        }
        self
    }

    pub fn set_repo_cache_locks(&self, locks: Arc<RepoCacheLockManager>) {
        if let Ok(mut configured) = self.repo_cache_locks.write() {
            *configured = Some(locks);
        }
    }

    pub fn set_workspace_exec_locks(
        &self,
        locks: Arc<crate::workspace_execution_lock::WorkspaceExecutionLockManager>,
    ) {
        if let Ok(mut configured) = self.workspace_exec_locks.write() {
            *configured = Some(locks);
        }
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn set_terminal_cleanup_handler(&self, handler: Arc<dyn WorkspaceCleanupObserver>) {
        match self.terminal_cleanup.write() {
            Ok(mut terminal_cleanup) => {
                *terminal_cleanup = Some(handler);
            }
            Err(error) => {
                tracing::warn!(%error, "workspace cleanup terminal handler lock poisoned");
            }
        }
    }

    pub fn spawn(self: Arc<Self>, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = interval(TICK_INTERVAL);

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(error) = self.tick().await {
                            tracing::warn!(%error, "workspace cleanup tick failed");
                        }
                    }
                    result = shutdown_rx.changed() => {
                        if result.is_err() || *shutdown_rx.borrow() {
                            break;
                        }
                    }
                }
            }
        })
    }

    pub async fn cleanup_now(&self, workspace_id: impl Into<String>) -> Result<()> {
        let workspace_id = workspace_id.into();
        match timeout(CLEANUP_TIMEOUT, self.cleanup_workspace(&workspace_id)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                tracing::warn!(%workspace_id, %error, "workspace cleanup failed");
                Ok(())
            }
            Err(_) => {
                tracing::warn!(%workspace_id, "workspace cleanup timed out");
                Ok(())
            }
        }
    }

    pub async fn schedule(&self, workspace_id: impl AsRef<str>, delay: Duration) -> Result<()> {
        let workspace_id = workspace_id.as_ref();
        let cleanup_after = chrono::Utc::now()
            + chrono::Duration::from_std(delay).map_err(|error| {
                crate::ServiceError::invalid_operation(format!("invalid cleanup delay: {error}"))
            })?;
        WorkspaceRepo::set_cleanup_after(
            &*self.db,
            workspace_id,
            Some(cleanup_after.to_rfc3339()),
            &now_rfc3339(),
        )
        .await?;
        info!(
            workspace_id,
            workspace_root = %self.workspace_root.display(),
            cleanup_after = %cleanup_after.to_rfc3339(),
            "workspace cleanup scheduled"
        );
        Ok(())
    }

    pub(crate) async fn tick(&self) -> Result<()> {
        let now = now_rfc3339();
        let workspaces = WorkspaceRepo::list_pending_cleanup(&*self.db, &now).await?;
        for pending in workspaces {
            let Some(workspace) = WorkspaceRepo::get_by_id(&*self.db, &pending.id).await? else {
                continue;
            };
            self.cleanup_workspace(&workspace.id).await?;
        }
        Ok(())
    }

    async fn cleanup_workspace(&self, workspace_id: &str) -> Result<()> {
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .ok_or_else(|| crate::ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, workspace_id).await?;
        let lock_key = match scope.as_ref().map(|scope| scope.kind) {
            Some(WorkspaceScopeKind::Integration) => {
                Some(format!("task-integration:{}", workspace.task_id))
            }
            Some(WorkspaceScopeKind::WorkUnit) => scope
                .as_ref()
                .and_then(|scope| scope.work_unit_id.as_ref())
                .map(|id| format!("work-unit-workspace:{id}")),
            None => None,
        };
        let locks = self
            .workspace_exec_locks
            .read()
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "Workspace execution lock configuration poisoned: {error}"
                ))
            })?
            .clone();
        let _scope_guard = match (locks.as_ref(), lock_key.as_deref()) {
            (Some(locks), Some(key)) => Some(locks.acquire(key).await),
            _ => None,
        };
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .ok_or_else(|| crate::ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        if scope
            .as_ref()
            .is_some_and(|scope| scope.kind == WorkspaceScopeKind::Integration)
            && !WorkUnitRepo::list_by_task(&*self.db, &workspace.task_id)
                .await?
                .is_empty()
        {
            WorkspaceRepo::set_cleanup_after(&*self.db, workspace_id, None, &now_rfc3339()).await?;
            return Ok(());
        }
        let _workspace_guard = match (locks.as_ref(), scope.as_ref()) {
            (Some(locks), Some(scope)) if scope.kind == WorkspaceScopeKind::WorkUnit => {
                Some(locks.acquire(workspace_id).await)
            }
            _ => None,
        };
        let running_execution: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM execution WHERE workspace_id = ? AND status = 'running')",
        )
        .bind(workspace_id)
        .fetch_one(self.db.pool())
        .await?;
        if running_execution != 0 {
            return Ok(());
        }
        let terminal_cleanup = self
            .terminal_cleanup
            .read()
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "workspace cleanup terminal handler lock poisoned: {error}"
                ))
            })?
            .clone();
        let integration_scope = scope
            .as_ref()
            .is_some_and(|scope| scope.kind == WorkspaceScopeKind::Integration);
        if integration_scope {
            if let Some(terminal_cleanup) = terminal_cleanup.as_ref() {
                terminal_cleanup
                    .cleanup_workspace_terminals(workspace_id)
                    .await?;
            }
        }
        let integration_operation = if integration_scope {
            Some(
                TaskIntegrationOperationManager::new(
                    Arc::clone(&self.db),
                    self.workspace_root.clone(),
                )
                .acquire(
                    &workspace.task_id,
                    db::TaskIntegrationOperationKind::IntegrationWorkspaceCleanup,
                    &workspace.id,
                )
                .await?,
            )
        } else {
            None
        };
        info!(
            workspace_id,
            task_id = %workspace.task_id,
            worktree_path = %workspace.worktree_path,
            workspace_root = %self.workspace_root.display(),
            "cleaning up workspace"
        );
        if !integration_scope {
            if let Some(terminal_cleanup) = terminal_cleanup {
                terminal_cleanup
                    .cleanup_workspace_terminals(workspace_id)
                    .await?;
            }
        }
        if let Some(scope) = scope.filter(|scope| scope.kind == WorkspaceScopeKind::WorkUnit) {
            let work_unit_id = scope.work_unit_id.ok_or_else(|| {
                ServiceError::invalid_operation("WorkUnit Workspace has no WorkUnit binding")
            })?;
            let work_unit = WorkUnitRepo::get_by_id(&*self.db, &work_unit_id)
                .await?
                .filter(|unit| unit.task_id == workspace.task_id)
                .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.clone()))?;
            let repo = RepoRepo::get_by_id(&*self.db, &workspace.repo_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("repo", workspace.repo_id.clone()))?;
            let repo_path =
                crate::task_service::workspace::resolve_repo_source(&repo, &self.workspace_root)
                    .await?;
            let mut manager = WorkspaceManager::new(self.workspace_root.clone());
            if let Some(locks) = self
                .repo_cache_locks
                .read()
                .map_err(|error| {
                    ServiceError::invalid_operation(format!(
                        "Repo cache lock configuration poisoned: {error}"
                    ))
                })?
                .as_ref()
            {
                manager = manager.with_repo_cache_locks(Arc::clone(locks));
            }
            manager
                .cleanup_work_unit_worktree(
                    &repo_path,
                    &workspace.task_id,
                    &work_unit.id,
                    &workspace.id,
                    &workspace.repo_id,
                )
                .await
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        } else {
            match crate::plan_artifact::capture_plan_revision(
                &self.db,
                &workspace.task_id,
                Path::new(&workspace.worktree_path),
                "final",
                None,
            ).await {
                Ok(_) | Err(crate::plan_artifact::PlanArtifactError::NotFound) => {}
                Err(error) => return Err(ServiceError::invalid_operation(format!(
                    "workspace cleanup blocked because the final plan could not be persisted: {error}"
                ))),
            }
            let manager = WorkspaceManager::new(self.workspace_root.clone());
            match manager.cleanup_worktree(&workspace.task_id).await {
                Ok(()) | Err(WorkspaceError::NotFound) => {}
                Err(error) => {
                    return Err(crate::ServiceError::invalid_operation(error.to_string()))
                }
            }
        }
        let now = now_rfc3339();
        let workspace = WorkspaceRepo::mark_cleaned(&*self.db, workspace_id, &now).await?;
        info!(
            workspace_id = %workspace.id,
            "workspace cleaned"
        );
        self.event_bus.publish(ForgeEvent {
            event_type: "workspace.cleaned".to_owned(),
            entity_id: workspace.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::WorkspaceCleaned {
                workspace_id: workspace.id,
                task_id: workspace.task_id,
                status: "cleaned".to_owned(),
            },
        });
        if let Some(operation) = integration_operation {
            operation
                .finish(db::TaskIntegrationOperationStatus::Succeeded)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, new_uuid_v4, run_migrations, CreateProject, CreateRepo, CreateTask,
        CreateWorkspace, ProjectRepo, RepoRepo, TaskRepo, UpdateProject, WorkspaceStatus,
    };
    use tempfile::TempDir;

    async fn sqlite_db() -> Arc<SqliteDb> {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        Arc::new(SqliteDb::new(pool))
    }

    async fn seed_workspace(
        db: &SqliteDb,
        workspace_root: &std::path::Path,
        status: WorkspaceStatus,
    ) -> (String, PathBuf) {
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let workspace_id = new_uuid_v4();
        let worktree_path = workspace_root.join(&task_id).join("repo");
        let branch = workspace::task_branch_name(&task_id);

        ProjectRepo::create(
            db,
            CreateProject {
                id: project_id.clone(),
                name: "Forge".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
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
                name: "repo".to_owned(),
                remote_url: "https://example.com/repo.git".to_owned(),
                local_path: None,
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
        TaskRepo::create(
            db,
            CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: Some(repo_id.clone()),
                parent_task_id: None,
                subtask_order: None,
                assignee_type: None,
                assignee_id: None,
                title: "Cleanup task".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "done".to_owned(),
                is_automation: false,
                priority: 0,
                task_state_config: None,
                merge_config: None,
                plan: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task creates");
        WorkspaceRepo::create(
            db,
            CreateWorkspace {
                id: workspace_id.clone(),
                task_id,
                repo_id,
                worktree_path: worktree_path.to_string_lossy().into_owned(),
                branch,
                status,
                before_sha: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("workspace creates");
        std::fs::create_dir_all(&worktree_path).expect("worktree creates");

        (workspace_id, worktree_path)
    }

    #[tokio::test]
    async fn cleanup_now_removes_worktree_and_marks_cleaned() {
        let db = sqlite_db().await;
        let event_bus = Arc::new(EventBus::new(16));
        let temp = TempDir::new().expect("temp dir creates");
        let (workspace_id, worktree_path) =
            seed_workspace(&db, temp.path(), WorkspaceStatus::Ready).await;
        let scheduler =
            WorkspaceCleanupScheduler::new(Arc::clone(&db), event_bus, temp.path().to_path_buf());

        scheduler
            .cleanup_now(workspace_id.clone())
            .await
            .expect("cleanup succeeds");

        assert!(!worktree_path.exists());
        let workspace = WorkspaceRepo::get_by_id(&*db, &workspace_id)
            .await
            .expect("workspace loads")
            .expect("workspace exists");
        assert_eq!(workspace.status, WorkspaceStatus::Cleaned);
    }

    #[tokio::test]
    async fn cleanup_marks_cleaned_when_worktree_is_missing() {
        let db = sqlite_db().await;
        let event_bus = Arc::new(EventBus::new(16));
        let temp = TempDir::new().expect("temp dir creates");
        let (workspace_id, worktree_path) =
            seed_workspace(&db, temp.path(), WorkspaceStatus::Ready).await;
        std::fs::remove_dir_all(worktree_path.parent().expect("task root exists"))
            .expect("worktree removes");
        let scheduler =
            WorkspaceCleanupScheduler::new(Arc::clone(&db), event_bus, temp.path().to_path_buf());

        scheduler
            .cleanup_now(workspace_id.clone())
            .await
            .expect("cleanup succeeds");

        let workspace = WorkspaceRepo::get_by_id(&*db, &workspace_id)
            .await
            .expect("workspace loads")
            .expect("workspace exists");
        assert_eq!(workspace.status, WorkspaceStatus::Cleaned);
        assert!(workspace.cleanup_after.is_none());
    }

    #[tokio::test]
    async fn cleanup_persists_final_plan_before_removing_workspace() {
        let db = sqlite_db().await;
        let event_bus = Arc::new(EventBus::new(16));
        let temp = TempDir::new().expect("temp dir creates");
        let (workspace_id, worktree_path) =
            seed_workspace(&db, temp.path(), WorkspaceStatus::Ready).await;
        let plan_path = worktree_path
            .parent()
            .expect("task root exists")
            .join("plan.md");
        std::fs::write(
            &plan_path,
            "# Final plan\n\n- [x] Implement\n- [x] Verify\n",
        )
        .expect("plan writes");
        let task_id: String = sqlx::query_scalar("SELECT task_id FROM workspace WHERE id = ?")
            .bind(&workspace_id)
            .fetch_one(db.pool())
            .await
            .expect("workspace task loads");
        let scheduler =
            WorkspaceCleanupScheduler::new(Arc::clone(&db), event_bus, temp.path().to_path_buf());

        scheduler
            .cleanup_now(workspace_id)
            .await
            .expect("cleanup succeeds");

        assert!(!plan_path.exists());
        let persisted: (String, String) = sqlx::query_as(
            "SELECT checkpoint, markdown FROM task_plan_revision WHERE task_id = ? ORDER BY revision DESC LIMIT 1",
        )
        .bind(task_id)
        .fetch_one(db.pool())
        .await
        .expect("final plan revision persists");
        assert_eq!(persisted.0, "final");
        assert!(persisted.1.contains("- [x] Verify"));
    }

    #[tokio::test]
    async fn schedule_writes_cleanup_after() {
        let db = sqlite_db().await;
        let event_bus = Arc::new(EventBus::new(16));
        let temp = TempDir::new().expect("temp dir creates");
        let (workspace_id, _) = seed_workspace(&db, temp.path(), WorkspaceStatus::Ready).await;
        let scheduler =
            WorkspaceCleanupScheduler::new(Arc::clone(&db), event_bus, temp.path().to_path_buf());

        scheduler
            .schedule(&workspace_id, Duration::from_secs(60))
            .await
            .expect("cleanup schedules");

        let workspace = WorkspaceRepo::get_by_id(&*db, &workspace_id)
            .await
            .expect("workspace loads")
            .expect("workspace exists");
        assert!(workspace.cleanup_after.is_some());
    }

    #[tokio::test]
    async fn loop_replays_past_due_workspaces() {
        let db = sqlite_db().await;
        let event_bus = Arc::new(EventBus::new(16));
        let temp = TempDir::new().expect("temp dir creates");
        let (workspace_id, _) = seed_workspace(&db, temp.path(), WorkspaceStatus::Ready).await;
        let cleanup_after = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        WorkspaceRepo::set_cleanup_after(&*db, &workspace_id, Some(cleanup_after), &now_rfc3339())
            .await
            .expect("cleanup deadline sets");
        let scheduler =
            WorkspaceCleanupScheduler::new(Arc::clone(&db), event_bus, temp.path().to_path_buf());

        scheduler.tick().await.expect("tick succeeds");

        let workspace = WorkspaceRepo::get_by_id(&*db, &workspace_id)
            .await
            .expect("workspace loads")
            .expect("workspace exists");
        assert_eq!(workspace.status, WorkspaceStatus::Cleaned);
    }
}
