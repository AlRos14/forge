#![forbid(unsafe_code)]

pub mod account_usage;
pub(crate) mod agent_capacity;
pub mod agent_service;
pub mod auth_service;
pub mod collaboration_service;
pub mod coordinator_outputs;
pub mod copilot_mcp;
pub mod credential_service;
pub mod daemon_monitor;
pub mod daemon_service;
pub mod daemon_transport;
pub mod default_agents;
pub mod demo;
pub mod diff;
pub mod domain_event_service;
pub mod embedded_daemon;
pub mod external_api;
pub mod external_sync;
pub mod gate_engine;
pub mod human_decision_service;
pub mod integration_service;
pub mod merge_service;
pub mod notification_service;
pub mod oauth_service;
pub mod operator_status;
pub mod operator_status_emitter;
pub mod orchestrator_runtime;
pub mod plan_artifact;
pub mod pr_external_action;
pub mod pr_feedback;
pub mod pr_feedback_followup;
pub mod pr_github_client;
pub mod pr_provider_settings;
pub mod pr_rework;
pub mod pr_rework_watcher;
pub mod pr_service;
pub(crate) mod project_actor_scope;
pub mod project_authority;
pub mod project_copilot;
pub mod project_deletion;
pub mod project_hooks;
pub mod project_member_service;
pub mod project_plan;
pub mod project_repository_service;
pub mod provider_authorization;
pub mod recovery;
pub mod release_history;
mod replan_classifier;
pub mod role_capabilities;
pub mod shared_media_cleanup;
pub mod shutdown;
pub mod task_autonomy;
pub mod task_coordination;
pub mod task_dispatcher;
pub mod task_failure_retry;
mod task_integration_operation;
pub mod task_lifecycle;
pub mod task_quality;
pub mod task_rework;
pub mod task_role_configuration;
pub mod task_service;
pub mod terminal_service;
pub mod types;
pub mod validation_catalog;
pub mod validation_check_runner;
pub(crate) mod validation_post_integration;
pub mod validation_service;
pub mod work_unit_service;
pub mod workspace_cleanup;
pub mod workspace_execution_lock;

pub use agent_service::AgentService;
pub use auth_service::AuthService;
pub use collaboration_service::{
    CollaborationActorSource, CollaborationService, CreateArtifactInput, CreateDecisionInput,
    CreateHandoffInput, CreateMessageInput, CreateProposalInput,
};
pub use credential_service::{
    ConnectApiKeyCredential, ConnectOAuthCredential, ConnectScmCredential, CredentialError,
    CredentialRevocationOutcome, CredentialService, OAuthCredentialBundle,
    ProviderEntryTestOutcome, ProviderUsageOutcome, ProviderUsageWindowOutcome, Secret,
};
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
pub use domain_event_service::DomainEventService;
pub use embedded_daemon::EmbeddedDaemon;
pub use external_sync::ExternalSyncService;
pub use human_decision_service::HumanDecisionService;
pub use integration_service::IntegrationService;
pub use merge_service::{MergeOutcome, MergeService};
pub use notification_service::NotificationService;
pub use oauth_service::{OAuthError, OAuthService};
pub use operator_status::OperatorStatusService;
pub use operator_status_emitter::OperatorStatusEmitter;
pub use orchestrator_runtime::{OrchestratorRun, OrchestratorRuntime};
pub use project_copilot::ProjectCopilotService;
pub use project_hooks::ProjectHookService;
pub use project_member_service::ProjectMemberService;
pub use project_repository_service::ProjectRepositoryService;
pub use provider_authorization::ProviderAuthorizationService;
pub use recovery::{CrashRecovery, HeartbeatMonitor};
pub use release_history::HistoricalReleaseReader;
pub use shared_media_cleanup::SharedMediaCleanupScheduler;
pub use shutdown::GracefulShutdown;
pub use task_coordination::{
    AuthorizationInput, CoordinationBlocker, CoordinationBrief, CoordinationEnvelope,
    CoordinatorView, InterventionInput, MaterializeInput, MaterializeOutcome, Overview,
    PlanDecisionInput, PlanView, PlanWorkUnit, QualityRequestInput, TaskCoordinationService,
    TeamMemberView, TeamRoleView, TurnRequestInput, WorkUnitPlan,
};
pub use task_dispatcher::{DispatchOutcome, TaskDispatcher};
pub use task_quality::TaskQualityService;
pub use task_rework::{
    RequestReworkInput, RequestReworkResult, TaskReworkService, WorkUnitReviewSubject,
};
pub use task_service::{NewSubtaskInput, TaskService};
pub use terminal_service::{TerminalActivityTracker, TerminalService};
pub use types::Assignee;
pub use validation_catalog::{PublishCheckInput, ValidationCatalogService};
pub use validation_check_runner::{
    enqueue_check_request, EnqueueCheckRequest, ValidationCheckRunner,
};
pub use validation_service::{ValidationCheckResult, ValidationService};
pub use work_unit_service::{CreateWorkUnitInput, WorkUnitReadiness, WorkUnitService};
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

    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },

    #[error("invalid operation: {message}")]
    InvalidOperation { message: String },

    #[error("authorization denied: {message}")]
    AuthorizationDenied { message: String },

    #[error("rate limited; retry after {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u64 },

    #[error("task action unavailable: {reason}")]
    TaskActionUnavailable {
        available_actions: Vec<api_types::TaskAction>,
        reason: String,
    },

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("coordination {code}: {message}")]
    Coordination { code: &'static str, message: String },

    #[error("daemon unavailable: {daemon_id}")]
    DaemonUnavailable { daemon_id: String },

    #[error("daemon command timed out for daemon {daemon_id}: {method}")]
    DaemonTimeout { daemon_id: String, method: String },

    #[error("{0}")]
    Domain(String),

    #[error("project {project_id} has no primary repo")]
    MissingPrimaryRepo { project_id: String },

    #[error("Task repositories are invalid")]
    TaskRepositoriesInvalid,

    #[error("a Task repository does not belong to the Project")]
    TaskRepositoryNotInProject,

    #[error("Task repository scope cannot change after an Execution or invalidate an explicit WorkUnit repository selection")]
    TaskRepositoryReplanRequired,

    #[error("Task repository {repo_id} still has in-flight work and cannot be removed by replan")]
    TaskRepositoryReplanRemovalBlocked { repo_id: String },

    #[error("WorkUnit repository scope is invalid")]
    WorkUnitRepositoriesInvalid,

    #[error("the Project primary repository must belong to the Project")]
    ProjectPrimaryRepoNotInProject,

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

    #[error("provider start outcome is uncertain for Execution {execution_id}")]
    ExecutionStartUncertain { execution_id: String },

    #[error("remote daemon rejected Execution start: {message}")]
    DaemonStartRejected { message: String },

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

    #[error("invalid terminal input: {message}")]
    TerminalInvalidInput { message: String },
}

