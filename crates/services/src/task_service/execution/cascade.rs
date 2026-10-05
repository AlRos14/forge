// Legacy retry/review cascade helpers remain stored until PR13 cleanup. The
// public completion entry point below is intentionally a no-op after PR9.
#![allow(dead_code)]

use super::*;
use db::WorkspaceRepo;
use sha2::{Digest, Sha256};

impl TaskService {
    pub async fn maybe_cascade_executor_completion(&self, execution_id: &str) -> Result<()> {
        // Execution completion is a durable fact consumed by event-driven
        // orchestration and Gate evaluation. It cannot advance aggregate Task
        // lifecycle or turn a reviewer execution into a verdict.
        let _ = execution_id;
        Ok(())
    }

    async fn handle_executor_completion_guard_rejection(
        &self,
        execution: &Execution,
        task: &Task,
        current_state: &api_types::StateDefinition,
        guard: &str,
        reason: &str,
    ) -> Result<()> {
        let current_workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
            .await?
            .map(|workspace| workspace.id);
        let resumable_session_id = resumable_external_session(
            &self.db,
            execution,
            execution.agent_id.as_deref(),
            current_workspace_id.as_deref(),
        )
        .await?;
        if guard == "subtask_sequence_complete" && task.parent_task_id.is_none() {
            if let Some(next_turn) = self.subtasks_handoff(task).await? {
                match next_turn {
                    super::subtasks::NextTurn::Prompt { user_prompt } => {
                        if resumable_session_id.is_none() {
                            tracing::warn!(
                                task_id = %task.id,
                                execution_id = %execution.id,
                                "subtask handoff cannot resume: missing agent_session_id; blocking task"
                            );
                            return self
                                .annotate_workflow_guard_block(execution, task, guard, reason)
                                .await;
                        }
                        self.resume_execution_for_workflow_guard(execution, task, user_prompt)
                            .await?;
                    }
                    super::subtasks::NextTurn::AllDone => {
                        self.retry_parent_cascade_after_last_subtask(task).await?;
                    }
                }
                return Ok(());
            }
        }

        let budget = crate::task_service::config::runtime_retry_budget(
            task,
            crate::task_service::config::RetryBudgetKind::Execution,
            Some(&current_state.config),
            current_state.gate_config.as_ref(),
        )?;
        let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "invalid task metadata for {}: {error}",
                task.id
            ))
        })?;
        let retry_count = metadata
            .extra
            .get("workflow_guard_retry_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);

        if budget <= 0
            || retry_count >= budget as u64
            || resumable_session_id.is_none()
            || execution.agent_id.is_none()
        {
            return self
                .annotate_workflow_guard_block(execution, task, guard, reason)
                .await;
        }

        let attempt = retry_count + 1;
        metadata.extra.insert(
            "workflow_guard_retry_count".to_owned(),
            Value::Number(serde_json::Number::from(attempt)),
        );
        metadata.extra.insert(
            "last_workflow_guard_rejection_at".to_owned(),
            Value::String(now_rfc3339()),
        );
        metadata.extra.insert(
            "last_workflow_guard_name".to_owned(),
            Value::String(guard.to_owned()),
        );
        metadata.extra.insert(
            "last_workflow_guard_reason".to_owned(),
            Value::String(reason.to_owned()),
        );
        TaskRepo::set_metadata_json(&*self.db, &task.id, metadata.to_json(), &now_rfc3339())
            .await?;

        let prompt = render_workflow_guard_follow_up_prompt(guard, reason, attempt, budget as u64);
        self.resume_execution_for_workflow_guard(execution, task, prompt)
            .await?;
        Ok(())
    }

    fn resume_execution_for_workflow_guard<'a>(
        &'a self,
        execution: &'a Execution,
        task: &'a Task,
        prompt: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Execution>> + Send + 'a>> {
        Box::pin(async move {
            let current_workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
                .await?
                .map(|workspace| workspace.id);
            let authoritative_memberships =
                if let Some(role) = db::canonical_task_role_name(&execution.role) {
                    crate::task_service::current_role_memberships_authoritative(
                        &self.db, &task.id, &role,
                    )
                    .await?
                } else {
                    None
                };
            let agent_id = if let Some(memberships) = authoritative_memberships.as_ref() {
                let selected = if task.repo_id.is_some() {
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
                        "no usable Agent is available for workflow-guard role {}",
                        execution.role
                    ))
                })?
            } else {
                execution.agent_id.clone().ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "execution {} missing agent_id",
                        execution.id
                    ))
                })?
            };
            let parent_actor_matches = matches!(
                execution.actor_ref(),
                Some(db::ActorRef::Agent(ref parent_agent_id)) if parent_agent_id == &agent_id
            );
            let (harness_session_id, updated_snapshot) = if parent_actor_matches {
                let agent_session_id = resumable_external_session(
                    &self.db,
                    execution,
                    Some(&agent_id),
                    current_workspace_id.as_deref(),
                )
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "execution {} has no reusable HarnessSession",
                        execution.id
                    ))
                })?;
                let continuity_execution = if execution.harness_session_id.is_none() {
                    materialize_historical_harness_session(&self.db, execution, &agent_session_id)
                        .await?
                        .ok_or_else(|| {
                            ServiceError::invalid_operation(format!(
                                "execution {} has unresolved legacy session authority",
                                execution.id
                            ))
                        })?
                } else {
                    execution.clone()
                };
                let snapshot_json = execution
                    .executor_config_snapshot_json
                    .as_deref()
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "execution {} missing executor config snapshot",
                            execution.id
                        ))
                    })?;
                (
                    continuity_execution.harness_session_id,
                    Some(executor_snapshot_for_harness_resume(snapshot_json)?),
                )
            } else {
                // The current RoleMembership Actor wins. A membership change
                // starts fresh work for the new Agent rather than inheriting
                // the completed Actor's external thread.
                let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
                (
                    None,
                    build_executor_config_snapshot(
                        &self.db,
                        task,
                        &agent,
                        None,
                        self.adapter_registry.as_deref(),
                    )
                    .await?,
                )
            };
            let artifact_input_ids = self
                .inherit_plan_artifact_inputs(&execution.id, &task.id)
                .await?;
            let execution_id = new_uuid_v4();
            let now = now_rfc3339();
            let resumed = self
                .create_running_execution_with_artifact_inputs(
                    CreateExecution {
                        id: execution_id.clone(),
                        task_id: task.id.clone(),
                        agent_id: Some(agent_id.clone()),
                        actor_ref: Some(db::ActorRef::Agent(agent_id)),
                        purpose: Some(execution.purpose.clone().unwrap_or_else(|| {
                            execution_purpose_for_workflow_state(
                                &task.task_type,
                                &task.status,
                                &execution.role,
                            )
                        })),
                        harness_session_id,
                        role: execution.role.clone(),
                        status: ExecutionStatus::Running,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        parent_execution_id: Some(execution.id.clone()),
                        // The generic HarnessSession is the authority. The
                        // DB layer projects its external id to the legacy
                        // field.
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: Some(prompt),
                        // Keep Artifact inputs insertable in the creation
                        // transaction; the runner/remote log writer assigns
                        // the durable path before producing output.
                        logs_path: None,
                        before_sha: execution.before_sha.clone(),
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: updated_snapshot,
                        workspace_id: current_workspace_id.clone(),
                        created_at: now.clone(),
                        updated_at: now,
                    },
                    false,
                    artifact_input_ids,
                )
                .await?;

            self.publish(ForgeEvent {
                event_type: "follow_up.dispatched".to_owned(),
                entity_id: task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::FollowUpDispatched {
                    task_id: task.id.clone(),
                    parent_execution_id: execution.id.clone(),
                    execution_id: resumed.id.clone(),
                    trigger: "workflow_guard_rejected".to_owned(),
                },
            });

            self.start_execution(resumed.id.clone()).await?;

            Ok(resumed)
        })
    }

    async fn subtasks_handoff(&self, task: &Task) -> Result<Option<super::subtasks::NextTurn>> {
        let subtasks = db::TaskRepo::list_subtasks_ordered(&*self.db, &task.id).await?;
        if subtasks.is_empty() {
            return Ok(None);
        }

        match super::subtasks::finish_current_turn_and_begin_next(
            &self.db,
            &self.event_bus,
            &self.workspace_root,
            &task.id,
        )
        .await
        {
            Ok(next_turn) => Ok(Some(next_turn)),
            Err(error) => {
                tracing::error!(
                    task_id = %task.id,
                    %error,
                    "subtask handoff failed, falling back to generic handling"
                );
                Ok(None)
            }
        }
    }

    async fn retry_parent_cascade_after_last_subtask(&self, task: &Task) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, &task.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            &task,
            &project.workflow_definition,
            &api_types::Actor::system(api_types::SystemComponent::Workflow),
        );
        let Some(target) = workflow
            .auto_transition_target(&task.status)
            .map(str::to_owned)
        else {
            return Ok(());
        };

        let from = task.status.clone();
        match self
            .transition(
                task.id.clone(),
                target.clone(),
                TransitionOptions {
                    version: task.version,
                    reason: Some("all subtasks completed".to_owned()),
                    triggered_by: api_types::Actor::system(api_types::SystemComponent::Workflow),
                    rejection: false,
                    defer_dispatch_seconds: None,
                },
            )
            .await
        {
            Ok(_) => {
                if let Err(error) = self.clear_workflow_guard_retry_metadata(&task.id).await {
                    tracing::warn!(
                        task_id = %task.id,
                        %error,
                        "failed to clear workflow guard retry metadata"
                    );
                }
                self.publish(ForgeEvent {
                    event_type: "task.auto_transitioned".to_owned(),
                    entity_id: task.id.clone(),
                    timestamp: event_timestamp(),
                    context: EventContext::TaskAutoTransitioned {
                        task_id: task.id,
                        from,
                        to: target,
                        reason: "all_subtasks_completed".to_owned(),
                    },
                });
                Ok(())
            }
            Err(ServiceError::Db(DbError::VersionConflict)) => {
                tracing::warn!(
                    task_id = %task.id,
                    "last subtask cascade version conflict"
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn annotate_workflow_guard_block(
        &self,
        execution: &Execution,
        task: &Task,
        guard: &str,
        reason: &str,
    ) -> Result<()> {
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: api_types::FailureKind::WorkflowGuardRejected,
            blocking_reason: guard.to_owned(),
            blocked_by: Some(
                api_types::Actor::system(api_types::SystemComponent::Workflow).display(),
            ),
            blocked_at: Some(now_rfc3339()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: execution.logs_path.clone(),
            }),
            message: Some(reason.to_owned()),
            hook: None,
            recovery_actions: vec![
                api_types::RecoveryAction::ResumeSession,
                api_types::RecoveryAction::Reexecute,
                api_types::RecoveryAction::CancelTask,
            ],
        };
        let annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize workflow-guard annotation: {error}"
            ))
        })?;
        let blocked_meta = json!({
            "reason": reason,
            "created_at": now_rfc3339(),
            "kind": api_types::FailureKind::WorkflowGuardRejected,
            "source": guard,
            "execution_id": execution.id,
        });
        let updated = TaskRepo::update_status(
            &*self.db,
            UpdateTaskStatus {
                id: task.id.clone(),
                expected_version: task.version,
                status: task.status.clone(),
                assignee_id: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: Some(Some(blocked_meta.to_string())),
                failed_json: Some(None),
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        self.publish_domain_event_by_dedupe(&format!(
            "task-status-update:{}:{}",
            updated.id, updated.version
        ))
        .await;

        self.publish(ForgeEvent {
            event_type: "task.blocked".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskBlocked {
                project_id: updated.project_id,
                reason: reason.to_owned(),
                kind: Some(api_types::FailureKind::WorkflowGuardRejected),
                source: Some(guard.to_owned()),
                execution_id: Some(execution.id.clone()),
            },
        });
        Ok(())
    }

    async fn clear_workflow_guard_retry_metadata(&self, task_id: &str) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "invalid task metadata for {}: {error}",
                task.id
            ))
        })?;
        let mut changed = false;
        for key in [
            "workflow_guard_retry_count",
            "last_workflow_guard_rejection_at",
            "last_workflow_guard_name",
            "last_workflow_guard_reason",
        ] {
            changed |= metadata.extra.remove(key).is_some();
        }
        if changed {
            TaskRepo::set_metadata_json(&*self.db, &task.id, metadata.to_json(), &now_rfc3339())
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn annotate_executor_failure_block(
        &self,
        execution: &Execution,
    ) -> Result<()> {
        self.annotate_executor_failure_block_inner(execution).await
    }

    pub(crate) async fn annotate_dispatch_failure_block(
        &self,
        execution: &Execution,
    ) -> Result<()> {
        self.annotate_executor_failure_block_inner(execution).await
    }

    /// Handle an execution that failed because no executor candidate could
    /// run (`FailureKind::ExecutorUnavailable`). Never consumes the task's
    /// execution retry budget: transient exhaustion (a retry time is known)
    /// schedules a deferred dispatch; permanent unavailability blocks the
    /// task for manual reconfiguration with no automatic redispatch loop.
    pub(crate) async fn annotate_executor_unavailable_block(
        &self,
        execution: &Execution,
        retry_at: Option<String>,
        attempts: Value,
    ) -> Result<()> {
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

        if execution.role != "interactive" {
            if let Some(retry_at) = retry_at.as_deref() {
                let dispatch_at = executor_unavailable_dispatch_time(retry_at, &task.id);
                crate::deferred_dispatch::set(
                    &self.db,
                    &task,
                    &task.status,
                    &dispatch_at,
                    "executor unavailable; retrying when usage recovers",
                )
                .await?;
                ExecutionRepo::update(
                    &*self.db,
                    db::UpdateExecution {
                        id: execution.id.clone(),
                        status: None,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: Some(Some(db::ResumePolicy::Auto)),
                        stopped_at: None,
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: None,
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                tracing::info!(
                    task_id = %task.id,
                    execution_id = %execution.id,
                    %dispatch_at,
                    "all executor candidates unavailable; deferred dispatch scheduled without consuming retry budget"
                );
                return Ok(());
            }
        }

        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: api_types::FailureKind::ExecutorUnavailable,
            blocking_reason: "executor_unavailable".to_owned(),
            blocked_by: Some(
                api_types::Actor::system(api_types::SystemComponent::Executor).display(),
            ),
            blocked_at: Some(now_rfc3339()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: execution.logs_path.clone(),
            }),
            message: Some(execution.error.clone().unwrap_or_else(|| {
                "No executor candidate is available (check CLI installs and authentication)"
                    .to_owned()
            })),
            hook: None,
            recovery_actions: vec![
                api_types::RecoveryAction::Reexecute,
                api_types::RecoveryAction::ResetToInitial,
                api_types::RecoveryAction::CancelTask,
            ],
        };
        let annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize executor-unavailable annotation: {error}"
            ))
        })?;

        let reason = execution
            .error
            .clone()
            .unwrap_or_else(|| "no executor candidate available".to_owned());
        let blocked_meta = json!({
            "reason": reason,
            "created_at": now_rfc3339(),
            "kind": api_types::FailureKind::ExecutorUnavailable,
            "execution_id": execution.id,
            "details": {
                "retry_at": retry_at,
                "attempts": attempts,
            },
        });

        let task = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .block(
            &task.id,
            crate::task_lifecycle::LifecycleCause::Execution(execution.id.clone()),
            "executor_unavailable",
            execution.id.clone(),
            format!("executor-unavailable:{}", execution.id),
        )
        .await?
        .task;

        let updated = TaskRepo::update_status(
            &*self.db,
            UpdateTaskStatus {
                id: task.id.clone(),
                expected_version: task.version,
                status: task.status.clone(),
                assignee_id: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: Some(Some(blocked_meta.to_string())),
                failed_json: Some(None),
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        self.publish_domain_event_by_dedupe(&format!(
            "task-status-update:{}:{}",
            updated.id, updated.version
        ))
        .await;

        tracing::info!(
            task_id = %task.id,
            execution_id = %execution.id,
            status = %task.status,
            kind = "executor_unavailable",
            "task blocked: no executor candidate available"
        );
        self.publish(ForgeEvent {
            event_type: "task.blocked".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskBlocked {
                project_id: updated.project_id,
                reason,
                kind: Some(api_types::FailureKind::ExecutorUnavailable),
                source: None,
                execution_id: Some(execution.id.clone()),
            },
        });
        Ok(())
    }

    async fn annotate_executor_failure_block_inner(&self, execution: &Execution) -> Result<()> {
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
            api_types::RecoveryAction::ResetToInitial,
            api_types::RecoveryAction::CancelTask,
        ];
        let current_workspace_id = WorkspaceRepo::get_by_task_id(&*self.db, &task.id)
            .await?
            .map(|workspace| workspace.id);
        let resumable_session_id = resumable_external_session(
            &self.db,
            execution,
            execution.agent_id.as_deref(),
            current_workspace_id.as_deref(),
        )
        .await?;
        if resumable_session_id.is_some() {
            recovery_actions.insert(0, api_types::RecoveryAction::ResumeSession);
        }
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: api_types::FailureKind::ExecutorFailed,
            blocking_reason: "executor_failed".to_owned(),
            blocked_by: Some(
                api_types::Actor::system(api_types::SystemComponent::Executor).display(),
            ),
            blocked_at: Some(now_rfc3339()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: execution.logs_path.clone(),
            }),
            message: Some(
                execution
                    .error
                    .clone()
                    .unwrap_or_else(|| "Execution failed".to_owned()),
            ),
            hook: None,
            recovery_actions,
        };
        let annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize executor-failure annotation: {error}"
            ))
        })?;

        let reason = execution
            .error
            .clone()
            .unwrap_or_else(|| "executor failed".to_owned());
        let blocked_meta = json!({
            "reason": reason,
            "created_at": now_rfc3339(),
            "kind": api_types::FailureKind::InternalCommandFailed,
            "execution_id": execution.id,
        });

        let updated = TaskRepo::update_status(
            &*self.db,
            UpdateTaskStatus {
                id: task.id.clone(),
                expected_version: task.version,
                status: task.status.clone(),
                assignee_id: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: Some(Some(blocked_meta.to_string())),
                failed_json: Some(None),
                updated_at: now_rfc3339(),
            },
        )
        .await?;

        self.publish_domain_event_by_dedupe(&format!(
            "task-status-update:{}:{}",
            updated.id, updated.version
        ))
        .await;

        tracing::info!(
            task_id = %task.id,
            execution_id = %execution.id,
            status = %task.status,
            kind = "internal_command_failed",
            "task blocked after executor failure"
        );
        self.publish(ForgeEvent {
            event_type: "task.blocked".to_owned(),
            entity_id: updated.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskBlocked {
                project_id: updated.project_id,
                reason,
                kind: Some(api_types::FailureKind::InternalCommandFailed),
                source: None,
                execution_id: Some(execution.id.clone()),
            },
        });
        Ok(())
    }

    async fn maybe_cascade_reviewer_completion(&self, execution: &Execution) -> Result<()> {
        if execution.purpose != Some(ExecutionPurpose::Review)
            || execution.status != ExecutionStatus::Completed
        {
            // Execution failure is not a cognitive Review verdict.
            return Ok(());
        }
        let task = match TaskRepo::get_by_id(&*self.db, &execution.task_id, false).await? {
            Some(task) => task,
            None => return Ok(()),
        };
        if task.status != crate::workflow::default_states::REVIEW {
            return Ok(());
        }
        let report = db::CollaborationRepo::get_execution_artifact_output(
            &*self.db,
            &execution.id,
            db::ArtifactKind::ReviewReport,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "completed Review Execution has no exact ReviewReport Artifact",
            )
        })?;
        if report.task_id != task.id
            || !matches!(
                &report.producer,
                db::ArtifactProducer::Execution { execution_id, .. } if execution_id == &execution.id
            )
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport producer does not match its exact Review Execution",
            ));
        }
        let content = report.content.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("ReviewReport content is unavailable")
        })?;
        let value: Value = serde_json::from_str(content).map_err(|error| {
            ServiceError::invalid_operation(format!("ReviewReport JSON is invalid: {error}"))
        })?;
        let subject = value.get("subject").ok_or_else(|| {
            ServiceError::invalid_operation("ReviewReport has no exact subject identity")
        })?;
        if value.get("kind").and_then(Value::as_str) != Some("review_report")
            || subject.get("task_id").and_then(Value::as_str) != Some(task.id.as_str())
            || subject.get("review_execution_id").and_then(Value::as_str)
                != Some(execution.id.as_str())
            || subject.get("workspace_id").and_then(Value::as_str)
                != execution.workspace_id.as_deref()
            || subject.get("base_commit_sha").and_then(Value::as_str)
                != execution.before_sha.as_deref()
            || subject.get("head_commit_sha").and_then(Value::as_str)
                != execution.after_sha.as_deref()
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport subject identity does not match its producer Execution",
            ));
        }
        let verdict = value
            .get("verdict")
            .and_then(Value::as_str)
            .ok_or_else(|| ServiceError::invalid_operation("ReviewReport verdict is missing"))?;
        match verdict {
            "pass" => {
                let agent_review = matches!(execution.actor_ref(), Some(db::ActorRef::Agent(_)));
                if agent_review && self.gate_requires_user_approval(&task).await? {
                    return Ok(());
                }
                self.cascade_completed_review_task(
                    &task,
                    crate::workflow::default_states::MERGING,
                    &format!("ReviewReport {} passed", report.id),
                    false,
                    &execution.id,
                )
                .await
            }
            "request_changes" | "questions" => {
                let role = db::TaskRoleRepo::get_by_task_and_role(
                    &*self.db,
                    &task.id,
                    crate::workflow::default_roles::CODER,
                )
                .await?;
                let target = role
                    .map(|role| db::CollaborationTarget::Role(role.id))
                    .unwrap_or(db::CollaborationTarget::Task);
                let message_id =
                    review_collaboration_message_id(&execution.id, &report.id, verdict);
                crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                    .create_message_with_id(
                        crate::CollaborationActorSource::Execution(execution.id.clone()),
                        crate::collaboration_service::CreateMessageInput {
                            task_id: task.id.clone(),
                            target,
                            work_unit_id: execution.work_unit_id.clone(),
                            body: if verdict == "request_changes" {
                                format!("ReviewReport {} requests changes.", report.id)
                            } else {
                                format!(
                                    "ReviewReport {} contains questions requiring clarification.",
                                    report.id
                                )
                            },
                            artifact_ids: vec![report.id.clone()],
                        },
                        message_id,
                    )
                    .await?;
                if verdict == "request_changes" {
                    let target = self.review_rework_target(&task).await?;
                    self.cascade_completed_review_task(
                        &task,
                        &target,
                        &format!("ReviewReport {} requests changes", report.id),
                        true,
                        &execution.id,
                    )
                    .await
                } else {
                    Ok(())
                }
            }
            _ => Err(ServiceError::invalid_operation(
                "ReviewReport verdict is outside the supported contract",
            )),
        }
    }

    async fn cascade_completed_review_task(
        &self,
        task: &Task,
        target: &str,
        reason: &str,
        rejection: bool,
        causing_execution_id: &str,
    ) -> Result<()> {
        let from = task.status.clone();
        match self
            .transition_caused_by_execution(
                task.id.clone(),
                target.to_owned(),
                TransitionOptions {
                    version: task.version,
                    reason: Some(reason.to_owned()),
                    triggered_by: api_types::Actor::system(api_types::SystemComponent::Workflow),
                    rejection,
                    defer_dispatch_seconds: None,
                },
                causing_execution_id,
            )
            .await
        {
            Ok(_) => {
                self.publish(ForgeEvent {
                    event_type: "task.auto_transitioned".to_owned(),
                    entity_id: task.id.clone(),
                    timestamp: event_timestamp(),
                    context: EventContext::TaskAutoTransitioned {
                        task_id: task.id.clone(),
                        from,
                        to: target.to_owned(),
                        reason: reason.to_owned(),
                    },
                });
                Ok(())
            }
            Err(ServiceError::Db(DbError::VersionConflict)) => {
                tracing::warn!(
                    task_id = %task.id,
                    "review completion cascade version conflict"
                );
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    async fn gate_requires_user_approval(&self, task: &Task) -> Result<bool> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            task,
            &project.workflow_definition,
            &api_types::Actor::system(api_types::SystemComponent::Workflow),
        );
        Ok(workflow
            .states
            .iter()
            .find(|state| state.name == task.status)
            .and_then(|state| state.gate_config.as_ref())
            .is_some_and(|gate_config| gate_config.requires_user_approval()))
    }

    async fn review_rework_target(&self, task: &Task) -> Result<String> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for_task(
            task,
            &project.workflow_definition,
            &api_types::Actor::system(api_types::SystemComponent::Workflow),
        );
        Ok(workflow
            .states
            .iter()
            .find(|state| state.name == task.status)
            .and_then(|state| {
                state
                    .gate_config
                    .as_ref()
                    .and_then(|gate| gate.reject_target.clone())
                    .or_else(|| {
                        state
                            .triggers
                            .get(&api_types::WorkflowTrigger::Reject)
                            .map(|trigger| trigger.to.clone())
                    })
            })
            .unwrap_or_else(|| crate::workflow::default_states::IN_PROGRESS.to_owned()))
    }
}

