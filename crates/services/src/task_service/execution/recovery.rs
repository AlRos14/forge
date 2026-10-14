use super::*;
use db::WorkspaceRepo;

impl TaskService {
    pub async fn recover_task(
        &self,
        task_id: impl Into<String>,
        action: api_types::RecoveryAction,
        reason: Option<String>,
        context: Option<String>,
    ) -> Result<Task> {
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        if action == api_types::RecoveryAction::CancelTask {
            return self.recover_cancel_task(task, reason).await;
        }
        if !matches!(
            action,
            api_types::RecoveryAction::ResumeSession | api_types::RecoveryAction::Reexecute
        ) {
            return Err(ServiceError::invalid_operation(
                "workflow and Task-scoped recovery actions are retired; recovery requires an exact blocked Execution",
            ));
        }
        if task.failed_json.is_some() {
            return Err(ServiceError::invalid_operation(
                "failed Task history has no exact resumable Execution; cancel or create a new WorkUnit",
            ));
        }
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            db::TaskLifecycleState::Done | db::TaskLifecycleState::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "cannot recover Task {} in terminal lifecycle {}",
                task.id, lifecycle.state
            )));
        }
        let annotation = self.recovery_annotation(&task)?;
        let blocking_execution_id =
            annotation.blocked_execution_id.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation("blocked Execution identity is missing")
            })?;
        if lifecycle.state != db::TaskLifecycleState::Blocked
            || lifecycle.reason_ref.as_deref() != Some(blocking_execution_id)
        {
            return Err(ServiceError::invalid_operation(
                "recovery annotation does not match the current TaskLifecycle block",
            ));
        }
        super::launch::validate_task_blocking_metadata_owner(&task, blocking_execution_id)?;
        self.validate_recovery_action(&annotation, &action)?;
        match action {
            api_types::RecoveryAction::ResumeSession => {
                self.recover_resume_session(task, &annotation, reason, context)
                    .await
            }
            api_types::RecoveryAction::Reexecute => {
                self.recover_reexecute(task, &annotation, reason, context)
                    .await
            }
            api_types::RecoveryAction::CancelTask => unreachable!(),
        }
    }

    fn parse_blocking_annotation(&self, task: &Task) -> Option<api_types::TaskBlockingAnnotation> {
        let annotation = task.error_annotation.as_deref()?;
        match serde_json::from_str::<api_types::TaskAnnotation>(annotation) {
            Ok(api_types::TaskAnnotation::Blocking(annotation)) => Some(annotation),
            Ok(api_types::TaskAnnotation::Legacy(_)) => None,
            Err(_) => None,
        }
    }

    pub(super) fn recovery_annotation(
        &self,
        task: &Task,
    ) -> Result<api_types::TaskBlockingAnnotation> {
        if let Some(annotation) = self.parse_blocking_annotation(task) {
            if annotation.blocked_execution_id.is_some()
                && annotation.annotation_type != api_types::FailureKind::Unknown
            {
                return Ok(annotation);
            }
        }
        if let Some(raw_blocked) = task.blocked_json.as_deref() {
            let annotation = metadata_recovery_annotation(
                raw_blocked,
                &[
                    api_types::RecoveryAction::ResumeSession,
                    api_types::RecoveryAction::Reexecute,
                ],
            )?;
            if annotation.blocked_execution_id.is_some()
                && annotation.annotation_type != api_types::FailureKind::Unknown
            {
                return Ok(annotation);
            }
        }

        Err(ServiceError::invalid_operation(
            "task has no blocking recovery annotation",
        ))
    }

    fn validate_recovery_action(
        &self,
        annotation: &api_types::TaskBlockingAnnotation,
        action: &api_types::RecoveryAction,
    ) -> Result<()> {
        if annotation.recovery_actions.contains(action) {
            Ok(())
        } else {
            Err(ServiceError::invalid_operation(format!(
                "recovery action '{}' is not allowed for this task",
                serde_json::to_string(action).unwrap_or_else(|_| "unknown".to_owned())
            )))
        }
    }

    pub(super) async fn clear_blocking_metadata(&self, task_id: &str) -> Result<Task> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let previous_reason = interruption_reason(task.blocked_json.as_deref());
        let updated = TaskRepo::update(
            &*self.db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: Some(None),
                blocked_json: Some(None),
                failed_json: None,
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        super::clear_execution_retry_metadata(&self.db, &updated).await?;
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if task.blocked_json.is_some()
            && matches!(
                lifecycle.state,
                db::TaskLifecycleState::Ready
                    | db::TaskLifecycleState::Active
                    | db::TaskLifecycleState::ReadyToMerge
            )
        {
            self.publish(ForgeEvent {
                event_type: "task.unblocked".to_owned(),
                entity_id: updated.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskUnblocked {
                    project_id: updated.project_id.clone(),
                    previous_reason,
                },
            });
        }
        Ok(updated)
    }

    pub async fn fail_task(
        &self,
        task_id: impl Into<String>,
        reason: impl Into<String>,
        kind: Option<api_types::FailureKind>,
        execution_id: Option<String>,
    ) -> Result<Task> {
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        if task.failed_json.is_some() {
            return Err(ServiceError::invalid_operation("task has already failed"));
        }
        if task.error_annotation.is_some() || task.blocked_json.is_some() {
            return Err(ServiceError::invalid_operation(
                "resolve the exact current Execution block before marking the Task failed",
            ));
        }
        let reason = reason.into();
        let lifecycle_cause = match execution_id.as_deref() {
            Some(execution_id) => {
                crate::task_lifecycle::LifecycleCause::Execution(execution_id.to_owned())
            }
            None => crate::task_lifecycle::LifecycleCause::System(
                api_types::SystemComponent::TaskDispatcher,
            ),
        };
        let lifecycle_reason_ref = execution_id
            .clone()
            .unwrap_or_else(|| format!("task-version:{}", task.version));
        let lifecycle_reason_kind = "task_failed";
        let block = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .block(
            &task_id,
            lifecycle_cause,
            lifecycle_reason_kind,
            lifecycle_reason_ref.clone(),
            format!("task-failed:{}:{lifecycle_reason_ref}", task.id),
        )
        .await?;
        if block.lifecycle.state != db::TaskLifecycleState::Blocked
            || block.lifecycle.reason_kind.as_deref() != Some(lifecycle_reason_kind)
            || block.lifecycle.reason_ref.as_deref() != Some(lifecycle_reason_ref.as_str())
        {
            return Err(ServiceError::invalid_operation(
                "another cause owns the current TaskLifecycle block",
            ));
        }
        let task = block.task;
        let failed_meta = json!({
            "reason": reason,
            "created_at": now_rfc3339(),
            "kind": kind,
            "execution_id": execution_id,
        });
        let updated = TaskRepo::update(
            &*self.db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: None,
                blocked_json: Some(None),
                failed_json: Some(Some(failed_meta.to_string())),
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        self.publish(ForgeEvent {
            event_type: "task.failed".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskFailed {
                project_id: updated.project_id.clone(),
                reason: reason.clone(),
                kind,
                execution_id: execution_id.clone(),
            },
        });
        tracing::info!(
            task_id = %updated.id,
            lifecycle_state = %updated.lifecycle_state,
            reason = %reason,
            kind = ?kind,
            execution_id = ?execution_id,
            "task marked as failed"
        );
        Ok(updated)
    }

    pub async fn restart_failed_task(&self, task_id: impl Into<String>) -> Result<Task> {
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        if task.failed_json.is_none() {
            return Err(ServiceError::invalid_operation("task has not failed"));
        }
        if task.error_annotation.is_some() || task.blocked_json.is_some() {
            return Err(ServiceError::invalid_operation(
                "restart cannot clear separate Execution blocking metadata",
            ));
        }
        let previous_reason = interruption_reason(task.failed_json.as_deref());
        let updated = TaskRepo::update(
            &*self.db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: Some(None),
                blocked_json: None,
                failed_json: Some(None),
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        self.publish(ForgeEvent {
            event_type: "task.restarted".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskRestarted {
                project_id: updated.project_id.clone(),
                previous_reason: previous_reason.clone(),
                new_execution_id: None,
            },
        });
        tracing::info!(
            task_id = %updated.id,
            lifecycle_state = %updated.lifecycle_state,
            previous_reason = ?previous_reason,
            "failed task restarted"
        );
        Ok(updated)
    }

    async fn recover_resume_session(
        &self,
        task: Task,
        annotation: &api_types::TaskBlockingAnnotation,
        reason: Option<String>,
        context: Option<String>,
    ) -> Result<Task> {
        let blocked_execution_id = annotation.blocked_execution_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("resume_session requires blocked_execution_id")
        })?;
        let blocked_execution = ExecutionRepo::get_by_id(&*self.db, blocked_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", blocked_execution_id.to_owned()))?;
        if !super::has_exact_work_unit_target(&self.db, &task.id, &blocked_execution).await? {
            return Err(ServiceError::invalid_operation(
                "resume_session requires an Execution bound to its exact open WorkUnit and Workspace",
            ));
        }
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if lifecycle.state != db::TaskLifecycleState::Blocked {
            return Err(ServiceError::invalid_operation(
                "recovery requires a blocked TaskLifecycle",
            ));
        }
        let work_unit_id = blocked_execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "resume_session cannot infer a WorkUnit from Task or Workspace identity",
            )
        })?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        let authoritative_memberships = if let Some(role) =
            db::canonical_task_role_name(&blocked_execution.role)
        {
            crate::task_service::current_role_memberships_authoritative(&self.db, &task.id, &role)
                .await?
        } else {
            None
        };
        let agent_id = if let Some(memberships) = authoritative_memberships.as_ref() {
            let lineage_agent_id = match blocked_execution.actor_ref() {
                Some(db::ActorRef::Agent(agent_id)) => Some(agent_id),
                Some(db::ActorRef::Human(_)) => None,
                None => blocked_execution.agent_id.clone(),
            };
            let lineage_is_usable = if let Some(agent_id) = lineage_agent_id.as_deref() {
                if task.repo_id.is_some() {
                    crate::task_service::is_usable_repository_agent(
                        &self.db,
                        &task.project_id,
                        memberships,
                        agent_id,
                    )
                    .await?
                } else {
                    crate::task_service::is_usable_active_agent(&self.db, memberships, agent_id)
                        .await?
                }
            } else {
                false
            };
            let selected = if lineage_is_usable {
                lineage_agent_id
            } else if task.repo_id.is_some() {
                crate::task_service::select_usable_repository_agent_id(
                    &self.db,
                    &task.project_id,
                    memberships,
                )
                .await?
            } else {
                crate::task_service::select_usable_agent_id(&self.db, memberships).await?
            };
            selected.ok_or_else(|| {
                ServiceError::invalid_operation(format!(
                    "no usable Agent is available for recovery role {}",
                    blocked_execution.role
                ))
            })?
        } else {
            blocked_execution
                .agent_id
                .as_ref()
                .cloned()
                .ok_or_else(|| {
                    ServiceError::invalid_operation("blocked execution has no assignee")
                })?
        };
        if work_unit.assigned_actor.as_ref() != Some(&db::ActorRef::Agent(agent_id.clone())) {
            return Err(ServiceError::invalid_operation(
                "resume_session cannot change the Actor assigned to the exact WorkUnit",
            ));
        }
        let parent_actor_matches = matches!(
            blocked_execution.actor_ref(),
            Some(db::ActorRef::Agent(ref parent_agent_id)) if parent_agent_id == &agent_id
        );
        self.ensure_task_runnable(&task).await?;
        let workspace = match blocked_execution.workspace_id.as_deref() {
            Some(_) => Some(
                exact_recovery_workspace(&self.db, &task, &blocked_execution)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(
                            "recovery requires the exact WorkUnit Workspace recorded on the Execution",
                        )
                    })?,
            ),
            None => None,
        };
        let (harness_session_id, updated_snapshot) = if parent_actor_matches {
            let snapshot_json = blocked_execution
                .executor_config_snapshot_json
                .clone()
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "blocked execution missing executor config snapshot",
                    )
                })?;
            let _external_session_id = resumable_external_session(
                &self.db,
                &blocked_execution,
                Some(&agent_id),
                blocked_execution.workspace_id.as_deref(),
            )
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "blocked execution has no reusable explicit HarnessSession",
                )
            })?;
            (
                blocked_execution.harness_session_id.clone(),
                executor_snapshot_for_harness_resume(&snapshot_json)?,
            )
        } else {
            // Recovery still creates a new Execution, but a current Actor
            // change makes the blocked Actor's HarnessSession ineligible.
            let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
            (
                None,
                build_executor_config_snapshot(
                    &self.db,
                    &task,
                    &agent,
                    None,
                    self.adapter_registry.as_deref(),
                )
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "current recovery Agent produced no executor config snapshot",
                    )
                })?,
            )
        };
        let artifact_input_ids = self
            .inherit_plan_artifact_inputs(&blocked_execution.id, &task.id)
            .await?;
        let updated_snapshot = if let Some(ctx) = context.as_deref() {
            let mut snap: serde_json::Value =
                serde_json::from_str(&updated_snapshot).map_err(|e| {
                    ServiceError::invalid_operation(format!("failed to parse snapshot: {e}"))
                })?;
            if let Some(obj) = snap.as_object_mut() {
                obj.insert(
                    "resume_context".to_owned(),
                    serde_json::Value::String(ctx.to_owned()),
                );
            }
            serde_json::to_string(&snap).map_err(|e| {
                ServiceError::invalid_operation(format!("failed to serialize snapshot: {e}"))
            })?
        } else {
            updated_snapshot
        };
        self.ensure_task_runnable(&task).await?;
        let now = now_rfc3339();
        let (updated_task, execution) = self
            .admit_recovery_work_unit_execution_with_artifact_inputs(
                &task,
                &lifecycle,
                &blocked_execution.id,
                work_unit_id,
                work_unit.version,
                CreateExecution {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    agent_id: Some(agent_id.clone()),
                    actor_ref: Some(db::ActorRef::Agent(agent_id)),
                    purpose: Some(blocked_execution.purpose.clone().unwrap_or_else(|| {
                        execution_purpose_for_task_type(&task.task_type, &blocked_execution.role)
                    })),
                    harness_session_id,
                    role: blocked_execution.role.clone(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: Some(blocked_execution.id.clone()),
                    // The generic HarnessSession is the authority. The DB
                    // layer projects its external id to the legacy field.
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: match (
                        context.as_deref(),
                        blocked_execution
                            .prompt
                            .as_ref()
                            .or(blocked_execution.summary.as_ref()),
                    ) {
                        (Some(ctx), Some(orig)) => Some(format!("[User context: {ctx}]\n\n{orig}")),
                        (Some(ctx), None) => Some(format!("[User context: {ctx}]")),
                        (None, s) => s.cloned(),
                    },
                    logs_path: None,
                    before_sha: workspace
                        .as_ref()
                        .and_then(|workspace| workspace.before_sha.clone())
                        .or_else(|| blocked_execution.before_sha.clone()),
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json: Some(updated_snapshot),
                    workspace_id: blocked_execution.workspace_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                artifact_input_ids,
            )
            .await?;
        self.publish(ForgeEvent {
            event_type: "task.execution_resumed".to_owned(),
            entity_id: updated_task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskRecovered {
                project_id: updated_task.project_id.clone(),
                reason: reason.unwrap_or_else(|| "resume_session".to_owned()),
            },
        });
        self.spawn_recovery_execution(
            updated_task.id.clone(),
            execution.id.clone(),
            "resume_session",
        )
        .await?;
        Ok(updated_task)
    }

    async fn recover_reexecute(
        &self,
        _task: Task,
        annotation: &api_types::TaskBlockingAnnotation,
        reason: Option<String>,
        context: Option<String>,
    ) -> Result<Task> {
        let blocked_execution_id = annotation.blocked_execution_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "reexecute requires blocked_execution_id; Task state cannot identify a WorkUnit target",
            )
        })?;
        let result = self
            .re_execute_execution_for_recovery(blocked_execution_id, context)
            .await?;
        self.spawn_recovery_execution(
            result.task.id.clone(),
            result.execution.id.clone(),
            "reexecute",
        )
        .await?;
        let recovered = result.task;
        self.publish(ForgeEvent {
            event_type: "task.recovery_action".to_owned(),
            entity_id: recovered.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskRecovered {
                project_id: recovered.project_id.clone(),
                reason: reason.unwrap_or_else(|| "reexecute".to_owned()),
            },
        });
        Ok(recovered)
    }

    async fn spawn_recovery_execution(
        &self,
        _task_id: String,
        execution_id: String,
        _recovery_action: &'static str,
    ) -> Result<()> {
        self.start_execution(execution_id).await?;
        Ok(())
    }

    async fn recover_cancel_task(&self, task: Task, _reason: Option<String>) -> Result<Task> {
        // Admit cancellation before removing its recovery evidence. If the
        // lifecycle CAS or cancellation policy rejects the request, retain the
        // annotation so the Task remains recoverable.
        let cancelled = self.cancel_task(task.id).await?;
        self.clear_blocking_metadata(&cancelled.id).await
    }
}

