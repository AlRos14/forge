use super::*;
use db::{WorkUnitRepo, WorkspaceRepo};

impl TaskService {
    /// Prepare a target Execution from one explicitly assigned WorkUnit.
    /// Repository WorkUnits must already have their exact isolated Workspace.
    pub async fn launch_work_unit_execution(
        &self,
        work_unit_id: &str,
        expected_work_unit_version: i64,
        purpose: ExecutionPurpose,
        prompt: String,
        input_artifact_ids: Vec<String>,
        overrides: Option<ExecutionOverrides>,
    ) -> Result<LaunchExecutionResult> {
        validate_required("work_unit_id", work_unit_id)?;
        validate_required("prompt", &prompt)?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        if work_unit.version != expected_work_unit_version
            || work_unit.status != db::WorkUnitStatus::Open
        {
            return Err(db::DbError::VersionConflict.into());
        }
        let task = TaskRepo::get_by_id(&*self.db, &work_unit.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", work_unit.task_id.clone()))?;
        let (agent_id, actor_ref) = match work_unit.assigned_actor.as_ref() {
            Some(db::ActorRef::Agent(agent_id)) => {
                (agent_id.clone(), db::ActorRef::Agent(agent_id.clone()))
            }
            Some(db::ActorRef::Human(_)) => {
                return Err(ServiceError::invalid_operation(
                    "a Human-assigned WorkUnit cannot be launched by the Harness runtime",
                ));
            }
            None => {
                return Err(ServiceError::invalid_operation(
                    "WorkUnit must have an exact assigned Actor before execution",
                ));
            }
        };
        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        if agent.paused {
            return Err(ServiceError::AgentPaused { agent_id });
        }
        if !crate::agent_capacity::has_running_execution_capacity(&self.db, &agent).await? {
            return Err(db::DbError::AgentAtCapacity.into());
        }

        let workspace = if work_unit.requires_integration {
            let workspace = db::WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                    "repository WorkUnit must have its exact Workspace prepared before execution",
                )
                })?;
            let scope = db::WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace scope", workspace.id.clone()))?;
            if workspace.task_id != task.id
                || workspace.status != db::WorkspaceStatus::Ready
                || scope.task_id != task.id
                || scope.kind != db::WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
            {
                return Err(ServiceError::invalid_operation(
                    "Workspace is not the ready Workspace for this exact WorkUnit",
                ));
            }
            Some(workspace)
        } else {
            if db::WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id)
                .await?
                .is_some()
            {
                return Err(ServiceError::invalid_operation(
                    "non-repository WorkUnit cannot bind a Workspace",
                ));
            }
            None
        };
        let _workspace_guard = match workspace.as_ref() {
            Some(workspace) => {
                if let Some(locks) = self.workspace_exec_locks.as_ref() {
                    Some(locks.acquire(&workspace.id).await)
                } else {
                    None
                }
            }
            None => None,
        };
        self.ensure_task_runnable(&task).await?;
        let task = self.activate_task_for_execution(task).await?;
        let executor_config_snapshot_json = build_executor_config_snapshot(
            &self.db,
            &task,
            &agent,
            overrides,
            self.adapter_registry.as_deref(),
        )
        .await?;
        let now = now_rfc3339();
        let execution = self
            .create_running_work_unit_execution_with_artifact_inputs(
                work_unit_id,
                expected_work_unit_version,
                CreateExecution {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    agent_id: Some(agent_id.clone()),
                    actor_ref: Some(actor_ref),
                    purpose: Some(purpose),
                    harness_session_id: None,
                    role: work_unit.role.clone(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: Some(prompt),
                    logs_path: None,
                    before_sha: workspace
                        .as_ref()
                        .and_then(|workspace| workspace.before_sha.clone()),
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: workspace.as_ref().map(|workspace| workspace.id.clone()),
                    created_at: now.clone(),
                    updated_at: now,
                },
                input_artifact_ids,
            )
            .await?;
        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id,
                execution_id: execution.id.clone(),
            },
        });
        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn dispatch_initial_role_execution(
        &self,
        task_id: &str,
        agent_id: &str,
        role: &str,
        purpose: ExecutionPurpose,
        prompt: String,
    ) -> Result<Execution> {
        self.dispatch_initial_role_execution_with_metadata(
            task_id, agent_id, role, purpose, prompt, None,
        )
        .await
    }

    pub async fn dispatch_initial_role_execution_with_metadata(
        &self,
        task_id: &str,
        agent_id: &str,
        role: &str,
        purpose: ExecutionPurpose,
        prompt: String,
        dispatch_metadata: Option<Value>,
    ) -> Result<Execution> {
        self.dispatch_initial_role_execution_with_artifacts(
            task_id,
            agent_id,
            role,
            purpose,
            prompt,
            Vec::new(),
            dispatch_metadata,
        )
        .await
    }

    pub async fn dispatch_initial_role_execution_with_artifacts(
        &self,
        _task_id: &str,
        _agent_id: &str,
        _role: &str,
        _purpose: ExecutionPurpose,
        _prompt: String,
        _input_artifact_ids: Vec<String>,
        _dispatch_metadata: Option<Value>,
    ) -> Result<Execution> {
        Err(ServiceError::invalid_operation(
            "workflow dispatch is retired; start an explicitly allocated WorkUnit revision",
        ))
    }

    pub async fn launch_execution(
        &self,
        _task_id: impl Into<String>,
        _agent_id: impl Into<String>,
        _summary: Option<String>,
        _overrides: Option<ExecutionOverrides>,
    ) -> Result<LaunchExecutionResult> {
        Err(ServiceError::invalid_operation(
            "Task-scoped launch is retired; start an explicitly allocated WorkUnit revision",
        ))
    }

    pub async fn follow_up_execution(
        &self,
        parent_execution_id: impl Into<String>,
        message: String,
        agent_id: Option<String>,
        overrides: Option<ExecutionOverrides>,
    ) -> Result<LaunchExecutionResult> {
        let parent_execution_id = parent_execution_id.into();
        validate_required("parent_execution_id", &parent_execution_id)?;
        let parent_execution = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if !matches!(
            parent_execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "follow-up requires a completed, failed, or cancelled execution, got {}",
                parent_execution.status
            )));
        }
        let task = TaskRepo::get_by_id(&*self.db, &parent_execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_execution.task_id.clone()))?;
        if !super::has_exact_work_unit_target(&self.db, &task.id, &parent_execution).await? {
            return Err(ServiceError::invalid_operation(
                "follow-up requires the exact open WorkUnit and Workspace recorded on its parent Execution",
            ));
        }
        let work_unit_id = parent_execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("follow-up cannot infer a WorkUnit from Task identity")
        })?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        let assigned_agent_id = match work_unit.assigned_actor.as_ref() {
            Some(db::ActorRef::Agent(agent_id)) => agent_id,
            Some(db::ActorRef::Human(_)) => {
                return Err(ServiceError::invalid_operation(
                    "a Human-assigned WorkUnit has no Harness Actor to continue",
                ));
            }
            None => {
                return Err(ServiceError::invalid_operation(
                    "follow-up WorkUnit has no exact assigned Actor",
                ));
            }
        };
        if let Some(requested_agent_id) = agent_id.as_deref() {
            if requested_agent_id != assigned_agent_id {
                return Err(ServiceError::AuthorizationDenied {
                    message: "follow-up Agent must match the exact WorkUnit assignment".to_owned(),
                });
            }
        }
        let resolved_agent_id = assigned_agent_id.clone();
        let agent = AgentRepo::get_by_id(&*self.db, assigned_agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", assigned_agent_id.clone()))?;
        let workspace = match parent_execution.workspace_id.as_deref() {
            Some(workspace_id) => Some(
                WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?,
            ),
            None => None,
        };
        let _workspace_guard = match workspace.as_ref() {
            Some(workspace) => {
                if let Some(locks) = self.workspace_exec_locks.as_ref() {
                    Some(locks.acquire(&workspace.id).await)
                } else {
                    None
                }
            }
            None => None,
        };
        self.ensure_task_runnable(&task).await?;
        let task = self.activate_task_for_execution(task).await?;
        let parent_actor_matches = matches!(
            parent_execution.actor_ref(),
            Some(db::ActorRef::Agent(ref parent_agent_id)) if parent_agent_id == &resolved_agent_id
        );
        let reusable_session = if parent_actor_matches {
            reusable_harness_session_for_agent(
                &self.db,
                &parent_execution,
                &resolved_agent_id,
                parent_execution.workspace_id.as_deref(),
            )
            .await?
        } else {
            None
        };
        if parent_actor_matches
            && parent_execution.harness_session_id.is_some()
            && reusable_session.is_none()
        {
            return Err(ServiceError::invalid_operation(
                "explicit HarnessSession is not resumable for this follow-up",
            ));
        }
        let reusable_external_session = reusable_session
            .as_ref()
            .filter(|session| matches!(&session.status, db::HarnessSessionStatus::Active))
            .and_then(|session| session.external_session_id.clone());
        let mut executor_config_snapshot_json = build_executor_config_snapshot(
            &self.db,
            &task,
            &agent,
            overrides,
            self.adapter_registry.as_deref(),
        )
        .await?;
        if let (Some(_session_id), Some(snapshot_json), Some(parent_snapshot_json)) = (
            reusable_external_session.as_deref(),
            executor_config_snapshot_json.as_deref(),
            parent_execution.executor_config_snapshot_json.as_deref(),
        ) {
            // Resume is candidate-identity-aware: the parent's winning
            // candidate is promoted when still routed; a candidate switch
            // starts a fresh session instead of replaying another
            // account's session id.
            executor_config_snapshot_json = Some(
                crate::task_service::config::executor_snapshot_with_sticky_candidate(
                    snapshot_json,
                    parent_snapshot_json,
                    self.adapter_registry.as_deref(),
                )?,
            );
        }

        let artifact_input_ids = self
            .inherit_plan_artifact_inputs(&parent_execution.id, &task.id)
            .await?;
        if task.error_annotation.is_some() {
            self.clear_blocking_metadata(&task.id).await?;
        }
        let now = now_rfc3339();
        let execution = self
            .create_running_work_unit_execution_with_artifact_inputs(
                work_unit_id,
                work_unit.version,
                CreateExecution {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    agent_id: Some(resolved_agent_id.clone()),
                    actor_ref: Some(db::ActorRef::Agent(resolved_agent_id.clone())),
                    purpose: Some(
                        parent_execution
                            .purpose
                            .clone()
                            .unwrap_or(ExecutionPurpose::General),
                    ),
                    harness_session_id: reusable_session.as_ref().and_then(|session| {
                        reusable_external_session
                            .as_ref()
                            .map(|_| session.id.clone())
                    }),
                    role: work_unit.role.clone(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: Some(parent_execution_id),
                    // The generic HarnessSession is the authority. The DB
                    // layer projects its external id to the legacy field.
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: Some(message),
                    logs_path: None,
                    before_sha: workspace
                        .as_ref()
                        .and_then(|workspace| workspace.before_sha.clone())
                        .or_else(|| parent_execution.before_sha.clone()),
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: parent_execution.workspace_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                artifact_input_ids,
            )
            .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %resolved_agent_id,
            execution_id = %execution.id,
            parent_execution_id = %parent_execution.id,
            role = %work_unit.role,
            "follow-up execution launched"
        );

        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id: resolved_agent_id,
                execution_id: execution.id.clone(),
            },
        });

        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn re_execute_execution(
        &self,
        parent_execution_id: impl Into<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context(parent_execution_id, None)
            .await
    }

    pub async fn re_execute_execution_with_context(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context_inner(parent_execution_id, context, false)
            .await
    }

    pub(super) async fn re_execute_execution_for_recovery(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context_inner(parent_execution_id, context, true)
            .await
    }

    async fn re_execute_execution_with_context_inner(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
        clear_recovery_metadata: bool,
    ) -> Result<LaunchExecutionResult> {
        let parent_execution_id = parent_execution_id.into();
        validate_required("parent_execution_id", &parent_execution_id)?;
        let parent_execution = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if !matches!(
            parent_execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "re-execute requires a completed, failed, or cancelled execution, got {}",
                parent_execution.status
            )));
        }
        let task = TaskRepo::get_by_id(&*self.db, &parent_execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_execution.task_id.clone()))?;
        if !super::has_exact_work_unit_target(&self.db, &task.id, &parent_execution).await? {
            return Err(ServiceError::invalid_operation(
                "re-execute requires the exact open WorkUnit and Workspace recorded on its parent Execution",
            ));
        }
        let work_unit_id = parent_execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("re-execute cannot infer a WorkUnit from Task identity")
        })?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        let agent_id = match work_unit.assigned_actor.as_ref() {
            Some(db::ActorRef::Agent(agent_id))
                if parent_execution.actor_ref().as_ref()
                    == Some(&db::ActorRef::Agent(agent_id.clone())) =>
            {
                agent_id.clone()
            }
            Some(db::ActorRef::Human(_)) => {
                return Err(ServiceError::invalid_operation(
                    "a Human-assigned WorkUnit cannot be re-executed by a Harness Agent",
                ));
            }
            _ => {
                return Err(ServiceError::invalid_operation(
                    "re-execute requires the parent Execution Actor to match its exact WorkUnit assignment",
                ));
            }
        };
        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        let workspace = match parent_execution.workspace_id.as_deref() {
            Some(workspace_id) => Some(
                WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?,
            ),
            None => None,
        };
        let _workspace_guard = match workspace.as_ref() {
            Some(workspace) => {
                if let Some(locks) = self.workspace_exec_locks.as_ref() {
                    Some(locks.acquire(&workspace.id).await)
                } else {
                    None
                }
            }
            None => None,
        };
        self.ensure_task_runnable(&task).await?;
        let mut task = self.activate_task_for_execution(task).await?;
        if clear_recovery_metadata {
            task = self.clear_blocking_metadata(&task.id).await?;
        }
        let executor_config_snapshot_json = build_executor_config_snapshot(
            &self.db,
            &task,
            &agent,
            None,
            self.adapter_registry.as_deref(),
        )
        .await?;
        let source_prompt = parent_execution
            .prompt
            .as_deref()
            .or(parent_execution.summary.as_deref())
            .unwrap_or_default();
        let summary = match context.as_deref() {
            Some(ctx) if !source_prompt.is_empty() => {
                format!("[User context: {ctx}]\n\n{source_prompt}")
            }
            Some(ctx) => format!("[User context: {ctx}]"),
            None => source_prompt.to_owned(),
        };
        let artifact_input_ids = self
            .inherit_plan_artifact_inputs(&parent_execution.id, &task.id)
            .await?;
        let now = now_rfc3339();
        let execution = self
            .create_running_work_unit_execution_with_artifact_inputs(
                work_unit_id,
                work_unit.version,
                CreateExecution {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    agent_id: Some(agent.id.clone()),
                    actor_ref: Some(db::ActorRef::Agent(agent.id.clone())),
                    purpose: Some(parent_execution.purpose.clone().unwrap_or_else(|| {
                        execution_purpose_for_task_type(&task.task_type, &work_unit.role)
                    })),
                    harness_session_id: None,
                    role: work_unit.role.clone(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: Some(parent_execution.id.clone()),
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: Some(summary),
                    logs_path: None,
                    before_sha: workspace
                        .as_ref()
                        .and_then(|workspace| workspace.before_sha.clone())
                        .or_else(|| parent_execution.before_sha.clone()),
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: parent_execution.workspace_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                artifact_input_ids,
            )
            .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent.id,
            execution_id = %execution.id,
            parent_execution_id = %parent_execution.id,
            role = %execution.role,
            "re-execute execution launched"
        );
        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id,
                execution_id: execution.id.clone(),
            },
        });
        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn cancel_execution(
        &self,
        execution_id: impl Into<String>,
        reason: String,
    ) -> Result<Execution> {
        self.stop_execution_with_actor(
            execution_id,
            reason,
            api_types::Actor::user(api_types::UserActionSource::Api),
            "user_cancelled",
            "Execution stopped by user",
        )
        .await
    }

    pub async fn pause_execution(
        &self,
        execution_id: impl Into<String>,
        reason: String,
    ) -> Result<Execution> {
        self.stop_execution_with_actor(
            execution_id,
            reason,
            api_types::Actor::user(api_types::UserActionSource::Api),
            "user_paused",
            "Task paused by user",
        )
        .await
    }

    async fn stop_execution_with_actor(
        &self,
        execution_id: impl Into<String>,
        reason: String,
        actor: api_types::Actor,
        blocking_reason: &str,
        annotation_message: &str,
    ) -> Result<Execution> {
        let execution_id = execution_id.into();
        let execution = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if execution.status != ExecutionStatus::Running {
            return Err(ServiceError::invalid_operation(format!(
                "can only cancel a running execution, got {}",
                execution.status
            )));
        }
        let now = now_rfc3339();
        self.cancel_active_execution(
            &execution,
            &reason,
            db::StopReason::UserCancelled,
            &actor,
            db::ResumePolicy::Manual,
        )
        .await?;
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let task = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .block(
            &task.id,
            crate::task_lifecycle::LifecycleCause::Execution(execution.id.clone()),
            format!("manual_stop:{blocking_reason}"),
            execution.id.clone(),
            format!("manual-stop:{}:{}", execution.id, blocking_reason),
        )
        .await?
        .task;
        let mut recovery_actions = vec![
            api_types::RecoveryAction::Reexecute,
            api_types::RecoveryAction::CancelTask,
        ];
        if resumable_external_session(
            &self.db,
            &execution,
            execution.agent_id.as_deref(),
            execution.workspace_id.as_deref(),
        )
        .await?
        .is_some()
        {
            recovery_actions.insert(0, api_types::RecoveryAction::ResumeSession);
        }
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: api_types::FailureKind::ManualStop,
            blocking_reason: blocking_reason.to_owned(),
            blocked_by: Some(actor.display()),
            blocked_at: Some(now.clone()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: None,
            }),
            message: Some(annotation_message.to_owned()),
            hook: None,
            recovery_actions,
        };
        let annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize manual-stop annotation: {error}"
            ))
        })?;
        let _ = TaskRepo::update(
            &*self.db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: None,
                failed_json: None,
                parent_task_id: None,
                updated_at: now.clone(),
            },
        )
        .await?;
        ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id))
    }
}

