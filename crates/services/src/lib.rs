#![forbid(unsafe_code)]

pub(crate) mod agent_capacity;
pub mod agent_service;
pub mod auth_service;
pub mod conversation_service;
pub mod daemon_monitor;
pub mod daemon_service;
pub mod daemon_transport;
pub mod default_agents;
pub(crate) mod deferred_dispatch;
pub mod demo;
pub mod diff;
pub mod embedded_daemon;
pub mod external_api;
pub mod external_sync;
pub mod integration_service;
pub mod lifecycle;
pub mod merge_service;
pub mod notification_service;
pub mod oauth_service;
pub mod operator_status;
pub mod operator_status_emitter;
pub mod plan_artifact;
pub mod pr_service;
pub mod project_member_service;
pub mod recovery;
pub mod shutdown;
pub mod task_diagnostics;
pub mod task_dispatcher;
pub mod task_service;
pub mod terminal_service;
pub mod types;
pub mod workflow;
pub mod workspace_cleanup;
pub mod workspace_execution_lock;

pub use agent_service::AgentService;
pub use auth_service::AuthService;
pub use conversation_service::ConversationService;
pub use daemon_monitor::DaemonMonitor;
pub use daemon_service::{
    DaemonRegisterInput, DaemonRegistration, DaemonReportInput, DaemonService,
};
pub use daemon_transport::{
    select_execution_provider, select_filesystem_provider, DaemonConnection,
    DaemonConnectionRegistry, EmbeddedExecutionProvider, EmbeddedFilesystemProvider,
    ExecutionProvider, FilesystemProvider, RemoteExecutionProvider, RemoteFilesystemProvider,
};
pub use default_agents::ensure_default_agents;
pub use demo::install_demo_data;
pub use diff::DiffService;
pub use embedded_daemon::EmbeddedDaemon;
pub use external_sync::ExternalSyncService;
pub use integration_service::IntegrationService;
pub use merge_service::{MergeOutcome, MergeService};
pub use notification_service::NotificationService;
pub use oauth_service::{OAuthError, OAuthService};
pub use operator_status::OperatorStatusService;
pub use operator_status_emitter::OperatorStatusEmitter;
pub use project_member_service::ProjectMemberService;
pub use recovery::{CrashRecovery, HeartbeatMonitor};
pub use shutdown::GracefulShutdown;
pub use task_dispatcher::TaskDispatcher;
pub use task_service::{NewSubtaskInput, TaskService};
pub use terminal_service::{TerminalActivityTracker, TerminalService};
pub use types::Assignee;
pub use workflow::template_service::WorkflowTemplateService;
pub use workspace_cleanup::WorkspaceCleanupScheduler;
pub use workspace_execution_lock::WorkspaceExecutionLockManager;

