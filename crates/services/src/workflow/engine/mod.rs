use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Instant};

use api_types::{FailurePolicy, StateDefinition, StateKind, WorkflowDefinition, WorkflowTrigger};
use db::{
    new_uuid_v4, now_rfc3339, CreateTransitionLog, ProjectRepo, TaskRepo, TransitionLogRepo,
    UpdateTask,
};
use events::{event_timestamp, EventBus, EventContext, ForgeEvent};
use executors::TaskExecutor;
use sqlx::query;
use tracing::Instrument;
use workspace::RepoCacheLockManager;

use self::{
    context::{latest_execution_context, latest_executor_context, latest_review},
    hooks::{
        effective_after_enter_hooks, elapsed_ms, hook_audience_matches, hook_result_entry,
        log_hook_result, log_hook_skipped_by_audience, log_hook_start, merged_state_config,
    },
};
use crate::{
    deferred_dispatch,
    merge_service::MergeService,
    terminal_service::TerminalActivityTracker,
    workflow::{default_workflow, inherited_subtask_workflow, registry, HookContext, HookResult},
    workspace_cleanup::WorkspaceCleanupScheduler,
    workspace_execution_lock::WorkspaceExecutionLockManager,
    ServiceError,
};

mod context;
mod hooks;
#[cfg(test)]
mod tests;

pub struct WorkflowEngine {
    pub db: Arc<db::SqliteDb>,
    pub event_bus: Arc<EventBus>,
    pub review_runner: Option<Arc<review::ReviewRunner>>,
    pub merge_service: Option<Arc<MergeService>>,
    pub cleanup_scheduler: Option<Arc<WorkspaceCleanupScheduler>>,
    pub task_executor: Option<Arc<dyn TaskExecutor>>,
    pub daemon_connections: Option<Arc<crate::daemon_transport::DaemonConnectionRegistry>>,
    pub workspace_exec_locks: Option<Arc<WorkspaceExecutionLockManager>>,
    pub terminal_activity: Option<Arc<TerminalActivityTracker>>,
    pub workspace_root: PathBuf,
    pub repo_cache_locks: Option<Arc<RepoCacheLockManager>>,
}

pub struct TransitionResult {
    pub task: db::Task,
    pub review: Option<db::Review>,
    pub cascaded: bool,
}

