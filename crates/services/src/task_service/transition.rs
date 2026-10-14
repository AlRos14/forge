use super::*;
use api_types::{Actor, SystemComponent};
use db::{TaskLifecycleRepo, UpdateTask};

impl TaskService {
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
        let result = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: task.id.clone(),
            expected_task_version: task.version,
            to_state: db::TaskLifecycleState::Cancelled,
            cause: crate::task_lifecycle::LifecycleCause::Actor(actor.clone()),
            reason_kind: Some("task_cancelled".to_owned()),
            reason_ref: Some(task.id.clone()),
            idempotency_key: format!("task-cancel:{}:{}", task.id, task.version),
        })
        .await?;
        self.cancel_running_executions_for_task(
            &result.task,
            "cancelled by task cancellation",
            actor,
        )
        .await?;
        let task =
            clear_transient_error_annotation_after_cancel(&self.db, &task, result.task).await?;
        Ok(task)
    }

    pub async fn soft_delete(&self, task_id: impl Into<String>) -> Result<Task> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        // Take SQLite's writer reservation before loading the Task and its
        // children. Parent-backed creates, reparents, detaches, and reorder
        // operations use this same structural boundary.
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let task = TaskRepo::get_by_id_in_tx(&*self.db, &mut transaction, &task_id, true)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle =
            TaskLifecycleRepo::get_task_lifecycle_in_tx(&*self.db, &mut transaction, &task.id)
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
        if task.deleted_at.is_some() {
            return Err(db::DbError::InvalidSoftDelete.into());
        }
        if TaskRepo::count_live_children_in_tx(&*self.db, &mut transaction, &task.id).await? != 0 {
            return Err(ServiceError::invalid_operation(
                "Tasks with live subtasks cannot be deleted; delete, detach, or move the subtasks first",
            ));
        }

        let deleted = TaskRepo::soft_delete_in_tx(
            &*self.db,
            &mut transaction,
            SoftDeleteTask {
                id: task_id.clone(),
                expected_version: task.version,
                deleted_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        transaction.commit().await?;

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
}

impl TaskService {
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
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .map_err(Into::into)
}
