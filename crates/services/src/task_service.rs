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
use ::review::ReviewRunner;
use ::workspace::{RepoCacheLockManager, WorkspaceManager};
use api_types::{Actor, ActorRef, ProjectSettings, UserActionSource};
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentRepo, ArchiveTask, AssigneeKind, ClaimTask, ClaimedTask,
    CommentAuthorType, CreateDomainEvent, CreateExecution, CreateTask, CreateTaskComment,
    CreateTaskRoleAssignment, CreateWorkspace, CreateWorkspaceLease, DbError, Execution,
    ExecutionPurpose, ExecutionRepo, ExecutionStatus, ExecutionUsageRepo, HarnessSession,
    HarnessSessionRepo, HarnessSessionStatus, PageRequest, ProjectRepo, RepoRepo, Review,
    SoftDeleteTask, SortBy, SortOrder, SqliteDb, Task, TaskComment, TaskCommentRepo,
    TaskDependencyRepo, TaskMetadata, TaskRepo, TaskRoleAssignment, TaskRoleAssignmentRepo,
    TaskStatus, TransitionLogRepo, UpsertExecutionUsage, UserRepo, WorkUnitRepo,
    WorkUnitWorkspaceRepo, Workspace, WorkspaceLeaseRepo, WorkspaceRepo, WorkspaceStatus,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    ExecutionContext, ExecutionOutcome, ExecutionOverrides, ExecutorKind, TaskExecutor,
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
pub(crate) mod execution;
pub use execution::resumable_external_session;
mod governance;
mod lifecycle_test;
pub(crate) mod logs;
mod memberships;
mod move_task;
mod orchestrator;
mod reorder_subtasks;
mod review;
mod roles;
mod subtask;
mod transition;
mod validation;
pub(crate) mod workspace;

pub use actions::TaskActionResult;
pub use create_subtasks::NewSubtaskInput;
pub use execution::subtasks::build_first_turn_prompt_from_context;
pub(crate) use memberships::{
    active_agent_membership, current_role_memberships_authoritative,
    human_is_active_role_member_authoritative, is_usable_active_agent, is_usable_repository_agent,
    repository_worker_identity_is_eligible, select_usable_agent_id,
    select_usable_repository_agent_id,
};
pub use subtask::{is_root_task, is_subtask, root_for};

#[cfg(test)]
use self::config::{
    execution_overrides_to_config_layer, merge_config_layers, override_value_or_empty,
    parse_config_override_layer, OverridesApplied,
};
use self::{
    config::{
        build_executor_config_snapshot, create_failed_execution_record,
        executor_snapshot_for_fresh_start, executor_snapshot_for_harness_resume, parse_json_value,
    },
    logs::execution_logs_path,
    validation::{serialize_config, validate_required},
    workspace::{default_workspace_root, prepare_workspace, reset_workspace},
};

pub(super) const DISPATCH_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(10);
pub(super) const DISPATCH_STATUS_WAIT_CEILING: Duration = Duration::from_secs(10 * 60);
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
    adapter_registry: Option<Arc<executors::HarnessAdapterRegistry>>,
    daemon_connections: Option<Arc<crate::daemon_transport::DaemonConnectionRegistry>>,
    workspace_exec_locks: Option<Arc<WorkspaceExecutionLockManager>>,
    terminal_activity: Option<Arc<TerminalActivityTracker>>,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
    workspace_root: PathBuf,
    move_operation_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    credential_env: Option<Arc<crate::credential_service::CredentialService>>,
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