impl TaskService {
    pub(super) async fn activate_task_for_execution(&self, mut task: Task) -> Result<Task> {
        let lifecycle = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        );
        let mut current = lifecycle
            .get(&task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;

        // A backlog Task must first cross the explicit backlog -> ready edge.
        // If execution startup crashes between edges, replay resumes from the
        // durable aggregate state.
        if current.state == db::TaskLifecycleState::Backlog {
            task = lifecycle
                .transition(crate::task_lifecycle::TransitionLifecycleInput {
                    task_id: task.id.clone(),
                    expected_task_version: task.version,
                    to_state: db::TaskLifecycleState::Ready,
                    cause: crate::task_lifecycle::LifecycleCause::System(
                        api_types::SystemComponent::Executor,
                    ),
                    reason_kind: Some("execution_start".to_owned()),
                    reason_ref: Some(format!("task:{}:backlog-to-ready", task.id)),
                    idempotency_key: format!("execution-start:{}:backlog", task.id),
                })
                .await?
                .task;
            current = lifecycle
                .get(&task.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        }

        if current.state == db::TaskLifecycleState::Active {
            return Ok(task);
        }
        if !matches!(
            current.state,
            db::TaskLifecycleState::Ready
                | db::TaskLifecycleState::Blocked
                | db::TaskLifecycleState::ReadyToMerge
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "cannot launch an Execution for Task lifecycle {}",
                current.state
            )));
        }

        Ok(lifecycle
            .transition(crate::task_lifecycle::TransitionLifecycleInput {
                task_id: task.id.clone(),
                expected_task_version: task.version,
                to_state: db::TaskLifecycleState::Active,
                cause: crate::task_lifecycle::LifecycleCause::System(
                    api_types::SystemComponent::Executor,
                ),
                reason_kind: Some("execution_start".to_owned()),
                reason_ref: Some(format!("task:{}:active", task.id)),
                idempotency_key: format!("execution-start:{}:active", task.id),
            })
            .await?
            .task)
    }
}