fn metadata_recovery_annotation(
    raw_metadata: &str,
    recovery_actions: &[api_types::RecoveryAction],
) -> Result<api_types::TaskBlockingAnnotation> {
    let metadata: Value = serde_json::from_str(raw_metadata).map_err(|error| {
        ServiceError::invalid_operation(format!("failed to parse interruption metadata: {error}"))
    })?;
    let reason = metadata
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("interrupted")
        .to_owned();
    let execution_id = metadata
        .get("execution_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let annotation_type = metadata
        .get("kind")
        .cloned()
        .and_then(|kind| serde_json::from_value::<api_types::FailureKind>(kind).ok())
        .unwrap_or(api_types::FailureKind::Unknown);
    // Unknown kinds are info-only: legacy rows the migration could not map
    // must not offer actions the service cannot classify.
    let recovery_actions = if annotation_type == api_types::FailureKind::Unknown {
        Vec::new()
    } else {
        recovery_actions.to_vec()
    };
    Ok(api_types::TaskBlockingAnnotation {
        annotation_type,
        blocking_reason: reason.clone(),
        blocked_by: Some(api_types::Actor::system(api_types::SystemComponent::Workflow).display()),
        blocked_at: metadata
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_owned),
        blocked_execution_id: execution_id.clone(),
        artifact: execution_id.map(|id| api_types::BlockingArtifact {
            kind: "execution".to_owned(),
            id: Some(id),
            log_path: None,
        }),
        message: Some(reason),
        hook: None,
        recovery_actions,
    })
}

fn interruption_reason(raw_metadata: Option<&str>) -> Option<String> {
    raw_metadata
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|metadata| {
            metadata
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

async fn exact_recovery_workspace(
    db: &SqliteDb,
    task: &Task,
    execution: &Execution,
) -> Result<Option<db::Workspace>> {
    if !super::has_exact_work_unit_target(db, &task.id, execution).await? {
        return Ok(None);
    }
    let Some(work_unit_id) = execution.work_unit_id.as_deref() else {
        return Ok(None);
    };
    let Some(workspace_id) = execution.workspace_id.as_deref() else {
        return Ok(None);
    };
    let Some(workspace) = WorkspaceRepo::get_by_id(db, workspace_id).await? else {
        return Ok(None);
    };
    let Some(scope) = WorkUnitWorkspaceRepo::get_scope_by_id(db, workspace_id).await? else {
        return Ok(None);
    };
    if workspace.task_id != task.id || scope.task_id != task.id {
        return Ok(None);
    }

    if scope.kind == db::WorkspaceScopeKind::WorkUnit
        && scope.work_unit_id.as_deref() == Some(work_unit_id)
    {
        Ok(Some(workspace))
    } else {
        Ok(None)
    }
}
