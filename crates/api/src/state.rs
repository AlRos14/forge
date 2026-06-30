use std::{path::PathBuf, sync::Arc};

use config::{default_config_path, ForgeConfig};
use db::SqliteDb;
use events::EventBus;
use executors::{AdapterExecutor, AdapterRegistry, TaskExecutor};
use services::{
    AgentService, AuthService, ConversationService, DaemonService, MemoryService, MergeService,
    NotificationService, OperatorStatusEmitter, OperatorStatusService, ProjectHookService,
    TaskService, TerminalActivityTracker, TerminalService, WorkspaceCleanupScheduler,
    WorkspaceExecutionLockManager,
};
use tokio::sync::watch;
use workspace::RepoCacheLockManager;

#[derive(Clone)]
pub struct ShutdownSignal {
    sender: Arc<watch::Sender<bool>>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }

    pub fn request(&self) {
        let _ = self.sender.send(true);
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.sender.subscribe()
    }

    pub async fn wait(&self) {
        let mut receiver = self.subscribe();
        if *receiver.borrow_and_update() {
            return;
        }

        while receiver.changed().await.is_ok() {
            if *receiver.borrow_and_update() {
                return;
            }
        }
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<SqliteDb>,
    pub task_service: Arc<TaskService>,
    pub agent_service: Arc<AgentService>,
    pub daemon_service: Arc<DaemonService>,
    pub daemon_connections: Arc<services::daemon_transport::DaemonConnectionRegistry>,
    pub conversation_service: Arc<ConversationService>,
    pub workflow_template_service:
        Arc<services::workflow::template_service::WorkflowTemplateService>,
    pub memory_service: Arc<MemoryService>,
    pub merge_service: Arc<MergeService>,
    pub notification_service: Arc<NotificationService>,
    pub project_hook_service: Arc<ProjectHookService>,
    pub terminal_service: Arc<TerminalService>,
    pub operator_status_service: Arc<OperatorStatusService>,
    pub operator_status_emitter: Arc<OperatorStatusEmitter>,
    pub cleanup_scheduler: Arc<WorkspaceCleanupScheduler>,
    pub review_runner: Arc<review::ReviewRunner>,
    pub adapter_registry: Arc<AdapterRegistry>,
    pub task_executor: Arc<dyn TaskExecutor>,
    pub task_dispatcher: Option<Arc<services::TaskDispatcher>>,
    pub workspace_exec_locks: Arc<WorkspaceExecutionLockManager>,
    pub repo_cache_locks: Arc<RepoCacheLockManager>,
    pub event_bus: Arc<EventBus>,
    pub shutdown_signal: ShutdownSignal,
    pub auth_service: Arc<AuthService>,
    pub oauth_service: Arc<services::OAuthService>,
    pub mcp_enabled: bool,
    pub config_path: Arc<PathBuf>,
    pub effective_config: Arc<ForgeConfig>,
}

