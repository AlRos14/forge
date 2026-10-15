use crate::{
    agent_service::{compute_effective_status, EffectiveStatus},
    merge_service::MergeService,
    terminal_service::TerminalActivityTracker,
    workspace_cleanup::WorkspaceCleanupScheduler,
    workspace_execution_lock::WorkspaceExecutionLockManager,
    Result, ServiceError,
};
use ::workspace::RepoCacheLockManager;
use api_types::Actor;
use db::{
    new_uuid_v4, now_rfc3339, Agent, AgentRepo, CommentAuthorType, CreateDomainEvent,
    CreateExecution, CreateTask, CreateTaskComment, CreateWorkspace, CreateWorkspaceLease,
    Execution, ExecutionPurpose, ExecutionRepo, ExecutionStatus, ExecutionUsageRepo,
    HarnessSession, HarnessSessionRepo, HarnessSessionStatus, PageRequest, ProjectRepo, RepoRepo,
    SoftDeleteTask, SortBy, SortOrder, SqliteDb, Task, TaskComment, TaskCommentRepo, TaskMetadata,
    TaskRepo, UpsertExecutionUsage, WorkUnitRepo, WorkUnitWorkspaceRepo, Workspace,
    WorkspaceLeaseRepo, WorkspaceRepo, WorkspaceStatus,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::{
    ExecutionContext, ExecutionOutcome, ExecutionOverrides, ExecutorKind, TaskExecutor,
};
use serde_json::{json, Value};
use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};
use tokio::process::Command;

mod actions;
mod common;
pub(crate) mod config;
mod create;
mod create_subtasks;
pub(crate) mod execution;
pub use execution::resumable_external_session;
mod governance;
pub(crate) mod logs;
mod memberships;
mod orchestrator;
mod reorder_subtasks;
mod review;
mod roles;
mod subtask;
mod transition;
mod update;
mod validation;
pub(crate) mod workspace;

pub use actions::TaskActionResult;
pub use create_subtasks::NewSubtaskInput;
pub(crate) use memberships::{
    current_role_memberships_authoritative, human_is_active_role_member_authoritative,
    is_usable_active_agent, is_usable_repository_agent, repository_worker_identity_is_eligible,
    select_usable_agent_id, select_usable_repository_agent_id,
};
pub use subtask::{is_root_task, is_subtask, root_for};

#[cfg(test)]
use self::config::{
    execution_overrides_to_config_layer, merge_config_layers, override_value_or_empty,
    parse_config_override_layer, OverridesApplied,
};
use self::{
    config::{
        build_executor_config_snapshot, executor_snapshot_for_harness_resume, parse_json_value,
    },
    logs::execution_logs_path,
    validation::{serialize_config, validate_required},
    workspace::default_workspace_root,
};

pub(super) const DISPATCH_STATUS_POLL_INTERVAL: Duration = Duration::from_secs(10);
pub(super) const DISPATCH_STATUS_WAIT_CEILING: Duration = Duration::from_secs(10 * 60);
#[derive(Clone)]
pub struct TaskService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    merge_service: Option<Arc<MergeService>>,
    cleanup_scheduler: Option<Arc<WorkspaceCleanupScheduler>>,
    task_executor: Option<Arc<dyn TaskExecutor>>,
    adapter_registry: Option<Arc<executors::HarnessAdapterRegistry>>,
    daemon_connections: Option<Arc<crate::daemon_transport::DaemonConnectionRegistry>>,
    workspace_exec_locks: Option<Arc<WorkspaceExecutionLockManager>>,
    terminal_activity: Option<Arc<TerminalActivityTracker>>,
    repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
    workspace_root: PathBuf,
    credential_env: Option<Arc<crate::credential_service::CredentialService>>,
    #[cfg(test)]
    automatic_recovery_precommit_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    provider_start_claim_barrier: Option<Arc<tokio::sync::Barrier>>,
    #[cfg(test)]
    execution_provider_override: Option<Arc<dyn crate::daemon_transport::ExecutionProvider>>,
}

pub(crate) struct WorkUnitAdmissionLifecyclePlan {
    pub(crate) expected_task_version: i64,
    pub(crate) lifecycle: db::TaskLifecycle,
    pub(crate) transitions: Vec<db::TransitionTaskLifecycle>,
    pub(crate) clear_blocking_metadata: Option<db::ClearTaskBlockingMetadata>,
    pub(crate) follow_up_receipt: Option<db::CreateFollowUpExecutionReceipt>,
    pub(crate) automatic_recovery_parent_id: Option<String>,
}

