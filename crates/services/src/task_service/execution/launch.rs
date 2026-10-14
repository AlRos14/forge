use super::*;
use db::{WorkUnitRepo, WorkspaceRepo};
use sha2::Digest as _;

impl TaskService {
    /// Admit one human-selected, exact manual crash-recovery target. The
    /// service records the decision and child Execution atomically; callers
    /// dispatch the provider only for a newly admitted child.
    pub async fn recover_work_unit_execution(
        &self,
        task_id: impl Into<String>,
        parent_execution_id: impl Into<String>,
        request: api_types::RecoverWorkUnitExecutionRequest,
        requested_by: impl Into<String>,
    ) -> Result<LaunchExecutionResult> {
        let task_id = task_id.into();
        let parent_execution_id = parent_execution_id.into();
        let requested_by = requested_by.into();
        validate_required("task_id", &task_id)?;
        validate_required("parent_execution_id", &parent_execution_id)?;
        validate_required("work_unit_id", &request.work_unit_id)?;
        validate_required("requested_by", &requested_by)?;
        if request.idempotency_key.trim().is_empty() || request.idempotency_key.len() > 200 {
            return Err(ServiceError::invalid_operation(
                "recovery idempotency_key must contain 1 to 200 characters",
            ));
        }
        let request_digest = explicit_recovery_request_digest(
            &task_id,
            &parent_execution_id,
            &request,
            &requested_by,
        );
        let receipt_dedupe_key = format!(
            "execution-recovery:{}:{}",
            task_id,
            hex::encode(sha2::Sha256::digest(request.idempotency_key.as_bytes()))
        );
        if let Some(replay) = self
            .replay_explicit_recovery(
                &task_id,
                &parent_execution_id,
                &request,
                &requested_by,
                &request_digest,
                &receipt_dedupe_key,
            )
            .await?
        {
            return Ok(replay);
        }

        let parent_execution = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if parent_execution.task_id != task_id
            || parent_execution.work_unit_id.as_deref() != Some(request.work_unit_id.as_str())
            || parent_execution.status != ExecutionStatus::Cancelled
            || !matches!(
                parent_execution.stop_reason,
                Some(
                    db::StopReason::CrashRecovery
                        | db::StopReason::AgentTimeout
                        | db::StopReason::GracefulShutdown
                )
            )
            || !matches!(
                parent_execution.resume_policy,
                Some(db::ResumePolicy::Manual | db::ResumePolicy::Auto)
            )
        {
            return Err(ServiceError::invalid_operation(
                "recovery requires an exact cancelled crash-recovery, timeout, or graceful-shutdown Execution without a child",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.clone()))?;
        if task.version != request.expected_task_version {
            return Err(db::DbError::TaskVersionConflict {
                expected: request.expected_task_version,
                actual: task.version,
            }
            .into());
        }
        if lifecycle.version != request.expected_lifecycle_version {
            return Err(db::DbError::VersionConflict.into());
        }
        let ambiguous_reason_ref = match lifecycle.state {
            db::TaskLifecycleState::Blocked => {
                if lifecycle.reason_kind.as_deref() != Some("recovery_target_ambiguous")
                    || task.blocked_json.is_some()
                {
                    return Err(ServiceError::invalid_operation(
                        "only the current ambiguous crash-recovery block accepts an explicit target",
                    ));
                }
                Some(lifecycle.reason_ref.clone().ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "ambiguous recovery block has no current reason reference",
                    )
                })?)
            }
            db::TaskLifecycleState::Active => {
                if task.error_annotation.is_some()
                    || task.blocked_json.is_some()
                    || task.failed_json.is_some()
                {
                    return Err(ServiceError::invalid_operation(
                        "active Task has pending blocking or failure metadata",
                    ));
                }
                None
            }
            state => {
                return Err(ServiceError::invalid_operation(format!(
                    "manual crash recovery cannot activate TaskLifecycle {state}"
                )))
            }
        };
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, &request.work_unit_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("work_unit", request.work_unit_id.clone()))?;
        if work_unit.task_id != task_id
            || work_unit.version != request.expected_work_unit_version
            || work_unit.status != db::WorkUnitStatus::Open
            || !super::has_exact_work_unit_target(&self.db, &task_id, &parent_execution).await?
        {
            return Err(db::DbError::VersionConflict.into());
        }
        let agent_id = match (
            work_unit.assigned_actor.as_ref(),
            parent_execution.actor_ref(),
        ) {
            (Some(db::ActorRef::Agent(assigned)), Some(db::ActorRef::Agent(parent)))
                if assigned == &parent
                    && parent_execution.agent_id.as_deref() == Some(parent.as_str()) =>
            {
                parent
            }
            _ => return Err(ServiceError::AuthorizationDenied {
                message:
                    "recovery requires the parent Actor to remain the exact assigned WorkUnit Agent"
                        .to_owned(),
            }),
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
        let workspace = match parent_execution.workspace_id.as_deref() {
            Some(workspace_id) => Some(
                WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?,
            ),
            None => None,
        };
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
        let summary = match request.context.as_deref() {
            Some(context) if !source_prompt.is_empty() => {
                format!("[User context: {context}]\n\n{source_prompt}")
            }
            Some(context) => format!("[User context: {context}]"),
            None => source_prompt.to_owned(),
        };
        let artifact_input_ids = self
            .inherit_plan_artifact_inputs(&parent_execution.id, &task.id)
            .await?;
        let now = now_rfc3339();
        let execution_input = CreateExecution {
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
        };
        let admission = super::super::ExplicitRecoveryAdmission {
            expected_task_version: request.expected_task_version,
            expected_lifecycle_version: request.expected_lifecycle_version,
            ambiguous_reason_ref,
            idempotency_key: request.idempotency_key.clone(),
            receipt_dedupe_key,
            request_digest,
            requested_by,
        };
        let (task, execution, idempotent_replay) = self
            .admit_recovery_work_unit_execution_with_options(
                &task,
                &lifecycle,
                &parent_execution.id,
                &request.work_unit_id,
                request.expected_work_unit_version,
                execution_input,
                artifact_input_ids,
                Some(admission),
            )
            .await?;
        if !idempotent_replay {
            tracing::info!(
                task_id = %task.id,
                agent_id = %agent.id,
                execution_id = %execution.id,
                parent_execution_id = %parent_execution.id,
                work_unit_id = %request.work_unit_id,
                "explicit WorkUnit crash recovery admitted"
            );
            self.publish(ForgeEvent {
                event_type: "task.execution_launched".to_owned(),
                entity_id: task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskAssigned {
                    project_id: task.project_id.clone(),
                    agent_id: agent.id,
                    execution_id: execution.id.clone(),
                },
            });
        }
        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
            idempotent_replay,
        })
    }

    async fn replay_explicit_recovery(
        &self,
        task_id: &str,
        parent_execution_id: &str,
        request: &api_types::RecoverWorkUnitExecutionRequest,
        requested_by: &str,
        request_digest: &str,
        receipt_dedupe_key: &str,
    ) -> Result<Option<LaunchExecutionResult>> {
        let Some(receipt) =
            db::DomainEventRepo::get_event_by_dedupe(&*self.db, receipt_dedupe_key).await?
        else {
            return Ok(None);
        };
        let payload: Value = serde_json::from_str(&receipt.payload_json).map_err(|error| {
            ServiceError::invalid_operation(format!("stored recovery receipt is invalid: {error}"))
        })?;
        if receipt.event_type != "execution.recovery_admitted"
            || receipt.entity_type != "execution"
            || receipt.actor_type != "human"
            || receipt.actor_id.as_deref() != Some(requested_by)
            || receipt.scope_type != "task"
            || receipt.scope_id != task_id
            || payload.get("task_id").and_then(Value::as_str) != Some(task_id)
            || payload.get("parent_execution_id").and_then(Value::as_str)
                != Some(parent_execution_id)
            || payload.get("work_unit_id").and_then(Value::as_str)
                != Some(request.work_unit_id.as_str())
            || payload.get("request_digest").and_then(Value::as_str) != Some(request_digest)
            || payload.get("requested_by").and_then(Value::as_str) != Some(requested_by)
            || payload.get("expected_task_version").and_then(Value::as_i64)
                != Some(request.expected_task_version)
            || payload
                .get("expected_lifecycle_version")
                .and_then(Value::as_i64)
                != Some(request.expected_lifecycle_version)
            || payload
                .get("expected_work_unit_version")
                .and_then(Value::as_i64)
                != Some(request.expected_work_unit_version)
        {
            return Err(db::DbError::IdempotencyConflict.into());
        }
        let execution = ExecutionRepo::get_by_id(&*self.db, &receipt.entity_id)
            .await?
            .filter(|execution| {
                execution.id
                    == payload
                        .get("execution_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                    && execution.task_id == task_id
                    && execution.work_unit_id.as_deref() == Some(request.work_unit_id.as_str())
                    && execution.parent_execution_id.as_deref() == Some(parent_execution_id)
            })
            .ok_or_else(|| {
                db::DbError::Check(
                    "recovery receipt does not resolve to its exact child Execution".to_owned(),
                )
            })?;
        let start_event = db::DomainEventRepo::get_event_by_dedupe(
            &*self.db,
            &format!("execution.started:{}", execution.id),
        )
        .await?;
        if !start_event.is_some_and(|event| {
            event.event_type == "execution.started"
                && event.entity_type == "execution"
                && event.entity_id == execution.id
                && event.scope_type == "task"
                && event.scope_id == task_id
        }) {
            return Err(db::DbError::Check(
                "recovery receipt child has no exact durable start event".to_owned(),
            )
            .into());
        }
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let workspace = match execution.workspace_id.as_deref() {
            Some(workspace_id) => WorkspaceRepo::get_by_id(&*self.db, workspace_id).await?,
            None => None,
        };
        Ok(Some(LaunchExecutionResult {
            task,
            execution,
            workspace,
            idempotent_replay: true,
        }))
    }

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
        self.ensure_task_runnable(&task).await?;
        let lifecycle_service = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        );
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if lifecycle.state == db::TaskLifecycleState::Blocked {
            return Err(ServiceError::invalid_operation(
                "a new WorkUnit cannot activate a blocked Task without its exact blocking Execution parent",
            ));
        }
        let lifecycle_transitions = lifecycle_service
            .plan_execution_activation(&task, &lifecycle)
            .await?;
        let lifecycle_plan = super::WorkUnitAdmissionLifecyclePlan {
            expected_task_version: task.version,
            lifecycle,
            transitions: lifecycle_transitions,
            clear_blocking_metadata: None,
            follow_up_receipt: None,
            automatic_recovery_parent_id: None,
        };
        let executor_config_snapshot_json = build_executor_config_snapshot(
            &self.db,
            &task,
            &agent,
            overrides,
            self.adapter_registry.as_deref(),
        )
        .await?;
        let now = now_rfc3339();
        let (task, execution, _) = self
            .admit_running_work_unit_execution_with_artifact_inputs(
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
                lifecycle_plan,
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
            idempotent_replay: false,
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
        idempotency_key: String,
    ) -> Result<LaunchExecutionResult> {
        let parent_execution_id = parent_execution_id.into();
        validate_required("parent_execution_id", &parent_execution_id)?;
        validate_required("idempotency_key", &idempotency_key)?;
        if idempotency_key.len() > 256 {
            return Err(ServiceError::invalid_operation(
                "follow-up idempotency key must be at most 256 bytes",
            ));
        }
        let request_bytes = serde_json::to_vec(&(
            parent_execution_id.as_str(),
            message.as_str(),
            agent_id.as_deref(),
            overrides.as_ref(),
        ))
        .map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to encode follow-up request identity: {error}"
            ))
        })?;
        let request_digest = hex::encode(sha2::Sha256::digest(request_bytes));
        if let Some(receipt) =
            db::WorkUnitExecutionRepo::get_follow_up_execution_receipt(&*self.db, &idempotency_key)
                .await?
        {
            if receipt.parent_execution_id != parent_execution_id
                || receipt.request_digest != request_digest
            {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            let parent = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
                .await?
                .ok_or_else(|| {
                    db::DbError::Check("follow-up receipt parent Execution is missing".to_owned())
                })?;
            let execution = ExecutionRepo::get_by_id(&*self.db, &receipt.execution_id)
                .await?
                .filter(|execution| {
                    execution.parent_execution_id.as_deref() == Some(parent_execution_id.as_str())
                        && execution.task_id == parent.task_id
                })
                .ok_or_else(|| {
                    db::DbError::Check(
                        "follow-up receipt does not resolve to its exact child Execution"
                            .to_owned(),
                    )
                })?;
            let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
            let workspace = match execution.workspace_id.as_deref() {
                Some(workspace_id) => Some(
                    WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("workspace", workspace_id))?,
                ),
                None => None,
            };
            return Ok(LaunchExecutionResult {
                task,
                execution,
                workspace,
                idempotent_replay: true,
            });
        }
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
        self.ensure_task_runnable(&task).await?;
        let lifecycle_service = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        );
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        let clear_blocking_metadata = if lifecycle.state == db::TaskLifecycleState::Blocked {
            Some(blocking_metadata_cleanup_plan(
                &task,
                &lifecycle,
                &parent_execution,
                work_unit_id,
            )?)
        } else {
            None
        };
        let lifecycle_transitions = lifecycle_service
            .plan_execution_activation(&task, &lifecycle)
            .await?;
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
        let lifecycle_plan = super::WorkUnitAdmissionLifecyclePlan {
            expected_task_version: task.version,
            lifecycle,
            transitions: lifecycle_transitions,
            clear_blocking_metadata,
            follow_up_receipt: Some(db::CreateFollowUpExecutionReceipt {
                idempotency_key,
                parent_execution_id: parent_execution_id.clone(),
                request_digest,
            }),
            automatic_recovery_parent_id: None,
        };
        let now = now_rfc3339();
        let (task, execution, idempotent_replay) = self
            .admit_running_work_unit_execution_with_artifact_inputs(
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
                lifecycle_plan,
            )
            .await?;

        if !idempotent_replay {
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
        }

        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
            idempotent_replay,
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
        self.re_execute_execution_with_context_inner(parent_execution_id, context, false, false)
            .await
    }

    pub(super) async fn re_execute_execution_for_recovery(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context_inner(parent_execution_id, context, true, false)
            .await
    }

    /// Admit an exact Auto recovery candidate as a child that resumes its
    /// parent's existing HarnessSession. Ordinary re-execution starts fresh.
    pub(crate) async fn re_execute_automatic_recovery_execution(
        &self,
        parent_execution_id: impl Into<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context_inner(parent_execution_id, None, false, true)
            .await
    }

    async fn re_execute_execution_with_context_inner(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
        clear_recovery_metadata: bool,
        automatic_recovery: bool,
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
        if automatic_recovery
            && (parent_execution.status != ExecutionStatus::Cancelled
                || parent_execution.resume_policy != Some(db::ResumePolicy::Auto)
                || !matches!(
                    parent_execution.stop_reason,
                    Some(db::StopReason::CrashRecovery | db::StopReason::GracefulShutdown)
                ))
        {
            return Err(ServiceError::invalid_operation(
                "automatic recovery requires an exact cancelled Auto crash-recovery Execution",
            ));
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
        if automatic_recovery {
            if let Some(memberships) = crate::task_service::current_role_memberships_authoritative(
                &self.db,
                &task.id,
                &work_unit.role,
            )
            .await?
            {
                let actor_is_active_member = memberships.iter().any(|membership| {
                    membership.status == db::RoleMembershipStatus::Active
                        && membership.actor_kind == db::ActorKind::Agent
                        && membership.actor_id == agent_id
                });
                if !actor_is_active_member {
                    return Err(ServiceError::AuthorizationDenied {
                        message: "automatic recovery requires the parent Agent's current active WorkUnit role membership"
                            .to_owned(),
                    });
                }
            }
        }
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
        self.ensure_task_runnable(&task).await?;
        let recovery_lifecycle = if clear_recovery_metadata {
            let annotation = self.recovery_annotation(&task)?;
            if annotation.blocked_execution_id.as_deref() != Some(parent_execution.id.as_str())
                || !annotation
                    .recovery_actions
                    .contains(&api_types::RecoveryAction::Reexecute)
            {
                return Err(ServiceError::invalid_operation(
                    "re-execute parent does not match the exact current recovery annotation",
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
            Some(lifecycle)
        } else {
            None
        };
        let ordinary_lifecycle_plan = if recovery_lifecycle.is_none() {
            let lifecycle_service = crate::task_lifecycle::TaskLifecycleService::new(
                Arc::clone(&self.db),
                Arc::clone(&self.event_bus),
            );
            let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
            let clear_blocking_metadata = if lifecycle.state == db::TaskLifecycleState::Blocked {
                Some(blocking_metadata_cleanup_plan(
                    &task,
                    &lifecycle,
                    &parent_execution,
                    work_unit_id,
                )?)
            } else {
                None
            };
            let lifecycle_transitions = lifecycle_service
                .plan_execution_activation(&task, &lifecycle)
                .await?;
            Some(super::WorkUnitAdmissionLifecyclePlan {
                expected_task_version: task.version,
                lifecycle,
                transitions: lifecycle_transitions,
                clear_blocking_metadata,
                follow_up_receipt: None,
                automatic_recovery_parent_id: automatic_recovery
                    .then(|| parent_execution.id.clone()),
            })
        } else {
            None
        };
        let (harness_session_id, executor_config_snapshot_json) = if automatic_recovery {
            let session_id = parent_execution.harness_session_id.clone().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "automatic recovery parent has no explicit HarnessSession",
                )
            })?;
            let _external_session_id = resumable_external_session(
                &self.db,
                &parent_execution,
                Some(&agent.id),
                parent_execution.workspace_id.as_deref(),
            )
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "automatic recovery parent HarnessSession is no longer resumable",
                )
            })?;
            let snapshot = parent_execution
                .executor_config_snapshot_json
                .as_deref()
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "automatic recovery parent is missing its executor snapshot",
                    )
                })?;
            (
                Some(session_id),
                Some(executor_snapshot_for_harness_resume(snapshot)?),
            )
        } else {
            (
                None,
                build_executor_config_snapshot(
                    &self.db,
                    &task,
                    &agent,
                    None,
                    self.adapter_registry.as_deref(),
                )
                .await?,
            )
        };
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
        let execution_input = CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id.clone(),
            agent_id: Some(agent.id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent.id.clone())),
            purpose: Some(parent_execution.purpose.clone().unwrap_or_else(|| {
                execution_purpose_for_task_type(&task.task_type, &work_unit.role)
            })),
            harness_session_id,
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
        };
        let (task, execution, idempotent_replay) =
            if let Some(lifecycle) = recovery_lifecycle.as_ref() {
                let (task, execution) = self
                    .admit_recovery_work_unit_execution_with_artifact_inputs(
                        &task,
                        lifecycle,
                        &parent_execution.id,
                        work_unit_id,
                        work_unit.version,
                        execution_input,
                        artifact_input_ids,
                    )
                    .await?;
                (task, execution, false)
            } else {
                self.admit_running_work_unit_execution_with_artifact_inputs(
                    work_unit_id,
                    work_unit.version,
                    execution_input,
                    artifact_input_ids,
                    ordinary_lifecycle_plan.expect("ordinary re-execution plan was prepared"),
                )
                .await?
            };

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent.id,
            execution_id = %execution.id,
            parent_execution_id = %parent_execution.id,
            role = %execution.role,
            "re-execute execution launched"
        );
        if !idempotent_replay {
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
        }
        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
            idempotent_replay,
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
        let lifecycle_reason_kind = format!("manual_stop:{blocking_reason}");
        let block = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
        )
        .block(
            &task.id,
            crate::task_lifecycle::LifecycleCause::Execution(execution.id.clone()),
            lifecycle_reason_kind.clone(),
            execution.id.clone(),
            format!("manual-stop:{}:{}", execution.id, blocking_reason),
        )
        .await?;
        if block.lifecycle.state != db::TaskLifecycleState::Blocked
            || block.lifecycle.reason_kind.as_deref() != Some(lifecycle_reason_kind.as_str())
            || block.lifecycle.reason_ref.as_deref() != Some(execution.id.as_str())
        {
            // The cancelled Execution remains historical, but it cannot
            // replace an unrelated WorkUnit's current aggregate block.
            return ExecutionRepo::get_by_id(&*self.db, &execution_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("execution", execution_id));
        }
        let task = block.task;
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

