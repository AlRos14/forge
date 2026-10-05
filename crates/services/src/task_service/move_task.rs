use super::*;
use crate::DomainEventService;
use api_types::{Actor, MoveTaskRequest, TaskMovedEventPayload, UserActionSource};
use db::{
    CompareAndMoveTask, MoveTaskIdentity, MoveTaskPersistence, MoveTaskResult, TaskBoardRepo,
};
use events::TASK_MOVED_EVENT;

use super::transition::should_clear_transient_error_annotation;

impl TaskService {
    pub async fn move_task(
        &self,
        task_id: impl Into<String>,
        request: MoveTaskRequest,
    ) -> Result<MoveTaskResult> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        validate_required("operation_id", &request.operation_id)?;
        Uuid::parse_str(&request.operation_id)
            .map_err(|_| ServiceError::invalid_operation("operation_id must be a valid UUID"))?;

        let operation_id = request.operation_id.clone();
        let operation_lock = {
            let mut locks = self.move_operation_locks.lock().await;
            Arc::clone(
                locks
                    .entry(operation_id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let guard = operation_lock.lock().await;
        let result = self.move_task_locked(task_id, request).await;
        drop(guard);
        let mut locks = self.move_operation_locks.lock().await;
        if Arc::strong_count(&operation_lock) == 2 {
            locks.remove(&operation_id);
        }
        result
    }

    async fn move_task_locked(
        &self,
        task_id: String,
        request: MoveTaskRequest,
    ) -> Result<MoveTaskResult> {
        let source_task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let target_lifecycle =
            db::TaskLifecycleState::from_legacy_status(&request.target_status)
                .ok_or_else(|| ServiceError::invalid_operation("unknown Task lifecycle state"))?;
        if matches!(
            target_lifecycle,
            db::TaskLifecycleState::ReadyToMerge
                | db::TaskLifecycleState::Merging
                | db::TaskLifecycleState::Done
        ) {
            return Err(ServiceError::invalid_operation(
                "board moves cannot satisfy a Gate or authorize merge completion",
            ));
        }
        let target_status = target_lifecycle.legacy_projection().to_owned();
        let target_column_statuses = vec![target_status.clone()];
        let mut request = request;
        request.target_status = target_status.clone();
        let identity = MoveTaskIdentity {
            project_id: source_task.project_id.clone(),
            task_id: task_id.clone(),
            task_version: request.task_version,
            board_revision: request.board_revision,
            target_status: request.target_status.clone(),
            before_id: request.before_id.clone(),
            after_id: request.after_id.clone(),
        };
        if let Some(replayed) =
            TaskBoardRepo::replay_move_task(&*self.db, &request.operation_id, &identity).await?
        {
            return Ok(replayed);
        }

        if source_task.version != request.task_version {
            return Err(DbError::TaskVersionConflict {
                expected: request.task_version,
                actual: source_task.version,
            }
            .into());
        }
        let actual_board_revision =
            TaskBoardRepo::board_revision(&*self.db, &source_task.project_id).await?;
        if actual_board_revision != request.board_revision {
            return Err(DbError::BoardRevisionConflict {
                expected: request.board_revision,
                actual: actual_board_revision,
            }
            .into());
        }
        if source_task.status == target_status {
            return self
                .reorder_within_column(source_task, request, target_column_statuses)
                .await;
        }

        let previous_status = source_task.status.clone();
        let was_blocked = source_task.blocked_json.is_some();
        let blocked_previous_reason = source_task
            .blocked_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|value| {
                value
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let persistence = TaskBoardRepo::compare_and_move_task(
            &*self.db,
            CompareAndMoveTask {
                operation_id: request.operation_id.clone(),
                project_id: source_task.project_id.clone(),
                task_id: source_task.id.clone(),
                task_version: request.task_version,
                board_revision: request.board_revision,
                target_status,
                target_column_statuses,
                before_id: request.before_id.clone(),
                after_id: request.after_id.clone(),
                entry_barrier_json: None,
                transition_log_id: new_uuid_v4(),
                trigger_name: None,
                triggered_by: Actor::user(UserActionSource::BoardDrag).display(),
                trigger_reason: "board move".to_owned(),
                rejection: false,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        let mut result = match persistence {
            MoveTaskPersistence::Replayed(result) => return Ok(*result),
            MoveTaskPersistence::Committed {
                result,
                transition_log,
            } => {
                if let Some(event) =
                    db::DomainEventRepo::get_event(&*self.db, &transition_log.id).await?
                {
                    DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                        .publish_committed(&event);
                }
                *result
            }
        };
        self.publish_domain_event_by_dedupe(&format!("task-board-move:{}", request.operation_id))
            .await;
        if target_lifecycle == db::TaskLifecycleState::Cancelled {
            self.cancel_running_executions_for_task(
                &result.task,
                "cancelled by board lifecycle",
                Actor::user(UserActionSource::BoardDrag),
            )
            .await?;
        }
        if was_blocked {
            self.publish(ForgeEvent {
                event_type: "task.unblocked".to_owned(),
                entity_id: result.task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskUnblocked {
                    project_id: result.task.project_id.clone(),
                    previous_reason: blocked_previous_reason,
                },
            });
        }
        if should_clear_transient_error_annotation(&result.task) {
            match TaskRepo::update(
                &*self.db,
                db::UpdateTask {
                    id: result.task.id.clone(),
                    expected_version: result.task.version,
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
            {
                Ok(updated) => result.task = updated,
                Err(DbError::VersionConflict) => {
                    result.task = TaskRepo::get_by_id(&*self.db, &result.task.id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", result.task.id.clone()))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if previous_status != result.task.status {
            super::execution::clear_execution_retry_metadata(&self.db, &result.task).await?;
            result.task = TaskRepo::get_by_id(&*self.db, &result.task.id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", result.task.id.clone()))?;
        }
        result.board_revision =
            TaskBoardRepo::board_revision(&*self.db, &source_task.project_id).await?;
        self.publish_move_event(&result);
        TaskBoardRepo::complete_move_operation(
            &*self.db,
            &request.operation_id,
            &result,
            &now_rfc3339(),
        )
        .await?;
        Ok(result)
    }

    async fn reorder_within_column(
        &self,
        task: Task,
        request: MoveTaskRequest,
        target_column_statuses: Vec<String>,
    ) -> Result<MoveTaskResult> {
        let persistence = TaskBoardRepo::compare_and_move_task(
            &*self.db,
            CompareAndMoveTask {
                operation_id: request.operation_id.clone(),
                project_id: task.project_id.clone(),
                task_id: task.id.clone(),
                task_version: request.task_version,
                board_revision: request.board_revision,
                target_status: request.target_status,
                target_column_statuses,
                before_id: request.before_id,
                after_id: request.after_id,
                entry_barrier_json: None,
                transition_log_id: new_uuid_v4(),
                trigger_name: None,
                triggered_by: Actor::user(UserActionSource::BoardDrag).display(),
                trigger_reason: "board reorder".to_owned(),
                rejection: false,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        match persistence {
            MoveTaskPersistence::Replayed(result) => Ok(*result),
            MoveTaskPersistence::Committed {
                result,
                transition_log,
            } => {
                if let Some(event) =
                    db::DomainEventRepo::get_event(&*self.db, &transition_log.id).await?
                {
                    DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                        .publish_committed(&event);
                }
                self.publish_move_event(&result);
                TaskBoardRepo::complete_move_operation(
                    &*self.db,
                    &request.operation_id,
                    &result,
                    &now_rfc3339(),
                )
                .await?;
                Ok(*result)
            }
        }
    }

    fn publish_move_event(&self, result: &MoveTaskResult) {
        self.publish(ForgeEvent {
            event_type: TASK_MOVED_EVENT.to_owned(),
            entity_id: result.task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskMoved(TaskMovedEventPayload {
                project_id: result.task.project_id.clone(),
                operation_id: result.operation_id.clone(),
                old_status: result.old_status.clone(),
                new_status: result.task.status.clone(),
                old_board_position: result.old_board_position,
                new_board_position: result.task.board_position,
                task_version: result.task.version,
                board_revision: result.board_revision,
                before_id: result.before_id.clone(),
                after_id: result.after_id.clone(),
            }),
        });
    }
}
