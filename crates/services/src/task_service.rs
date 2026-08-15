use crate::{
    agent_service::{compute_effective_status, EffectiveStatus},
    lifecycle::{LifecycleHookContext, LifecycleHookRun, LifecycleHookRunner},
    memory::MemoryService,
    merge_service::MergeService,
    terminal_service::TerminalActivityTracker,
    workflow::{default_states, engine::WorkflowEngine},
    workspace_cleanup::WorkspaceCleanupScheduler,
    workspace_execution_lock::WorkspaceExecutionLockManager,
    Assignee, Result, ServiceError,
};
use ::review::{ReviewRequest, ReviewRunner};
use ::workspace::{RepoCacheLockManager, WorkspaceManager};
use api_types::{Actor, ProjectSettings, UserActionSource};
use cli_adapters::codex::protocol::RESUME_THREAD_ID_CONFIG_KEY;
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentRepo, ArchiveTask, AssigneeKind, ClaimTask, ClaimedTask,
    CommentAuthorType, CreateExecution, CreateTask, CreateTaskComment, CreateTaskRoleAssignment,
    CreateWorkspace, CreateWorkspaceLease, DbError, Execution, ExecutionRepo, ExecutionStatus,
    ExecutionUsageRepo, PageRequest, ProjectRepo, RepoRepo, Review, ReviewRepo, ReviewStatus,
    SoftDeleteTask, SortBy, SortOrder, SqliteDb, Task, TaskComment, TaskCommentRepo,
    TaskDependencyRepo, TaskMetadata, TaskRepo, TaskRoleAssignment, TaskRoleAssignmentRepo,
    TaskStatus, TransitionLogRepo, UpsertExecutionUsage, Workspace, WorkspaceLeaseRepo,
    WorkspaceRepo, WorkspaceStatus,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    merge_overrides, resolve_config_value, ExecutionContext, ExecutionOutcome, ExecutionOverrides,
    ExecutorKind, TaskExecutor,
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

pub mod action_resolver;
mod actions;
mod claim;
mod common;
pub(crate) mod config;
mod create;
mod create_subtasks;
mod execution;
mod governance;
mod lifecycle_test;
pub(crate) mod logs;
mod move_task;
mod reorder_subtasks;
mod review;
mod review_config;
mod roles;
mod subtask;
mod transition;
mod validation;
pub(crate) mod workspace;

pub use actions::TaskActionResult;
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
    memory_service: Arc<MemoryService>,
    move_operation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    credential_env: Option<Arc<crate::embedded_agent_service::EmbeddedAgentService>>,
}

#[derive(Debug)]
pub struct TransitionResult {
    pub task: Task,
    pub review: Option<Review>,
}

pub struct TransitionOptions {
    pub version: i64,
    pub reason: Option<String>,
    pub triggered_by: Actor,
    pub rejection: bool,
    pub defer_dispatch_seconds: Option<i64>,
}

impl From<i64> for TransitionOptions {
    fn from(version: i64) -> Self {
        Self {
            version,
            reason: None,
            triggered_by: Actor::system(api_types::SystemComponent::General),
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
            triggered_by: Actor::user(UserActionSource::Api),
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
            triggered_by: Actor::user(UserActionSource::Api),
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
        let memory_service = Arc::new(MemoryService::new(Arc::clone(&db)));
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
            memory_service,
            move_operation_locks: Arc::new(Mutex::new(HashMap::new())),
            credential_env: None,
        }
    }

    pub fn with_merge_service(mut self, merge_service: Arc<MergeService>) -> Self {
        self.merge_service = Some(merge_service);
        self
    }