fn render_workflow_guard_follow_up_prompt(
    guard: &str,
    reason: &str,
    attempt: u64,
    budget: u64,
) -> String {
    format!(
        "Your previous execution completed, but Forge could not move the task to the next workflow state.\n\nWorkflow guard failed: {guard}\n\nFailure:\n{reason}\n\nMake sure you complete all tasks and Fix what is needed for this guard, update any completed checklist items to `- [x]`, you dont need to commit anything if all tasks are complete.\n\nRetry {attempt}/{budget}."
    )
}

/// Dispatch time for a transient executor-unavailable retry: the structured
/// retry hint plus a small deterministic jitter, floored at ten seconds out
/// so a stale hint cannot hot-loop.
fn executor_unavailable_dispatch_time(retry_at: &str, task_id: &str) -> String {
    let jitter_seconds = i64::from(
        task_id
            .bytes()
            .fold(0u8, |acc, byte| acc.wrapping_add(byte))
            % 30,
    );
    let hinted = chrono::DateTime::parse_from_rfc3339(retry_at)
        .map(|at| at.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::minutes(15));
    let floor = chrono::Utc::now() + chrono::Duration::seconds(10);
    (hinted + chrono::Duration::seconds(jitter_seconds))
        .max(floor)
        .to_rfc3339()
}

pub(crate) fn should_block_task_for_failed_execution(execution: &Execution) -> bool {
    matches!(
        execution.role.as_str(),
        "interactive" | "executor" | crate::workflow::default_roles::CODER
    )
}

fn review_collaboration_message_id(execution_id: &str, report_id: &str, verdict: &str) -> String {
    let digest = Sha256::digest(
        format!("review-collaboration:{execution_id}:{report_id}:{verdict}").as_bytes(),
    );
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
