use super::execution::resumable_external_session;
use super::*;

use api_types::{Actor, TaskAction, UserActionSource};
use db::{ExecutionRepo, PageRequest, SortBy, SortOrder, TaskLifecycleRepo, TaskRepo};

#[derive(Debug)]
pub struct TaskActionResult {
    pub task: Task,
    pub action: TaskAction,
}

impl TaskService {
    /// Return available aggregate lifecycle and Execution actions.
    pub async fn available_task_actions(
        &self,
        task_id: impl Into<String>,
    ) -> Result<Vec<TaskAction>> {
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        self.available_task_actions_for(&task, lifecycle.state)
            .await
    }

    pub async fn perform_task_action(
        &self,
        task_id: impl Into<String>,
        action: TaskAction,
        reason: Option<String>,
        requested_version: Option<i64>,
    ) -> Result<TaskActionResult> {
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        // A stale client version is a conflict for every action, not only the ones whose
        // inner path happens to re-check it.
        if let Some(version) = requested_version {
            if version != task.version {
                return Err(ServiceError::Db(db::DbError::TaskVersionConflict {
                    expected: version,
                    actual: task.version,
                }));
            }
        }

        // Cancelling an already-cancelled Task stays idempotent.
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if action == TaskAction::Cancel && lifecycle.state == db::TaskLifecycleState::Cancelled {
            let task = self
                .cancel_task_as(task.id.clone(), Actor::user(UserActionSource::Api))
                .await?;
            return Ok(TaskActionResult { task, action });
        }

        let available = self
            .available_task_actions_for(&task, lifecycle.state)
            .await?;
        if !available.contains(&action) {
            return Err(ServiceError::TaskActionUnavailable {
                available_actions: available,
                reason: unavailable_reason(action, lifecycle.state),
            });
        }

        let actor = Actor::user(UserActionSource::Api);
        let result = match action {
            TaskAction::Start => {
                return Err(ServiceError::invalid_operation(
                    "Task-level start is retired; create and allocate an exact WorkUnit first",
                ));
            }
            TaskAction::Pause => {
                let running = self
                    .task_executions(&task.id)
                    .await?
                    .into_iter()
                    .filter(|execution| execution.status == ExecutionStatus::Running)
                    .collect::<Vec<_>>();
                if running.len() != 1
                    || !crate::task_service::execution::has_exact_work_unit_target(
                        &self.db,
                        &task.id,
                        &running[0],
                    )
                    .await?
                {
                    return Err(ServiceError::TaskActionUnavailable {
                        available_actions: Vec::new(),
                        reason: "pause requires exactly one running Execution bound to an exact WorkUnit; use the Execution action when targets are ambiguous".to_owned(),
                    });
                }
                let execution = &running[0];
                self.pause_execution(
                    execution.id.clone(),
                    reason
                        .clone()
                        .unwrap_or_else(|| "paused by user".to_owned()),
                )
                .await?;
                self.create_system_comment(
                    &task.id,
                    reason
                        .map(|value| format!("Task paused by user: {value}"))
                        .unwrap_or_else(|| "Task paused by user".to_owned()),
                )
                .await?;
                TaskRepo::get_by_id(&*self.db, &task.id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?
            }
            TaskAction::Resume => self.resume_task_execution(&task, reason).await?,
            TaskAction::Submit => {
                return Err(ServiceError::invalid_operation(
                    "Task submission cannot satisfy a Gate; use an exact GateEvaluation",
                ));
            }
            TaskAction::RequestChanges => {
                return Err(ServiceError::invalid_operation(
                    "Review changes must be recorded in an exact ReviewReport and evaluated by a Gate",
                ));
            }
            TaskAction::Approve => {
                return Err(ServiceError::invalid_operation(
                    "Approval must be recorded as an exact Decision and evaluated by a Gate",
                ));
            }
            TaskAction::Cancel => self.cancel_task_as(task.id.clone(), actor).await?,
        };

        Ok(TaskActionResult {
            task: result,
            action,
        })
    }

    async fn available_task_actions_for(
        &self,
        task: &Task,
        lifecycle_state: db::TaskLifecycleState,
    ) -> Result<Vec<TaskAction>> {
        let executions = self.task_executions(&task.id).await?;
        let is_terminal = matches!(
            lifecycle_state,
            db::TaskLifecycleState::Done | db::TaskLifecycleState::Cancelled
        );
        let running_executions = executions
            .iter()
            .filter(|execution| execution.status == ExecutionStatus::Running)
            .collect::<Vec<_>>();
        let running = running_executions.len() == 1
            && crate::task_service::execution::has_exact_work_unit_target(
                &self.db,
                &task.id,
                running_executions[0],
            )
            .await?;
        let mut exact_stopped = Vec::new();
        for execution in &executions {
            if execution.status != ExecutionStatus::Running
                && crate::task_service::execution::has_exact_work_unit_target(
                    &self.db, &task.id, execution,
                )
                .await?
            {
                exact_stopped.push(execution);
            }
        }
        let annotated_execution = task
            .error_annotation
            .as_deref()
            .and_then(|raw| serde_json::from_str::<api_types::TaskAnnotation>(raw).ok())
            .and_then(|annotation| match annotation {
                api_types::TaskAnnotation::Blocking(annotation) => annotation.blocked_execution_id,
                api_types::TaskAnnotation::Legacy(_) => None,
            })
            .and_then(|id| {
                exact_stopped
                    .iter()
                    .copied()
                    .find(|execution| execution.id == id)
            });
        let resume_candidate =
            annotated_execution.or_else(|| (exact_stopped.len() == 1).then(|| exact_stopped[0]));
        let resumable = if let Some(execution) = resume_candidate {
            resumable_external_session(
                &self.db,
                execution,
                execution.agent_id.as_deref(),
                execution.workspace_id.as_deref(),
            )
            .await?
            .is_some()
        } else {
            false
        };
        let has_previous_execution = annotated_execution.is_some() || exact_stopped.len() == 1;
        let retry_budget_exhausted =
            crate::task_failure_retry::TaskFailureRetryService::has_exhausted_retry_budget(
                &self.db, &task.id,
            )
            .await?;

        let mut actions = Vec::new();
        if running {
            actions.push(TaskAction::Pause);
        }
        if should_offer_resume(
            lifecycle_state,
            is_terminal,
            retry_budget_exhausted,
            resumable,
            has_previous_execution,
        ) {
            actions.push(TaskAction::Resume);
        }
        if should_offer_cancel(lifecycle_state) {
            actions.push(TaskAction::Cancel);
        }
        Ok(actions)
    }

    async fn resume_task_execution(&self, task: &Task, reason: Option<String>) -> Result<Task> {
        let context = reason.filter(|value| !value.trim().is_empty());
        if let Some(annotation) = task
            .error_annotation
            .as_deref()
            .and_then(|raw| serde_json::from_str::<api_types::TaskAnnotation>(raw).ok())
            .and_then(|annotation| match annotation {
                api_types::TaskAnnotation::Blocking(annotation) => Some(annotation),
                api_types::TaskAnnotation::Legacy(_) => None,
            })
        {
            if annotation
                .recovery_actions
                .contains(&api_types::RecoveryAction::ResumeSession)
                && annotation.blocked_execution_id.is_some()
            {
                return self
                    .recover_task(
                        task.id.clone(),
                        api_types::RecoveryAction::ResumeSession,
                        Some("resumed by user".to_owned()),
                        context,
                    )
                    .await;
            }
        }

        let executions = self.task_executions(&task.id).await?;
        let mut exact_stopped = Vec::new();
        for execution in &executions {
            if execution.status != ExecutionStatus::Running
                && crate::task_service::execution::has_exact_work_unit_target(
                    &self.db, &task.id, execution,
                )
                .await?
            {
                exact_stopped.push(execution);
            }
        }
        if exact_stopped.len() == 1 {
            let execution = exact_stopped[0];
            let launched = self
                .re_execute_execution_with_context(execution.id.clone(), context)
                .await?;
            self.start_execution(launched.execution.id).await?;
            return Ok(launched.task);
        }
        let reason = if exact_stopped.len() > 1 {
            "Task has multiple stopped WorkUnit Executions; recovery requires an exact blocked_execution_id"
        } else {
            "Task has no stopped Execution bound to an exact open WorkUnit"
        };
        Err(ServiceError::TaskActionUnavailable {
            available_actions: Vec::new(),
            reason: reason.to_owned(),
        })
    }

    async fn task_executions(&self, task_id: &str) -> Result<Vec<Execution>> {
        let mut executions = Vec::new();
        let mut cursor = None;
        loop {
            let page = ExecutionRepo::list_by_task(
                &*self.db,
                task_id,
                PageRequest {
                    cursor,
                    limit: 100,
                    include_total: false,
                    sort_by: SortBy::CreatedAt,
                    sort_order: SortOrder::Desc,
                },
            )
            .await?;
            executions.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(executions);
            }
        }
    }
}

