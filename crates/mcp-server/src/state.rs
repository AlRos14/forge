use std::path::PathBuf;
use std::sync::Arc;

use db::SqliteDb;
use events::EventBus;
use services::{AgentService, TaskService, WorkUnitService, WorkspaceExecutionLockManager};
use workspace::RepoCacheLockManager;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<SqliteDb>,
    pub task_service: Arc<TaskService>,
    pub work_unit_service: Arc<WorkUnitService>,
    pub agent_service: Arc<AgentService>,
    pub event_bus: Arc<EventBus>,
}

impl AppState {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        let task_service = Arc::new(TaskService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let agent_service = Arc::new(AgentService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let workspace_root = std::env::var("FORGE_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("forge").join("worktrees"));
        let repo_cache_locks = Arc::new(RepoCacheLockManager::default());
        let workspace_locks = Arc::new(WorkspaceExecutionLockManager::default());
        let work_unit_service = Arc::new(WorkUnitService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root,
            repo_cache_locks,
            workspace_locks,
        ));
        Self::with_task_service(
            db,
            event_bus,
            task_service,
            agent_service,
            work_unit_service,
        )
    }

    pub fn with_task_service(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
        agent_service: Arc<AgentService>,
        work_unit_service: Arc<WorkUnitService>,
    ) -> Self {
        Self {
            db,
            task_service,
            work_unit_service,
            agent_service,
            event_bus,
        }
    }
}