fn explicit_recovery_request_digest(
    task_id: &str,
    parent_execution_id: &str,
    request: &api_types::RecoverWorkUnitExecutionRequest,
    requested_by: &str,
) -> String {
    let request_identity = json!({
        "task_id": task_id,
        "parent_execution_id": parent_execution_id,
        "work_unit_id": request.work_unit_id,
        "expected_task_version": request.expected_task_version,
        "expected_lifecycle_version": request.expected_lifecycle_version,
        "expected_work_unit_version": request.expected_work_unit_version,
        "idempotency_key": request.idempotency_key,
        "requested_by": requested_by,
        "context": request.context,
    });
    hex::encode(sha2::Sha256::digest(
        request_identity.to_string().as_bytes(),
    ))
}

pub(super) fn validate_task_blocking_metadata_owner(
    task: &Task,
    blocking_execution_id: &str,
) -> Result<()> {
    if let Some(raw) = task.error_annotation.as_deref() {
        let annotation =
            serde_json::from_str::<api_types::TaskAnnotation>(raw).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "Task {} has an unreadable blocking annotation: {error}",
                    task.id
                ))
            })?;
        let api_types::TaskAnnotation::Blocking(annotation) = annotation else {
            return Err(ServiceError::invalid_operation(
                "Task blocking annotation has no exact Execution identity",
            ));
        };
        if annotation.blocked_execution_id.as_deref() != Some(blocking_execution_id)
            || annotation.artifact.as_ref().is_some_and(|artifact| {
                artifact.kind != "execution"
                    || artifact.id.as_deref() != Some(blocking_execution_id)
            })
        {
            return Err(ServiceError::invalid_operation(
                "Task blocking annotation belongs to a different or unknown Execution",
            ));
        }
    }
    if let Some(raw) = task.blocked_json.as_deref() {
        let metadata: Value = serde_json::from_str(raw).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "Task {} has unreadable blocked metadata: {error}",
                task.id
            ))
        })?;
        if metadata.get("execution_id").and_then(Value::as_str) != Some(blocking_execution_id) {
            return Err(ServiceError::invalid_operation(
                "Task blocked metadata belongs to a different or unknown Execution",
            ));
        }
    }
    Ok(())
}