impl AppState {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, mcp_enabled: bool) -> Self {
        Self::with_adapter_registry(db, event_bus, mcp_enabled, Arc::new(AdapterRegistry::new()))
    }

    pub fn with_adapter_registry(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        mcp_enabled: bool,
        adapter_registry: Arc<AdapterRegistry>,
    ) -> Self {
        Self::with_adapter_registry_and_shutdown(
            db,
            event_bus,
            mcp_enabled,
            adapter_registry,
            ShutdownSignal::new(),
        )
    }

    pub fn with_adapter_registry_and_shutdown(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        mcp_enabled: bool,
        adapter_registry: Arc<AdapterRegistry>,
        shutdown_signal: ShutdownSignal,
    ) -> Self {
        let workspace_root = default_workspace_root();
        let workflows_dir = std::env::temp_dir().join("forge-test-workflows");
        let merge_service = Arc::new(MergeService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
        ));
        let cleanup_scheduler = Arc::new(WorkspaceCleanupScheduler::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root,
        ));
        let review_runner = Arc::new(review::ReviewRunner::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            Arc::clone(&adapter_registry),
        ));
        Self::with_adapter_registry_services_and_shutdown(
            db,
            event_bus,
            mcp_enabled,
            adapter_registry,
            merge_service,
            cleanup_scheduler,
            review_runner,
            shutdown_signal,
            workflows_dir,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_adapter_registry_services_and_shutdown(
        db: Arc<SqliteDb>,
        event_bus: Arc<EventBus>,
        mcp_enabled: bool,
        adapter_registry: Arc<AdapterRegistry>,
        merge_service: Arc<MergeService>,
        cleanup_scheduler: Arc<WorkspaceCleanupScheduler>,
        review_runner: Arc<review::ReviewRunner>,
        shutdown_signal: ShutdownSignal,
        workflows_dir: PathBuf,
    ) -> Self {
        let workspace_root = cleanup_scheduler.workspace_root().to_path_buf();
        let effective_config = effective_config_for_workspace(workspace_root.clone());
        let task_executor: Arc<dyn TaskExecutor> =
            Arc::new(AdapterExecutor::new(Arc::clone(&adapter_registry)));
        let workspace_exec_locks = Arc::new(WorkspaceExecutionLockManager::default());
        let repo_cache_locks = Arc::new(RepoCacheLockManager::default());
        let terminal_activity = Arc::new(TerminalActivityTracker::default());
        let memory_service = Arc::new(MemoryService::new(Arc::clone(&db)));
        let workflow_template_service = Arc::new(
            services::workflow::template_service::WorkflowTemplateService::new(workflows_dir),
        );
        let execution_events = Arc::new(services::daemon_transport::ServerExecutionEventSink::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            workspace_root.clone(),
        ));
        let execution_event_handler: Arc<
            dyn services::daemon_transport::DaemonExecutionEventHandler,
        > = execution_events.clone();
        let daemon_connections =
            Arc::new(services::daemon_transport::DaemonConnectionRegistry::new(
                Arc::clone(&event_bus),
                execution_event_handler,
            ));
        let terminal_service = Arc::new(TerminalService::new_with_activity_tracker(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            Arc::clone(&daemon_connections),
            Arc::clone(&workspace_exec_locks),
            effective_config.terminal.clone(),
            workspace_root.clone(),
            Arc::clone(&terminal_activity),
        ));
        // Use the same root for workspace creation and cleanup so done tasks remove what claim made.
        let task_service = Arc::new(
            TaskService::new(Arc::clone(&db), Arc::clone(&event_bus))
                .with_merge_service(Arc::clone(&merge_service))
                .with_cleanup_scheduler(Arc::clone(&cleanup_scheduler))
                .with_review_runner(Arc::clone(&review_runner))
                .with_task_executor(Arc::clone(&task_executor))
                .with_daemon_connections(Arc::clone(&daemon_connections))
                .with_workspace_exec_locks(Arc::clone(&workspace_exec_locks))
                .with_terminal_activity_tracker(Arc::clone(&terminal_activity))
                .with_repo_cache_locks(Arc::clone(&repo_cache_locks))
                .with_memory_service(Arc::clone(&memory_service))
                .with_workspace_root(workspace_root.clone()),
        );
        execution_events.set_task_service(Arc::downgrade(&task_service));
        daemon_connections.set_embedded_execution_context(
            Arc::downgrade(&task_service),
            Arc::clone(&task_executor),
        );
        let agent_service = Arc::new(AgentService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let daemon_service = Arc::new(DaemonService::new(Arc::clone(&db), Arc::clone(&event_bus)));
        let terminal_cleanup_handler: Arc<
            dyn services::workspace_cleanup::WorkspaceCleanupObserver,
        > = terminal_service.clone();
        cleanup_scheduler.set_terminal_cleanup_handler(terminal_cleanup_handler);
        let terminal_event_handler: Arc<
            dyn services::daemon_transport::DaemonTerminalEventHandler,
        > = terminal_service.clone();
        daemon_connections.set_terminal_event_handler(terminal_event_handler);
        let notification_service = Arc::new(NotificationService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
        ));
        let _notification_service_handle = Arc::clone(&notification_service).start();
        let project_hook_service = Arc::new(ProjectHookService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
            Arc::clone(&task_service),
            Arc::clone(&notification_service),
        ));
        let operator_status_service = Arc::new(OperatorStatusService::new(Arc::clone(&db)));
        let operator_status_emitter =
            Arc::new(OperatorStatusEmitter::start(Arc::clone(&event_bus)));
        let conversation_service = Arc::new(
            ConversationService::new(Arc::clone(&db), Arc::clone(&event_bus))
                .with_memory_service(Arc::clone(&memory_service)),
        );
        let auth_service = Arc::new(AuthService::new(
            Arc::clone(&db),
            b"test-jwt-secret-for-development".to_vec(),
            4, // low cost for tests
        ));
        let oauth_service = Arc::new(services::OAuthService::new(
            Arc::clone(&db),
            Arc::clone(&auth_service),
            effective_config.mcp_resource_url(),
        ));

        Self {
            db,
            task_service,
            agent_service,
            daemon_service,
            daemon_connections,
            conversation_service,
            workflow_template_service,
            memory_service,
            merge_service,
            notification_service,
            project_hook_service,
            terminal_service,
            operator_status_service,
            operator_status_emitter,
            cleanup_scheduler,
            review_runner,
            adapter_registry,
            task_executor,
            auth_service,
            oauth_service,
            task_dispatcher: None,
            workspace_exec_locks,
            repo_cache_locks,
            event_bus,
            shutdown_signal,
            mcp_enabled,
            config_path: Arc::new(default_config_path()),
            effective_config: Arc::new(effective_config),
        }
    }

    pub fn with_config_path(mut self, config_path: PathBuf) -> Self {
        self.config_path = Arc::new(config_path);
        self
    }

    pub fn with_effective_config(mut self, config: ForgeConfig) -> Self {
        self.oauth_service = Arc::new(services::OAuthService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.auth_service),
            config.mcp_resource_url(),
        ));
        let terminal_activity = self.terminal_service.activity_tracker();
        let terminal_service = Arc::new(TerminalService::new_with_activity_tracker(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
            Arc::clone(&self.daemon_connections),
            Arc::clone(&self.workspace_exec_locks),
            config.terminal.clone(),
            self.cleanup_scheduler.workspace_root().to_path_buf(),
            terminal_activity,
        ));
        let terminal_cleanup_handler: Arc<
            dyn services::workspace_cleanup::WorkspaceCleanupObserver,
        > = terminal_service.clone();
        self.cleanup_scheduler
            .set_terminal_cleanup_handler(terminal_cleanup_handler);
        let terminal_event_handler: Arc<
            dyn services::daemon_transport::DaemonTerminalEventHandler,
        > = terminal_service.clone();
        self.daemon_connections
            .set_terminal_event_handler(terminal_event_handler);
        self.terminal_service = terminal_service;
        self.effective_config = Arc::new(config);
        self
    }

    pub fn with_task_dispatcher(mut self, task_dispatcher: Arc<services::TaskDispatcher>) -> Self {
        self.task_dispatcher = Some(task_dispatcher);
        self
    }
}

fn default_workspace_root() -> PathBuf {
    std::env::var("FORGE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("forge").join("worktrees"))
}

fn effective_config_for_workspace(workspace_root: PathBuf) -> ForgeConfig {
    let mut config = ForgeConfig::default();
    config.workspace.root = workspace_root;
    config
}