    pub(crate) async fn publish_domain_event_by_dedupe(&self, dedupe_key: &str) {
        let service =
            crate::DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        if let Err(error) = service.publish_by_dedupe(dedupe_key).await {
            tracing::warn!(dedupe_key, %error, "failed to mirror committed domain event");
        }
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

    pub fn with_memory_service(mut self, memory_service: Arc<MemoryService>) -> Self {
        self.memory_service = memory_service;
        self
    }

    /// Enables `auth_source: forge_provider` dispatch: harness executions for
    /// agents referencing a provider entry get the entry's API key injected
    /// into their in-memory executor environment.
    pub fn with_provider_credential_env(
        mut self,
        embedded: Arc<crate::embedded_agent_service::EmbeddedAgentService>,
    ) -> Self {
        self.credential_env = Some(embedded);
        self
    }

    fn publish(&self, event: ForgeEvent) {
        self.event_bus.publish(event);
    }

    /// Create a running execution and remove a freshly prepared workspace if
    /// the authoritative in-transaction admission guard rejects it. Existing
    /// workspaces are intentionally retained for retries/recovery; only a
    /// workspace created by this attempt is rolled back.
    pub(crate) async fn create_running_execution(
        &self,
        input: CreateExecution,
        workspace_created_by_attempt: bool,
    ) -> Result<Execution> {
        let repository_context = if let Some(workspace_id) = input.workspace_id.as_deref() {
            let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            Some((task, workspace))
        } else {
            let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
            if task.repo_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "repository execution requires a scheduler WorkspaceLease-backed workspace",
                ));
            }
            None
        };

        // The lease is FK-bound to the concrete execution attempt.  Create
        // that attempt first, then issue the authority; a rejected lease is
        // immediately terminalized so no running execution can exist without
        // an active scheduler grant.
        let execution = match ExecutionRepo::create(&*self.db, input.clone()).await {
            Ok(execution) => execution,
            Err(error) => {
                if workspace_created_by_attempt {
                    self.cleanup_fresh_execution_workspace_by_id(
                        &input.task_id,
                        input.workspace_id.as_deref(),
                    )
                    .await;
                }
                return Err(error.into());
            }
        };
        if let Some((task, workspace)) = repository_context.as_ref() {
            if let Err(error) = self
                .issue_workspace_lease(
                    task,
                    workspace,
                    &input.role,
                    input.agent_id.as_deref(),
                    &input.id,
                )
                .await
            {
                if let Err(mark_error) = self
                    .fail_execution_before_dispatch(&execution.id, error.to_string())
                    .await
                {
                    tracing::warn!(
                        execution_id = %execution.id,
                        %mark_error,
                        "failed to terminalize execution after WorkspaceLease rejection"
                    );
                }
                if workspace_created_by_attempt {
                    self.cleanup_fresh_execution_workspace(task, workspace)
                        .await;
                }
                return Err(error);
            }
        }
        Ok(execution)
    }

    pub(crate) async fn cleanup_fresh_execution_workspace(
        &self,
        task: &Task,
        workspace: &Workspace,
    ) {
        self.cleanup_fresh_execution_workspace_by_id(&task.id, Some(&workspace.id))
            .await;
    }

    async fn cleanup_fresh_execution_workspace_by_id(
        &self,
        task_id: &str,
        workspace_id: Option<&str>,
    ) {
        let mut removed_workspace = false;
        if let Some(workspace_id) = workspace_id {
            // Delete only our workspace row and only while no execution has
            // acquired it. This protects a concurrent launch which reused
            // the same Task workspace after this attempt lost admission.
            match sqlx::query(
                "DELETE FROM workspace
                 WHERE id = ? AND task_id = ?
                   AND NOT EXISTS (
                       SELECT 1 FROM execution
                       WHERE execution.workspace_id = workspace.id
                   )",
            )
            .bind(workspace_id)
            .bind(task_id)
            .execute(self.db.pool())
            .await
            {
                Ok(result) => removed_workspace = result.rows_affected() == 1,
                Err(cleanup_error) => tracing::warn!(
                    task_id,
                    workspace_id,
                    %cleanup_error,
                    "failed to remove workspace row after rejected execution"
                ),
            }
        }
        let mut manager = WorkspaceManager::new(self.workspace_root.clone());
        if let Some(locks) = self.repo_cache_locks.clone() {
            manager = manager.with_repo_cache_locks(locks);
        }
        if removed_workspace {
            if let Err(cleanup_error) = manager.cleanup_worktree(task_id).await {
                tracing::warn!(
                    task_id,
                    %cleanup_error,
                    "failed to remove fresh worktree after rejected execution"
                );
            }
        }
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
        let outcome = notification.status.as_deref().unwrap_or(if succeeded {
            "completed"
        } else {
            "failed"
        });
        let (status, stop_reason, stopped_by, resume_policy, stopped_at, error) = match outcome {
            "completed" => (
                ExecutionStatus::Completed,
                None,
                None,
                None,
                None,
                Some(None),
            ),
            "cancelled" => (
                ExecutionStatus::Cancelled,
                Some(Some(db::StopReason::ExecutorCancelled)),
                Some(Some(
                    Actor::system(api_types::SystemComponent::Executor).display(),
                )),
                Some(Some(db::ResumePolicy::Manual)),
                Some(Some(notification.ts.clone())),
                Some(None),
            ),
            _ => (
                ExecutionStatus::Failed,
                Some(Some(db::StopReason::ExecutorFailed)),
                Some(Some(
                    Actor::system(api_types::SystemComponent::Executor).display(),
                )),
                Some(Some(db::ResumePolicy::Manual)),
                Some(Some(notification.ts.clone())),
                Some(Some(remote_terminal_error_message(
                    notification.exit_code,
                    signal,
                    error,
                ))),
            ),
        };

        let executor_unavailable = notification.failure_class
            == Some(api_types::RemoteExecutionFailureClass::ExecutorUnavailable);
        let route_outcome = crate::task_service::config::RouteOutcome {
            selected: notification.resolved_candidate.as_ref().map(|candidate| {
                (
                    candidate.candidate_key.clone(),
                    candidate.executor_type.clone(),
                    candidate.config.clone(),
                )
            }),
            attempts: notification
                .route_attempts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|attempt| (attempt.candidate_key.clone(), attempt.outcome.clone()))
                .collect(),
            unavailable_retry_at: executor_unavailable.then(|| notification.retry_at.clone()),
        };
        let snapshot_update = match current_execution.executor_config_snapshot_json.as_deref() {
            Some(snapshot) => crate::task_service::config::apply_route_outcome_to_snapshot(
                snapshot,
                &route_outcome,
            )?,
            None => None,
        };

        let execution_id = notification.execution_id.clone();
        let terminal_ts = notification.ts.clone();
        let updated = ExecutionRepo::update(
            &*self.db,
            db::UpdateExecution {
                id: execution_id,
                status: Some(status),
                stop_reason,
                stopped_by,
                resume_policy,
                stopped_at,
                agent_session_id: notification.agent_session_id.map(Some),
                agent_message_id: None,
                last_activity_at: Some(Some(terminal_ts)),
                summary: notification.summary.map(Some),
                logs_path: None,
                before_sha: None,
                after_sha: notification.after_sha.map(Some),
                error,
                executor_config_snapshot_json: snapshot_update.map(Some),
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        if updated.status != ExecutionStatus::Running {
            self.revoke_active_workspace_lease_for_execution(&task.id, &updated.id)
                .await;
        }

        if let Some(usage) = notification.usage {
            let provider = execution::usage_provider_from_snapshot(
                current_execution.executor_config_snapshot_json.as_deref(),
            );
            let model = usage.model.unwrap_or_else(|| "default".to_owned());
            if let Err(error) = ExecutionUsageRepo::upsert(
                &*self.db,
                UpsertExecutionUsage {
                    execution_id: updated.id.clone(),
                    provider,
                    model,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_tokens,
                    cache_write_tokens: usage.cache_write_tokens,
                    cost_usd: usage.cost_usd,
                },
            )
            .await
            {
                tracing::warn!(
                    execution_id = %updated.id,
                    %error,
                    "failed to record remote execution token usage"
                );
            }
        }

        execution::publish_terminal_execution_event(self, &updated);

        if let Err(error) = self
            .memory_service
            .record_execution_summary_if_present(&task.project_id, &updated)
            .await
        {
            tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
        }

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
            && executor_unavailable
            && execution::should_block_task_for_failed_execution(&updated)
        {
            let attempts = serde_json::Value::Array(
                route_outcome
                    .attempts
                    .iter()
                    .map(|(candidate_key, outcome)| {
                        serde_json::json!({"candidate_key": candidate_key, "outcome": outcome})
                    })
                    .collect(),
            );
            if let Err(error) = self
                .annotate_executor_unavailable_block(
                    &updated,
                    notification.retry_at.clone(),
                    attempts,
                )
                .await
            {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to handle executor-unavailable daemon execution"
                );
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