fn blocking_metadata_cleanup_plan(
    task: &Task,
    lifecycle: &db::TaskLifecycle,
    blocking_execution: &Execution,
    blocking_work_unit_id: &str,
) -> Result<db::ClearTaskBlockingMetadata> {
    if lifecycle.state != db::TaskLifecycleState::Blocked
        || lifecycle.reason_kind.is_none()
        || lifecycle.reason_ref.as_deref() != Some(blocking_execution.id.as_str())
        || blocking_execution.task_id != task.id
        || blocking_execution.work_unit_id.as_deref() != Some(blocking_work_unit_id)
        || task.failed_json.is_some()
    {
        return Err(ServiceError::invalid_operation(
            "only the exact current Execution block for this WorkUnit can be resolved by admission",
        ));
    }
    validate_task_blocking_metadata_owner(task, &blocking_execution.id)?;
    let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata for {}: {error}", task.id))
    })?;
    for key in ["execution_retry_count", "last_execution_failure_at"] {
        metadata.extra.remove(key);
    }
    Ok(db::ClearTaskBlockingMetadata {
        blocking_execution_id: blocking_execution.id.clone(),
        blocking_work_unit_id: blocking_work_unit_id.to_owned(),
        expected_lifecycle_reason_kind: lifecycle
            .reason_kind
            .clone()
            .expect("blocking lifecycle reason kind checked above"),
        expected_error_annotation: task.error_annotation.clone(),
        expected_blocked_json: task.blocked_json.clone(),
        expected_metadata_json: task.metadata_json.clone(),
        replacement_metadata_json: metadata.to_json(),
    })
}