pub(crate) fn execution_domain_event(
    input: &CreateExecution,
    event_type: &str,
) -> CreateDomainEvent {
    let actor = input.actor_ref.as_ref();
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: input.id.clone(),
        actor_type: actor
            .map(|actor| actor.kind().to_string())
            .unwrap_or_else(|| "system".to_owned()),
        actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
        scope_type: "task".to_owned(),
        scope_id: input.task_id.clone(),
        correlation_id: input.id.clone(),
        causation_id: input.parent_execution_id.clone(),
        causation_depth: if input.parent_execution_id.is_some() {
            1
        } else {
            0
        },
        dedupe_key: Some(format!("{event_type}:{}", input.id)),
        payload_json: serde_json::json!({
            "execution_id": input.id,
            "task_id": input.task_id,
            "role": input.role,
            "purpose": input.purpose.as_ref().map(ToString::to_string),
            "actor_kind": actor.as_ref().map(|actor| actor.kind().to_string()),
            "actor_id": actor.map(|actor| actor.id()),
            "work_unit_id": null,
        })
        .to_string(),
        created_at: input.created_at.clone(),
    }
}

pub(crate) fn execution_status_domain_event(
    execution: &Execution,
    status: &ExecutionStatus,
    created_at: &str,
) -> CreateDomainEvent {
    let event_type = match status {
        ExecutionStatus::Running => "execution.started",
        ExecutionStatus::Completed => "execution.completed",
        ExecutionStatus::Failed => "execution.failed",
        ExecutionStatus::Cancelled => "execution.cancelled",
    };
    let snapshot = execution
        .executor_config_snapshot_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .unwrap_or(Value::Null);
    let wake = snapshot.get("pr6_orchestrator_wake");
    let actor = execution.actor_ref();
    let correlation_id = wake
        .and_then(|value| value.get("correlation_id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| execution.id.clone());
    let causation_id = wake
        .and_then(|value| value.get("execution_started_event_id"))
        .or_else(|| wake.and_then(|value| value.get("event_id")))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| execution.parent_execution_id.clone());
    let causation_depth = wake
        .and_then(|value| value.get("causation_depth"))
        .and_then(Value::as_i64)
        .map(|depth| depth.saturating_add(2).min(16))
        .unwrap_or_else(|| i64::from(execution.parent_execution_id.is_some()));
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: execution.id.clone(),
        actor_type: actor
            .as_ref()
            .map(|actor| actor.kind().to_string())
            .unwrap_or_else(|| "system".to_owned()),
        actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
        scope_type: "task".to_owned(),
        scope_id: execution.task_id.clone(),
        correlation_id,
        causation_id,
        causation_depth,
        dedupe_key: Some(format!("{event_type}:{}", execution.id)),
        payload_json: serde_json::json!({
            "execution_id": execution.id,
            "task_id": execution.task_id,
            "role": execution.role,
            "purpose": execution.purpose.as_ref().map(ToString::to_string),
            "actor_kind": actor.as_ref().map(|actor| actor.kind().to_string()),
            "actor_id": execution.actor_id,
            "work_unit_id": execution.work_unit_id,
            "status": status.to_string(),
        })
        .to_string(),
        created_at: created_at.to_owned(),
    }
}

