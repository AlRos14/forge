use super::*;

impl TaskService {
    pub async fn dispatch_follow_up(
        &self,
        task_id: &str,
        review_outcome: ::review::ReviewOutcome,
        parent_execution_id: String,
    ) -> Result<Execution> {
        validate_required("task_id", task_id)?;
        validate_required("parent_execution_id", &parent_execution_id)?;

        let (trigger, prompt) = match &review_outcome {
            ::review::ReviewOutcome::Passed => {
                return Err(ServiceError::invalid_operation(
                    "cannot dispatch follow-up for a passed review",
                ));
            }
            ::review::ReviewOutcome::PassedCiOnly => {
                return Err(ServiceError::invalid_operation(
                    "cannot dispatch follow-up for a passed CI-only review",
                ));
            }
            ::review::ReviewOutcome::AuditorFailed { reason } => {
                let diff = self.best_effort_git_diff(task_id).await;
                (
                    "review_failed",
                    ::review::follow_up::render_review_fail_prompt(reason, &diff),
                )
            }
            ::review::ReviewOutcome::CiFailed { failing_steps } => (
                "ci_failed",
                ::review::follow_up::render_ci_fail_prompt(failing_steps),
            ),
            ::review::ReviewOutcome::MergeConflict {
                conflict_paths,
                conflict_summary,
            } => (
                "merge_failed",
                ::review::follow_up::render_merge_conflict_prompt(conflict_paths, conflict_summary),
            ),
        };
        dispatch_role_follow_up_impl(
            self.clone(),
            task_id.to_owned(),
            crate::workflow::default_roles::CODER.to_owned(),
            parent_execution_id,
            prompt,
            trigger.to_owned(),
            ExecutionPurpose::Implement,
            None,
        )
        .await
    }

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

    async fn active_state_for_role(&self, task: &Task, role: &str) -> Result<String> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = crate::workflow::engine::WorkflowEngine::resolve_workflow_for_task(
            task,
            &project.workflow_definition,
            &api_types::Actor::system(api_types::SystemComponent::Dispatch),
        );
        if workflow
            .states
            .iter()
            .any(|state| state.name == task.status && state.role.as_deref() == Some(role))
        {
            return Ok(task.status.clone());
        }
        Ok(workflow
            .states
            .iter()
            .find(|state| {
                state.kind == api_types::StateKind::Active && state.role.as_deref() == Some(role)
            })
            .or_else(|| {
                workflow
                    .states
                    .iter()
                    .find(|state| state.role.as_deref() == Some(role))
            })
            .map(|state| state.name.clone())
            .unwrap_or_else(|| crate::workflow::default_states::IN_PROGRESS.to_owned()))
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
        let mut lineage_parent = if execution_role_matches(&supplied_parent_execution, &role) {
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
            // Actor selection is authoritative and happens before continuity.
            // A usable lineage Agent is not automatically the current role's
            // selected Actor; a membership change must therefore be able to
            // force a fresh Agent-owned session.
            let selected = if task.repo_id.is_some() {
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
        // Actor selection is authoritative and precedes continuity. A
        // legacy agent_session_id without an explicit HarnessSession never
        // causes a new follow-up to inherit a thread.
        let same_actor = matches!(
            lineage_parent.actor_ref(),
            Some(db::ActorRef::Agent(ref parent_agent_id)) if parent_agent_id == &agent_id
        );
        if same_actor && lineage_parent.harness_session_id.is_none() {
            if let Some(external_session_id) = lineage_parent.agent_session_id.clone() {
                if let Some(reconciled) = materialize_historical_harness_session(
                    &service.db,
                    &lineage_parent,
                    &external_session_id,
                )
                .await?
                {
                    lineage_parent = reconciled;
                }
            }
        }
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
        let reusable_external_session = reusable_session
            .as_ref()
            .filter(|session| matches!(&session.status, db::HarnessSessionStatus::Active))
            .and_then(|session| session.external_session_id.clone());
        let executor_config_snapshot_json =
            if let Some(agent_session_id) = reusable_external_session.as_deref() {
                let snapshot_json = lineage_parent
                    .executor_config_snapshot_json
                    .as_deref()
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "parent execution {} missing executor config snapshot",
                            lineage_parent.id
                        ))
                    })?;
                Some(executor_snapshot_with_resume_thread(
                    snapshot_json,
                    agent_session_id,
                )?)
            } else {
                build_executor_config_snapshot(&service.db, &task, &agent, None).await?
            };
        let execution_id = new_uuid_v4();
        let logs_path = execution_logs_path(
            &service.workspace_root,
            &task.project_id,
            &task_id,
            &execution_id,
        );
        // Establish the final Task state/version before minting the
        // execution-scoped WorkspaceLease. A transition after issuance would
        // immediately make the exact-version authority stale.
        let active_state = service.active_state_for_role(&task, &role).await?;
        let task = if task.status != active_state {
            service
                .transition(task_id.clone(), active_state, task.version)
                .await?
                .task
        } else if task.error_annotation.is_some() {
            match TaskRepo::update(
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
                    plan: None,
                    blocked_json: None,
                    failed_json: None,
                    task_state_config: None,
                    parent_task_id: None,
                },
            )
            .await
            {
                Ok(updated) => updated,
                Err(error) => {
                    tracing::warn!(%error, task_id = %task_id, "failed to clear error annotation before follow-up dispatch");
                    TaskRepo::get_by_id(&*service.db, &task_id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?
                }
            }
        } else {
            task
        };
        service.ensure_task_runnable(&task).await?;
        let now = now_rfc3339();
        let execution = service
            .create_running_execution(
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
                    logs_path: Some(logs_path),
                    before_sha: None,
                    after_sha: None,
                    error: None,
                    executor_config_snapshot_json,
                    workspace_id: current_workspace_id.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                },
                false,
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