pub(crate) struct ExplicitRecoveryAdmission {
    pub(crate) expected_task_version: i64,
    pub(crate) expected_lifecycle_version: i64,
    pub(crate) ambiguous_reason_ref: Option<String>,
    pub(crate) idempotency_key: String,
    pub(crate) receipt_dedupe_key: String,
    pub(crate) request_digest: String,
    pub(crate) requested_by: String,
}

pub(crate) struct ExecutionWorkingContext {
    pub(crate) path: PathBuf,
    pub(crate) lock_key: String,
    pub(crate) workspace: Option<Workspace>,
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

fn create_isolated_context_directory(root: &std::path::Path, segments: &[&str]) -> Result<PathBuf> {
    std::fs::create_dir_all(root).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "failed to prepare isolated execution root: {error}"
        ))
    })?;
    let canonical_root = std::fs::canonicalize(root).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "failed to resolve isolated execution root: {error}"
        ))
    })?;
    let mut current = canonical_root.clone();
    for segment in segments {
        let segment_path = std::path::Path::new(segment);
        if segment_path.components().count() != 1
            || !matches!(
                segment_path.components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(ServiceError::invalid_operation(
                "isolated execution context identity contains an invalid path component",
            ));
        }
        current.push(segment_path);
        match std::fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ServiceError::invalid_operation(format!(
                    "failed to prepare isolated execution context: {error}"
                )));
            }
        }
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to inspect isolated execution context: {error}"
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ServiceError::invalid_operation(
                "isolated execution context path must contain only real directories",
            ));
        }
        current = std::fs::canonicalize(&current).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to resolve isolated execution context: {error}"
            ))
        })?;
        if !current.starts_with(&canonical_root) {
            return Err(ServiceError::invalid_operation(
                "isolated execution context escaped its configured root",
            ));
        }
    }
    Ok(current)
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

pub struct LaunchExecutionResult {
    pub task: Task,
    pub execution: Execution,
    pub workspace: Option<Workspace>,
    pub idempotent_replay: bool,
}