pub type Result<T> = std::result::Result<T, ServiceError>;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("dependency gate")]
    DependencyGate,

    #[error(transparent)]
    Db(db::DbError),

    #[error(transparent)]
    Git(git::GitError),

    #[error(transparent)]
    Review(review::ReviewError),

    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },

    #[error("invalid operation: {message}")]
    InvalidOperation { message: String },

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("daemon unavailable: {daemon_id}")]
    DaemonUnavailable { daemon_id: String },

    #[error("daemon command timed out for daemon {daemon_id}: {method}")]
    DaemonTimeout { daemon_id: String, method: String },

    #[error("{0}")]
    Domain(String),

    #[error("project {project_id} has no primary repo")]
    MissingPrimaryRepo { project_id: String },

    #[error("repo does not match primary repo for project {project_id}")]
    RepoMismatch { project_id: String },

    #[error("PR provider missing for repo {repo_id}")]
    PrProviderMissing { repo_id: String },

    #[error("PR provider token missing for repo {repo_id}")]
    PrProviderTokenMissing { repo_id: String },

    #[error("PR sync failure for task {task_id}: {details}")]
    PrSyncFailure { task_id: String, details: String },

    #[error("agent {agent_id} is paused and cannot accept new work")]
    AgentPaused { agent_id: String },

    #[error("project {project_id} is paused")]
    ProjectPaused { project_id: String },

    #[error("guard rejected: {guard}: {reason}")]
    GuardRejection { guard: String, reason: String },

    #[error("nested subtasks are unsupported")]
    NestedSubtaskUnsupported,

    #[error("subtask assignee unsupported: root coder {root_coder_id:?}, attempted {attempted}")]
    SubtaskAssigneeUnsupported {
        root_coder_id: Option<String>,
        attempted: String,
    },

    #[error("subtask sequence already started for task {task_id}")]
    SubtaskSequenceStarted { task_id: String },

    #[error("subtask {task_id} is managed by root {root_task_id}")]
    SubtaskManagedByRoot {
        task_id: String,
        root_task_id: String,
    },

    #[error("parent workspace required for task {parent_task_id}")]
    ParentWorkspaceRequired { parent_task_id: String },

    #[error("workspace reset required for task {task_id}: {reason}")]
    WorkspaceResetRequired { task_id: String, reason: String },

    #[error("task sequence already started for task {task_id}")]
    TaskSequenceAlreadyStarted { task_id: String },

    #[error("terminal access is disabled")]
    TerminalDisabled,

    #[error("terminal workspace is not ready")]
    TerminalWorkspaceNotReady,

    #[error("terminal session limit reached for {scope}")]
    TerminalSessionLimit { scope: String },

    #[error("terminal daemon unavailable: {daemon_id}")]
    TerminalDaemonUnavailable { daemon_id: String },

    #[error("terminal blocked by active execution in workspace {workspace_id}")]
    TerminalActiveExecution { workspace_id: String },

    #[error("terminal attach token is invalid")]
    TerminalAttachTokenInvalid,

    #[error("terminal path guardrail rejected the workspace path")]
    TerminalPathGuardrail,

    #[error("terminal session not found")]
    TerminalNotFound,
}

impl From<db::DbError> for ServiceError {
    fn from(error: db::DbError) -> Self {
        match error {
            db::DbError::DependencyGate => Self::DependencyGate,
            error => Self::Db(error),
        }
    }
}

impl From<sqlx::Error> for ServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

impl From<git::GitError> for ServiceError {
    fn from(error: git::GitError) -> Self {
        Self::Git(error)
    }
}

impl From<review::ReviewError> for ServiceError {
    fn from(error: review::ReviewError) -> Self {
        Self::Review(error)
    }
}

impl From<executors::ExecutorError> for ServiceError {
    fn from(error: executors::ExecutorError) -> Self {
        Self::InvalidOperation {
            message: error.to_string(),
        }
    }
}

impl ServiceError {
    pub(crate) fn not_found(entity: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity,
            id: id.into(),
        }
    }

    pub(crate) fn invalid_operation(message: impl Into<String>) -> Self {
        Self::InvalidOperation {
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    pub fn nested_subtask_unsupported() -> Self {
        Self::NestedSubtaskUnsupported
    }

    pub fn subtask_assignee_unsupported(root_coder_id: Option<String>, attempted: String) -> Self {
        Self::SubtaskAssigneeUnsupported {
            root_coder_id,
            attempted,
        }
    }

    pub fn subtask_sequence_started(task_id: impl Into<String>) -> Self {
        Self::SubtaskSequenceStarted {
            task_id: task_id.into(),
        }
    }

    pub fn subtask_managed_by_root(
        task_id: impl Into<String>,
        root_task_id: impl Into<String>,
    ) -> Self {
        Self::SubtaskManagedByRoot {
            task_id: task_id.into(),
            root_task_id: root_task_id.into(),
        }
    }

    pub fn parent_workspace_required(parent_task_id: impl Into<String>) -> Self {
        Self::ParentWorkspaceRequired {
            parent_task_id: parent_task_id.into(),
        }
    }

    pub fn task_sequence_already_started(task_id: impl Into<String>) -> Self {
        Self::TaskSequenceAlreadyStarted {
            task_id: task_id.into(),
        }
    }
}
