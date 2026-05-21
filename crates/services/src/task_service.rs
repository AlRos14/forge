use crate::{
    agent_service::{compute_effective_status, EffectiveStatus},
    lifecycle::{LifecycleHookContext, LifecycleHookRun, LifecycleHookRunner},
    merge_service::MergeService,
    terminal_service::TerminalActivityTracker,
    workflow::{default_states, engine::WorkflowEngine},
    workspace_cleanup::WorkspaceCleanupScheduler,
    workspace_execution_lock::WorkspaceExecutionLockManager,
    Assignee, Result, ServiceError,
};
use ::review::{ReviewRequest, ReviewRunner};
use ::workspace::{RepoCacheLockManager, WorkspaceManager};
use api_types::ProjectSettings;
use cli_adapters::codex::protocol::RESUME_THREAD_ID_CONFIG_KEY;
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentRepo, ArchiveTask, AssigneeKind, ClaimTask, ClaimedTask,
    CommentAuthorType, CreateExecution, CreateTask, CreateTaskComment, CreateTaskRoleAssignment,
    CreateWorkspace, DbError, Execution, ExecutionRepo, ExecutionStatus, PageRequest, ProjectRepo,
    RepoRepo, Review, ReviewRepo, ReviewStatus, SoftDeleteTask, SortBy, SortOrder, SqliteDb, Task,
    TaskCommentRepo, TaskDependencyRepo, TaskMetadata, TaskRepo, TaskRoleAssignment,
    TaskRoleAssignmentRepo, TaskStatus, TransitionLogRepo, Workspace, WorkspaceRepo,
    WorkspaceStatus,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    merge_overrides, resolve_config_value, ExecutionContext, ExecutionOutcome, ExecutionOverrides,
    ExecutorKind, TaskExecutor,
};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::process::Command;
use uuid::Uuid;

pub mod action_resolver;
mod claim;
mod common;
pub(crate) mod config;
mod create;
mod create_subtasks;
mod execution;
mod lifecycle_test;
pub(crate) mod logs;
mod reorder_subtasks;
mod reorder_task;
mod review;
mod review_config;
mod roles;
mod subtask;
mod transition;
mod validation;
pub(crate) mod workspace;

pub use create_subtasks::NewSubtaskInput;
pub use execution::subtasks::build_first_turn_prompt_from_context;
pub use subtask::{is_root_task, is_subtask, root_for};

#[cfg(test)]
use self::config::{
    execution_overrides_to_config_layer, merge_config_layers, override_value_or_empty,
    parse_config_override_layer, OverridesApplied,
};
use self::{
    config::{
        build_executor_config_snapshot, create_failed_execution_record,
        executor_snapshot_with_resume_thread, parse_json_value, truncate_utf8_bytes,
    },
    logs::execution_logs_path,
    review_config::review_config_from_json,
    validation::{serialize_config, validate_required},
    workspace::{default_workspace_root, prepare_workspace, reset_workspace},
};

pub(super) const DISPATCH_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(10);
pub(super) const DISPATCH_STATUS_WAIT_CEILING: Duration = Duration::from_secs(10 * 60);
pub(super) const MAX_FOLLOW_UP_DIFF_BYTES: usize = 64 * 1024;

pub(super) fn is_transient_error_annotation(raw_annotation: &str) -> bool {
    let Ok(annotation) = serde_json::from_str::<Value>(raw_annotation) else {
        return false;
    };

    matches!(
        annotation.get("type").and_then(Value::as_str),
        Some(
            "merge_conflict"
                | "dirty_worktree"
                | "target_repo_dirty"
                | "executor_failed"
                | "review_budget_exhausted"
                | "merge_fix_budget_exhausted"
                | "merge_fix_ci_failed"
        )
    )
}

#[derive(Clone)]
pub struct TaskService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    merge_service: Option<Arc<MergeService>>,
    cleanup_scheduler: Option<Arc<WorkspaceCleanupScheduler>>,
    review_runner: Option<Arc<ReviewRunner>>,
    task_executor: Option<Arc<dyn TaskExecutor>>,
    daemon_connections: Option<Arc<crate::daemon_transport::DaemonConnectionRegistry>>,
    workspace_exec_locks: Option<Arc<WorkspaceExecutionLockManager>>,
    terminal_activity: Option<Arc<TerminalActivityTracker>>,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
    workspace_root: PathBuf,
}

#[derive(Debug)]
pub struct TransitionResult {
    pub task: Task,
    pub review: Option<Review>,
}

pub struct TransitionOptions {
    pub version: i64,
    pub reason: Option<String>,
    pub triggered_by: String,
    pub rejection: bool,
    pub defer_dispatch_seconds: Option<i64>,
}

impl From<i64> for TransitionOptions {
    fn from(version: i64) -> Self {
        Self {
            version,
            reason: None,
            triggered_by: "system".to_owned(),
            rejection: false,
            defer_dispatch_seconds: None,
        }
    }
}

impl From<(i64, Option<String>)> for TransitionOptions {
    fn from((version, reason): (i64, Option<String>)) -> Self {
        Self {
            version,
            reason,
            triggered_by: "user:api".to_owned(),
            rejection: false,
            defer_dispatch_seconds: None,
        }
    }
}

impl From<(i64, Option<String>, bool)> for TransitionOptions {
    fn from((version, reason, rejection): (i64, Option<String>, bool)) -> Self {
        Self {
            version,
            reason,
            triggered_by: "user:api".to_owned(),
            rejection,
            defer_dispatch_seconds: None,
        }
    }
}

