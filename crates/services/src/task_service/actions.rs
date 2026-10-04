use super::execution::resumable_external_session;
use super::*;

use api_types::{Actor, TaskAction, UserActionSource};
use db::{
    AssigneeKind, ExecutionRepo, PageRequest, SortBy, SortOrder, TaskLifecycleRepo, TaskRepo,
    TaskRoleAssignmentRepo, WorkspaceRepo,
};

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
                reason: unavailable_reason(action, &task, lifecycle.state),
            });
        }

        let actor = Actor::user(UserActionSource::Api);
        let result = match action {
            TaskAction::Start => {
                let agent_id = self.action_agent_id(&task).await?;
                self.claim_task(task.id.clone(), Assignee::Agent(agent_id), None)
                    .await?
                    .task
            }
            TaskAction::Pause => {
                let execution =
                    self.latest_running_execution(&task.id)
                        .await?
                        .ok_or_else(|| ServiceError::TaskActionUnavailable {
                            available_actions: Vec::new(),
                            reason: "task has no running execution to pause".to_owned(),
                        })?;
                self.pause_execution(
                    execution.id,
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
        let current_workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
            .await?
            .map(|workspace| workspace.id);
        let is_terminal = matches!(
            lifecycle_state,
            db::TaskLifecycleState::Done | db::TaskLifecycleState::Cancelled
        );
        let running = executions
            .iter()
            .any(|execution| execution.status == ExecutionStatus::Running);
        let mut resumable = false;
        for execution in &executions {
            if execution.status != ExecutionStatus::Running
                && resumable_external_session(
                    &self.db,
                    execution,
                    execution.agent_id.as_deref(),
                    current_workspace_id.as_deref(),
                )
                .await?
                .is_some()
            {
                resumable = true;
                break;
            }
        }
        let has_previous_execution = executions.iter().any(|execution| {
            execution.status != ExecutionStatus::Running && execution.agent_id.is_some()
        });
        let has_agent = self.action_agent_id(task).await.is_ok();
        let retry_budget_exhausted =
            crate::task_failure_retry::TaskFailureRetryService::has_exhausted_retry_budget(
                &self.db, &task.id,
            )
            .await?;

        let mut actions = Vec::new();
        if lifecycle_state == db::TaskLifecycleState::Ready && has_agent {
            actions.push(TaskAction::Start);
        }
        if running {
            actions.push(TaskAction::Pause);
        }
        if should_offer_resume(
            lifecycle_state,
            is_terminal,
            retry_budget_exhausted,
            resumable,
            has_previous_execution,
            has_agent,
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
            {
                match self
                    .recover_task(
                        task.id.clone(),
                        api_types::RecoveryAction::ResumeSession,
                        Some("resumed by user".to_owned()),
                        context.clone(),
                    )
                    .await
                {
                    Ok(task) => return Ok(task),
                    Err(ServiceError::InvalidOperation { message })
                        if message.contains("no resumable session") => {}
                    Err(error) => return Err(error),
                }
            }
        }

        let executions = self.task_executions(&task.id).await?;
        let current_workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
            .await?
            .map(|workspace| workspace.id);
        let mut resumable_execution = None;
        for execution in &executions {
            if execution.status != ExecutionStatus::Running
                && resumable_external_session(
                    &self.db,
                    execution,
                    execution.agent_id.as_deref(),
                    current_workspace_id.as_deref(),
                )
                .await?
                .is_some()
            {
                resumable_execution = Some(execution);
                break;
            }
        }
        if let Some(execution) = resumable_execution {
            let launched = self
                .follow_up_execution(
                    execution.id.clone(),
                    context.clone().unwrap_or_else(|| {
                        "Resume work from the latest worker session.".to_owned()
                    }),
                    // The selected Execution supplies causal lineage only.
                    // Follow-up must select the current RoleMembership Actor
                    // before deciding whether that Actor may reuse the
                    // lineage HarnessSession.
                    None,
                    None,
                )
                .await?;
            self.start_execution(launched.execution.id).await?;
            return Ok(launched.task);
        }

        if let Some(execution) = executions.iter().find(|execution| {
            execution.status != ExecutionStatus::Running && execution.agent_id.is_some()
        }) {
            let launched = self
                .re_execute_execution_with_context(execution.id.clone(), context.clone())
                .await?;
            self.start_execution(launched.execution.id).await?;
            return Ok(launched.task);
        }

        let agent_id = self.action_agent_id(task).await?;
        let role = task_execution_role(task);
        let _launched = self
            .dispatch_initial_role_execution(
                &task.id,
                &agent_id,
                role,
                crate::task_service::execution::execution_purpose_for_task_type(
                    &task.task_type,
                    role,
                ),
                context.unwrap_or_else(|| "Resume task work.".to_owned()),
            )
            .await?;
        TaskRepo::get_by_id(&*self.db, &task.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))
    }

    async fn action_agent_id(&self, task: &Task) -> Result<String> {
        let role = task_execution_role(task);
        if let Some(memberships) =
            crate::task_service::current_role_memberships_authoritative(&self.db, &task.id, role)
                .await?
        {
            let selected = if task.repo_id.is_some() {
                crate::task_service::select_usable_repository_agent_id(
                    &self.db,
                    &task.project_id,
                    &memberships,
                )
                .await?
            } else {
                crate::task_service::select_usable_agent_id(&self.db, &memberships).await?
            };
            return selected.ok_or_else(|| {
                ServiceError::invalid_operation(format!(
                    "no active Agent membership is available for TaskRole {role}"
                ))
            });
        }

        let legacy_roles: &[&str] = if role == "implementer" {
            &["implementer", "coder", "worker", "assignee", "executor"]
        } else {
            &[role]
        };
        for legacy_role in legacy_roles {
            if let Some(assignment) =
                TaskRoleAssignmentRepo::get_by_task_and_role(&*self.db, &task.id, legacy_role)
                    .await?
            {
                if assignment.assignee_type == Some(AssigneeKind::Agent) {
                    if let Some(agent_id) = assignment.assignee_id {
                        return Ok(agent_id);
                    }
                }
            }
        }
        Err(ServiceError::invalid_operation(format!(
            "TaskRole {role} has no assigned Agent"
        )))
    }

    async fn task_executions(&self, task_id: &str) -> Result<Vec<Execution>> {
        Ok(ExecutionRepo::list_by_task(
            &*self.db,
            task_id,
            PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?
        .items)
    }

    async fn latest_running_execution(&self, task_id: &str) -> Result<Option<Execution>> {
        Ok(self
            .task_executions(task_id)
            .await?
            .into_iter()
            .find(|execution| {
                execution.work_unit_id.is_none() && execution.status == ExecutionStatus::Running
            }))
    }
}

fn should_offer_resume(
    lifecycle: db::TaskLifecycleState,
    is_terminal: bool,
    retry_budget_exhausted: bool,
    resumable: bool,
    has_previous_execution: bool,
    has_agent: bool,
) -> bool {
    !matches!(
        lifecycle,
        db::TaskLifecycleState::ReadyToMerge | db::TaskLifecycleState::Merging
    ) && !is_terminal
        && !retry_budget_exhausted
        && (resumable
            || has_previous_execution
            || (lifecycle == db::TaskLifecycleState::Active && has_agent))
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
            true,
        ));
        assert!(should_offer_resume(
            TaskLifecycleState::Active,
            false,
            false,
            false,
            true,
            false,
        ));
        assert!(!should_offer_resume(
            TaskLifecycleState::Merging,
            false,
            false,
            false,
            true,
            true,
        ));
        assert!(!should_offer_cancel(TaskLifecycleState::Merging));
        assert!(should_offer_cancel(TaskLifecycleState::Active));
    }
}

fn task_execution_role(task: &Task) -> &'static str {
    crate::task_service::execution::task_role_for_task_type(&task.task_type)
}

fn unavailable_reason(
    action: TaskAction,
    task: &Task,
    lifecycle: db::TaskLifecycleState,
) -> String {
    format!(
        "action '{}' is not available while Task lifecycle is '{}' (legacy status '{}')",
        action_name(action),
        lifecycle,
        task.status
    )
}