impl TaskService {
    async fn prepare_non_repository_work_unit_context(
        &self,
        task: &Task,
        work_unit: &db::WorkUnit,
        actor: &db::ActorRef,
        executor_config_snapshot_json: Option<&str>,
    ) -> Result<PathBuf> {
        if work_unit.task_id != task.id
            || work_unit.requires_integration
            || work_unit.status != db::WorkUnitStatus::Open
            || work_unit.assigned_actor.as_ref() != Some(actor)
        {
            return Err(ServiceError::invalid_operation(
                "isolated execution context requires the exact open non-repository WorkUnit",
            ));
        }
        let db::ActorRef::Agent(agent_id) = actor else {
            return Err(ServiceError::invalid_operation(
                "non-repository WorkUnit execution requires an exact Agent Actor",
            ));
        };
        let snapshot = executor_config_snapshot_json.ok_or_else(|| {
            ServiceError::invalid_operation(
                "non-repository WorkUnit Execution has no immutable provider snapshot",
            )
        })?;
        let snapshot = parse_json_value("executor config snapshot", snapshot)?;
        config::validate_agent_routing_snapshot(&snapshot)?;
        if snapshot.get("agent_id").and_then(Value::as_str) != Some(agent_id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "non-repository WorkUnit provider snapshot does not match its exact Agent",
            ));
        }
        let executor_type = snapshot
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "non-repository WorkUnit provider snapshot has no executor type",
                )
            })?;
        let kind = executor_type
            .parse::<ExecutorKind>()
            .map_err(ServiceError::invalid_operation)?;
        let adapter_is_supported = if let Some(registry) = self.adapter_registry.as_deref() {
            registry.get(&kind).is_some()
        } else {
            let registry = cli_adapters::default_registry();
            registry.get(&kind).is_some()
        };
        if !adapter_is_supported {
            return Err(ServiceError::invalid_operation(format!(
                "non-repository WorkUnit provider {executor_type} has no local CLI adapter"
            )));
        }
        let daemon_id = snapshot
            .get("resolved_daemon_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "non-repository WorkUnit provider snapshot has no exact execution host",
                )
            })?;
        let daemon = db::DaemonRepo::get_by_id(&*self.db, daemon_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("daemon", daemon_id.to_owned()))?;
        if daemon.status != db::DaemonStatus::Online
            || !crate::embedded_daemon::is_embedded_daemon_machine(&daemon.machine_id)
        {
            return Err(ServiceError::invalid_operation(
                "non-repository WorkUnit context is supported only by the online embedded execution host",
            ));
        }
        create_isolated_context_directory(
            &self.workspace_root,
            &[".forge-workunit-context", &task.id, &work_unit.id],
        )
    }

    pub(crate) async fn resolve_execution_working_context(
        &self,
        execution: &Execution,
    ) -> Result<ExecutionWorkingContext> {
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        if execution.role == "orchestrator"
            && execution.purpose == Some(ExecutionPurpose::Orchestrate)
        {
            self.validate_orchestrator_execution(execution).await?;
            return Ok(ExecutionWorkingContext {
                path: create_isolated_context_directory(
                    &self.workspace_root,
                    &[".forge-orchestrator-context", &execution.id],
                )?,
                lock_key: format!("orchestrator-context:{}", execution.id),
                workspace: None,
            });
        }
        if let Some(workspace_id) = execution.workspace_id.as_deref() {
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            self.verify_active_workspace_lease(
                &task,
                &workspace,
                &execution.role,
                execution.agent_id.as_deref(),
                &execution.id,
            )
            .await?;
            return Ok(ExecutionWorkingContext {
                path: PathBuf::from(&workspace.worktree_path),
                lock_key: workspace.id.clone(),
                workspace: Some(workspace),
            });
        }
        let work_unit_id = execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("Execution has no repository or WorkUnit context")
        })?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .filter(|unit| unit.task_id == task.id)
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        if work_unit.requires_integration {
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit Execution requires its exact Workspace",
            ));
        }
        self.verify_execution_workspace_authority(execution).await?;
        let actor = execution.actor_ref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkUnit Execution has no exact Actor")
        })?;
        let path = self
            .prepare_non_repository_work_unit_context(
                &task,
                &work_unit,
                &actor,
                execution.executor_config_snapshot_json.as_deref(),
            )
            .await?;
        Ok(ExecutionWorkingContext {
            path,
            lock_key: format!("work-unit-context:{}:{}", task.id, work_unit.id),
            workspace: None,
        })
    }

    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            db,
            event_bus,
            merge_service: None,
            cleanup_scheduler: None,
            task_executor: None,
            adapter_registry: None,
            daemon_connections: None,
            workspace_exec_locks: None,
            terminal_activity: None,
            repo_cache_locks: None,
            workspace_root: default_workspace_root(),
            credential_env: None,
            #[cfg(test)]
            automatic_recovery_precommit_barrier: None,
            #[cfg(test)]
            provider_start_claim_barrier: None,
            #[cfg(test)]
            execution_provider_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_automatic_recovery_precommit_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.automatic_recovery_precommit_barrier = Some(barrier);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_provider_start_claim_barrier(
        mut self,
        barrier: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.provider_start_claim_barrier = Some(barrier);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_execution_provider_override(
        mut self,
        provider: Arc<dyn crate::daemon_transport::ExecutionProvider>,
    ) -> Self {
        self.execution_provider_override = Some(provider);
        self
    }

    pub fn with_merge_service(mut self, merge_service: Arc<MergeService>) -> Self {
        self.merge_service = Some(merge_service);
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

    /// Create a running Execution under one exact WorkUnit revision. The
    /// Execution, Artifact inputs, WorkspaceLease, and start event commit in
    /// the WorkUnit repository transaction.
    #[cfg(test)]
    pub(crate) async fn create_running_work_unit_execution_with_artifact_inputs(
        &self,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
    ) -> Result<Execution> {
        self.create_running_work_unit_execution_inner(
            work_unit_id,
            expected_work_unit_version,
            input,
            artifact_input_ids,
            None,
        )
        .await
        .map(|(_, execution, _)| execution)
    }

    pub(crate) async fn admit_running_work_unit_execution_with_artifact_inputs(
        &self,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
        lifecycle_plan: WorkUnitAdmissionLifecyclePlan,
    ) -> Result<(Task, Execution, bool)> {
        self.create_running_work_unit_execution_inner(
            work_unit_id,
            expected_work_unit_version,
            input,
            artifact_input_ids,
            Some(lifecycle_plan),
        )
        .await
    }

    async fn create_running_work_unit_execution_inner(
        &self,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
        lifecycle_plan: Option<WorkUnitAdmissionLifecyclePlan>,
    ) -> Result<(Task, Execution, bool)> {
        validate_required("work_unit_id", work_unit_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &input.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.task_id.clone()))?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        if work_unit.task_id != task.id
            || work_unit.version != expected_work_unit_version
            || work_unit.status != db::WorkUnitStatus::Open
            || work_unit.role != input.role
            || input.status != ExecutionStatus::Running
            || input.purpose.is_none()
        {
            return Err(ServiceError::Db(db::DbError::VersionConflict));
        }
        let actor = input.actor_ref.as_ref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkUnit Execution requires an exact ActorRef")
        })?;
        if work_unit.assigned_actor.as_ref() != Some(actor) {
            return Err(ServiceError::AuthorizationDenied {
                message: "Execution Actor does not match the exact WorkUnit assignment".to_owned(),
            });
        }
        match actor {
            db::ActorRef::Agent(agent_id) if input.agent_id.as_deref() != Some(agent_id) => {
                return Err(ServiceError::invalid_operation(
                    "WorkUnit Agent Execution agent_id must match its ActorRef",
                ));
            }
            db::ActorRef::Human(_) if input.agent_id.is_some() => {
                return Err(ServiceError::invalid_operation(
                    "Human WorkUnit Execution cannot carry an Agent identity",
                ));
            }
            _ => {}
        }
        if let db::ActorRef::Agent(agent_id) = actor {
            let agent = db::AgentRepo::get_by_id(&*self.db, agent_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
            let is_automatic_recovery = lifecycle_plan
                .as_ref()
                .is_some_and(|plan| plan.automatic_recovery_parent_id.is_some());
            if !is_automatic_recovery
                && !crate::agent_capacity::has_running_execution_capacity(&self.db, &agent).await?
            {
                return Err(db::DbError::AgentAtCapacity.into());
            }
        }
        self.ensure_task_runnable(&task).await?;

        let mut admitted_task = task.clone();
        if let Some(plan) = lifecycle_plan.as_ref() {
            if plan.lifecycle.task_id != task.id {
                return Err(ServiceError::invalid_operation(
                    "WorkUnit admission lifecycle plan belongs to another Task",
                ));
            }
            if task.version != plan.expected_task_version {
                return Err(db::DbError::VersionConflict.into());
            }
            let current_lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
            if current_lifecycle != plan.lifecycle {
                return Err(db::DbError::VersionConflict.into());
            }
            let lifecycle_steps = i64::try_from(plan.transitions.len()).map_err(|_| {
                ServiceError::invalid_operation("too many TaskLifecycle activation steps")
            })?;
            let metadata_step = i64::from(plan.clear_blocking_metadata.is_some());
            admitted_task.version = task
                .version
                .checked_add(lifecycle_steps + metadata_step)
                .ok_or_else(|| ServiceError::invalid_operation("Task version overflow"))?;
            admitted_task.lifecycle_state = db::TaskLifecycleState::Active;
            if let Some(clear) = plan.clear_blocking_metadata.as_ref() {
                admitted_task.error_annotation = None;
                admitted_task.blocked_json = None;
                admitted_task.metadata_json = clear.replacement_metadata_json.clone();
            }
        }

        // Keep the Workspace lock through the durable admission. The outer
        // launch paths do not hold this lock, so this single guard covers the
        // final Workspace read, lease preparation, and transaction commit.
        let _workspace_guard = match (
            self.workspace_exec_locks.as_ref(),
            input.workspace_id.as_deref(),
        ) {
            (Some(locks), Some(workspace_id)) => Some(locks.acquire(workspace_id).await),
            _ => None,
        };

        let workspace_lease = if work_unit.requires_integration {
            let workspace_id = input.workspace_id.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "repository WorkUnit Execution requires its exact Workspace",
                )
            })?;
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            if workspace.task_id != task.id
                || workspace.status != WorkspaceStatus::Ready
                || scope.task_id != task.id
                || scope.kind != db::WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
            {
                return Err(ServiceError::invalid_operation(
                    "Execution Workspace does not belong to its exact WorkUnit",
                ));
            }
            Some(
                self.prepare_work_unit_workspace_lease(
                    &admitted_task,
                    &work_unit,
                    &workspace,
                    &input,
                    actor,
                )
                .await?,
            )
        } else {
            if input.workspace_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "non-mutating WorkUnit Execution cannot bind a repository Workspace",
                ));
            }
            self.prepare_non_repository_work_unit_context(
                &task,
                &work_unit,
                actor,
                input.executor_config_snapshot_json.as_deref(),
            )
            .await?;
            None
        };

        let should_publish_unblocked = lifecycle_plan.as_ref().is_some_and(|plan| {
            plan.lifecycle.state == db::TaskLifecycleState::Blocked
                && plan.clear_blocking_metadata.is_some()
        });
        let previous_unblock_reason = task
            .blocked_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|metadata| {
                metadata
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| {
                task.error_annotation
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<api_types::TaskAnnotation>(raw).ok())
                    .and_then(|annotation| match annotation {
                        api_types::TaskAnnotation::Blocking(annotation) => {
                            Some(annotation.blocking_reason)
                        }
                        api_types::TaskAnnotation::Legacy(_) => None,
                    })
            });
        let event = execution_domain_event(&input, "execution.started");
        let admission = db::CreateWorkUnitExecution {
            execution: input,
            work_unit_id: work_unit.id,
            work_unit_version: expected_work_unit_version,
            workspace_lease,
        };
        let (execution, task, idempotent_replay) = if let Some(plan) = lifecycle_plan {
            let write = db::WorkUnitExecutionRepo::admit_with_lifecycle_and_artifact_inputs(
                &*self.db,
                db::AdmitWorkUnitExecutionWithLifecycle {
                    admission,
                    expected_task_version: plan.expected_task_version,
                    expected_lifecycle_state: plan.lifecycle.state,
                    expected_lifecycle_version: plan.lifecycle.version,
                    lifecycle_transitions: plan.transitions,
                    clear_blocking_metadata: plan.clear_blocking_metadata,
                    follow_up_receipt: plan.follow_up_receipt,
                    automatic_recovery_parent_id: plan.automatic_recovery_parent_id,
                },
                artifact_input_ids,
                event,
            )
            .await?;
            for lifecycle_event in &write.lifecycle_events {
                self.publish_committed_domain_event(lifecycle_event);
            }
            if let Some(event) = write.execution_event.as_ref() {
                self.publish_committed_domain_event(event);
            }
            let task = TaskRepo::get_by_id(&*self.db, &task.id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
            (write.execution, task, write.idempotent_replay)
        } else {
            let write = db::WorkUnitExecutionRepo::create_for_work_unit_with_artifact_inputs(
                &*self.db,
                admission,
                artifact_input_ids,
                event,
            )
            .await?;
            self.publish_committed_domain_event(&write.event);
            (write.record, task, false)
        };
        if should_publish_unblocked && !idempotent_replay {
            self.publish(ForgeEvent {
                event_type: "task.unblocked".to_owned(),
                entity_id: task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskUnblocked {
                    project_id: task.project_id.clone(),
                    previous_reason: previous_unblock_reason,
                },
            });
        }
        Ok((task, execution, idempotent_replay))
    }

    /// Recovery admission changes a blocked Task to Active and creates its
    /// exact child Execution in one SQLite transaction. Explicit operator
    /// recovery can also admit a Manual crash-recovery Execution while the
    /// aggregate Task remains Active.
    pub(crate) async fn admit_recovery_work_unit_execution_with_artifact_inputs(
        &self,
        task: &Task,
        lifecycle: &db::TaskLifecycle,
        blocked_execution_id: &str,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
    ) -> Result<(Task, Execution)> {
        let (task, execution, _) = self
            .admit_recovery_work_unit_execution_with_options(
                task,
                lifecycle,
                blocked_execution_id,
                work_unit_id,
                expected_work_unit_version,
                input,
                artifact_input_ids,
                None,
            )
            .await?;
        Ok((task, execution))
    }

    pub(crate) async fn admit_recovery_work_unit_execution_with_options(
        &self,
        task: &Task,
        lifecycle: &db::TaskLifecycle,
        blocked_execution_id: &str,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
        explicit: Option<ExplicitRecoveryAdmission>,
    ) -> Result<(Task, Execution, bool)> {
        let parent_execution_id = input.parent_execution_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("recovery Execution requires exact parent lineage")
        })?;
        let is_legacy_blocked_recovery = explicit.is_none();
        if (is_legacy_blocked_recovery
            && (lifecycle.state != db::TaskLifecycleState::Blocked
                || parent_execution_id != blocked_execution_id))
            || input.task_id != task.id
            || input.parent_execution_id.as_deref() != Some(blocked_execution_id)
            || input.status != ExecutionStatus::Running
            || input.purpose.is_none()
        {
            return Err(ServiceError::invalid_operation(
                "recovery admission requires the exact blocked Task, parent Execution, and WorkUnit target",
            ));
        }
        if let Some(explicit) = explicit.as_ref() {
            if explicit.expected_task_version != task.version
                || explicit.expected_lifecycle_version != lifecycle.version
                || task.lifecycle_state != lifecycle.state
                || !matches!(
                    lifecycle.state,
                    db::TaskLifecycleState::Active | db::TaskLifecycleState::Blocked
                )
            {
                return Err(ServiceError::Db(db::DbError::VersionConflict));
            }
            match lifecycle.state {
                db::TaskLifecycleState::Blocked => {
                    let annotation = task
                        .error_annotation
                        .as_deref()
                        .map(serde_json::from_str::<Value>)
                        .transpose()
                        .map_err(|error| {
                            ServiceError::invalid_operation(format!(
                                "invalid ambiguous recovery annotation: {error}"
                            ))
                        })?;
                    if task.blocked_json.is_some()
                        || lifecycle.reason_kind.as_deref() != Some("recovery_target_ambiguous")
                        || explicit.ambiguous_reason_ref.as_deref()
                            != lifecycle.reason_ref.as_deref()
                        || annotation.as_ref().is_some_and(|annotation| {
                            annotation.get("type").and_then(Value::as_str)
                                != Some("recovery_required")
                                || annotation.get("blocking_reason").and_then(Value::as_str)
                                    != Some("recovery_target_ambiguous")
                                || !annotation
                                    .get("blocked_execution_id")
                                    .is_some_and(Value::is_null)
                                || annotation
                                    .get("recovery_actions")
                                    .and_then(Value::as_array)
                                    .is_none_or(|actions| !actions.is_empty())
                        })
                    {
                        return Err(ServiceError::Db(db::DbError::VersionConflict));
                    }
                }
                db::TaskLifecycleState::Active => {
                    if explicit.ambiguous_reason_ref.is_some()
                        || task.error_annotation.is_some()
                        || task.blocked_json.is_some()
                        || task.failed_json.is_some()
                    {
                        return Err(ServiceError::Db(db::DbError::VersionConflict));
                    }
                }
                _ => unreachable!("explicit lifecycle state was checked above"),
            }
        }
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        if work_unit.task_id != task.id
            || work_unit.version != expected_work_unit_version
            || work_unit.status != db::WorkUnitStatus::Open
            || work_unit.role != input.role
        {
            return Err(ServiceError::Db(db::DbError::VersionConflict));
        }
        let actor = input.actor_ref.as_ref().ok_or_else(|| {
            ServiceError::invalid_operation("recovery Execution requires an exact ActorRef")
        })?;
        if work_unit.assigned_actor.as_ref() != Some(actor) {
            return Err(ServiceError::AuthorizationDenied {
                message: "recovery Actor does not match the exact WorkUnit assignment".to_owned(),
            });
        }
        match actor {
            db::ActorRef::Agent(agent_id) if input.agent_id.as_deref() != Some(agent_id) => {
                return Err(ServiceError::invalid_operation(
                    "recovery Agent Execution agent_id must match its ActorRef",
                ));
            }
            db::ActorRef::Human(_) if input.agent_id.is_some() => {
                return Err(ServiceError::invalid_operation(
                    "Human recovery Execution cannot carry an Agent identity",
                ));
            }
            _ => {}
        }
        if let db::ActorRef::Agent(agent_id) = actor {
            let agent = AgentRepo::get_by_id(&*self.db, agent_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
            if !crate::agent_capacity::has_running_execution_capacity(&self.db, &agent).await? {
                return Err(db::DbError::AgentAtCapacity.into());
            }
        }
        self.ensure_task_runnable(task).await?;

        let transitions_lifecycle = lifecycle.state == db::TaskLifecycleState::Blocked;
        let admitted_task_version = if transitions_lifecycle {
            task.version.checked_add(1).ok_or_else(|| {
                ServiceError::invalid_operation("Task version cannot be incremented for recovery")
            })?
        } else {
            task.version
        };
        let admitted_lifecycle_version = if transitions_lifecycle {
            lifecycle.version.checked_add(1).ok_or_else(|| {
                ServiceError::invalid_operation(
                    "TaskLifecycle version cannot be incremented for recovery",
                )
            })?
        } else {
            lifecycle.version
        };
        let mut admitted_task = task.clone();
        admitted_task.version = admitted_task_version;
        let _workspace_guard = match (
            self.workspace_exec_locks.as_ref(),
            work_unit
                .requires_integration
                .then(|| input.workspace_id.as_deref())
                .flatten(),
        ) {
            (Some(locks), Some(workspace_id)) => Some(locks.acquire(workspace_id).await),
            _ => None,
        };
        let workspace_lease = if work_unit.requires_integration {
            let workspace_id = input.workspace_id.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "repository recovery requires the exact WorkUnit Workspace",
                )
            })?;
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            if workspace.task_id != task.id
                || workspace.status != WorkspaceStatus::Ready
                || scope.task_id != task.id
                || scope.kind != db::WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
            {
                return Err(ServiceError::invalid_operation(
                    "recovery Workspace is not ready for the exact WorkUnit",
                ));
            }
            Some(
                self.prepare_work_unit_workspace_lease(
                    &admitted_task,
                    &work_unit,
                    &workspace,
                    &input,
                    actor,
                )
                .await?,
            )
        } else {
            if input.workspace_id.is_some()
                || WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id)
                    .await?
                    .is_some()
            {
                return Err(ServiceError::invalid_operation(
                    "non-repository recovery cannot select a WorkUnit Workspace",
                ));
            }
            self.prepare_non_repository_work_unit_context(
                &task,
                &work_unit,
                actor,
                input.executor_config_snapshot_json.as_deref(),
            )
            .await?;
            None
        };

        let updated_at = now_rfc3339();
        let lifecycle_transition = if transitions_lifecycle {
            let idempotency_key = explicit.as_ref().map_or_else(
                || format!("recovery:{}:active", blocked_execution_id),
                |request| {
                    format!(
                        "recovery:{}:{}",
                        blocked_execution_id, request.idempotency_key
                    )
                },
            );
            let lifecycle_event = CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.lifecycle_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task.id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task.id.clone(),
                correlation_id: idempotency_key.clone(),
                causation_id: Some(blocked_execution_id.to_owned()),
                causation_depth: 1,
                dedupe_key: Some(format!("task-lifecycle:{}:{}", task.id, idempotency_key)),
                payload_json: json!({
                    "task_id": task.id,
                    "from_state": db::TaskLifecycleState::Blocked,
                    "to_state": db::TaskLifecycleState::Active,
                    "from_version": lifecycle.version,
                    "to_version": admitted_lifecycle_version,
                    "cause_kind": "execution",
                    "cause_ref": blocked_execution_id,
                    "gate_evaluation_id": null,
                    "reason_kind": "execution_recovery",
                    "reason_ref": blocked_execution_id,
                })
                .to_string(),
                created_at: updated_at.clone(),
            };
            Some(db::TransitionTaskLifecycle {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                expected_task_version: explicit
                    .as_ref()
                    .map_or(task.version, |request| request.expected_task_version),
                expected_lifecycle_version: explicit
                    .as_ref()
                    .map_or(lifecycle.version, |request| {
                        request.expected_lifecycle_version
                    }),
                expected_state: db::TaskLifecycleState::Blocked,
                to_state: db::TaskLifecycleState::Active,
                cause_kind: "execution".to_owned(),
                cause_ref: Some(blocked_execution_id.to_owned()),
                reason_kind: Some("execution_recovery".to_owned()),
                reason_ref: Some(blocked_execution_id.to_owned()),
                gate_evaluation_id: None,
                idempotency_key,
                updated_at: updated_at.clone(),
                event: lifecycle_event,
            })
        } else {
            None
        };
        let execution_event = execution_domain_event(&input, "execution.started");
        let request_receipt = explicit.as_ref().map(|request| CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "execution.recovery_admitted".to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: input.id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(request.requested_by.clone()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: request.receipt_dedupe_key.clone(),
            causation_id: input.parent_execution_id.clone(),
            causation_depth: 1,
            dedupe_key: Some(request.receipt_dedupe_key.clone()),
            payload_json: json!({
                "task_id": task.id,
                "parent_execution_id": input.parent_execution_id,
                "work_unit_id": work_unit.id,
                "execution_id": input.id,
                "request_digest": request.request_digest,
                "requested_by": request.requested_by,
                "expected_task_version": request.expected_task_version,
                "expected_lifecycle_version": request.expected_lifecycle_version,
                "expected_work_unit_version": expected_work_unit_version,
            })
            .to_string(),
            created_at: updated_at.clone(),
        });
        let write = db::WorkUnitExecutionRepo::admit_recovery_for_work_unit_with_artifact_inputs(
            &*self.db,
            db::RecoverWorkUnitExecution {
                admission: db::CreateWorkUnitExecution {
                    execution: input.clone(),
                    work_unit_id: work_unit.id.clone(),
                    work_unit_version: expected_work_unit_version,
                    workspace_lease,
                },
                expected_task_version: explicit
                    .as_ref()
                    .map_or(task.version, |request| request.expected_task_version),
                expected_lifecycle_state: lifecycle.state,
                expected_lifecycle_version: explicit
                    .as_ref()
                    .map_or(lifecycle.version, |request| {
                        request.expected_lifecycle_version
                    }),
                lifecycle_transition,
                ambiguous_block_selection: explicit
                    .as_ref()
                    .and_then(|request| request.ambiguous_reason_ref.as_ref())
                    .map(|expected_reason_ref| db::ResolveAmbiguousRecoveryBlock {
                        expected_reason_ref: expected_reason_ref.clone(),
                    }),
                expected_error_annotation: task.error_annotation.clone(),
                expected_blocked_json: task.blocked_json.clone(),
                expected_metadata_json: task.metadata_json.clone(),
                request_receipt,
            },
            artifact_input_ids,
            execution_event,
        )
        .await?;
        if write.idempotent_replay {
            let current_task = TaskRepo::get_by_id(&*self.db, &task.id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
            return Ok((current_task, write.execution, true));
        }
        if let Some(event) = write.lifecycle_event.as_ref() {
            self.publish_committed_domain_event(event);
        }
        if let Some(event) = write.execution_event.as_ref() {
            self.publish_committed_domain_event(event);
        }
        if let Some(event) = write.request_receipt_event.as_ref() {
            self.publish_committed_domain_event(event);
        }

        let mut recovered_task = admitted_task;
        if transitions_lifecycle {
            recovered_task.lifecycle_state = db::TaskLifecycleState::Active;
            recovered_task.error_annotation = None;
            recovered_task.blocked_json = None;
            recovered_task.updated_at = updated_at;
        }
        if transitions_lifecycle && (task.error_annotation.is_some() || task.blocked_json.is_some())
        {
            let previous_reason = task
                .blocked_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .and_then(|metadata| {
                    metadata
                        .get("reason")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| {
                    task.error_annotation
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<api_types::TaskAnnotation>(raw).ok())
                        .and_then(|annotation| match annotation {
                            api_types::TaskAnnotation::Blocking(annotation) => {
                                Some(annotation.blocking_reason)
                            }
                            api_types::TaskAnnotation::Legacy(_) => None,
                        })
                });
            self.publish(ForgeEvent {
                event_type: "task.unblocked".to_owned(),
                entity_id: recovered_task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskUnblocked {
                    project_id: recovered_task.project_id.clone(),
                    previous_reason,
                },
            });
        }
        Ok((recovered_task, write.execution, false))
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
            && ExecutionRepo::is_provider_start_pending(&*self.db, &current_execution.id).await?
        {
            return Err(ServiceError::invalid_operation(
                "remote terminal report refers to an Execution whose provider Start was never attempted",
            ));
        }
        if current_execution.status == ExecutionStatus::Running
            && current_execution.role == "reviewer"
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
            && current_execution.role == "reviewer"
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
        if current_execution.role == "reviewer"
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
                external_session_id: notification.agent_session_id.map(Some),
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
