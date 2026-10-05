use super::*;
use api_types::{Actor, SystemComponent};
use db::{TaskLifecycleRepo, UpdateTask};

impl TaskService {
    pub async fn transition(
        &self,
        task_id: impl Into<String>,
        new_status: TaskStatus,
        options: impl Into<TransitionOptions>,
    ) -> Result<TransitionResult> {
        self.transition_inner(task_id.into(), new_status, options.into(), None)
            .await
    }

    #[allow(dead_code)] // Historical review path; GateEvaluation owns lifecycle transitions now.
    pub(crate) async fn transition_caused_by_execution(
        &self,
        task_id: impl Into<String>,
        new_status: TaskStatus,
        options: TransitionOptions,
        causing_execution_id: &str,
    ) -> Result<TransitionResult> {
        let task_id = task_id.into();
        let cause = ExecutionRepo::get_by_id(&*self.db, causing_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", causing_execution_id.to_owned()))?;
        if cause.task_id != task_id
            || cause.role != crate::workflow::default_roles::REVIEWER
            || cause.purpose != Some(ExecutionPurpose::Review)
            || cause.status != ExecutionStatus::Completed
        {
            return Err(ServiceError::invalid_operation(
                "Review rework transition requires its exact completed reviewer Execution",
            ));
        }
        self.transition_inner(task_id, new_status, options, Some(causing_execution_id))
            .await
    }

    async fn transition_inner(
        &self,
        task_id: String,
        new_status: TaskStatus,
        options: TransitionOptions,
        causing_execution_id: Option<&str>,
    ) -> Result<TransitionResult> {
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let target = crate::task_lifecycle::lifecycle_state_for_legacy_status(&new_status)
            .ok_or_else(|| ServiceError::invalid_operation("unknown Task lifecycle projection"))?;
        if target == db::TaskLifecycleState::Active
            && task.repo_id.is_some()
            && options.version == task.version
        {
            // Preserve the repository admission boundary while aggregate
            // progress replaces workflow state transitions.
            self.ensure_task_runnable(&task).await?;
        }
        let cause = causing_execution_id
            .map(|id| crate::task_lifecycle::LifecycleCause::Execution(id.to_owned()))
            .unwrap_or_else(|| {
                crate::task_lifecycle::LifecycleCause::Actor(options.triggered_by.clone())
            });
        let result = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: task_id.clone(),
            expected_task_version: options.version,
            to_state: target,
            cause,
            reason_kind: Some("task_transition".to_owned()),
            reason_ref: Some(options.reason.unwrap_or_else(|| "user action".to_owned())),
            idempotency_key: format!("task-transition:{task_id}:{}:{target}", options.version),
        })
        .await?;
        if task.status != result.task.status {
            self.publish(ForgeEvent {
                event_type: "task.status_changed".to_owned(),
                entity_id: result.task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskStatusChanged {
                    project_id: result.task.project_id.clone(),
                    old_status: task.status,
                    new_status: result.task.status.clone(),
                },
            });
        }
        if target == db::TaskLifecycleState::Cancelled {
            self.cancel_running_executions_for_task(
                &result.task,
                "cancelled by aggregate Task lifecycle",
                options.triggered_by,
            )
            .await?;
        }
        Ok(TransitionResult {
            task: result.task,
            review: None,
        })
    }

    pub async fn is_awaiting_human(&self, task_id: impl Into<String>) -> Result<bool> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid task metadata: {error}"))
        })?;
        if metadata
            .extra
            .get("awaiting_human")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(true);
        }
        let executions = ExecutionRepo::list_by_task(
            &*self.db,
            &task_id,
            PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        Ok(executions.items.iter().any(|execution| {
            execution.purpose == Some(ExecutionPurpose::Review)
                && execution
                    .actor_ref()
                    .is_some_and(|actor| matches!(actor, db::ActorRef::Human(_)))
                && execution.status == ExecutionStatus::Running
        }))
    }

    pub async fn executor_attempt_count(&self, task_id: &str) -> Result<i64> {
        validate_required("task_id", task_id)?;
        let executor =
            ExecutionRepo::count_by_task_and_role(&*self.db, task_id, "executor").await?;
        let coder = ExecutionRepo::count_by_task_and_role(&*self.db, task_id, "coder").await?;
        Ok(executor + coder)
    }

    pub async fn cancel_task(&self, task_id: impl Into<String>) -> Result<Task> {
        self.cancel_task_as(task_id, Actor::system(SystemComponent::CancelTask))
            .await
    }

    pub async fn cancel_task_as(&self, task_id: impl Into<String>, actor: Actor) -> Result<Task> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if lifecycle.state == db::TaskLifecycleState::Cancelled {
            self.cancel_running_executions_for_task(&task, "cancelled by task cancellation", actor)
                .await?;
            let source_task = task.clone();
            return clear_transient_error_annotation_after_cancel(&self.db, &source_task, task)
                .await;
        }
        if lifecycle.state == db::TaskLifecycleState::Done {
            return Err(ServiceError::invalid_operation(
                "a completed Task cannot be cancelled",
            ));
        }
        if lifecycle.state == db::TaskLifecycleState::Merging {
            return Err(ServiceError::invalid_operation(
                "an admitted TaskMerge must finish or recover before Task cancellation",
            ));
        }
        // The lifecycle transition commits before the execution cleanup. If
        // cleanup fails or the process stops, a repeated cancellation must
        // still reconcile any Running Execution left behind.
        let result = self
            .transition(
                task_id,
                "cancelled".to_owned(),
                TransitionOptions {
                    version: task.version,
                    reason: Some("cancel task".to_owned()),
                    triggered_by: actor,
                    rejection: false,
                    defer_dispatch_seconds: None,
                },
            )
            .await?;
        let task =
            clear_transient_error_annotation_after_cancel(&self.db, &task, result.task).await?;
        Ok(task)
    }

    pub async fn advance_to_next_state(&self, task_id: impl Into<String>) -> Result<Task> {
        let _ = task_id.into();
        Err(ServiceError::invalid_operation(
            "manual workflow advancement is retired; Task progress requires an aggregate lifecycle transition and exact Gate facts",
        ))
    }

    pub async fn soft_delete(&self, task_id: impl Into<String>) -> Result<Task> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, true)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            db::TaskLifecycleState::Active
                | db::TaskLifecycleState::ReadyToMerge
                | db::TaskLifecycleState::Merging
        ) {
            return Err(ServiceError::invalid_operation(
                "Tasks with active aggregate work cannot be deleted",
            ));
        }

        let deleted = TaskRepo::soft_delete(
            &*self.db,
            SoftDeleteTask {
                id: task_id,
                expected_version: task.version,
                deleted_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        self.publish(ForgeEvent {
            event_type: "task.deleted".to_owned(),
            entity_id: deleted.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskDeleted {
                project_id: deleted.project_id.clone(),
            },
        });

        Ok(deleted)
    }

    pub async fn archive_task(&self, task_id: impl Into<String>) -> Result<Task> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        let task = TaskRepo::get_by_id(&*self.db, &task_id, true)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let now = now_rfc3339();
        let archived = TaskRepo::archive(
            &*self.db,
            ArchiveTask {
                id: task_id,
                expected_version: task.version,
                archived_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        self.publish(ForgeEvent {
            event_type: "task.archived".to_owned(),
            entity_id: archived.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskUpdated {
                project_id: archived.project_id.clone(),
            },
        });

        Ok(archived)
    }
}

impl TaskService {
    #[allow(dead_code)] // Retained for legacy WorkflowEngine teardown until PR13.
    pub(super) async fn cancel_active_execution_for_user_transition(
        &self,
        task: &Task,
        target_status: &str,
        workflow: &api_types::WorkflowDefinition,
        actor: &Actor,
    ) -> Result<()> {
        if !actor.is_user() {
            return Ok(());
        }

        if task.status == target_status {
            return Ok(());
        }

        if !workflow
            .states
            .iter()
            .any(|state| state.name.as_str() == target_status)
        {
            return Ok(());
        }

        let page = ExecutionRepo::list_by_task(
            &*self.db,
            &task.id,
            PageRequest {
                cursor: None,
                limit: 20,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        for execution in page
            .items
            .into_iter()
            .filter(|execution| execution.status == ExecutionStatus::Running)
        {
            self.cancel_active_execution(
                &execution,
                "cancelled by user transition",
                db::StopReason::UserCancelled,
                actor,
                db::ResumePolicy::None,
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn cancel_running_executions_for_task(
        &self,
        task: &Task,
        reason: &str,
        actor: Actor,
    ) -> Result<()> {
        let page = ExecutionRepo::list_by_task(
            &*self.db,
            &task.id,
            PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        for execution in page
            .items
            .into_iter()
            .filter(|execution| execution.status == ExecutionStatus::Running)
        {
            self.cancel_active_execution(
                &execution,
                reason,
                db::StopReason::TaskCancelled,
                &actor,
                db::ResumePolicy::None,
            )
            .await?;
        }
        Ok(())
    }
}

async fn clear_transient_error_annotation_after_cancel(
    db: &SqliteDb,
    source_task: &Task,
    advanced_task: Task,
) -> Result<Task> {
    if source_task.error_annotation.is_none()
        || source_task.error_annotation != advanced_task.error_annotation
    {
        return Ok(advanced_task);
    }

    TaskRepo::update(
        db,
        UpdateTask {
            id: advanced_task.id.clone(),
            expected_version: advanced_task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(None),
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .map_err(Into::into)
}

#[allow(dead_code)] // Historical ReviewRunner metadata cleanup; PR13 storage cleanup.
pub(super) async fn clear_manual_review_awaiting_metadata(
    db: &SqliteDb,
    task: &Task,
) -> Result<Task> {
    let current = TaskRepo::get_by_id(db, &task.id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
    let mut metadata = TaskMetadata::parse(current.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata for {}: {error}", task.id))
    })?;
    if metadata
        .extra
        .get("awaiting_human_reason")
        .and_then(Value::as_str)
        != Some("manual_review")
    {
        return Ok(current);
    }

    metadata.extra.remove("awaiting_human");
    metadata.extra.remove("awaiting_human_reason");
    TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    TaskRepo::get_by_id(db, &task.id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))
}

pub(super) fn should_clear_transient_error_annotation(task: &Task) -> bool {
    if task.status.as_str() == default_states::MERGE_FAILED {
        return false;
    }

    task.error_annotation
        .as_deref()
        .is_some_and(is_transient_error_annotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_failure_annotations_are_transient() {
        let annotation = serde_json::json!({
            "type": "executor_failed",
            "blocking_reason": "executor_failed",
            "message": "executor stopped before workflow could continue"
        });

        assert!(is_transient_error_annotation(&annotation.to_string()));

        let target_dirty = serde_json::json!({
            "type": "target_repo_dirty",
            "message": "target repository has uncommitted changes"
        });

        assert!(is_transient_error_annotation(&target_dirty.to_string()));
    }
}