impl WorkflowEngine {
    #[tracing::instrument(
        skip(self, workflow),
        fields(
            task_id = %task_id,
            target_state = %target_state,
            version = version,
            triggered_by = %triggered_by,
            reason = %reason,
            rejection = rejection,
        )
    )]
    #[allow(clippy::too_many_arguments)]
    pub async fn transition(
        &self,
        task_id: &str,
        target_state: &str,
        version: i64,
        workflow: &WorkflowDefinition,
        triggered_by: &str,
        reason: &str,
        rejection: bool,
    ) -> crate::Result<TransitionResult> {
        self.transition_with_deferred_dispatch(
            task_id,
            target_state,
            version,
            workflow,
            triggered_by,
            reason,
            rejection,
            None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn transition_with_deferred_dispatch(
        &self,
        task_id: &str,
        target_state: &str,
        version: i64,
        workflow: &WorkflowDefinition,
        triggered_by: &str,
        reason: &str,
        rejection: bool,
        defer_dispatch_until: Option<String>,
    ) -> crate::Result<TransitionResult> {
        self.transition_inner(
            task_id.to_string(),
            target_state.to_string(),
            version,
            workflow,
            triggered_by.to_string(),
            reason.to_string(),
            rejection,
            false,
            defer_dispatch_until,
            0,
        )
        .await
    }

    pub async fn manual_override_transition(
        &self,
        task_id: &str,
        target_state: &str,
        version: i64,
        workflow: &WorkflowDefinition,
        reason: &str,
        rejection: bool,
    ) -> crate::Result<TransitionResult> {
        self.transition_inner(
            task_id.to_string(),
            target_state.to_string(),
            version,
            workflow,
            "system".to_string(),
            reason.to_string(),
            rejection,
            true,
            None,
            0,
        )
        .await
    }

    pub async fn retry_entry_barrier(
        &self,
        task_id: &str,
        version: i64,
        workflow: &WorkflowDefinition,
        triggered_by: &str,
        reason: &str,
    ) -> crate::Result<TransitionResult> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        if task.version != version {
            return Err(db::DbError::VersionConflict.into());
        }
        let Some(raw_barrier) = task.entry_barrier_json.as_deref() else {
            return Err(ServiceError::invalid_operation(
                "task has no blocked entry barrier to retry",
            ));
        };
        let barrier: serde_json::Value = serde_json::from_str(raw_barrier).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid entry barrier metadata: {error}"))
        })?;
        if barrier.get("status").and_then(serde_json::Value::as_str) != Some("blocked") {
            return Err(ServiceError::invalid_operation(
                "task entry barrier is not blocked",
            ));
        }
        let target_state = barrier
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(task.status.as_str())
            .to_owned();
        if target_state != task.status {
            return Err(ServiceError::invalid_operation(format!(
                "blocked entry barrier targets state '{}' but task is in '{}'",
                target_state, task.status
            )));
        }
        let state = Self::find_state(workflow, &target_state).ok_or_else(|| {
            ServiceError::InvalidOperation {
                message: format!("state '{target_state}' is not defined in workflow"),
            }
        })?;

        let started_at = barrier
            .get("started_at")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(now_rfc3339);
        let retry_started_at = now_rfc3339();
        let running_barrier = serde_json::json!({
            "state": target_state.as_str(),
            "status": "running",
            "started_at": started_at.as_str(),
            "retry_started_at": retry_started_at.as_str(),
            "retry_reason": reason,
        })
        .to_string();
        let mut task = TaskRepo::set_entry_barrier(
            &*self.db,
            task_id,
            task.version,
            Some(running_barrier),
            &retry_started_at,
        )
        .await?;

        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let state_config =
            merged_state_config(state, Some(&project), task.task_state_config.as_deref());
        let workflow_ctx = Arc::new(workflow.clone());
        let latest_execution = latest_execution_context(&self.db, &task.id).await?;
        let latest_executor = latest_executor_context(&self.db, &task.id).await?;
        let workspace_id = latest_execution
            .as_ref()
            .and_then(|execution| execution.workspace_id.clone())
            .or_else(|| {
                latest_executor
                    .as_ref()
                    .and_then(|execution| execution.workspace_id.clone())
            });
        let execution_id = latest_executor
            .as_ref()
            .map(|execution| execution.id.clone())
            .or_else(|| {
                latest_execution
                    .as_ref()
                    .map(|execution| execution.id.clone())
            });
        let enter_ctx = HookContext {
            task_id: task.id.clone(),
            project_id: task.project_id.clone(),
            from_state: target_state.clone(),
            to_state: target_state.clone(),
            db: Arc::clone(&self.db),
            event_bus: Arc::clone(&self.event_bus),
            gate_config: state.gate_config.clone(),
            workflow: Arc::clone(&workflow_ctx),
            triggered_by: triggered_by.to_owned(),
            review_runner: self.review_runner.clone(),
            merge_service: self.merge_service.clone(),
            cleanup_scheduler: self.cleanup_scheduler.clone(),
            task_executor: self.task_executor.clone(),
            daemon_connections: self.daemon_connections.clone(),
            workspace_exec_locks: self.workspace_exec_locks.clone(),
            terminal_activity: self.terminal_activity.clone(),
            workspace_root: self.workspace_root.clone(),
            repo_cache_locks: self.repo_cache_locks.clone(),
            workspace_id,
            agent_id: latest_execution
                .as_ref()
                .and_then(|execution| execution.agent_id.clone()),
            execution_id,
            state_config,
        };

        let mut cascade: Option<(String, String)> = None;
        let mut blocked = false;
        for hook in &state.hooks.before_enter {
            if !hook_audience_matches(hook.applies_to, triggered_by) {
                log_hook_skipped_by_audience(
                    &task.id,
                    &target_state,
                    &target_state,
                    "before_enter",
                    hook,
                    triggered_by,
                );
                continue;
            }
            let action = registry::resolve_action(&hook.action)?;
            log_hook_start(
                &task.id,
                &target_state,
                &target_state,
                "before_enter",
                hook,
                triggered_by,
            );
            let started = Instant::now();
            let result = action.execute(&enter_ctx).await;
            let duration_ms = elapsed_ms(started);
            log_hook_result(
                &task.id,
                &target_state,
                &target_state,
                "before_enter",
                hook,
                &result,
                duration_ms,
            );
            match result {
                HookResult::Failed { reason: error } => {
                    if matches!(hook.on_failure, FailurePolicy::Block) {
                        let blocked_at = now_rfc3339();
                        let blocked_barrier = serde_json::json!({
                            "state": target_state.as_str(),
                            "status": "blocked",
                            "started_at": started_at.as_str(),
                            "updated_at": blocked_at.as_str(),
                            "blocking_reason": error.as_str(),
                            "retry_reason": reason,
                        })
                        .to_string();
                        task = TaskRepo::set_entry_barrier(
                            &*self.db,
                            task_id,
                            task.version,
                            Some(blocked_barrier),
                            &blocked_at,
                        )
                        .await?;
                        blocked = true;
                        break;
                    }
                }
                HookResult::Cascade {
                    to,
                    reason: cascade_reason,
                } => {
                    cascade = Some((to, cascade_reason));
                    break;
                }
                HookResult::Ok | HookResult::Skipped { .. } => {}
            }
        }

        if blocked {
            let review = latest_review(&self.db, &task.id).await?;
            return Ok(TransitionResult {
                task,
                review,
                cascaded: false,
            });
        }

        if cascade.is_none() {
            let cleared_at = now_rfc3339();
            task = TaskRepo::set_entry_barrier(&*self.db, task_id, task.version, None, &cleared_at)
                .await?;
            task = TaskRepo::update(
                &*self.db,
                UpdateTask {
                    id: task.id.clone(),
                    expected_version: task.version,
                    title: None,
                    description: None,
                    priority: None,
                    merge_config: None,
                    plan: None,
                    error_annotation: Some(None),
                    blocked_json: Some(None),
                    failed_json: None,
                    task_state_config: None,
                    parent_task_id: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;

            for hook in &state.hooks.on_enter {
                if !hook_audience_matches(hook.applies_to, triggered_by) {
                    continue;
                }
                let action = registry::resolve_action(&hook.action)?;
                let result = action.execute(&enter_ctx).await;
                if let HookResult::Cascade {
                    to,
                    reason: cascade_reason,
                } = result
                {
                    cascade = Some((to, cascade_reason));
                    break;
                }
            }
        }

        if cascade.is_none() {
            let effective_after_enter_hooks = effective_after_enter_hooks(state);
            for hook in &effective_after_enter_hooks {
                if !hook_audience_matches(hook.applies_to, triggered_by) {
                    continue;
                }
                let action = registry::resolve_action(&hook.action)?;
                let result = action.execute(&enter_ctx).await;
                if let HookResult::Cascade {
                    to,
                    reason: cascade_reason,
                } = result
                {
                    cascade = Some((to, cascade_reason));
                    break;
                }
            }
        }

        if let Some((cascade_to, cascade_reason)) = cascade {
            let mut cascaded = self
                .transition_inner(
                    task_id.to_owned(),
                    cascade_to,
                    task.version,
                    workflow,
                    "system".to_string(),
                    cascade_reason,
                    false,
                    false,
                    None,
                    1,
                )
                .await?;
            cascaded.cascaded = true;
            return Ok(cascaded);
        }

        let review = latest_review(&self.db, &task.id).await?;
        Ok(TransitionResult {
            task,
            review,
            cascaded: false,
        })
    }

    #[tracing::instrument(
        skip(self, workflow),
        fields(task_id = %task_id, target_state = %target_state, version = version, triggered_by = %triggered_by, reason = %reason)
    )]
    pub async fn reset_to_initial(
        &self,
        task_id: &str,
        target_state: &str,
        version: i64,
        workflow: &WorkflowDefinition,
        triggered_by: &str,
        reason: &str,
    ) -> crate::Result<db::Task> {
        let to_state = Self::find_state(workflow, target_state).ok_or_else(|| {
            ServiceError::InvalidOperation {
                message: format!("state '{target_state}' is not defined in workflow"),
            }
        })?;
        if to_state.kind != StateKind::Initial {
            return Err(ServiceError::InvalidOperation {
                message: format!("state '{target_state}' is not the workflow initial state"),
            });
        }

        let result = self
            .transition_inner(
                task_id.to_string(),
                target_state.to_string(),
                version,
                workflow,
                triggered_by.to_string(),
                reason.to_string(),
                false,
                true,
                None,
                0,
            )
            .await?;
        Ok(result.task)
    }

    pub fn validate_claimable(
        workflow: &WorkflowDefinition,
        current_status: &str,
    ) -> crate::Result<()> {
        if let Some(state) = Self::find_state(workflow, current_status) {
            if state.kind == StateKind::Backlog {
                return Err(ServiceError::InvalidOperation {
                    message: "task is in backlog and cannot be claimed".to_string(),
                });
            }
        }
        Ok(())
    }

    fn transition_requires_system_actor(
        trigger: WorkflowTrigger,
        from_state: &StateDefinition,
        to_state: &StateDefinition,
    ) -> bool {
        if !trigger.system_only() {
            return false;
        }

        let is_direct_work_start = trigger == WorkflowTrigger::Retry
            && from_state.kind == StateKind::Initial
            && to_state.kind == StateKind::Active;
        !is_direct_work_start
    }

    pub fn resolve_workflow(workflow_definition_json: &str) -> WorkflowDefinition {
        let raw = workflow_definition_json.trim();
        if raw.is_empty() || raw == "{}" {
            return default_workflow::default_workflow();
        }

        serde_json::from_str(raw).unwrap_or_else(|_| default_workflow::default_workflow())
    }

    pub fn resolve_subtask_workflow() -> WorkflowDefinition {
        inherited_subtask_workflow()
    }

    pub fn resolve_workflow_for(
        task: &db::Task,
        workflow_definition_json: &str,
    ) -> WorkflowDefinition {
        if task.parent_task_id.is_some() {
            return inherited_subtask_workflow();
        }

        Self::resolve_workflow(workflow_definition_json)
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_inner<'a>(
        &'a self,
        task_id: String,
        target_state: String,
        version: i64,
        workflow: &'a WorkflowDefinition,
        triggered_by: String,
        reason: String,
        rejection: bool,
        skip_before_exit: bool,
        defer_dispatch_until: Option<String>,
        depth: u8,
    ) -> Pin<Box<dyn Future<Output = crate::Result<TransitionResult>> + Send + 'a>> {
        let span = tracing::info_span!(
            "workflow.transition_inner",
            task_id = %task_id,
            target_state = %target_state,
            version = version,
            triggered_by = %triggered_by,
            reason = %reason,
            rejection = rejection,
            skip_before_exit = skip_before_exit,
            defer_dispatch = defer_dispatch_until.is_some(),
            depth = depth,
        );

        Box::pin(async move {
            let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;

            if task.version != version {
                tracing::warn!(
                    task_id = %task.id,
                    expected_version = version,
                    actual_version = task.version,
                    current_state = %task.status,
                    target_state = %target_state,
                    triggered_by = %triggered_by,
                    "workflow transition rejected by version conflict"
                );
                return Err(db::DbError::VersionConflict.into());
            }

            let current_status = task.status.to_string();
            tracing::debug!(
                task_id = %task.id,
                from_state = %current_status,
                to_state = %target_state,
                triggered_by = %triggered_by,
                reason = %reason,
                depth = depth,
                "workflow transition requested"
            );
            let from_state = Self::find_state(workflow, &current_status).ok_or_else(|| {
                ServiceError::InvalidOperation {
                    message: format!("state '{current_status}' is not defined in workflow"),
                }
            })?;
            let to_state = Self::find_state(workflow, &target_state).ok_or_else(|| {
                ServiceError::InvalidOperation {
                    message: format!("state '{target_state}' is not defined in workflow"),
                }
            })?;
            let transition = workflow.trigger_between(&current_status, &target_state);
            let trigger_name = transition.map(|trigger| trigger.as_str().to_owned());
            let effective_skip_before_exit = match transition {
                Some(trigger) => {
                    if !skip_before_exit
                        && Self::transition_requires_system_actor(
                            trigger, from_state, to_state,
                        )
                        && triggered_by != "system"
                    {
                        tracing::warn!(
                            task_id = %task.id,
                            from_state = %current_status,
                            to_state = %target_state,
                            workflow_trigger = ?trigger,
                            triggered_by = %triggered_by,
                            "workflow transition rejected because it is system-only"
                        );
                        return Err(ServiceError::InvalidOperation {
                            message: format!(
                                "transition {} -> {} is system-only",
                                current_status, target_state
                            ),
                        });
                    }
                    skip_before_exit
                }
                None if skip_before_exit && to_state.kind == StateKind::Initial => true,
                None if skip_before_exit && current_status == target_state => true,
                None if Self::is_cancellation_target(workflow, &target_state)
                    && from_state.kind != StateKind::Terminal =>
                {
                    true
                }
                None => {
                    tracing::warn!(
                        task_id = %task.id,
                        from_state = %current_status,
                        to_state = %target_state,
                        from_kind = ?from_state.kind,
                        to_kind = ?to_state.kind,
                        triggered_by = %triggered_by,
                        reason = %reason,
                        "workflow transition rejected because no transition is defined"
                    );
                    return Err(ServiceError::Db(db::DbError::InvalidTransition));
                }
            };
            tracing::info!(
                task_id = %task.id,
                from_state = %current_status,
                to_state = %target_state,
                from_kind = ?from_state.kind,
                to_kind = ?to_state.kind,
                workflow_trigger = ?transition,
                triggered_by = %triggered_by,
                reason = %reason,
                rejection = rejection,
                skip_before_exit = effective_skip_before_exit,
                depth = depth,
                "workflow transition accepted"
            );

            let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
            let from_state_config =
                merged_state_config(from_state, Some(&project), task.task_state_config.as_deref());
            let to_state_config =
                merged_state_config(to_state, Some(&project), task.task_state_config.as_deref());
            let workflow_ctx = Arc::new(workflow.clone());
            let latest_execution = latest_execution_context(&self.db, &task.id).await?;
            let latest_executor = latest_executor_context(&self.db, &task.id).await?;
            let workspace_id = latest_execution
                .as_ref()
                .and_then(|execution| execution.workspace_id.clone())
                .or_else(|| {
                    latest_executor
                        .as_ref()
                        .and_then(|execution| execution.workspace_id.clone())
                });
            let execution_id = latest_executor
                .as_ref()
                .map(|execution| execution.id.clone())
                .or_else(|| {
                    latest_execution
                        .as_ref()
                        .map(|execution| execution.id.clone())
                });

            let exit_ctx = HookContext {
                task_id: task.id.clone(),
                project_id: task.project_id.clone(),
                from_state: current_status.clone(),
                to_state: target_state.clone(),
                db: Arc::clone(&self.db),
                event_bus: Arc::clone(&self.event_bus),
                gate_config: from_state.gate_config.clone(),
                workflow: Arc::clone(&workflow_ctx),
                triggered_by: triggered_by.clone(),
                review_runner: self.review_runner.clone(),
                merge_service: self.merge_service.clone(),
                cleanup_scheduler: self.cleanup_scheduler.clone(),
                task_executor: self.task_executor.clone(),
                daemon_connections: self.daemon_connections.clone(),
                workspace_exec_locks: self.workspace_exec_locks.clone(),
                terminal_activity: self.terminal_activity.clone(),
                workspace_root: self.workspace_root.clone(),
                repo_cache_locks: self.repo_cache_locks.clone(),
                workspace_id: workspace_id.clone(),
                agent_id: latest_execution
                    .as_ref()
                    .and_then(|execution| execution.agent_id.clone()),
                execution_id: execution_id.clone(),
                state_config: from_state_config,
            };
            let enter_ctx = HookContext {
                task_id: task.id.clone(),
                project_id: task.project_id.clone(),
                from_state: current_status.clone(),
                to_state: target_state.clone(),
                db: Arc::clone(&self.db),
                event_bus: Arc::clone(&self.event_bus),
                gate_config: to_state.gate_config.clone(),
                workflow: Arc::clone(&workflow_ctx),
                triggered_by: triggered_by.clone(),
                review_runner: self.review_runner.clone(),
                merge_service: self.merge_service.clone(),
                cleanup_scheduler: self.cleanup_scheduler.clone(),
                task_executor: self.task_executor.clone(),
                daemon_connections: self.daemon_connections.clone(),
                workspace_exec_locks: self.workspace_exec_locks.clone(),
                terminal_activity: self.terminal_activity.clone(),
                workspace_root: self.workspace_root.clone(),
                repo_cache_locks: self.repo_cache_locks.clone(),
                workspace_id,
                agent_id: latest_execution
                    .as_ref()
                    .and_then(|execution| execution.agent_id.clone()),
                execution_id,
                state_config: to_state_config,
            };

            let mut hook_results = Vec::new();
            let mut cascade: Option<(String, String)> = None;
            let mut skip_target_enter_hooks = false;
            let has_blocking_before_enter = to_state
                .hooks
                .before_enter
                .iter()
                .any(|hook| matches!(hook.on_failure, FailurePolicy::Block));

            if !effective_skip_before_exit {
                for hook in &from_state.hooks.before_exit {
                    if !hook_audience_matches(hook.applies_to, &triggered_by) {
                        log_hook_skipped_by_audience(
                            &task.id,
                            &current_status,
                            &target_state,
                            "before_exit",
                            hook,
                            &triggered_by,
                        );
                        continue;
                    }
                    let action = registry::resolve_action(&hook.action)?;
                    log_hook_start(
                        &task.id,
                        &current_status,
                        &target_state,
                        "before_exit",
                        hook,
                        &triggered_by,
                    );
                    let started = Instant::now();
                    let result = action.execute(&exit_ctx).await;
                    let duration_ms = elapsed_ms(started);
                    log_hook_result(
                        &task.id,
                        &current_status,
                        &target_state,
                        "before_exit",
                        hook,
                        &result,
                        duration_ms,
                    );
                    hook_results.push(hook_result_entry(
                        &hook.action,
                        "before_exit",
                        &result,
                        duration_ms,
                    ));

                    if let HookResult::Failed {
                        reason: guard_reason,
                    } = result
                    {
                        if matches!(hook.on_failure, FailurePolicy::Block) {
                            tracing::warn!(
                                task_id = %task.id,
                                from_state = %current_status,
                                to_state = %target_state,
                                guard_name = %hook.action,
                                reason = %guard_reason,
                                "workflow guard rejected transition"
                            );
                            self.event_bus.publish(ForgeEvent {
                                event_type: "transition.guard_rejected".to_string(),
                                entity_id: task.id.clone(),
                                timestamp: event_timestamp(),
                                context: EventContext::TransitionGuardRejected {
                                    task_id: task.id.clone(),
                                    from_state: current_status.clone(),
                                    to_state: target_state.clone(),
                                    guard_name: hook.action.clone(),
                                    reason: guard_reason.clone(),
                                },
                            });

                            return Err(ServiceError::GuardRejection {
                                guard: hook.action.clone(),
                                reason: guard_reason,
                            });
                        }
                    }
                }
            }

            let updated_at = now_rfc3339();
            let entry_barrier_started_at = updated_at.clone();
            let entry_barrier_json = has_blocking_before_enter
                .then(|| {
                    serde_json::json!({
                        "state": target_state.as_str(),
                        "status": "running",
                        "started_at": entry_barrier_started_at.as_str(),
                    })
                    .to_string()
                });
            let reopens_visible_work = from_state.kind == StateKind::Terminal
                && to_state.kind != StateKind::Terminal
                && !task.is_automation;
            let mut transaction = self.db.pool().begin().await?;
            let update = query(
                "UPDATE task\n                 SET status = ?, version = version + 1, updated_at = ?, blocked_json = NULL, entry_barrier_json = ?\n                 WHERE id = ? AND version = ? AND deleted_at IS NULL",
            )
            .bind(&target_state)
            .bind(&updated_at)
            .bind(entry_barrier_json.as_deref())
            .bind(&task_id)
            .bind(version)
            .execute(&mut *transaction)
            .await?;

            if update.rows_affected() != 1 {
                return Err(db::DbError::VersionConflict.into());
            }
            if reopens_visible_work {
                ProjectRepo::increment_project_work_epoch(
                    &*self.db,
                    &mut transaction,
                    &task.project_id,
                    1,
                )
                .await?;
            }
            transaction.commit().await?;

            let mut task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
            let should_defer_dispatch = defer_dispatch_until.is_some()
                && to_state.kind != StateKind::Active
                && to_state
                    .hooks
                    .on_enter
                    .iter()
                    .any(|hook| hook.action == "dispatch_role_agent");
            if should_defer_dispatch {
                deferred_dispatch::set(
                    &self.db,
                    &task,
                    &target_state,
                    defer_dispatch_until
                        .as_deref()
                        .expect("deferred dispatch timestamp exists"),
                    "board drag dispatch cooldown",
                )
                .await?;
            } else if deferred_dispatch::pending_until(&task).is_some() {
                deferred_dispatch::clear(&self.db, &task).await?;
                task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
            }

            let transition_log = TransitionLogRepo::insert(
                &*self.db,
                CreateTransitionLog {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    from_state: current_status.clone(),
                    to_state: target_state.clone(),
                    trigger_name,
                    triggered_by: triggered_by.clone(),
                    trigger_reason: reason.clone(),
                    hook_results_json: None,
                    rejection,
                    created_at: updated_at,
                },
            )
            .await?;

            tracing::info!(
                task_id = %task.id,
                from_state = %current_status,
                to_state = %target_state,
                triggered_by = %triggered_by,
                reason = %reason,
                transition_log_id = %transition_log.id,
                "workflow transition applied"
            );

            self.event_bus.publish(ForgeEvent {
                event_type: "task.status_changed".to_string(),
                entity_id: task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskStatusChanged {
                    project_id: task.project_id.clone(),
                    old_status: current_status.clone(),
                    new_status: task.status.to_string(),
                },
            });

            for hook in &from_state.hooks.on_exit {
                if !hook_audience_matches(hook.applies_to, &triggered_by) {
                    log_hook_skipped_by_audience(
                        &task.id,
                        &current_status,
                        &target_state,
                        "on_exit",
                        hook,
                        &triggered_by,
                    );
                    continue;
                }

                let action = registry::resolve_action(&hook.action)?;
                log_hook_start(
                    &task.id,
                    &current_status,
                    &target_state,
                    "on_exit",
                    hook,
                    &triggered_by,
                );
                let started = Instant::now();
                let result = action.execute(&exit_ctx).await;
                let duration_ms = elapsed_ms(started);
                log_hook_result(
                    &task.id,
                    &current_status,
                    &target_state,
                    "on_exit",
                    hook,
                    &result,
                    duration_ms,
                );
                hook_results.push(hook_result_entry(
                    &hook.action,
                    "on_exit",
                    &result,
                    duration_ms,
                ));

                match result {
                    HookResult::Failed { reason: error } => {
                        tracing::warn!(
                            action = %hook.action,
                            task_id = %task.id,
                            from_state = %current_status,
                            to_state = %target_state,
                            %error,
                            "workflow effect failed on_exit"
                        );
                        self.event_bus.publish(ForgeEvent {
                            event_type: "transition.effect_failed".to_string(),
                            entity_id: task.id.clone(),
                            timestamp: event_timestamp(),
                            context: EventContext::TransitionEffectFailed {
                                task_id: task.id.clone(),
                                from_state: current_status.clone(),
                                to_state: target_state.clone(),
                                action: hook.action.clone(),
                                error,
                            },
                        });
                    }
                    HookResult::Cascade {
                        to,
                        reason: cascade_reason,
                    } => {
                        cascade = Some((to, cascade_reason));
                        break;
                    }
                    HookResult::Ok | HookResult::Skipped { .. } => {}
                }
            }

            if cascade.is_none() {
                let mut before_enter_blocked = false;
                let mut before_enter_barrier_resolved = false;
                for hook in &to_state.hooks.before_enter {
                    if !hook_audience_matches(hook.applies_to, &triggered_by) {
                        log_hook_skipped_by_audience(
                            &task.id,
                            &current_status,
                            &target_state,
                            "before_enter",
                            hook,
                            &triggered_by,
                        );
                        continue;
                    }
                    let action = registry::resolve_action(&hook.action)?;
                    log_hook_start(
                        &task.id,
                        &current_status,
                        &target_state,
                        "before_enter",
                        hook,
                        &triggered_by,
                    );
                    let started = Instant::now();
                    let result = action.execute(&enter_ctx).await;
                    let duration_ms = elapsed_ms(started);
                    log_hook_result(
                        &task.id,
                        &current_status,
                        &target_state,
                        "before_enter",
                        hook,
                        &result,
                        duration_ms,
                    );
                    hook_results.push(hook_result_entry(
                        &hook.action,
                        "before_enter",
                        &result,
                        duration_ms,
                    ));

                    match result {
                        HookResult::Failed { reason: error } => {
                            tracing::warn!(
                                action = %hook.action,
                                task_id = %task.id,
                                from_state = %current_status,
                                to_state = %target_state,
                                %error,
                                "workflow effect failed before_enter"
                            );
                            self.event_bus.publish(ForgeEvent {
                                event_type: "transition.effect_failed".to_string(),
                                entity_id: task.id.clone(),
                                timestamp: event_timestamp(),
                                context: EventContext::TransitionEffectFailed {
                                    task_id: task.id.clone(),
                                    from_state: current_status.clone(),
                                    to_state: target_state.clone(),
                                    action: hook.action.clone(),
                                    error: error.clone(),
                                },
                            });

                            if matches!(hook.on_failure, FailurePolicy::Block) {
                                if target_state == crate::workflow::default_states::REVIEW {
                                    if let Some(reject_target) = to_state
                                        .gate_config
                                        .as_ref()
                                        .and_then(|config| config.reject_target.clone())
                                    {
                                        let existing_rejections =
                                            TransitionLogRepo::count_gate_rejections(
                                                &*self.db,
                                                &task_id,
                                                &target_state,
                                            )
                                            .await
                                            .unwrap_or(0);
                                        let max_rejections = to_state
                                            .gate_config
                                            .as_ref()
                                            .and_then(|gc| gc.max_rejections)
                                            .unwrap_or(i32::MAX);

                                        if existing_rejections + 1 >= i64::from(max_rejections) {
                                            let blocked_at = now_rfc3339();
                                            let barrier = serde_json::json!({
                                                "state": target_state.as_str(),
                                                "status": "blocked",
                                                "started_at": entry_barrier_started_at.as_str(),
                                                "updated_at": blocked_at.as_str(),
                                                "blocking_reason": "review retry budget exhausted",
                                            })
                                            .to_string();
                                            task = TaskRepo::set_entry_barrier(
                                                &*self.db,
                                                &task_id,
                                                task.version,
                                                Some(barrier),
                                                &blocked_at,
                                            )
                                            .await?;
                                            before_enter_blocked = true;
                                            skip_target_enter_hooks = true;
                                        } else {
                                            let clear_updated_at = now_rfc3339();
                                            task = TaskRepo::set_entry_barrier(
                                                &*self.db,
                                                &task_id,
                                                task.version,
                                                None,
                                                &clear_updated_at,
                                            )
                                            .await?;
                                            before_enter_barrier_resolved = true;
                                            cascade = Some((reject_target, error));
                                        }
                                    } else {
                                        let blocked_at = now_rfc3339();
                                        let barrier = serde_json::json!({
                                            "state": target_state.as_str(),
                                            "status": "blocked",
                                            "started_at": entry_barrier_started_at.as_str(),
                                            "updated_at": blocked_at.as_str(),
                                            "blocking_reason": error.as_str(),
                                        })
                                        .to_string();
                                        task = TaskRepo::set_entry_barrier(
                                            &*self.db,
                                            &task_id,
                                            task.version,
                                            Some(barrier),
                                            &blocked_at,
                                        )
                                        .await?;
                                        before_enter_blocked = true;
                                        skip_target_enter_hooks = true;
                                    }
                                } else {
                                    let blocked_at = now_rfc3339();
                                    let barrier = serde_json::json!({
                                        "state": target_state.as_str(),
                                        "status": "blocked",
                                        "started_at": entry_barrier_started_at.as_str(),
                                        "updated_at": blocked_at.as_str(),
                                        "blocking_reason": error.as_str(),
                                    })
                                    .to_string();
                                    task = TaskRepo::set_entry_barrier(
                                        &*self.db,
                                        &task_id,
                                        task.version,
                                        Some(barrier),
                                        &blocked_at,
                                    )
                                    .await?;
                                    before_enter_blocked = true;
                                    skip_target_enter_hooks = true;
                                }
                                break;
                            }
                        }
                        HookResult::Cascade {
                            to,
                            reason: cascade_reason,
                        } => {
                            cascade = Some((to, cascade_reason));
                            break;
                        }
                        HookResult::Ok | HookResult::Skipped { .. } => {}
                    }
                }

                if has_blocking_before_enter
                    && !before_enter_blocked
                    && !before_enter_barrier_resolved
                {
                    let clear_updated_at = now_rfc3339();
                    let cleared_task = TaskRepo::set_entry_barrier(
                        &*self.db,
                        &task_id,
                        task.version,
                        None,
                        &clear_updated_at,
                    )
                    .await?;
                    task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", cleared_task.id))?;
                }
            }

            if cascade.is_none() && !skip_target_enter_hooks {
                for hook in &to_state.hooks.on_enter {
                    if !hook_audience_matches(hook.applies_to, &triggered_by) {
                        log_hook_skipped_by_audience(
                            &task.id,
                            &current_status,
                            &target_state,
                            "on_enter",
                            hook,
                            &triggered_by,
                        );
                        continue;
                    }
                    if should_defer_dispatch && hook.action == "dispatch_role_agent" {
                        let result = HookResult::Skipped {
                            reason: "dispatch deferred after board drag".to_owned(),
                        };
                        log_hook_result(
                            &task.id,
                            &current_status,
                            &target_state,
                            "on_enter",
                            hook,
                            &result,
                            0,
                        );
                        hook_results.push(hook_result_entry(&hook.action, "on_enter", &result, 0));
                        continue;
                    }

                    let action = registry::resolve_action(&hook.action)?;
                    log_hook_start(
                        &task.id,
                        &current_status,
                        &target_state,
                        "on_enter",
                        hook,
                        &triggered_by,
                    );
                    let started = Instant::now();
                    let result = action.execute(&enter_ctx).await;
                    let duration_ms = elapsed_ms(started);
                    log_hook_result(
                        &task.id,
                        &current_status,
                        &target_state,
                        "on_enter",
                        hook,
                        &result,
                        duration_ms,
                    );
                    hook_results.push(hook_result_entry(
                        &hook.action,
                        "on_enter",
                        &result,
                        duration_ms,
                    ));

                    match result {
                        HookResult::Failed { reason: error } => {
                            tracing::warn!(
                                action = %hook.action,
                                task_id = %task.id,
                                from_state = %current_status,
                                to_state = %target_state,
                                %error,
                                "workflow effect failed on_enter"
                            );
                            self.event_bus.publish(ForgeEvent {
                                event_type: "transition.effect_failed".to_string(),
                                entity_id: task.id.clone(),
                                timestamp: event_timestamp(),
                                context: EventContext::TransitionEffectFailed {
                                    task_id: task.id.clone(),
                                    from_state: current_status.clone(),
                                    to_state: target_state.clone(),
                                    action: hook.action.clone(),
                                    error,
                                },
                            });
                        }
                        HookResult::Cascade {
                            to,
                            reason: cascade_reason,
                        } => {
                            cascade = Some((to, cascade_reason));
                            break;
                        }
                        HookResult::Ok | HookResult::Skipped { .. } => {}
                    }
                }
            }

            if cascade.is_none() && !skip_target_enter_hooks {
                let effective_after_enter_hooks = effective_after_enter_hooks(to_state);
                for hook in &effective_after_enter_hooks {
                    if !hook_audience_matches(hook.applies_to, &triggered_by) {
                        log_hook_skipped_by_audience(
                            &task.id,
                            &current_status,
                            &target_state,
                            "after_enter",
                            hook,
                            &triggered_by,
                        );
                        continue;
                    }

                    let action = registry::resolve_action(&hook.action)?;
                    log_hook_start(
                        &task.id,
                        &current_status,
                        &target_state,
                        "after_enter",
                        hook,
                        &triggered_by,
                    );
                    let started = Instant::now();
                    let result = action.execute(&enter_ctx).await;
                    let duration_ms = elapsed_ms(started);
                    log_hook_result(
                        &task.id,
                        &current_status,
                        &target_state,
                        "after_enter",
                        hook,
                        &result,
                        duration_ms,
                    );
                    hook_results.push(hook_result_entry(
                        &hook.action,
                        "after_enter",
                        &result,
                        duration_ms,
                    ));

                    match result {
                        HookResult::Failed { reason: error } => {
                            tracing::warn!(
                                action = %hook.action,
                                task_id = %task.id,
                                from_state = %current_status,
                                to_state = %target_state,
                                %error,
                                "workflow validator failed"
                            );
                            self.event_bus.publish(ForgeEvent {
                                event_type: "transition.effect_failed".to_string(),
                                entity_id: task.id.clone(),
                                timestamp: event_timestamp(),
                                context: EventContext::TransitionEffectFailed {
                                    task_id: task.id.clone(),
                                    from_state: current_status.clone(),
                                    to_state: target_state.clone(),
                                    action: hook.action.clone(),
                                    error,
                                },
                            });
                        }
                        HookResult::Cascade {
                            to,
                            reason: cascade_reason,
                        } => {
                            cascade = Some((to, cascade_reason));
                            break;
                        }
                        HookResult::Ok | HookResult::Skipped { .. } => {}
                    }
                }
            }

            if let Ok(payload) = serde_json::to_string(&hook_results) {
                if let Err(error) =
                    TransitionLogRepo::update_hook_results(&*self.db, &transition_log.id, &payload)
                        .await
                {
                    tracing::warn!(
                        task_id = %task.id,
                        transition_log_id = %transition_log.id,
                        %error,
                        "workflow failed to persist hook results"
                    );
                }
            }

            if let Some((cascade_to, cascade_reason)) = cascade {
                if to_state.kind == StateKind::Gate
                    && to_state
                        .gate_config
                        .as_ref()
                        .is_some_and(|gate_config| gate_config.requires_user_approval())
                    && !to_state.gate_config.as_ref().is_some_and(|gate_config| {
                        gate_config.optional_when_unassigned()
                            && cascade_reason.starts_with("gate skipped:")
                    })
                {
                    tracing::info!(
                        task_id = %task.id,
                        state = %target_state,
                        cascade_to = %cascade_to,
                        cascade_reason = %cascade_reason,
                        "workflow cascade paused because gate requires user approval"
                    );
                    let review = latest_review(&self.db, &task.id).await?;
                    return Ok(TransitionResult {
                        task,
                        review,
                        cascaded: false,
                    });
                }

                if depth >= 3 {
                    tracing::warn!(
                        task_id = %task.id,
                        state = %target_state,
                        cascade_to = %cascade_to,
                        cascade_reason = %cascade_reason,
                        depth = depth,
                        "workflow cascade depth exceeded"
                    );
                    self.event_bus.publish(ForgeEvent {
                        event_type: "transition.cascade_depth_exceeded".to_string(),
                        entity_id: task.id.clone(),
                        timestamp: event_timestamp(),
                        context: EventContext::TransitionCascadeDepthExceeded {
                            task_id: task.id.clone(),
                            state: target_state,
                            depth,
                        },
                    });
                } else {
                    task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
                    let cascade_rejection = to_state.kind == StateKind::Gate
                        && !cascade_reason.starts_with("gate skipped:")
                        && !Self::is_terminal(workflow, &cascade_to);

                    tracing::info!(
                        task_id = %task.id,
                        from_state = %target_state,
                        cascade_to = %cascade_to,
                        cascade_reason = %cascade_reason,
                        cascade_rejection = cascade_rejection,
                        depth = depth,
                        next_depth = depth + 1,
                        "workflow executing cascade transition"
                    );
                    let mut cascaded = self
                        .transition_inner(
                            task_id,
                            cascade_to,
                            task.version,
                            workflow,
                            "system".to_string(),
                            cascade_reason,
                            cascade_rejection,
                            false,
                            None,
                            depth + 1,
                        )
                        .await?;
                    cascaded.cascaded = true;
                    return Ok(cascaded);
                }
            }

            let review = latest_review(&self.db, &task.id).await?;

            Ok(TransitionResult {
                task,
                review,
                cascaded: false,
            })
        }
        .instrument(span))
    }

    fn find_state<'a>(workflow: &'a WorkflowDefinition, name: &str) -> Option<&'a StateDefinition> {
        workflow.states.iter().find(|s| s.name == name)
    }

    fn is_terminal(workflow: &WorkflowDefinition, name: &str) -> bool {
        Self::find_state(workflow, name)
            .map(|state| state.kind == StateKind::Terminal)
            .unwrap_or(false)
    }

    fn is_cancellation_target(workflow: &WorkflowDefinition, target_state: &str) -> bool {
        workflow
            .cancellation_state
            .as_deref()
            .map(|state| state == target_state)
            .unwrap_or(false)
    }
}
