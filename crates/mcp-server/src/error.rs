use db::DbError;
use serde_json::{json, Value};
use services::ServiceError;

use api_types::{
    TERMINAL_ACTIVE_EXECUTION, TERMINAL_ATTACH_TOKEN_INVALID, TERMINAL_DAEMON_UNAVAILABLE,
    TERMINAL_DISABLED, TERMINAL_INVALID_INPUT, TERMINAL_NOT_FOUND, TERMINAL_PATH_GUARDRAIL,
    TERMINAL_SESSION_LIMIT, TERMINAL_USER_LIMIT, TERMINAL_WORKSPACE_NOT_READY,
};

use crate::protocol::{error_response, McpResponse};

#[derive(Debug)]
pub(crate) struct McpToolError {
    pub(crate) code: i64,
    message: String,
    data: Option<Value>,
}

impl McpToolError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub(crate) fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub(crate) fn not_found(entity: &'static str, id: String) -> Self {
        Self::new(-32004, format!("{entity} not found: {id}"))
    }

    pub(crate) fn into_response(self, id: Value) -> McpResponse {
        error_response(id, self.code, self.message, self.data)
    }
}

impl From<ServiceError> for McpToolError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::DependencyGate => Self::new(-32029, "dependency gate"),
            ServiceError::NotFound { entity, id } => Self::not_found(entity, id),
            ServiceError::InvalidOperation { message } => Self::new(-32602, message),
            ServiceError::AuthorizationDenied { message } => {
                Self::new(-32003, message).with_data(json!({ "code": "authorization.invalid" }))
            }
            ServiceError::RateLimited {
                retry_after_seconds,
            } => Self::new(-32029, "rate limit exceeded").with_data(json!({
                "code": "rate_limited",
                "retry_after_seconds": retry_after_seconds,
            })),
            ServiceError::TaskActionUnavailable {
                available_actions,
                reason,
            } => Self::new(-32029, reason.clone()).with_data(json!({
                "available_actions": available_actions,
                "reason": reason,
            })),
            ServiceError::Conflict(message) => Self::new(-32602, message),
            ServiceError::MissingPrimaryRepo { project_id } => Self::new(
                -32002,
                format!("project {project_id} has no primary repo configured"),
            ),
            ServiceError::RepoMismatch { project_id } => Self::new(
                -32002,
                format!("provided repo_id does not match project {project_id} primary repo"),
            ),
            ServiceError::PrProviderMissing { repo_id } => Self::new(
                -32002,
                format!("repo {repo_id} has no PR provider configured"),
            ),
            ServiceError::PrProviderTokenMissing { repo_id } => Self::new(
                -32002,
                format!("repo {repo_id} PR provider has no token configured"),
            ),
            ServiceError::PrSyncFailure { task_id, details } => Self::new(
                -32002,
                format!("PR sync failed for task {task_id}: {details}"),
            ),
            ServiceError::NestedSubtaskUnsupported => {
                Self::new(-32602, "nested subtasks are unsupported").with_data(json!({
                    "code": "NESTED_SUBTASK_UNSUPPORTED"
                }))
            }
            ServiceError::SubtaskAssigneeUnsupported {
                root_coder_id,
                attempted,
            } => Self::new(-32602, "subtask assignee unsupported").with_data(json!({
                "code": "SUBTASK_ASSIGNEE_UNSUPPORTED",
                "root_coder_id": root_coder_id,
                "attempted": attempted
            })),
            ServiceError::SubtaskSequenceStarted { task_id } => Self::new(
                -32602,
                format!("subtask sequence already started for task {task_id}"),
            )
            .with_data(json!({
                "code": "SUBTASK_SEQUENCE_STARTED",
                "task_id": task_id
            })),
            ServiceError::SubtaskManagedByRoot {
                task_id,
                root_task_id,
            } => Self::new(
                -32029,
                format!("subtask {task_id} is managed by root {root_task_id}"),
            )
            .with_data(json!({
                "code": "SUBTASK_MANAGED_BY_ROOT",
                "task_id": task_id,
                "root_task_id": root_task_id
            })),
            ServiceError::ParentWorkspaceRequired { parent_task_id } => Self::new(
                -32602,
                format!("parent workspace required for task {parent_task_id}"),
            )
            .with_data(json!({
                "code": "PARENT_WORKSPACE_REQUIRED",
                "parent_task_id": parent_task_id
            })),
            ServiceError::WorkspaceResetRequired { task_id, reason } => Self::new(
                -32602,
                format!("workspace reset required for task {task_id}: {reason}"),
            )
            .with_data(json!({
                "code": "WORKSPACE_RESET_REQUIRED",
                "task_id": task_id,
                "reason": reason
            })),
            ServiceError::TaskSequenceAlreadyStarted { task_id } => Self::new(
                -32602,
                format!("task sequence already started for task {task_id}"),
            )
            .with_data(json!({
                "code": "TASK_SEQUENCE_ALREADY_STARTED",
                "task_id": task_id
            })),
            ServiceError::TerminalDisabled => Self::new(-32029, "terminal access is disabled")
                .with_data(json!({
                    "code": TERMINAL_DISABLED
                })),
            ServiceError::TerminalWorkspaceNotReady => {
                Self::new(-32029, "task workspace is not ready for terminal access").with_data(
                    json!({
                        "code": TERMINAL_WORKSPACE_NOT_READY
                    }),
                )
            }
            ServiceError::TerminalSessionLimit { scope } => {
                let code = if scope == "user" {
                    TERMINAL_USER_LIMIT
                } else {
                    TERMINAL_SESSION_LIMIT
                };
                Self::new(
                    -32029,
                    format!("terminal session limit reached for {scope}"),
                )
                .with_data(json!({
                    "code": code,
                    "scope": scope
                }))
            }
            ServiceError::TerminalDaemonUnavailable { daemon_id } => {
                Self::new(-32029, format!("terminal daemon {daemon_id} unavailable")).with_data(
                    json!({
                        "code": TERMINAL_DAEMON_UNAVAILABLE,
                        "daemon_id": daemon_id
                    }),
                )
            }
            ServiceError::TerminalActiveExecution { workspace_id } => Self::new(
                -32029,
                format!("workspace {workspace_id} has active terminal or execution work"),
            )
            .with_data(json!({
                "code": TERMINAL_ACTIVE_EXECUTION,
                "workspace_id": workspace_id
            })),
            ServiceError::TerminalAttachTokenInvalid => {
                Self::new(-32602, "terminal attach token is invalid").with_data(json!({
                    "code": TERMINAL_ATTACH_TOKEN_INVALID
                }))
            }
            ServiceError::TerminalPathGuardrail => Self::new(
                -32602,
                "terminal workspace path failed guardrail validation",
            )
            .with_data(json!({
                "code": TERMINAL_PATH_GUARDRAIL
            })),
            ServiceError::TerminalNotFound => Self::new(-32004, "terminal session not found")
                .with_data(json!({
                    "code": TERMINAL_NOT_FOUND
                })),
            ServiceError::TerminalInvalidInput { message } => {
                Self::new(-32602, message).with_data(json!({
                    "code": TERMINAL_INVALID_INPUT
                }))
            }
            ServiceError::DaemonUnavailable { daemon_id } => {
                Self::new(-32029, format!("daemon {daemon_id} unavailable"))
                    .with_data(json!({"code": "daemon_unavailable", "daemon_id": daemon_id}))
            }
            ServiceError::DaemonTimeout { daemon_id, method } => {
                Self::new(-32029, format!("daemon {daemon_id} timed out on {method}")).with_data(
                    json!({"code": "daemon_timeout", "daemon_id": daemon_id, "method": method}),
                )
            }
            ServiceError::Domain(message) => Self::new(-32602, message),
            ServiceError::Db(error) => error.into(),
            ServiceError::Git(error) => Self::new(-32603, "git error").with_data(json!({
                "details": error.to_string()
            })),
            ServiceError::Review(error) => Self::new(-32603, "review error").with_data(json!({
                "details": error.to_string()
            })),
            ServiceError::GuardRejection { guard, reason } => {
                if reason.starts_with("SUBTASK_SEQUENCE_NOT_COMPLETE") {
                    Self::new(-32029, "subtask sequence is not complete").with_data(json!({
                        "code": "SUBTASK_SEQUENCE_NOT_COMPLETE",
                        "guard": guard,
                        "reason": reason,
                    }))
                } else {
                    Self::new(-32029, format!("guard rejected: {guard}: {reason}"))
                }
            }
            ServiceError::AgentPaused { agent_id } => {
                Self::new(-32029, format!("agent {agent_id} is paused")).with_data(json!({
                    "code": "agent_paused",
                    "agent_id": agent_id
                }))
            }
            ServiceError::ProjectPaused { project_id } => {
                Self::new(-32029, format!("project {project_id} is paused")).with_data(json!({
                    "code": "project_paused",
                    "project_id": project_id
                }))
            }
        }
    }
}

impl From<DbError> for McpToolError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::NotFound => Self::new(-32004, "not found"),
            DbError::VersionConflict => Self::new(-32009, "version conflict"),
            DbError::InvalidTransition => Self::new(-32010, "invalid transition"),
            DbError::InvalidSoftDelete => Self::new(-32010, "invalid soft delete"),
            DbError::AgentAtCapacity => Self::new(-32029, "agent at capacity"),
            DbError::InvalidCursor => Self::new(-32602, "invalid cursor"),
            error => Self::new(-32603, "internal error").with_data(json!({
                "details": error.to_string()
            })),
        }
    }
}