/// Commit-range ReviewReport fields are not accepted for a WorkUnit result
/// review, the only review submission currently implemented.
pub(crate) fn reject_range_report_fields(
    report: &api_types::SubmitReviewReportRequest,
) -> Result<()> {
    if report.structured_findings.is_some() || report.resolutions.is_some() {
        return Err(ServiceError::Coordination {
            code: "review.report.invalid_findings",
            message: "structured_findings and resolutions are only valid for a commit-range review"
                .to_owned(),
        });
    }
    Ok(())
}

impl From<db::DbError> for ServiceError {
    fn from(error: db::DbError) -> Self {
        match error {
            db::DbError::Sqlx(error)
                if error
                    .as_database_error()
                    .is_some_and(|error| error.message() == "gate.review_coverage_missing") =>
            {
                Self::Coordination {
                    code: "gate.review_coverage_missing",
                    message: "required review coverage is missing".to_owned(),
                }
            }
            db::DbError::DependencyGate => Self::DependencyGate,
            db::DbError::AgentPaused { agent_id } => Self::AgentPaused { agent_id },
            db::DbError::QualityRequestBudgetExhausted => Self::Coordination {
                code: "coordination.budget_exhausted",
                message: "the Task quality-request budget is exhausted".to_owned(),
            },
            db::DbError::ValidationCheckStalePin => Self::Coordination {
                code: "validation.check_stale_pin",
                message: error.to_string(),
            },
            db::DbError::ValidationCheckNotAuthorized => Self::Coordination {
                code: "validation.check_not_authorized",
                message: error.to_string(),
            },
            db::DbError::ValidationWorkspaceLeased => Self::Coordination {
                code: "validation.workspace_leased",
                message: error.to_string(),
            },
            db::DbError::TaskRepositoryReplanBaseStale => Self::Coordination {
                code: "coordination.authorization_stale",
                message: "the repository-set revision changed after this replan was proposed"
                    .to_owned(),
            },
            db::DbError::TaskRepositoryReplanPrimaryImmutable => Self::Coordination {
                code: "task.repositories.replan_primary_immutable",
                message: "coordinator replan cannot change the Task primary repository".to_owned(),
            },
            db::DbError::TaskRepositoryReplanRemovalBlocked { repo_id } => {
                Self::TaskRepositoryReplanRemovalBlocked { repo_id }
            }
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

    pub(crate) fn terminal_invalid_input(message: impl Into<String>) -> Self {
        Self::TerminalInvalidInput {
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