pub(crate) fn execution_stalled_domain_event(
    execution: &Execution,
    stale_before: &str,
    created_at: &str,
) -> CreateDomainEvent {
    let mut event = execution_status_domain_event(execution, &ExecutionStatus::Failed, created_at);
    event.event_type = "execution.stalled".to_owned();
    event.dedupe_key = Some(format!("execution.stalled:{}", execution.id));
    let mut payload = serde_json::from_str::<Value>(&event.payload_json).unwrap_or(Value::Null);
    if let Some(payload) = payload.as_object_mut() {
        payload.insert("stale_before".to_owned(), json!(stale_before));
    }
    event.payload_json = payload.to_string();
    event
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
        Self {
            db,
            event_bus,
            merge_service: None,
            cleanup_scheduler: None,
            review_runner: None,
            task_executor: None,
            adapter_registry: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            repo_cache_locks: None,
            workspace_root: default_workspace_root(),
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

    pub fn with_adapter_registry(
        mut self,
        adapter_registry: Arc<executors::HarnessAdapterRegistry>,
    ) -> Self {
        self.adapter_registry = Some(adapter_registry);
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

    /// Enables `auth_source: forge_provider` dispatch: harness executions for
    /// agents referencing a provider entry get the entry's API key injected
    /// into their in-memory executor environment.
    pub fn with_provider_credential_env(
        mut self,
        credentials: Arc<crate::credential_service::CredentialService>,
    ) -> Self {
        self.credential_env = Some(credentials);
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
        self.create_running_execution_with_artifact_inputs(
            input,
            workspace_created_by_attempt,
            Vec::new(),
        )
        .await
    }

    pub(crate) async fn create_running_execution_with_artifact_inputs(
        &self,
        input: CreateExecution,
        workspace_created_by_attempt: bool,
        artifact_input_ids: Vec<String>,
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
        let event = execution_domain_event(&input, "execution.started");
        let (execution, committed_event) =
            match ExecutionRepo::create_with_artifact_inputs_and_event(
                &*self.db,
                input.clone(),
                artifact_input_ids,
                event,
            )
            .await
            {
                Ok(result) => result,
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
        self.publish_committed_domain_event(&committed_event);
        Ok(execution)
    }

    pub(crate) async fn inherit_plan_artifact_inputs(
        &self,
        parent_execution_id: &str,
        task_id: &str,
    ) -> Result<Vec<String>> {
        let artifacts = crate::plan_artifact::plan_artifacts_for_execution(
            &self.db,
            task_id,
            parent_execution_id,
        )
        .await?;
        Ok(artifacts.into_iter().map(|artifact| artifact.id).collect())
    }

    fn publish_committed_domain_event(&self, event: &db::DomainEvent) {
        crate::DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .publish_committed(event);
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
        host_identity: Option<&str>,
    ) -> Result<Execution> {
        validate_required("execution_id", &notification.execution_id)?;
        let mut current_execution = ExecutionRepo::get_by_id(&*self.db, &notification.execution_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("execution", notification.execution_id.clone())
            })?;
        if current_execution.status == ExecutionStatus::Running
            && current_execution.role == crate::workflow::default_roles::REVIEWER
            && current_execution.purpose == Some(ExecutionPurpose::Review)
        {
            let expected_output = notification
                .assistant_output
                .as_deref()
                .filter(|output| !output.trim().is_empty());
            if let Some((completed, _report)) = self
                .reconcile_existing_review_report(&current_execution, None, expected_output)
                .await?
            {
                return Ok(completed);
            }
        }
        let _workspace_review_guard = if current_execution.status == ExecutionStatus::Running
            && current_execution.role == crate::workflow::default_roles::REVIEWER
            && current_execution.purpose == Some(ExecutionPurpose::Review)
        {
            if let (Some(workspace_id), Some(locks)) = (
                current_execution.workspace_id.as_deref(),
                self.workspace_exec_locks.as_ref(),
            ) {
                Some(locks.acquire(workspace_id).await)
            } else {
                None
            }
        } else {
            None
        };
        if _workspace_review_guard.is_some() {
            current_execution = ExecutionRepo::get_by_id(&*self.db, &notification.execution_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("execution", notification.execution_id.clone())
                })?;
        }
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
        let mut outcome = notification
            .status
            .as_deref()
            .unwrap_or(if succeeded { "completed" } else { "failed" })
            .to_owned();
        let mut review_result_error = None;
        if current_execution.purpose == Some(ExecutionPurpose::Plan) && outcome == "completed" {
            let assistant_output = notification
                .assistant_output
                .as_deref()
                .filter(|output| !output.trim().is_empty())
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "completed remote Plan Execution did not return a complete assistant result",
                    )
                })?;
            crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                .create_plan_artifact_from_execution(&current_execution.id, assistant_output)
                .await?;
        }
        if current_execution.role == crate::workflow::default_roles::REVIEWER
            && current_execution.purpose == Some(ExecutionPurpose::Review)
            && outcome == "completed"
        {
            let assistant_output = notification
                .assistant_output
                .as_deref()
                .filter(|output| !output.trim().is_empty());
            let materialized = if let Some(assistant_output) = assistant_output {
                let collaboration = crate::CollaborationService::new(
                    Arc::clone(&self.db),
                    Arc::clone(&self.event_bus),
                );
                match collaboration
                    .ensure_review_subject_current(&current_execution)
                    .await
                {
                    Ok(()) => collaboration
                        .create_review_report_from_execution(
                            &current_execution.id,
                            assistant_output,
                        )
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                }
            } else {
                Err(ServiceError::invalid_operation(
                    "completed remote Review Execution did not return a complete structured result",
                ))
            };
            if let Err(error) = materialized {
                outcome = "failed".to_owned();
                review_result_error = Some(format!(
                    "completed Review Execution result could not be materialized: {error}"
                ));
            }
        }
        let (status, stop_reason, stopped_by, resume_policy, stopped_at, error) =
            match outcome.as_str() {
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
                    Some(Some(review_result_error.unwrap_or_else(|| {
                        remote_terminal_error_message(notification.exit_code, signal, error)
                    }))),
                ),
            };

        let executor_unavailable = notification.failure_class
            == Some(api_types::RemoteExecutionFailureClass::ExecutorUnavailable);
        let route_effective_cwd = if notification
            .resolved_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.effective_policy.is_none())
        {
            match current_execution.workspace_id.as_deref() {
                Some(workspace_id) => WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                    .await?
                    .map(|workspace| workspace.worktree_path),
                None => None,
            }
        } else {
            None
        };
        let route_outcome = crate::task_service::config::RouteOutcome {
            selected: notification.resolved_candidate.as_ref().map(|candidate| {
                let harness_capabilities = candidate
                    .harness_capabilities
                    .as_ref()
                    .and_then(|snapshot| serde_json::to_value(snapshot).ok())
                    .unwrap_or_else(|| {
                        serde_json::to_value(api_types::HarnessCapabilities::unknown().snapshot())
                            .expect("unknown harness capability snapshot serializes")
                    });
                let effective_policy = candidate
                    .effective_policy
                    .as_ref()
                    .and_then(|policy| serde_json::to_value(policy).ok())
                    .or_else(|| {
                        crate::task_service::config::recompute_effective_policy_for_route_winner(
                            &candidate.executor_type,
                            &candidate.config,
                            self.adapter_registry.as_deref(),
                            route_effective_cwd.as_deref(),
                        )
                    });
                (
                    candidate.candidate_key.clone(),
                    candidate.executor_type.clone(),
                    candidate.config.clone(),
                    harness_capabilities,
                    effective_policy,
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
        let winner_snapshot = snapshot_update
            .as_deref()
            .or(current_execution.executor_config_snapshot_json.as_deref())
            .map(ToOwned::to_owned);

        let execution_id = notification.execution_id.clone();
        let terminal_ts = notification.ts.clone();
        let updated_at = now_rfc3339();
        let lifecycle_event =
            execution_status_domain_event(&current_execution, &status, &updated_at);
        let (updated, committed_event) = ExecutionRepo::update_with_event(
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
                updated_at,
            },
            lifecycle_event,
        )
        .await?;
        self.publish_committed_domain_event(&committed_event);

        if updated.status != ExecutionStatus::Running {
            self.revoke_active_workspace_lease_for_execution(&task.id, &updated.id)
                .await;
        }

        if let Some(account_usage) = notification.account_usage.as_ref() {
            if let Err(error) = execution::persist_account_usage_snapshot_with_host(
                &self.db,
                winner_snapshot.as_deref(),
                &updated.id,
                account_usage,
                host_identity,
            )
            .await
            {
                tracing::warn!(execution_id = %updated.id, %error, "failed to persist remote account usage snapshot");
            }
        }

        if let Some(usage) = notification.usage {
            let provider = execution::usage_provider_from_snapshot(winner_snapshot.as_deref());
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

        if updated.status == ExecutionStatus::Completed {
            if let Err(error) = execution::clear_execution_retry_metadata(&self.db, &task).await {
                tracing::warn!(
                    task_id = %task.id,
                    execution_id = %updated.id,
                    %error,
                    "failed to clear execution retry metadata"
                );
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
