use super::*;

impl TaskService {
    pub async fn dispatch_role_follow_up(
        &self,
        task_id: &str,
        role: &str,
        parent_execution_id: String,
        prompt: String,
        trigger: &str,
        purpose: ExecutionPurpose,
    ) -> Result<Execution> {
        dispatch_role_follow_up_impl(
            self.clone(),
            task_id.to_owned(),
            role.to_owned(),
            parent_execution_id,
            prompt,
            trigger.to_owned(),
            purpose,
            None,
        )
        .await
    }

    pub async fn dispatch_role_follow_up_with_agent(
        &self,
        task_id: &str,
        role: &str,
        parent_execution_id: String,
        agent_id: String,
        prompt: String,
        trigger: &str,
        purpose: ExecutionPurpose,
    ) -> Result<Execution> {
        dispatch_role_follow_up_impl(
            self.clone(),
            task_id.to_owned(),
            role.to_owned(),
            parent_execution_id,
            prompt,
            trigger.to_owned(),
            purpose,
            Some(agent_id),
        )
        .await
    }
}

fn dispatch_role_follow_up_impl(
    service: TaskService,
    task_id: String,
    role: String,
    parent_execution_id: String,
    prompt: String,
    trigger: String,
    purpose: ExecutionPurpose,
    agent_override: Option<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Execution>> + Send>> {
    Box::pin(async move {
        validate_required("task_id", &task_id)?;
        validate_required("role", &role)?;
        validate_required("parent_execution_id", &parent_execution_id)?;

        let supplied_parent_execution =
            ExecutionRepo::get_by_id(&*service.db, &parent_execution_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        let task = TaskRepo::get_by_id(&*service.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        // Continuity may come from the supplied causal Execution or its
        // explicit causal parent. Never substitute the latest Execution for a
        // role: recency is not session authority.
        let lineage_parent = if execution_role_matches(&supplied_parent_execution, &role) {
            supplied_parent_execution.clone()
        } else if let Some(parent_id) = supplied_parent_execution.parent_execution_id.as_deref() {
            ExecutionRepo::get_by_id(&*service.db, parent_id)
                .await?
                .unwrap_or_else(|| supplied_parent_execution.clone())
        } else {
            supplied_parent_execution.clone()
        };
        // Follow-up continuity is scoped by the Task's current workspace, not
        // only by the lineage snapshot. If the workspace was replaced after
        // the parent ran, the explicit session must fail the compatibility
        // check and the child must start without inheriting it.
        let current_workspace_id = WorkspaceRepo::get_by_task_id(&*service.db, &task_id)
            .await?
            .map(|workspace| workspace.id);
        let authoritative_memberships =
            crate::task_service::current_role_memberships_authoritative(
                &service.db,
                &task_id,
                &role,
            )
            .await?;
        let agent_id = if let Some(agent_id) = agent_override {
            if let Some(memberships) = authoritative_memberships.as_ref() {
                if crate::task_service::active_agent_membership(memberships, &agent_id).is_none() {
                    return Err(ServiceError::conflict(format!(
                        "Agent {agent_id} is not an active member of follow-up role {role}"
                    )));
                }
                if task.repo_id.is_some()
                    && !crate::task_service::repository_worker_identity_is_eligible(
                        &service.db,
                        &task.project_id,
                        &agent_id,
                    )
                    .await?
                {
                    return Err(ServiceError::conflict(format!(
                        "Agent {agent_id} cannot receive repository workspace authority"
                    )));
                }
            }
            agent_id
        } else if let Some(memberships) = authoritative_memberships.as_ref() {
            // Preserve causal continuity only while the parent Agent remains
            // an active, usable member of this exact role. A suspended or
            // unavailable lineage Agent falls back to deterministic current
            // membership selection.
            let lineage_agent_id = match lineage_parent.actor_ref() {
                Some(db::ActorRef::Agent(agent_id)) => Some(agent_id),
                Some(db::ActorRef::Human(_)) => None,
                None => lineage_parent.agent_id.clone(),
            };
            let lineage_is_usable = if let Some(agent_id) = lineage_agent_id.as_deref() {
                if task.repo_id.is_some() {
                    crate::task_service::is_usable_repository_agent(
                        &service.db,
                        &task.project_id,
                        memberships,
                        agent_id,
                    )
                    .await?
                } else {
                    crate::task_service::is_usable_active_agent(&service.db, memberships, agent_id)
                        .await?
                }
            } else {
                false
            };
            let selected = if lineage_is_usable {
                lineage_agent_id
            } else if task.repo_id.is_some() {
                crate::task_service::select_usable_repository_agent_id(
                    &service.db,
                    &task.project_id,
                    memberships,
                )
                .await?
            } else {
                crate::task_service::select_usable_agent_id(&service.db, memberships).await?
            };
            selected.ok_or_else(|| {
                ServiceError::invalid_operation(format!(
                    "no usable Agent is available for follow-up role {role}"
                ))
            })?
        } else {
            assigned_agent_for_follow_up(&service, &task_id, &role)
                .await?
                .or_else(|| lineage_parent.agent_id.clone())
                .or_else(|| supplied_parent_execution.agent_id.clone())
                .ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "no assigned agent available for follow-up role {role}"
                    ))
                })?
        };
        let agent = AgentRepo::get_by_id(&*service.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        // Actor selection is authoritative and precedes continuity. Legacy
        // session projections never authorize a follow-up to inherit a thread.
        let same_actor = matches!(
            lineage_parent.actor_ref(),
            Some(db::ActorRef::Agent(ref parent_agent_id)) if parent_agent_id == &agent_id
        );
        let reusable_session = if same_actor {
            reusable_harness_session_for_agent(
                &service.db,
                &lineage_parent,
                &agent_id,
                current_workspace_id.as_deref(),
            )
            .await?
        } else {
            None
        };
        if same_actor && lineage_parent.harness_session_id.is_some() && reusable_session.is_none() {
            return Err(ServiceError::invalid_operation(
                "explicit HarnessSession is not resumable for this follow-up",
            ));
        }
        let reusable_external_session = reusable_session
            .as_ref()
            .filter(|session| matches!(&session.status, db::HarnessSessionStatus::Active))
            .and_then(|session| session.external_session_id.clone());
        let executor_config_snapshot_json = if reusable_external_session.is_some() {
            let snapshot_json = lineage_parent
                .executor_config_snapshot_json
                .as_deref()
                .ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "parent execution {} missing executor config snapshot",
                        lineage_parent.id
                    ))
                })?;
            Some(executor_snapshot_for_harness_resume(snapshot_json)?)
        } else {
            build_executor_config_snapshot(
                &service.db,
                &task,
                &agent,
                None,
                service.adapter_registry.as_deref(),
            )
            .await?
        };
        let execution_id = new_uuid_v4();
        // Establish the final Task state/version before minting the
        // execution-scoped WorkspaceLease. A transition after issuance would
        // immediately make the exact-version authority stale.
        service.ensure_task_runnable(&task).await?;
        let task = service.activate_task_for_execution(task).await?;
        if task.error_annotation.is_some() {
            if let Err(error) = TaskRepo::update(
                &*service.db,
                UpdateTask {
                    id: task.id.clone(),
                    expected_version: task.version,
                    error_annotation: Some(None),
                    updated_at: now_rfc3339(),
                    title: None,
                    description: None,
                    priority: None,
                    merge_config: None,
                    blocked_json: None,
                    failed_json: None,
                    task_state_config: None,
                    parent_task_id: None,
                },
            )
            .await
            {
                tracing::warn!(%error, task_id = %task_id, "failed to clear error annotation before follow-up dispatch");
            }
        }
        let artifact_input_ids = service
            .inherit_plan_artifact_inputs(&supplied_parent_execution.id, &task_id)
            .await?;
        let now = now_rfc3339();
        let execution = service
            .create_running_execution_with_artifact_inputs(
                CreateExecution {
                    id: execution_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: Some(agent_id.clone()),
                    actor_ref: Some(db::ActorRef::Agent(agent_id.clone())),
                    purpose: Some(purpose),
                    harness_session_id: reusable_session.as_ref().and_then(|session| {
                        reusable_external_session
                            .as_ref()
                            .map(|_| session.id.clone())
                    }),
                    role: role.clone(),
                    status: ExecutionStatus::Running,
                    stop_reason: None,
                    stopped_by: None,
                    resume_policy: None,
                    stopped_at: None,
                    parent_execution_id: Some(supplied_parent_execution.id.clone()),
                    // The generic HarnessSession is the authority. The DB
                    // layer projects its external id to the legacy field.
                    agent_session_id: None,
                    agent_message_id: None,
                    last_activity_at: None,
                    summary: Some(prompt),
                    // Pin exact Artifact inputs before start_execution records
                    // its log path and launches the adapter.
                    logs_path: None,
                    before_sha: None,
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: current_workspace_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                false,
                artifact_input_ids,
            )
            .await?;

        tracing::info!(
            task_id = %task_id,
            role = %role,
            execution_id = %execution.id,
            parent_execution_id = %supplied_parent_execution.id,
            trigger = %trigger,
            "role follow-up dispatched"
        );

        service.publish(ForgeEvent {
            event_type: "follow_up.dispatched".to_owned(),
            entity_id: task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::FollowUpDispatched {
                task_id: task_id.clone(),
                parent_execution_id: supplied_parent_execution.id.clone(),
                execution_id: execution.id.clone(),
                trigger: trigger.clone(),
            },
        });

        service.start_execution(execution.id.clone()).await?;

        Ok(execution)
    })
}

async fn assigned_agent_for_follow_up(
    service: &TaskService,
    task_id: &str,
    role: &str,
) -> Result<Option<String>> {
    let assignment =
        TaskRoleAssignmentRepo::get_by_task_and_role(&*service.db, task_id, role).await?;
    Ok(assignment.and_then(|assignment| {
        (assignment.assignee_type == Some(AssigneeKind::Agent))
            .then_some(assignment.assignee_id)
            .flatten()
    }))
}

fn execution_role_matches(execution: &Execution, role: &str) -> bool {
    execution.role == role
        || (role == crate::workflow::default_roles::CODER && execution.role == "executor")
}
