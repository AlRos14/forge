use super::*;

impl TaskService {
    pub(crate) async fn annotate_executor_failure_block(
        &self,
        execution: &Execution,
    ) -> Result<()> {
        self.annotate_work_unit_failure(execution, api_types::FailureKind::ExecutorFailed, None)
            .await
    }

    pub(crate) async fn annotate_dispatch_failure_block(
        &self,
        execution: &Execution,
    ) -> Result<()> {
        self.annotate_executor_failure_block(execution).await
    }

    pub(crate) async fn annotate_executor_unavailable_block(
        &self,
        execution: &Execution,
        retry_at: Option<String>,
        attempts: Value,
    ) -> Result<()> {
        self.annotate_work_unit_failure(
            execution,
            api_types::FailureKind::ExecutorUnavailable,
            Some(json!({"retry_at": retry_at, "attempts": attempts})),
        )
        .await
    }

    async fn annotate_work_unit_failure(
        &self,
        execution: &Execution,
        kind: api_types::FailureKind,
        details: Option<Value>,
    ) -> Result<()> {
        let reason = execution
            .error
            .clone()
            .unwrap_or_else(|| "Execution failed".to_owned());
        self.annotate_execution_block(execution, kind, reason, details)
            .await
    }

    pub(super) async fn annotate_execution_block(
        &self,
        execution: &Execution,
        kind: api_types::FailureKind,
        reason: String,
        details: Option<Value>,
    ) -> Result<()> {
        if execution.work_unit_id.is_none() {
            return Err(ServiceError::invalid_operation(
                "failure recovery requires an exact WorkUnit-bound Execution",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            db::TaskLifecycleState::Done | db::TaskLifecycleState::Cancelled
        ) {
            return Ok(());
        }

        let mut recovery_actions = vec![
            api_types::RecoveryAction::Reexecute,
            api_types::RecoveryAction::CancelTask,
        ];
        if super::resumable_external_session(
            &self.db,
            execution,
            execution.agent_id.as_deref(),
            execution.workspace_id.as_deref(),
        )
        .await?
        .is_some()
        {
            recovery_actions.insert(0, api_types::RecoveryAction::ResumeSession);
        }
        let now = now_rfc3339();
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: kind,
            blocking_reason: failure_kind_key(&kind).to_owned(),
            blocked_by: Some(
                api_types::Actor::system(api_types::SystemComponent::Executor).display(),
            ),
            blocked_at: Some(now.clone()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: execution.logs_path.clone(),
            }),
            message: Some(reason.clone()),
            hook: None,
            recovery_actions,
        };
        let raw_annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize execution failure annotation: {error}"
            ))
        })?;
        let blocked = json!({
            "reason": reason,
            "created_at": now,
            "kind": failure_kind_key(&kind),
            "execution_id": execution.id,
            "details": details,
        });

        let blocked_task = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .block(
            &task.id,
            crate::task_lifecycle::LifecycleCause::Execution(execution.id.clone()),
            failure_kind_key(&kind),
            execution.id.clone(),
            format!("execution-failure:{}", execution.id),
        )
        .await?
        .task;
        let updated = TaskRepo::update(
            &*self.db,
            db::UpdateTask {
                id: blocked_task.id.clone(),
                expected_version: blocked_task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: Some(Some(raw_annotation)),
                blocked_json: Some(Some(blocked.to_string())),
                failed_json: Some(None),
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        self.publish(ForgeEvent {
            event_type: "task.blocked".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskBlocked {
                project_id: updated.project_id,
                reason,
                kind: Some(kind),
                source: None,
                execution_id: Some(execution.id.clone()),
            },
        });
        Ok(())
    }
}

fn failure_kind_key(kind: &api_types::FailureKind) -> &'static str {
    match kind {
        api_types::FailureKind::ExecutorUnavailable => "executor_unavailable",
        api_types::FailureKind::MaxTurnsExceeded => "max_turns_exceeded",
        _ => "executor_failed",
    }
}