fn should_offer_resume(
    lifecycle: db::TaskLifecycleState,
    is_terminal: bool,
    retry_budget_exhausted: bool,
    resumable: bool,
    has_previous_execution: bool,
) -> bool {
    !matches!(
        lifecycle,
        db::TaskLifecycleState::ReadyToMerge | db::TaskLifecycleState::Merging
    ) && !is_terminal
        && !retry_budget_exhausted
        && (resumable || has_previous_execution)
}

fn should_offer_cancel(lifecycle: db::TaskLifecycleState) -> bool {
    !matches!(
        lifecycle,
        db::TaskLifecycleState::Merging
            | db::TaskLifecycleState::Done
            | db::TaskLifecycleState::Cancelled
    )
}

fn action_name(action: TaskAction) -> &'static str {
    match action {
        TaskAction::Start => "start",
        TaskAction::Pause => "pause",
        TaskAction::Resume => "resume",
        TaskAction::Submit => "submit",
        TaskAction::RequestChanges => "request_changes",
        TaskAction::Approve => "approve",
        TaskAction::Cancel => "cancel",
    }
}

fn unavailable_reason(action: TaskAction, lifecycle: db::TaskLifecycleState) -> String {
    format!(
        "action '{}' is not available while Task lifecycle is '{}'",
        action_name(action),
        lifecycle
    )
}

#[cfg(test)]
mod tests {
    use super::{should_offer_cancel, should_offer_resume};
    use db::TaskLifecycleState;

    #[test]
    fn merge_ready_task_does_not_offer_resume_without_a_gate_rework_effect() {
        assert!(!should_offer_resume(
            TaskLifecycleState::ReadyToMerge,
            false,
            false,
            true,
            true,
        ));
        assert!(should_offer_resume(
            TaskLifecycleState::Active,
            false,
            false,
            false,
            true,
        ));
        assert!(!should_offer_resume(
            TaskLifecycleState::Merging,
            false,
            false,
            false,
            true,
        ));
        assert!(!should_offer_cancel(TaskLifecycleState::Merging));
        assert!(should_offer_cancel(TaskLifecycleState::Active));
    }
}