pub struct LaunchExecutionResult {
    pub task: Task,
    pub execution: Execution,
    pub workspace: Workspace,
}

impl TaskService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            db,
            event_bus,
            merge_service: None,
            cleanup_scheduler: None,
            review_runner: None,
            task_executor: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            repo_cache_locks: None,
            workspace_root: default_workspace_root(),
        }
    }

    pub fn with_merge_service(mut self, merge_service: Arc<MergeService>) -> Self {
        self.merge_service = Some(merge_service);
        self
    }

    pub fn with_review_runner(mut self, review_runner: Arc<ReviewRunner>) -> Self {
        self.review_runner = Some(review_runner);
        self
    }

    pub fn with_task_executor(mut self, task_executor: Arc<dyn TaskExecutor>) -> Self {
        self.task_executor = Some(task_executor);
        self
    }

    pub fn with_daemon_connections(
        mut self,
        daemon_connections: Arc<crate::daemon_transport::DaemonConnectionRegistry>,
    ) -> Self {
        self.daemon_connections = Some(daemon_connections);
        self
    }

    pub fn with_workspace_exec_locks(mut self, locks: Arc<WorkspaceExecutionLockManager>) -> Self {
        self.workspace_exec_locks = Some(locks);
        self
    }

    pub fn with_terminal_activity_tracker(
        mut self,
        terminal_activity: Arc<TerminalActivityTracker>,
    ) -> Self {
        self.terminal_activity = Some(terminal_activity);
        self
    }

    pub fn with_repo_cache_locks(mut self, locks: Arc<RepoCacheLockManager>) -> Self {
        self.repo_cache_locks = Some(locks);
        self
    }

    pub fn with_cleanup_scheduler(
        mut self,
        cleanup_scheduler: Arc<WorkspaceCleanupScheduler>,
    ) -> Self {
        self.cleanup_scheduler = Some(cleanup_scheduler);
        self
    }

    pub fn with_workspace_root(mut self, workspace_root: PathBuf) -> Self {
        self.workspace_root = workspace_root;
        self
    }

    fn publish(&self, event: ForgeEvent) {
        self.event_bus.publish(event);
    }

    pub(crate) async fn complete_remote_execution(
        &self,
        notification: api_types::ExecutionTerminalNotification,
    ) -> Result<Execution> {
        validate_required("execution_id", &notification.execution_id)?;
        let current_execution = ExecutionRepo::get_by_id(&*self.db, &notification.execution_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("execution", notification.execution_id.clone())
            })?;
        if current_execution.status != ExecutionStatus::Running {
            return Ok(current_execution);
        }

        let task = TaskRepo::get_by_id(&*self.db, &current_execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", current_execution.task_id.clone()))?;
        let signal = notification
            .signal
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let error = notification
            .error
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let succeeded = notification.exit_code == Some(0) && signal.is_none() && error.is_none();
        let (status, stop_reason, stopped_by, resume_policy, stopped_at, error) = if succeeded {
            (
                ExecutionStatus::Completed,
                None,
                None,
                None,
                None,
                Some(None),
            )
        } else {
            (
                ExecutionStatus::Failed,
                Some(Some(db::StopReason::ExecutorFailed)),
                Some(Some("system:daemon".to_owned())),
                Some(Some(db::ResumePolicy::Manual)),
                Some(Some(notification.ts.clone())),
                Some(Some(remote_terminal_error_message(
                    notification.exit_code,
                    signal,
                    error,
                ))),
            )
        };

        let updated = ExecutionRepo::update(
            &*self.db,
            db::UpdateExecution {
                id: notification.execution_id,
                status: Some(status),
                stop_reason,
                stopped_by,
                resume_policy,
                stopped_at,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some(notification.ts)),
                summary: Some(None),
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        execution::publish_terminal_execution_event(self, &updated);

        if updated.status == ExecutionStatus::Completed {
            if let Err(error) = execution::clear_execution_retry_metadata(&self.db, &task).await {
                tracing::warn!(
                    task_id = %task.id,
                    execution_id = %updated.id,
                    %error,
                    "failed to clear execution retry metadata"
                );
            }
            if updated.role == crate::workflow::default_roles::PLANNER
                && task.status == crate::workflow::default_states::PLANNING
            {
                if let Err(error) = execution::set_planning_awaiting_review_metadata(
                    &self.db,
                    &task,
                    Some(&updated.id),
                    true,
                )
                .await
                {
                    tracing::warn!(
                        task_id = %task.id,
                        execution_id = %updated.id,
                        %error,
                        "failed to mark planning awaiting review"
                    );
                }
            }
        } else if updated.status == ExecutionStatus::Failed
            && execution::should_block_task_for_failed_execution(&updated)
        {
            if let Err(error) = self.annotate_executor_failure_block(&updated).await {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to block task after daemon execution failure"
                );
            }
        }

        Ok(updated)
    }
}

fn remote_terminal_error_message(
    exit_code: Option<i32>,
    signal: Option<&str>,
    error: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if let Some(error) = error {
        parts.push(error.to_owned());
    }
    if let Some(exit_code) = exit_code {
        parts.push(format!("exit code {exit_code}"));
    }
    if let Some(signal) = signal {
        parts.push(format!("signal {signal}"));
    }
    if parts.is_empty() {
        "remote execution failed".to_owned()
    } else {
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests;
