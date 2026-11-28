use super::*;
use std::path::Path;

const EXECUTION_LOG_BATCH_MAX_ENTRIES: usize = 50;
const EXECUTION_LOG_BATCH_MAX_WAIT: Duration = Duration::from_millis(500);

/// Merge the Execution Git identity into `runtime_env`. Existing identity
/// keys are replaced so Agent-authored configuration cannot supply a human
/// identity.
fn merge_agent_git_identity(
    agent_config: &mut Value,
    identity: &executors::git_identity::AgentGitIdentity,
) -> Result<()> {
    let runtime_env = agent_config
        .as_object_mut()
        .map(|snapshot| snapshot.entry("runtime_env").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            ServiceError::invalid_operation("executor config snapshot is not injectable")
        })?;
    for (key, value) in identity.runtime_env() {
        runtime_env.insert(key, value);
    }
    Ok(())
}

fn provider_start_receipt_event(execution: &Execution, event_type: &str) -> db::CreateDomainEvent {
    let actor = execution.actor_ref();
    db::CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "execution".to_owned(),
        entity_id: execution.id.clone(),
        actor_type: actor
            .as_ref()
            .map(|actor| actor.kind().to_string())
            .unwrap_or_else(|| "system".to_owned()),
        actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
        scope_type: "task".to_owned(),
        scope_id: execution.task_id.clone(),
        correlation_id: execution.id.clone(),
        causation_id: execution.parent_execution_id.clone(),
        causation_depth: i64::from(execution.parent_execution_id.is_some()),
        dedupe_key: Some(format!("{event_type}:{}", execution.id)),
        payload_json: serde_json::json!({
            "execution_id": execution.id,
            "task_id": execution.task_id,
            "agent_id": execution.agent_id,
            "outcome": event_type.strip_prefix("execution.provider_start_").unwrap_or_default(),
        })
        .to_string(),
        created_at: now_rfc3339(),
    }
}

impl TaskService {
    pub(crate) async fn freeze_review_subject_and_inputs(
        &self,
        execution: Execution,
    ) -> Result<Execution> {
        if !crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            return Ok(execution);
        }
        let Some(workspace_id) = execution.workspace_id.as_deref() else {
            return Ok(execution);
        };
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        if workspace.task_id != execution.task_id || workspace.status != WorkspaceStatus::Ready {
            return Err(ServiceError::invalid_operation(
                "Review Execution requires its exact Ready same-Task Workspace",
            ));
        }
        let diff = crate::DiffService::new(Arc::clone(&self.db))
            .workspace_diff(&workspace.id)
            .await?;
        let snapshot_digest =
            crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
        if diff.head_sha.is_empty()
            || execution
                .after_sha
                .as_deref()
                .is_some_and(|sha| sha != diff.head_sha)
            || execution
                .before_sha
                .as_deref()
                .is_some_and(|sha| sha != diff.base_sha)
        {
            return Err(ServiceError::invalid_operation(
                "Review Execution commit identity changed before dispatch",
            ));
        }
        let timestamp = now_rfc3339();
        let actor = execution.actor_ref();
        let event = db::CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "execution.review_subject_frozen".to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: execution.id.clone(),
            actor_type: actor
                .as_ref()
                .map(|actor| actor.kind().to_string())
                .unwrap_or_else(|| "system".to_owned()),
            actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: execution.task_id.clone(),
            correlation_id: execution.id.clone(),
            causation_id: execution.parent_execution_id.clone(),
            causation_depth: i64::from(execution.parent_execution_id.is_some()),
            dedupe_key: Some(format!("review-subject-frozen:{}", execution.id)),
            payload_json: serde_json::json!({
                "execution_id": execution.id,
                "task_id": execution.task_id,
                "workspace_id": workspace.id,
                "base_commit_sha": diff.base_sha,
                "head_commit_sha": diff.head_sha,
                "workspace_snapshot_digest": snapshot_digest,
            })
            .to_string(),
            created_at: timestamp.clone(),
        };
        let write = ExecutionRepo::freeze_review_execution_subject(
            &*self.db,
            db::CreateReviewExecutionSubject {
                execution_id: execution.id.clone(),
                task_id: execution.task_id.clone(),
                workspace_id: workspace.id.clone(),
                base_commit_sha: diff.base_sha,
                head_commit_sha: diff.head_sha,
                workspace_snapshot_digest: snapshot_digest,
                created_at: timestamp.clone(),
            },
            &timestamp,
            event,
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            self.publish_committed_domain_event(event);
        }

        let runs = db::ValidationRunRepo::list_validation_runs_for_subject(
            &*self.db,
            &write.subject.task_id,
            &write.subject.workspace_id,
            &write.subject.head_commit_sha,
            &write.subject.workspace_snapshot_digest,
            None,
        )
        .await?;
        let mut evidence_ids = Vec::new();
        for run in runs {
            for evidence in
                db::ValidationRunRepo::list_evidence_for_validation_run(&*self.db, &run.id).await?
            {
                if evidence.task_id != execution.task_id
                    || evidence.producer_validation_run_id != run.id
                    || run.workspace_id != write.subject.workspace_id
                    || run.commit_sha != write.subject.head_commit_sha
                    || run.workspace_snapshot_digest != write.subject.workspace_snapshot_digest
                {
                    return Err(ServiceError::invalid_operation(
                        "Validation Evidence does not match its exact same-Task producer",
                    ));
                }
                evidence_ids.push(evidence.id);
            }
        }
        evidence_ids.sort();
        evidence_ids.dedup();
        if !evidence_ids.is_empty() {
            db::ValidationRunRepo::pin_execution_evidence_inputs(
                &*self.db,
                &execution.id,
                &evidence_ids,
                &now_rfc3339(),
            )
            .await?;
        }
        Ok(write.execution)
    }

    pub async fn start_execution(
        &self,
        execution_id: impl Into<String>,
    ) -> Result<api_types::ExecutionStartResult> {
        let execution_id = execution_id.into();
        validate_required("execution_id", &execution_id)?;
        let execution = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if execution.status != ExecutionStatus::Running {
            return Err(ServiceError::invalid_operation(
                "only running executions can be started",
            ));
        }
        let task_is_cancelled =
            db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &execution.task_id)
                .await?
                .is_some_and(|lifecycle| lifecycle.state == db::TaskLifecycleState::Cancelled);
        if task_is_cancelled {
            self.cancel_active_execution(
                &execution,
                "cancelled by task cancellation",
                db::StopReason::TaskCancelled,
                &api_types::Actor::system(api_types::SystemComponent::CancelTask),
                db::ResumePolicy::None,
            )
            .await?;
            return Ok(api_types::ExecutionStartResult {
                execution_id: execution.id,
                accepted: false,
            });
        }
        if crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
            && self
                .reconcile_existing_review_report(&execution, None, None)
                .await?
                .is_some()
        {
            return Ok(api_types::ExecutionStartResult {
                execution_id: execution.id,
                accepted: false,
            });
        }
        // Remote and local adapters both require the scheduler-issued lease;
        // checking at this boundary closes the gap between execution-row
        // creation and adapter launch/recovery.
        if let Err(error) = self.verify_execution_workspace_authority(&execution).await {
            // Authority can be revoked or superseded between execution-row
            // creation and this dispatch attempt.  Stop the attempt and
            // revoke any remaining grant before surfacing the denial.
            let failure_message = error.to_string();
            if let Err(mark_error) = self
                .fail_execution_before_dispatch(&execution.id, failure_message)
                .await
            {
                tracing::warn!(
                    execution_id = %execution.id,
                    %mark_error,
                    "failed to terminalize execution after initial WorkspaceLease verification failure"
                );
            }
            return Err(error);
        }
        if let Some(agent_id) = execution.agent_id.as_deref() {
            let agent = AgentRepo::get_by_id(&*self.db, agent_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?;
            if agent.paused {
                return Err(ServiceError::AgentPaused {
                    agent_id: agent_id.to_owned(),
                });
            }
        }
        let prepared = async {
            if crate::role_capabilities::execution_has_capability(
                &self.db,
                &execution.id,
                api_types::TaskRoleCapability::FormalReview,
            )
            .await?
            {
                self.verify_execution_workspace_authority(&execution)
                    .await?;
            }
            let execution = self.freeze_review_subject_and_inputs(execution).await?;
            crate::project_plan::ProjectPlanService::new(
                Arc::clone(&self.db),
                Arc::clone(&self.event_bus),
            )
            .verify_execution_handoff_workspace_base(&execution)
            .await?;
            let provider = self.execution_provider_for_execution(&execution).await?;
            let params = self.execution_start_params(&execution).await?;
            if snapshot_credential_ref(&params.executor_config)?.is_some() {
                ensure_snapshot_credential_transport(provider.as_ref(), &params.executor_config)?;
                if self.credential_env.is_none() {
                    return Err(ServiceError::invalid_operation(
                        "snapshot credential cannot be resolved by this execution host",
                    ));
                }
            }
            Ok::<_, ServiceError>((execution, provider, params))
        }
        .await;

        let (execution, provider, params) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure_message = error.to_string();
                if let Err(mark_error) = self
                    .fail_execution_before_dispatch(&execution_id, failure_message)
                    .await
                {
                    tracing::warn!(
                        %execution_id,
                        %mark_error,
                        "failed to mark execution failed after dispatch start error"
                    );
                }
                return Err(error);
            }
        };

        #[cfg(test)]
        if let Some(barrier) = self.provider_start_claim_barrier.as_ref() {
            barrier.wait().await;
            barrier.wait().await;
        }

        let claim = ExecutionRepo::claim_provider_start(
            &*self.db,
            &execution.id,
            provider_start_receipt_event(&execution, "execution.provider_start_attempted"),
        )
        .await;
        let claim = match claim {
            Ok(claim) => claim,
            Err(
                error @ (db::DbError::ProjectPlanBatchInactive
                | db::DbError::ProjectDependencyUnsatisfied
                | db::DbError::ProjectPlanApplyIncomplete),
            ) => {
                self.fail_execution_before_dispatch(
                    &execution.id,
                    "Project plan batch authority or exact prerequisite evidence became invalid before provider Start".to_owned(),
                )
                .await?;
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        match claim {
            db::ProviderStartClaim::Accepted => {
                return Ok(api_types::ExecutionStartResult {
                    execution_id: execution.id,
                    accepted: true,
                });
            }
            db::ProviderStartClaim::Rejected => {
                self.fail_execution_before_dispatch(
                    &execution.id,
                    "provider durably rejected the execution Start".to_owned(),
                )
                .await?;
                return Ok(api_types::ExecutionStartResult {
                    execution_id: execution.id,
                    accepted: false,
                });
            }
            db::ProviderStartClaim::Uncertain => {
                return Err(ServiceError::ExecutionStartUncertain {
                    execution_id: execution.id,
                });
            }
            db::ProviderStartClaim::AuthorityLost => {
                let error = ServiceError::invalid_operation(
                    "TaskRole membership, WorkUnit assignment, or WorkspaceLease authority changed before provider Start was claimed",
                );
                self.fail_execution_before_dispatch(&execution.id, error.to_string())
                    .await?;
                return Err(error);
            }
            db::ProviderStartClaim::TaskCancelled => {
                self.cancel_active_execution(
                    &execution,
                    "cancelled by task cancellation",
                    db::StopReason::TaskCancelled,
                    &api_types::Actor::system(api_types::SystemComponent::CancelTask),
                    db::ResumePolicy::None,
                )
                .await?;
                return Ok(api_types::ExecutionStartResult {
                    execution_id: execution.id,
                    accepted: false,
                });
            }
            db::ProviderStartClaim::TaskPaused => {
                return Ok(api_types::ExecutionStartResult {
                    execution_id: execution.id,
                    accepted: false,
                });
            }
            db::ProviderStartClaim::ProjectPlanBatchPaused => {
                return Ok(api_types::ExecutionStartResult {
                    execution_id: execution.id,
                    accepted: false,
                });
            }
            db::ProviderStartClaim::Claimed => {}
        }

        let result = provider.start(params).await;
        let outcome_event_type = match &result {
            Ok(result) if result.accepted => "execution.provider_start_accepted",
            Ok(_) => "execution.provider_start_rejected",
            Err(ServiceError::DaemonStartRejected { .. }) => "execution.provider_start_rejected",
            Err(_) => "execution.provider_start_uncertain",
        };
        let outcome_recorded = match ExecutionRepo::record_provider_start_outcome(
            &*self.db,
            &execution.id,
            provider_start_receipt_event(&execution, outcome_event_type),
        )
        .await
        {
            Ok(_) => true,
            Err(record_error) => {
                tracing::warn!(
                    execution_id = %execution.id,
                    %record_error,
                    "failed to persist provider Start outcome; the durable attempt receipt prevents replay"
                );
                false
            }
        };

        match result {
            Ok(result) if result.accepted => Ok(result),
            Ok(result) if outcome_recorded => {
                self.fail_execution_before_dispatch(
                    &execution.id,
                    "provider durably rejected the execution Start".to_owned(),
                )
                .await?;
                Ok(result)
            }
            Ok(_) => Err(ServiceError::ExecutionStartUncertain {
                execution_id: execution.id,
            }),
            Err(ServiceError::DaemonStartRejected { message }) if outcome_recorded => {
                self.fail_execution_before_dispatch(&execution.id, message.clone())
                    .await?;
                Err(ServiceError::invalid_operation(message))
            }
            Err(error) => {
                tracing::warn!(
                    %execution_id,
                    %error,
                    "provider Start outcome is uncertain; retain the Running Execution and never replay the external request"
                );
                Err(ServiceError::ExecutionStartUncertain { execution_id })
            }
        }
    }

    /// Attach the Execution's Git attribution to the in-memory invocation
    /// snapshot. It rides in `runtime_env`, which overrides Agent-configured
    /// `env` and is never persisted or published.
    async fn inject_agent_git_identity(
        &self,
        execution: &db::Execution,
        agent_config: &mut Value,
    ) -> Result<()> {
        let Some(agent_id) = execution.agent_id.as_deref() else {
            return Ok(());
        };
        let agent = AgentRepo::get_by_id(&*self.db, agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent_identity", agent_id.to_owned()))?;
        let identity = executors::git_identity::AgentGitIdentity::new(
            &execution.role,
            &agent.name,
            &agent.id,
            &execution.id,
        );
        merge_agent_git_identity(agent_config, &identity)
    }

    pub async fn run_execution(
        &self,
        execution_id: impl Into<String>,
        executor: &dyn TaskExecutor,
    ) -> Result<db::Execution> {
        let execution_id = execution_id.into();
        validate_required("execution_id", &execution_id)?;
        tracing::info!(%execution_id, "execution dispatch starting");
        let mut execution = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if execution.status != ExecutionStatus::Running {
            return Err(ServiceError::invalid_operation(
                "only running executions can be executed",
            ));
        }
        if crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            if let Some((completed, _report)) = self
                .reconcile_existing_review_report(&execution, None, None)
                .await?
            {
                return Ok(completed);
            }
        }
        if let Some(failed) = self
            .wait_for_agent_active_before_dispatch(&execution)
            .await?
        {
            return Ok(failed);
        }
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let working_context = self.resolve_execution_working_context(&execution).await?;
        let worktree_path = working_context.path.to_string_lossy().into_owned();
        let execution_lock_key = working_context.lock_key.as_str();
        let snapshot = execution
            .executor_config_snapshot_json
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("execution missing executor config snapshot")
            })?;
        let mut agent_config = parse_json_value("executor config snapshot", snapshot)?;
        crate::task_service::config::validate_agent_routing_snapshot(&agent_config)?;
        // The immutable Execution snapshot selects the provider entry. Never
        // let the Agent's currently selected profile redirect this invocation.
        if snapshot_credential_ref(&agent_config)?.is_some() {
            self.credential_env
                .as_ref()
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "snapshot credential cannot be resolved by this execution host",
                    )
                })?
                .inject_snapshot_credential_env(&mut agent_config)
                .await?;
        }
        if !working_context.workspaces.is_empty() {
            self.inject_agent_git_identity(&execution, &mut agent_config)
                .await?;
        }
        let max_turns = self.resolve_max_turns(&task).await?;
        let logs_path = self
            .resolve_execution_logs_path(&execution, &task, &execution_id)
            .await?;
        if let Some(parent) = std::path::Path::new(&logs_path).parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ServiceError::invalid_operation(format!("failed to create log directory: {error}"))
            })?;
        }

        if let Some(terminal_activity) = self.terminal_activity.as_ref() {
            for workspace in &working_context.workspaces {
                if terminal_activity
                    .workspace_has_active_terminal(&workspace.id)
                    .await
                {
                    return Err(ServiceError::TerminalActiveExecution {
                        workspace_id: workspace.id.clone(),
                    });
                }
            }
        }
        let _exec_lock_guard = if let Some(locks) = self.workspace_exec_locks.as_ref() {
            if let Some(guard) = locks.try_acquire(execution_lock_key) {
                Some(guard)
            } else {
                for workspace in &working_context.workspaces {
                    self.event_bus.publish(events::ForgeEvent {
                        event_type: "workspace.execution_waiting".to_owned(),
                        entity_id: workspace.id.clone(),
                        timestamp: events::event_timestamp(),
                        context: events::EventContext::WorkspaceExecutionWaiting {
                            workspace_id: workspace.id.clone(),
                            task_id: task.id.clone(),
                        },
                    });
                }
                Some(locks.acquire(execution_lock_key).await)
            }
        } else {
            None
        };
        let mut workspace_ids = working_context
            .workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>();
        workspace_ids.sort_unstable();
        workspace_ids.dedup();
        let mut _workspace_guards = Vec::with_capacity(workspace_ids.len());
        if let Some(locks) = self.workspace_exec_locks.as_ref() {
            for workspace_id in workspace_ids {
                _workspace_guards.push(locks.acquire(workspace_id).await);
            }
        }
        if let Some(terminal_activity) = self.terminal_activity.as_ref() {
            for workspace in &working_context.workspaces {
                if terminal_activity
                    .workspace_has_active_terminal(&workspace.id)
                    .await
                {
                    return Err(ServiceError::TerminalActiveExecution {
                        workspace_id: workspace.id.clone(),
                    });
                }
            }
        }

        let execution_before_launch = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if execution_before_launch.status != ExecutionStatus::Running {
            tracing::info!(
                %execution_id,
                status = %execution_before_launch.status,
                "execution dispatch stopped before adapter launch"
            );
            return Ok(execution_before_launch);
        }

        // Workspace lock acquisition and pre-launch preparation can outlive
        // a lease or a baseline supersession.  Re-read the execution/task
        // bindings and acknowledge the lease immediately before handing
        // control to an executor.
        if let Err(error) = self
            .verify_execution_workspace_authority(&execution_before_launch)
            .await
        {
            let failure_message = error.to_string();
            if let Err(mark_error) = self
                .fail_execution_before_dispatch(&execution_before_launch.id, failure_message)
                .await
            {
                tracing::warn!(
                    execution_id = %execution_before_launch.id,
                    %mark_error,
                    "failed to terminalize execution after final WorkspaceLease verification failure"
                );
            }
            return Err(error);
        }

        let is_review_execution = crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution_before_launch.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        if is_review_execution && working_context.workspace_group.is_some() {
            if let Err(error) = self
                .verify_read_only_group_members(&execution_before_launch, &working_context)
                .await
            {
                return self
                    .fail_execution_before_dispatch(
                        &execution_before_launch.id,
                        format!("Review WorkspaceGroup failed exact subject verification: {error}"),
                    )
                    .await;
            }
        }
        let mut review_worktree_snapshot = None;
        let mut review_snapshot_digest = None;
        if is_review_execution && execution_before_launch.workspace_id.is_some() {
            let worktree_path = std::path::Path::new(&worktree_path);
            let digest_before =
                match crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
                    .await
                {
                    Ok(digest) => digest,
                    Err(error) => {
                        return self
                            .fail_execution_before_dispatch(
                                &execution_before_launch.id,
                                format!("could not verify the pre-review workspace state: {error}"),
                            )
                            .await;
                    }
                };
            let snapshot = match git::capture_worktree_state(worktree_path).await {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return self
                        .fail_execution_before_dispatch(
                            &execution_before_launch.id,
                            format!(
                                "could not capture the exact pre-review workspace state: {error}"
                            ),
                        )
                        .await;
                }
            };
            let digest_after =
                match crate::ValidationService::snapshot_digest(&worktree_path.to_string_lossy())
                    .await
                {
                    Ok(digest) => digest,
                    Err(error) => {
                        return self
                            .fail_execution_before_dispatch(
                                &execution_before_launch.id,
                                format!(
                                "could not verify the captured pre-review workspace state: {error}"
                            ),
                            )
                            .await;
                    }
                };
            let current_head = match git::get_current_sha(worktree_path).await {
                Ok(head) => head,
                Err(error) => {
                    return self
                        .fail_execution_before_dispatch(
                            &execution_before_launch.id,
                            format!("could not verify the captured pre-review HEAD: {error}"),
                        )
                        .await;
                }
            };
            if current_head != snapshot.head_sha() || digest_before != digest_after {
                return self
                    .fail_execution_before_dispatch(
                        &execution_before_launch.id,
                        "workspace state changed while the pre-review snapshot was captured"
                            .to_owned(),
                    )
                    .await;
            }
            review_snapshot_digest = Some(digest_after);
            review_worktree_snapshot = Some(snapshot);
        }

        execution = self
            .freeze_review_subject_and_inputs(execution_before_launch.clone())
            .await?;
        let review_subject = if let Some(snapshot) = review_worktree_snapshot.as_ref() {
            let workspace = working_context.workspace.as_ref().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Review Execution has no persisted Workspace for its frozen subject",
                )
            })?;
            let subject = ExecutionRepo::get_review_execution_subject(&*self.db, &execution.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "local Review Execution did not persist its exact workspace subject",
                    )
                })?;
            let live_digest = crate::ValidationService::snapshot_digest(&worktree_path).await?;
            let live_head = git::get_current_sha(std::path::Path::new(&worktree_path)).await?;
            if subject.workspace_id != workspace.id
                || subject.head_commit_sha != snapshot.head_sha()
                || Some(subject.workspace_snapshot_digest.as_str())
                    != review_snapshot_digest.as_deref()
                || live_digest != subject.workspace_snapshot_digest
                || live_head != subject.head_commit_sha
            {
                return self
                    .fail_execution_before_dispatch(
                        &execution.id,
                        "Review subject changed while its exact pre-review workspace state was frozen"
                            .to_owned(),
                    )
                    .await;
            }
            Some(subject)
        } else {
            None
        };
        let launch_activity_at = now_rfc3339();
        ExecutionRepo::update(
            &*self.db,
            db::UpdateExecution {
                id: execution_id.clone(),
                status: None,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: Some(Some(launch_activity_at)),
                summary: None,
                logs_path: Some(Some(logs_path.clone())),
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(ServiceError::from)?;

        let description = execution_description(&execution, &task);

        let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel::<executors::LogEntry>();
        let max_turns_exceeded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let assistant_turn_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

        // Spawn a task that forwards log entries to the event bus
        let event_bus = self.event_bus.clone();
        let activity_db = Arc::clone(&self.db);
        let cancellation_executor = self.task_executor.clone();
        let sse_execution_id = execution_id.clone();
        let sse_task_id = task.id.clone();
        let live_usage_snapshot = execution.executor_config_snapshot_json.clone();
        let log_max_turns = max_turns;
        let log_max_turns_exceeded = Arc::clone(&max_turns_exceeded);
        let log_assistant_turn_count = Arc::clone(&assistant_turn_count);
        tokio::spawn(async move {
            let mut last_db_update: Option<std::time::Instant> = None;
            let mut last_usage_persist: Option<std::time::Instant> = None;
            let mut assistant_turn_count = 0_u32;
            let mut pending_batch: Vec<executors::LogEntry> = Vec::new();
            let mut flush_deadline: Option<tokio::time::Instant> = None;

            let flush_batch = |batch: &mut Vec<executors::LogEntry>| {
                if batch.is_empty() {
                    return;
                }
                let logs = batch
                    .iter()
                    .map(|entry| serde_json::to_value(entry).unwrap_or_default())
                    .collect::<Vec<_>>();
                let first_log = logs.first().cloned().unwrap_or_default();
                let timestamp = batch
                    .last()
                    .map(|entry| entry.timestamp.clone())
                    .unwrap_or_else(events::event_timestamp);
                event_bus.publish(events::ForgeEvent {
                    event_type: "execution.log".to_owned(),
                    entity_id: sse_execution_id.clone(),
                    timestamp,
                    context: events::EventContext::ExecutionLog {
                        task_id: sse_task_id.clone(),
                        log: first_log,
                        logs: Some(logs),
                    },
                });
                batch.clear();
            };

            loop {
                let next_entry = if let Some(deadline) = flush_deadline {
                    tokio::select! {
                        biased;
                        maybe_entry = log_rx.recv() => maybe_entry,
                        _ = tokio::time::sleep_until(deadline) => {
                            flush_batch(&mut pending_batch);
                            flush_deadline = None;
                            continue;
                        }
                    }
                } else {
                    log_rx.recv().await
                };

                let Some(entry) = next_entry else {
                    flush_batch(&mut pending_batch);
                    break;
                };

                if last_db_update
                    .map(|instant| instant.elapsed() >= Duration::from_secs(30))
                    .unwrap_or(true)
                {
                    if let Err(error) = ExecutionRepo::update_last_activity_at(
                        activity_db.as_ref(),
                        &sse_execution_id,
                        &entry.timestamp,
                    )
                    .await
                    {
                        tracing::warn!(
                            execution_id = %sse_execution_id,
                            %error,
                            "failed to update execution activity timestamp"
                        );
                    }
                    last_db_update = Some(std::time::Instant::now());
                }
                if let Some(account_usage) = super::account_usage_from_log_entry(&entry) {
                    let should_persist = last_usage_persist
                        .map(|instant| instant.elapsed() >= Duration::from_secs(5))
                        .unwrap_or(true);
                    if should_persist {
                        if let Err(error) = super::persist_account_usage_snapshot(
                            activity_db.as_ref(),
                            live_usage_snapshot.as_deref(),
                            &sse_execution_id,
                            &account_usage,
                        )
                        .await
                        {
                            tracing::warn!(
                                execution_id = %sse_execution_id,
                                %error,
                                "failed to persist live account usage snapshot"
                            );
                        }
                        last_usage_persist = Some(std::time::Instant::now());
                    }
                }
                if entry.kind == executors::LogKind::Assistant {
                    assistant_turn_count = assistant_turn_count.saturating_add(1);
                    log_assistant_turn_count
                        .store(assistant_turn_count, std::sync::atomic::Ordering::SeqCst);
                    if let Some(limit) = log_max_turns {
                        if assistant_turn_count >= limit
                            && !log_max_turns_exceeded
                                .swap(true, std::sync::atomic::Ordering::SeqCst)
                        {
                            tracing::warn!(
                                execution_id = %sse_execution_id,
                                assistant_turn_count,
                                max_turns = limit,
                                "execution exceeded max turns"
                            );
                            if let Some(executor) = cancellation_executor.as_ref() {
                                if let Err(error) = executor.cancel(&sse_execution_id).await {
                                    tracing::warn!(
                                        execution_id = %sse_execution_id,
                                        %error,
                                        "failed to cancel execution after max turns"
                                    );
                                }
                            }
                        }
                    }
                }

                pending_batch.push(entry);
                if flush_deadline.is_none() {
                    flush_deadline =
                        Some(tokio::time::Instant::now() + EXECUTION_LOG_BATCH_MAX_WAIT);
                }
                if pending_batch.len() >= EXECUTION_LOG_BATCH_MAX_ENTRIES {
                    flush_batch(&mut pending_batch);
                    flush_deadline = None;
                }
            }
        });

        let existing_plan_output = if execution.role == "planner" {
            db::CollaborationRepo::get_execution_artifact_output(
                &*self.db,
                &execution.id,
                db::ArtifactKind::Plan,
            )
            .await?
        } else {
            None
        };
        let invocation = if existing_plan_output.is_some() {
            api_types::HarnessInvocation::Start
        } else {
            super::harness_invocation_for_execution(
                &self.db,
                &execution,
                execution.workspace_id.as_deref(),
            )
            .await?
        };
        let prelaunch_review_error = if let Some(subject) = review_subject.as_ref() {
            let current_head = git::get_current_sha(std::path::Path::new(&worktree_path)).await;
            let current_digest = crate::ValidationService::snapshot_digest(&worktree_path).await;
            match (current_head, current_digest) {
                (Ok(head), Ok(digest))
                    if head == subject.head_commit_sha
                        && digest == subject.workspace_snapshot_digest =>
                {
                    None
                }
                (Ok(_), Ok(_)) => Some(
                    "Review workspace changed after its subject was frozen and before launch"
                        .to_owned(),
                ),
                (Err(error), _) => Some(format!(
                    "could not read the frozen Review HEAD before launch: {error}"
                )),
                (_, Err(error)) => Some(format!(
                    "could not read the frozen Review snapshot before launch: {error}"
                )),
            }
        } else {
            None
        };
        if let Some(error) = prelaunch_review_error {
            return self
                .fail_execution_before_dispatch(&execution.id, error)
                .await;
        }

        let execution_result = if let Some(artifact) = existing_plan_output.as_ref() {
            let content = artifact.content.clone().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "persisted Plan Artifact output has no inline content",
                )
            })?;
            Ok(executors::ExecutionResult {
                status: ExecutionOutcome::Completed,
                assistant_output: Some(content.clone()),
                summary: Some(content),
                ..Default::default()
            })
        } else {
            executor
                .execute(ExecutionContext {
                    invocation,
                    task_id: task.id.clone(),
                    execution_id: execution_id.clone(),
                    role: execution.role.clone(),
                    task_role_capabilities: crate::role_capabilities::execution_capabilities(
                        &self.db,
                        &execution.id,
                    )
                    .await?,
                    has_repository_workspace: working_context.has_repository_workspace,
                    review_read_paths: self.review_read_paths_for_execution(&execution).await?,
                    worktree_path: worktree_path.clone(),
                    description,
                    agent_config,
                    logs_path: logs_path.clone(),
                    heartbeat_interval_seconds: 30,
                    max_turns,
                    log_sender: Some(log_tx),
                })
                .await
        };
        if let Err(error) = executors::LogWriter::compact(std::path::Path::new(&logs_path)).await {
            tracing::warn!(%execution_id, %error, "failed to compress final execution log segment; plain log retained");
        }
        let mut review_restore_error = None;
        if let (Some(snapshot), Some(subject)) =
            (review_worktree_snapshot.as_ref(), review_subject.as_ref())
        {
            let restore = snapshot.restore(std::path::Path::new(&worktree_path)).await;
            match restore {
                Ok(()) => {
                    let restored_head =
                        git::get_current_sha(std::path::Path::new(&worktree_path)).await;
                    let restored_digest =
                        crate::ValidationService::snapshot_digest(&worktree_path).await;
                    match (restored_head, restored_digest) {
                        (Ok(head), Ok(digest))
                            if head == subject.head_commit_sha
                                && digest == subject.workspace_snapshot_digest => {}
                        (Ok(head), Ok(digest)) => {
                            review_restore_error = Some(format!(
                                "restored workspace does not match the frozen Review subject (HEAD {head}, snapshot {digest})"
                            ));
                        }
                        (Err(error), _) => {
                            review_restore_error =
                                Some(format!("could not read restored Review HEAD: {error}"));
                        }
                        (_, Err(error)) => {
                            review_restore_error =
                                Some(format!("could not read restored Review snapshot: {error}"));
                        }
                    }
                }
                Err(error) => {
                    review_restore_error = Some(format!(
                        "could not restore the exact pre-review workspace state: {error}"
                    ));
                }
            }
        }
        let mut result = if let Some(error) = review_restore_error {
            let snapshot = review_worktree_snapshot
                .take()
                .expect("Review restore failure retains its isolated snapshot");
            let backup_path = snapshot.preserve_for_diagnostics();
            tracing::error!(
                execution_id = %execution_id,
                workspace_path = %worktree_path,
                snapshot_backup_path = %backup_path.display(),
                %error,
                "Review workspace could not be restored to its exact subject"
            );
            executors::ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: review_subject
                    .as_ref()
                    .map(|subject| subject.head_commit_sha.clone()),
                error: Some(format!(
                    "{error}; pre-review state backup retained at {}",
                    backup_path.display()
                )),
                ..Default::default()
            }
        } else {
            let mut result = execution_result?;
            if let Some(subject) = review_subject.as_ref() {
                result.after_sha = Some(subject.head_commit_sha.clone());
            }
            result
        };
        if working_context.workspace_group.is_some() {
            if let Err(error) = self
                .verify_read_only_group_members(&execution_before_launch, &working_context)
                .await
            {
                result.status = ExecutionOutcome::Failed;
                result.error = Some(format!(
                    "WorkspaceGroup context or Review repository changed: {error}"
                ));
            }
        }
        if result.status == ExecutionOutcome::Completed
            && result.after_sha.is_none()
            && execution.workspace_id.is_some()
            && working_context.workspace_group.is_none()
        {
            let workspace_path = std::path::Path::new(&worktree_path);
            match git::is_worktree_clean(workspace_path).await {
                Ok(true) => match git::get_current_sha(workspace_path).await {
                    Ok(head) => result.after_sha = Some(head),
                    Err(error) => {
                        result.status = ExecutionOutcome::Failed;
                        result.error = Some(format!(
                            "completed repository Execution has no exact result SHA: {error}"
                        ));
                    }
                },
                Ok(false) => {
                    result.status = ExecutionOutcome::Failed;
                    result.error = Some(
                        "repository Execution completed with uncommitted changes and no exact result SHA"
                            .to_owned(),
                    );
                }
                Err(error) => {
                    result.status = ExecutionOutcome::Failed;
                    result.error = Some(format!(
                        "completed repository Execution has no exact result SHA: {error}"
                    ));
                }
            }
        }
        let max_turns_exceeded = max_turns_exceeded.load(std::sync::atomic::Ordering::SeqCst);
        let assistant_turn_count = assistant_turn_count.load(std::sync::atomic::Ordering::SeqCst);
        if max_turns_exceeded {
            result.status = ExecutionOutcome::Failed;
            result.error = Some(match max_turns {
                Some(limit) => format!("max turns exceeded ({assistant_turn_count}/{limit})"),
                None => "max turns exceeded".to_owned(),
            });
        }

        let current_execution = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if current_execution.status != ExecutionStatus::Running {
            tracing::info!(
                %execution_id,
                status = %current_execution.status,
                "execution dispatch already stopped externally"
            );
            return Ok(current_execution);
        }

        if is_review_execution && result.status == ExecutionOutcome::Completed {
            if let Some(session_id) = result.agent_session_id.as_deref() {
                if let Err(error) = ExecutionRepo::record_harness_session_result(
                    &*self.db,
                    &current_execution.id,
                    session_id,
                    &now_rfc3339(),
                )
                .await
                {
                    result.status = ExecutionOutcome::Failed;
                    result.error = Some(format!(
                        "Review native session isolation could not be established: {error}"
                    ));
                }
            }
        }
        if current_execution.role == "planner"
            && result.status == ExecutionOutcome::Completed
            && existing_plan_output.is_none()
        {
            let output = result
                .assistant_output
                .as_deref()
                .filter(|content| !content.trim().is_empty());
            match output {
                Some(output) => {
                    if let Err(error) = crate::CollaborationService::new(
                        Arc::clone(&self.db),
                        Arc::clone(&self.event_bus),
                    )
                    .create_plan_artifact_from_execution(&current_execution.id, output)
                    .await
                    {
                        result.status = ExecutionOutcome::Failed;
                        result.error = Some(format!(
                            "completed Plan Execution result could not be materialized: {error}"
                        ));
                    }
                }
                None => {
                    result.status = ExecutionOutcome::Failed;
                    result.error = Some(
                        "completed Plan Execution did not return a complete assistant result"
                            .to_owned(),
                    );
                }
            }
        }

        if current_execution.work_unit_id.is_some()
            && current_execution.role != "planner"
            && !is_review_execution
            && result.status == ExecutionOutcome::Completed
        {
            if let Some(output) = result
                .assistant_output
                .as_deref()
                .filter(|output| !output.trim().is_empty())
            {
                if let Err(error) =
                    crate::CollaborationService::new(self.db.clone(), self.event_bus.clone())
                        .create_delivery_artifact_from_execution(&current_execution.id, output)
                        .await
                {
                    result.status = ExecutionOutcome::Failed;
                    result.error =
                        Some(format!("WorkUnit delivery could not be recorded: {error}"));
                }
            }
        }
        if crate::role_capabilities::execution_has_capability(
            &self.db,
            &current_execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
            && result.status == ExecutionOutcome::Completed
        {
            let output = result
                .assistant_output
                .as_deref()
                .filter(|content| !content.trim().is_empty());
            match output {
                Some(output) => {
                    let collaboration = crate::CollaborationService::new(
                        Arc::clone(&self.db),
                        Arc::clone(&self.event_bus),
                    )
                    .with_review_workspace_root(self.workspace_root.clone());
                    let materialized = match collaboration
                        .ensure_review_subject_current(&current_execution)
                        .await
                    {
                        Ok(()) => collaboration
                            .create_review_report_from_execution(&current_execution.id, output)
                            .await
                            .map(|_| ()),
                        Err(error) => Err(error),
                    };
                    if let Err(error) = materialized {
                        result.status = ExecutionOutcome::Failed;
                        result.error = Some(format!(
                            "completed Review Execution result could not be materialized: {error}"
                        ));
                    }
                }
                None => {
                    result.status = ExecutionOutcome::Failed;
                    result.error = Some(
                        "completed Review Execution did not return a complete structured result"
                            .to_owned(),
                    );
                }
            }
        }

        if result.status == ExecutionOutcome::Completed && current_execution.work_unit_id.is_some()
        {
            match self
                .record_execution_repository_results(&current_execution.id)
                .await
            {
                Ok(repository_results) if !repository_results.is_empty() => {
                    result.after_sha =
                        current_execution
                            .workspace_id
                            .as_deref()
                            .and_then(|workspace_id| {
                                repository_results
                                    .iter()
                                    .find(|repository| repository.workspace_id == workspace_id)
                                    .and_then(|repository| repository.after_sha.clone())
                            });
                }
                Ok(_) => {}
                Err(error) => {
                    result.status = ExecutionOutcome::Failed;
                    result.after_sha = None;
                    result.error = Some(format!(
                        "repository result capture failed before terminalization: {error}"
                    ));
                }
            }
        }

        let executor_unavailable =
            result.failure_class == Some(executors::ExecutionFailureClass::ExecutorUnavailable);
        let unavailable_retry_at = result.retry_after.map(|retry_after| {
            let delay = chrono::Duration::from_std(retry_after)
                .unwrap_or_else(|_| chrono::Duration::minutes(15));
            (chrono::Utc::now() + delay).to_rfc3339()
        });
        let route_outcome = crate::task_service::config::RouteOutcome {
            selected: result.resolved_candidate.as_ref().map(|candidate| {
                (
                    candidate.candidate_key.clone(),
                    candidate.executor_type.to_string(),
                    candidate.config.clone(),
                    serde_json::to_value(candidate.harness_capabilities.snapshot())
                        .unwrap_or(Value::Null),
                    Some(serde_json::to_value(&candidate.effective_policy).unwrap_or(Value::Null)),
                )
            }),
            attempts: result
                .route_attempts
                .iter()
                .map(|attempt| {
                    (
                        attempt.candidate_key.clone(),
                        attempt.outcome.as_str().to_owned(),
                    )
                })
                .collect(),
            unavailable_retry_at: executor_unavailable.then(|| unavailable_retry_at.clone()),
        };
        let snapshot_update = match current_execution.executor_config_snapshot_json.as_deref() {
            Some(snapshot) => crate::task_service::config::apply_route_outcome_to_snapshot(
                snapshot,
                &route_outcome,
            )?,
            None => None,
        };
        let winner_snapshot = snapshot_update
            .as_deref()
            .or(current_execution.executor_config_snapshot_json.as_deref())
            .map(ToOwned::to_owned);
        let winner_snapshot_value = winner_snapshot
            .as_deref()
            .and_then(|snapshot| serde_json::from_str::<Value>(snapshot).ok());
        let usage_provider = winner_snapshot_value
            .as_ref()
            .map(super::usage_provider_from_agent_config)
            .unwrap_or_else(|| "unknown".to_owned());
        let usage_model_fallback = winner_snapshot_value
            .as_ref()
            .and_then(usage_model_fallback);

        let now = now_rfc3339();
        let (status, stop_reason, stopped_by, resume_policy, stopped_at) = match result.status {
            ExecutionOutcome::Completed => (ExecutionStatus::Completed, None, None, None, None),
            ExecutionOutcome::Failed => (
                ExecutionStatus::Failed,
                Some(Some(db::StopReason::ExecutorFailed)),
                Some(Some(
                    api_types::Actor::system(api_types::SystemComponent::Executor).display(),
                )),
                Some(Some(db::ResumePolicy::Manual)),
                Some(Some(now.clone())),
            ),
            ExecutionOutcome::Cancelled => (
                ExecutionStatus::Cancelled,
                Some(Some(db::StopReason::ExecutorCancelled)),
                Some(Some(
                    api_types::Actor::system(api_types::SystemComponent::Executor).display(),
                )),
                Some(Some(db::ResumePolicy::Manual)),
                Some(Some(now.clone())),
            ),
        };
        tracing::info!(
            %execution_id,
            task_id = %task.id,
            status = %status,
            logs_path = %logs_path,
            "execution dispatch completed"
        );

        let lifecycle_event =
            super::super::execution_status_domain_event(&current_execution, &status, &now);
        let (updated, committed_event) = ExecutionRepo::update_with_event(
            &*self.db,
            db::UpdateExecution {
                id: execution_id,
                status: Some(status),
                stop_reason,
                stopped_by,
                resume_policy,
                stopped_at,
                external_session_id: Some(result.agent_session_id),
                agent_message_id: None,
                last_activity_at: Some(Some(now.clone())),
                summary: Some(result.summary),
                logs_path: Some(Some(logs_path)),
                before_sha: None,
                after_sha: Some(result.after_sha),
                error: Some(result.error),
                executor_config_snapshot_json: snapshot_update.map(Some),
                updated_at: now_rfc3339(),
            },
            lifecycle_event,
        )
        .await?;

        crate::DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .publish_committed(&committed_event);

        self.revoke_active_workspace_lease_for_execution(&task.id, &updated.id)
            .await;

        if let Some(account_usage) = result.account_usage.as_ref() {
            if let Err(error) = super::persist_account_usage_snapshot(
                &self.db,
                winner_snapshot.as_deref(),
                &updated.id,
                account_usage,
            )
            .await
            {
                tracing::warn!(execution_id = %updated.id, %error, "failed to persist account usage snapshot");
            }
        }

        if let Some(token_usage) = result.usage {
            let model = token_usage
                .model
                .or_else(|| usage_model_fallback.clone())
                .unwrap_or_else(|| "default".to_owned());
            if let Err(error) = ExecutionUsageRepo::upsert(
                &*self.db,
                db::UpsertExecutionUsage {
                    execution_id: updated.id.clone(),
                    provider: usage_provider,
                    model,
                    input_tokens: token_usage.input_tokens,
                    output_tokens: token_usage.output_tokens,
                    cache_read_tokens: token_usage.cache_read_tokens,
                    cache_write_tokens: token_usage.cache_write_tokens,
                    cost_usd: token_usage.cost_usd,
                },
            )
            .await
            {
                tracing::warn!(
                    execution_id = %updated.id,
                    %error,
                    "failed to record execution token usage"
                );
            }
        }

        super::publish_terminal_execution_event(self, &updated);

        if updated.status == ExecutionStatus::Completed {
            if let Err(error) = super::clear_execution_retry_metadata(&self.db, &task).await {
                tracing::warn!(
                    task_id = %task.id,
                    execution_id = %updated.id,
                    %error,
                    "failed to clear execution retry metadata"
                );
            }
        } else if updated.status == ExecutionStatus::Failed && max_turns_exceeded {
            if let Err(error) = self
                .annotate_max_turns_exceeded_block(&updated, max_turns)
                .await
            {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to block task after max turns exceeded"
                );
            }
        } else if updated.status == ExecutionStatus::Failed
            && executor_unavailable
            && should_block_task_for_failed_execution(&updated)
        {
            let attempts = serde_json::Value::Array(
                route_outcome
                    .attempts
                    .iter()
                    .map(|(candidate_key, outcome)| {
                        serde_json::json!({"candidate_key": candidate_key, "outcome": outcome})
                    })
                    .collect(),
            );
            if let Err(error) = self
                .annotate_executor_unavailable_block(&updated, unavailable_retry_at, attempts)
                .await
            {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to handle executor-unavailable execution"
                );
            }
        } else if updated.status == ExecutionStatus::Failed
            && should_block_task_for_failed_execution(&updated)
        {
            if let Err(error) = self.annotate_executor_failure_block(&updated).await {
                tracing::warn!(
                    execution_id = %updated.id,
                    task_id = %updated.task_id,
                    %error,
                    "failed to block task after executor failure"
                );
            }
        }

        Ok(updated)
    }
}

impl TaskService {
    pub(crate) async fn cancel_execution_with_provider(
        &self,
        execution: &Execution,
        reason: &str,
    ) -> Result<()> {
        let provider = self.execution_provider_for_execution(execution).await?;
        provider
            .cancel(api_types::ExecutionCancelParams {
                execution_id: execution.id.clone(),
                reason: Some(reason.to_owned()),
            })
            .await?;
        Ok(())
    }

    async fn execution_provider_for_execution(
        &self,
        execution: &Execution,
    ) -> Result<Arc<dyn crate::daemon_transport::ExecutionProvider>> {
        #[cfg(test)]
        if let Some(provider) = self.execution_provider_override.as_ref() {
            return Ok(Arc::clone(provider));
        }
        let daemon_id =
            crate::daemon_transport::router::resolved_daemon_id_for_execution(execution)?;
        if let Some(registry) = self.daemon_connections.as_ref() {
            return crate::daemon_transport::select_execution_provider(
                Some(&daemon_id),
                &self.db,
                registry,
            )
            .await
            .inspect_err(|error| {
                if let ServiceError::DaemonUnavailable { daemon_id } = error {
                    tracing::warn!(
                        execution_id = %execution.id,
                        daemon_id = %daemon_id,
                        "remote daemon unavailable for execution dispatch"
                    );
                }
            });
        }

        let daemon = db::DaemonRepo::get_by_id(&*self.db, &daemon_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("daemon", daemon_id.clone()))?;
        if daemon.status != db::DaemonStatus::Online
            || !crate::embedded_daemon::is_embedded_daemon_machine(&daemon.machine_id)
        {
            return Err(ServiceError::DaemonUnavailable { daemon_id });
        }

        let task_executor = self.task_executor.clone().ok_or_else(|| {
            ServiceError::invalid_operation(
                "task executor is not configured for execution dispatch",
            )
        })?;
        Ok(Arc::new(
            crate::daemon_transport::EmbeddedExecutionProvider::new(
                Arc::new(self.clone()),
                task_executor,
            ),
        ))
    }

    /// Review may read only explicitly bound result/workspace records. No
    /// Agent profile, Task repository default, or latest Execution selects it.
    pub(crate) async fn review_read_paths_for_execution(
        &self,
        execution: &Execution,
    ) -> Result<Vec<String>> {
        if !crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        if let Some(id) = execution.work_unit_id.as_deref() {
            if crate::work_unit_service::range_review::frozen_range(&self.db, id)
                .await?
                .is_some()
            {
                // Only the verified read-only range checkout is readable.
                let mut paths = Vec::new();
                for member in crate::work_unit_service::range_review::verify_range_checkout(
                    &self.db,
                    &execution.task_id,
                    id,
                )
                .await?
                {
                    let workspace = WorkspaceRepo::get_by_id(&*self.db, &member.workspace_id)
                        .await?
                        .ok_or_else(|| {
                            ServiceError::invalid_operation("Exact review Workspace is unavailable")
                        })?;
                    paths.push(workspace.worktree_path);
                }
                paths.sort();
                return Ok(paths);
            }
            let subject = crate::task_rework::get_review_subject(&self.db, id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "Formal WorkUnit review requires its exact persisted subject",
                    )
                })?;
            if subject.task_id != execution.task_id {
                return Err(ServiceError::invalid_operation(
                    "Review subject belongs to another Task",
                ));
            }
            for repository in WorkUnitRepo::list_review_subject_repositories(&*self.db, id).await? {
                if repository.result_workspace_id.is_none()
                    || repository.result_execution_id.is_none()
                    || repository.result_commit_sha.is_none()
                {
                    return Err(ServiceError::invalid_operation(format!(
                        "Review subject repository {} is not frozen to an exact result",
                        repository.repo_id
                    )));
                }
                ids.push(repository.result_workspace_id.ok_or_else(|| {
                    ServiceError::invalid_operation("Review subject result Workspace is missing")
                })?);
            }
            if let Some(id) = subject.result_workspace_id {
                ids.push(id);
            }
        }
        if let Some(id) = execution.workspace_id.as_ref() {
            ids.push(id.clone());
        }
        ids.sort();
        ids.dedup();
        let mut paths = Vec::new();
        for id in ids {
            let workspace = WorkspaceRepo::get_by_id(&*self.db, &id)
                .await?
                .filter(|workspace| workspace.task_id == execution.task_id)
                .ok_or_else(|| {
                    ServiceError::invalid_operation("Exact review Workspace is unavailable")
                })?;
            if let Some(work_unit_id) = execution.work_unit_id.as_deref() {
                let subject_rows =
                    WorkUnitRepo::list_review_subject_repositories(&*self.db, work_unit_id).await?;
                if !subject_rows.is_empty()
                    && !subject_rows.iter().any(|subject| {
                        subject.result_workspace_id.as_deref() == Some(workspace.id.as_str())
                            && subject.repo_id == workspace.repo_id
                    })
                    && WorkspaceRepo::get_group_by_work_unit_id(&*self.db, work_unit_id)
                        .await?
                        .is_some()
                {
                    return Err(ServiceError::invalid_operation(
                        "Review read path is outside the exact repository subject",
                    ));
                }
            }
            paths.push(workspace.worktree_path);
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    async fn verify_read_only_group_members(
        &self,
        execution: &Execution,
        working_context: &super::super::ExecutionWorkingContext,
    ) -> Result<()> {
        let Some(group) = working_context.workspace_group.as_ref() else {
            return Ok(());
        };
        let is_review = crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        let task_repositories = TaskRepo::list_repositories(&*self.db, &execution.task_id).await?;
        let work_unit_id = execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceGroup Execution has no WorkUnit")
        })?;
        let selected = WorkUnitRepo::list_repository_ids(&*self.db, work_unit_id).await?;
        if is_review
            && crate::work_unit_service::range_review::frozen_range(&self.db, work_unit_id)
                .await?
                .is_some()
        {
            // No-write verification of the lease-free range checkout.
            let checkout = crate::work_unit_service::range_review::verify_range_checkout(
                &self.db,
                &execution.task_id,
                work_unit_id,
            )
            .await?;
            let expected_repos = checkout
                .into_iter()
                .map(|member| member.repo_id)
                .collect::<HashSet<_>>();
            return ensure_no_unexpected_git_repositories(
                Path::new(&group.root_path),
                &expected_repos,
            );
        }
        let mut expected_context = HashSet::new();
        if !is_review {
            expected_context.extend(
                task_repositories
                    .iter()
                    .filter(|repository| repository.role == db::TaskRepositoryRole::Context)
                    .map(|repository| repository.repo_id.clone()),
            );
        }
        let subject_rows = if is_review {
            let work_unit_id = execution.work_unit_id.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation("Review Execution has no exact WorkUnit")
            })?;
            WorkUnitRepo::list_review_subject_repositories(&*self.db, work_unit_id).await?
        } else {
            Vec::new()
        };
        let expected_group_repos = if is_review {
            subject_rows
                .iter()
                .map(|repository| repository.repo_id.clone())
                .collect::<HashSet<_>>()
        } else {
            task_repositories
                .iter()
                .filter(|repository| {
                    repository.role == db::TaskRepositoryRole::Context
                        || (matches!(
                            repository.role,
                            db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                        ) && (selected.is_empty() || selected.contains(&repository.repo_id)))
                })
                .map(|repository| repository.repo_id.clone())
                .collect::<HashSet<_>>()
        };
        if expected_group_repos.len() != working_context.workspaces.len()
            || working_context
                .workspaces
                .iter()
                .any(|workspace| !expected_group_repos.contains(&workspace.repo_id))
        {
            return Err(ServiceError::invalid_operation(
                "WorkspaceGroup changed outside its exact Task and WorkUnit repository set",
            ));
        }
        ensure_no_unexpected_git_repositories(Path::new(&group.root_path), &expected_group_repos)?;
        for workspace in &working_context.workspaces {
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace scope", workspace.id.clone()))?;
            if scope.task_id != execution.task_id
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
                || scope.workspace_group_id.as_deref() != Some(group.id.as_str())
                || workspace.status != WorkspaceStatus::Ready
            {
                return Err(ServiceError::invalid_operation(format!(
                    "repository {} is not an exact ready WorkspaceGroup member",
                    workspace.repo_id
                )));
            }
            let is_pinned_read_only = is_review || expected_context.contains(&workspace.repo_id);
            if is_pinned_read_only && !workspace.branch.starts_with("detached:") {
                return Err(ServiceError::invalid_operation(format!(
                    "repository {} is not an exact read-only WorkspaceGroup member",
                    workspace.repo_id
                )));
            }
            if !is_pinned_read_only {
                let expected_branch = ::workspace::work_unit_branch_name(
                    &execution.task_id,
                    work_unit_id,
                    &workspace.id,
                )
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
                if workspace.branch != expected_branch {
                    return Err(ServiceError::invalid_operation(format!(
                        "writable repository {} is not on its exact WorkUnit branch",
                        workspace.repo_id
                    )));
                }
            }
            let expected_sha = if is_review {
                let subject = subject_rows
                    .iter()
                    .find(|subject| subject.repo_id == workspace.repo_id)
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "repository {} is outside the exact Review subject",
                            workspace.repo_id
                        ))
                    })?;
                if subject.result_workspace_id.is_none()
                    || subject.result_execution_id.is_none()
                    || subject.workspace_snapshot_digest.is_none()
                {
                    return Err(ServiceError::invalid_operation(format!(
                        "repository {} Review subject is not frozen to a complete result",
                        workspace.repo_id
                    )));
                }
                subject
                    .result_commit_sha
                    .as_deref()
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "repository {} Review subject has no exact result SHA",
                            workspace.repo_id
                        ))
                    })?
                    .to_owned()
            } else if expected_context.contains(&workspace.repo_id) {
                let pinned_sha: Option<String> = sqlx::query_scalar(
                    "SELECT commit_sha FROM task_repository_context_snapshot
                     WHERE task_id = ? AND repo_id = ?",
                )
                .bind(&execution.task_id)
                .bind(&workspace.repo_id)
                .fetch_optional(self.db.pool())
                .await?;
                pinned_sha.ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "context repository {} has no immutable Task snapshot",
                        workspace.repo_id
                    ))
                })?
            } else {
                continue;
            };
            if workspace.branch != format!("detached:{expected_sha}") {
                return Err(ServiceError::invalid_operation(format!(
                    "read-only repository {} is not detached at its pinned commit {}",
                    workspace.repo_id, expected_sha
                )));
            }
            let path = Path::new(&workspace.worktree_path);
            let head = git::get_current_sha(path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "repository {} HEAD could not be verified: {error}",
                    workspace.repo_id
                ))
            })?;
            let clean = git::is_worktree_clean(path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "repository {} working tree could not be verified: {error}",
                    workspace.repo_id
                ))
            })?;
            if head != expected_sha
                || !clean
                || workspace.before_sha.as_deref() != Some(expected_sha.as_str())
            {
                return Err(ServiceError::invalid_operation(format!(
                    "context/review repository {} changed from pinned commit {} (HEAD {}, clean {})",
                    workspace.repo_id, expected_sha, head, clean
                )));
            }
            if is_review {
                let subject = subject_rows
                    .iter()
                    .find(|subject| subject.repo_id == workspace.repo_id)
                    .ok_or_else(|| ServiceError::invalid_operation("Review subject changed"))?;
                let digest = crate::ValidationService::snapshot_digest(&workspace.worktree_path)
                    .await
                    .map_err(|error| {
                        ServiceError::invalid_operation(format!(
                            "review repository {} snapshot could not be verified: {error}",
                            workspace.repo_id
                        ))
                    })?;
                if subject.workspace_snapshot_digest.as_deref() != Some(digest.as_str()) {
                    return Err(ServiceError::invalid_operation(format!(
                        "review repository {} working tree differs from its frozen snapshot",
                        workspace.repo_id
                    )));
                }
            }
        }
        Ok(())
    }

    pub async fn record_execution_repository_results(
        &self,
        execution_id: &str,
    ) -> Result<Vec<db::ExecutionRepositoryResult>> {
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.to_owned()))?;
        let Some(work_unit_id) = execution.work_unit_id.as_deref() else {
            return Ok(Vec::new());
        };
        if execution.status != ExecutionStatus::Running {
            return Err(ServiceError::invalid_operation(
                "repository results require the exact running WorkUnit Execution",
            ));
        }
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .filter(|unit| unit.task_id == execution.task_id)
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        let group = WorkspaceRepo::get_group_by_work_unit_id(&*self.db, work_unit_id).await?;
        let formal_review = crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        let result_workspaces = if let Some(group) = group.as_ref() {
            if group.task_id != execution.task_id || group.work_unit_id != work_unit_id {
                return Err(ServiceError::invalid_operation(
                    "repository results require the exact WorkUnit WorkspaceGroup",
                ));
            }
            let working_context = self.resolve_execution_working_context(&execution).await?;
            self.verify_read_only_group_members(&execution, &working_context)
                .await?;
            if formal_review {
                return Ok(Vec::new());
            }
            working_context.workspaces
        } else if formal_review {
            return Ok(Vec::new());
        } else if !unit.requires_integration {
            if execution.workspace_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "non-repository WorkUnit Execution cannot produce repository results",
                ));
            }
            return Ok(Vec::new());
        } else {
            let _working_context = self.resolve_execution_working_context(&execution).await?;
            let workspaces =
                WorkUnitWorkspaceRepo::list_by_work_unit_id(&*self.db, work_unit_id).await?;
            let primary_repo_id = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
                .await?
                .and_then(|task| task.repo_id)
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "group-less repository WorkUnit requires its primary Task repository",
                    )
                })?;
            if workspaces.len() != 1
                || workspaces[0].repo_id != primary_repo_id
                || execution.workspace_id.as_deref() != Some(workspaces[0].id.as_str())
            {
                return Err(ServiceError::invalid_operation(
                    "group-less repository WorkUnit requires its exact single primary Workspace",
                ));
            }
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspaces[0].id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("workspace scope", workspaces[0].id.clone())
                })?;
            if scope.task_id != execution.task_id
                || scope.kind != db::WorkspaceScopeKind::WorkUnit
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
                || scope.workspace_group_id.is_some()
                || workspaces[0].status != WorkspaceStatus::Ready
            {
                return Err(ServiceError::invalid_operation(
                    "group-less repository result is outside its exact single WorkUnit Workspace",
                ));
            }
            workspaces
        };
        let task_repositories = TaskRepo::list_repositories(&*self.db, &execution.task_id).await?;
        let selected = WorkUnitRepo::list_repository_ids(&*self.db, work_unit_id).await?;
        let writable_ids = task_repositories
            .iter()
            .filter(|repository| {
                matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                ) && (selected.is_empty() || selected.contains(&repository.repo_id))
            })
            .map(|repository| repository.repo_id.as_str())
            .collect::<HashSet<_>>();
        let mut rows = Vec::with_capacity(writable_ids.len());
        for workspace in &result_workspaces {
            if !writable_ids.contains(workspace.repo_id.as_str()) {
                continue;
            }
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace scope", workspace.id.clone()))?;
            if scope.task_id != execution.task_id
                || scope.work_unit_id.as_deref() != Some(work_unit_id)
                || scope.workspace_group_id.as_deref()
                    != group.as_ref().map(|group| group.id.as_str())
                || workspace.status != WorkspaceStatus::Ready
            {
                return Err(ServiceError::invalid_operation(format!(
                    "repository {} result Workspace is outside the exact WorkUnit scope",
                    workspace.repo_id
                )));
            }
            let path = Path::new(&workspace.worktree_path);
            if !git::is_worktree_clean(path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "repository {} result working tree could not be checked: {error}",
                    workspace.repo_id
                ))
            })? {
                return Err(ServiceError::invalid_operation(format!(
                    "repository {} has uncommitted changes; a successful result SHA cannot be recorded",
                    workspace.repo_id
                )));
            }
            let after_sha = git::get_current_sha(path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "repository {} result SHA could not be read: {error}",
                    workspace.repo_id
                ))
            })?;
            let snapshot_digest =
                crate::ValidationService::snapshot_digest(&workspace.worktree_path)
                    .await
                    .map_err(|error| {
                        ServiceError::invalid_operation(format!(
                            "repository {} result snapshot could not be read: {error}",
                            workspace.repo_id
                        ))
                    })?;
            rows.push(db::ExecutionRepositoryResult {
                execution_id: execution.id.clone(),
                task_id: execution.task_id.clone(),
                repo_id: workspace.repo_id.clone(),
                workspace_id: workspace.id.clone(),
                before_sha: workspace.before_sha.clone(),
                after_sha: Some(after_sha),
                snapshot_digest: Some(snapshot_digest),
                created_at: now_rfc3339(),
            });
        }
        if rows.len() != writable_ids.len() {
            return Err(ServiceError::invalid_operation(
                "WorkUnit scope is missing a writable repository result Workspace",
            ));
        }
        let recorded =
            ExecutionRepo::record_repository_results(&*self.db, execution_id, &rows).await?;
        let actor = execution.actor_ref();
        crate::DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .append(db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.repository_results_recorded".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: execution.id.clone(),
                actor_type: actor
                    .as_ref()
                    .map(|actor| actor.kind().to_string())
                    .unwrap_or_else(|| "system".to_owned()),
                actor_id: actor.as_ref().map(|actor| actor.id().to_owned()),
                scope_type: "task".to_owned(),
                scope_id: execution.task_id.clone(),
                correlation_id: execution.id.clone(),
                causation_id: execution.parent_execution_id.clone(),
                causation_depth: i64::from(execution.parent_execution_id.is_some()),
                dedupe_key: Some(format!(
                    "execution.repository_results_recorded:{}",
                    execution.id
                )),
                payload_json: serde_json::json!({
                    "task_id": execution.task_id,
                    "execution_id": execution.id,
                    "repository_results": recorded.iter().map(|result| serde_json::json!({
                        "repo_id": result.repo_id,
                        "workspace_id": result.workspace_id,
                        "before_sha": result.before_sha,
                        "after_sha": result.after_sha,
                        "snapshot_digest": result.snapshot_digest,
                    })).collect::<Vec<_>>(),
                })
                .to_string(),
                created_at: now_rfc3339(),
            })
            .await?;
        Ok(recorded)
    }

    async fn execution_start_params(
        &self,
        execution: &Execution,
    ) -> Result<api_types::ExecutionStartParams> {
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let workspace_path = self
            .resolve_execution_working_context(execution)
            .await?
            .path
            .to_string_lossy()
            .into_owned();
        let snapshot = execution
            .executor_config_snapshot_json
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("execution missing executor config snapshot")
            })?;
        let executor_config = parse_json_value("executor config snapshot", snapshot)?;
        crate::task_service::config::validate_agent_routing_snapshot(&executor_config)?;
        let executor_type = executor_config
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation("executor config snapshot missing executor_type")
            })?
            .to_owned();
        let description = execution_description(execution, &task);
        let max_turns = self.resolve_max_turns(&task).await?;
        let invocation = super::harness_invocation_for_execution(
            &self.db,
            execution,
            execution.workspace_id.as_deref(),
        )
        .await?;

        Ok(api_types::ExecutionStartParams {
            task_id: task.id.clone(),
            execution_id: execution.id.clone(),
            role: execution.role.clone(),
            task_role_capabilities: crate::role_capabilities::execution_capabilities(
                &self.db,
                &execution.id,
            )
            .await?,
            review_read_paths: self.review_read_paths_for_execution(execution).await?,
            workspace_path,
            executor_type,
            executor_config,
            prompt: json!({ "description": description }),
            invocation,
            max_turns,
        })
    }
}

fn ensure_no_unexpected_git_repositories(
    group_root: &Path,
    expected_repo_ids: &HashSet<String>,
) -> Result<()> {
    if group_root.join(".git").exists() {
        return Err(ServiceError::invalid_operation(
            "unexpected Git repository at the WorkspaceGroup root",
        ));
    }
    let entries = std::fs::read_dir(group_root).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "could not inspect WorkspaceGroup root for unexpected Git repositories: {error}"
        ))
    })?;
    let mut seen = HashSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            ServiceError::invalid_operation(format!(
                "could not inspect WorkspaceGroup entry: {error}"
            ))
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type().map_err(|error| {
            ServiceError::invalid_operation(format!(
                "could not inspect WorkspaceGroup entry {}: {error}",
                path.display()
            ))
        })?;
        if expected_repo_ids.contains(&name) {
            if !file_type.is_dir() || !path.join(".git").exists() {
                return Err(ServiceError::invalid_operation(format!(
                    "expected repository {name} is not a Git worktree beneath the WorkspaceGroup root"
                )));
            }
            if let Some(nested) = nested_git_repository(&path, true)? {
                return Err(ServiceError::invalid_operation(format!(
                    "unexpected nested Git repository beneath authorized repository {name}: {}",
                    nested.display()
                )));
            }
            seen.insert(name);
        } else if file_type.is_dir() {
            if let Some(unexpected) = nested_git_repository(&path, false)? {
                return Err(ServiceError::invalid_operation(format!(
                    "unexpected Git repository beneath WorkspaceGroup root: {}",
                    unexpected.display()
                )));
            }
        } else if file_type.is_symlink() && path.join(".git").exists() {
            return Err(ServiceError::invalid_operation(format!(
                "unexpected Git repository through WorkspaceGroup symlink: {}",
                path.display()
            )));
        }
    }
    if seen != *expected_repo_ids {
        return Err(ServiceError::invalid_operation(
            "WorkspaceGroup is missing one or more exact repository worktrees",
        ));
    }
    Ok(())
}

fn nested_git_repository(path: &Path, allow_root_git: bool) -> Result<Option<std::path::PathBuf>> {
    let entries = std::fs::read_dir(path).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "could not inspect {} for nested Git repositories: {error}",
            path.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ServiceError::invalid_operation(format!(
                "could not inspect nested WorkspaceGroup entry: {error}"
            ))
        })?;
        let child = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            if !allow_root_git {
                return Ok(Some(child));
            }
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            ServiceError::invalid_operation(format!(
                "could not inspect {}: {error}",
                child.display()
            ))
        })?;
        if file_type.is_dir() {
            if let Some(nested) = nested_git_repository(&child, false)? {
                return Ok(Some(nested));
            }
        } else if file_type.is_symlink() && child.join(".git").exists() {
            return Ok(Some(child));
        }
    }
    Ok(None)
}

fn execution_description(execution: &Execution, task: &Task) -> String {
    execution
        .summary
        .clone()
        .or_else(|| task.description.clone())
        .unwrap_or_else(|| task.title.clone())
}

fn snapshot_credential_ref(snapshot: &Value) -> Result<Option<&str>> {
    match snapshot.get("credential_ref") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(reference)) if !reference.trim().is_empty() => Ok(Some(reference)),
        Some(_) => Err(ServiceError::invalid_operation(
            "snapshot credential reference is invalid",
        )),
    }
}

fn ensure_snapshot_credential_transport(
    provider: &dyn crate::daemon_transport::ExecutionProvider,
    executor_config: &Value,
) -> Result<()> {
    if snapshot_credential_ref(executor_config)?.is_some()
        && !provider.accepts_snapshot_credentials()
    {
        return Err(ServiceError::invalid_operation(
            "credential-backed Agent Execution cannot run on this remote daemon until it supports the exact snapshot credential identity",
        ));
    }
    Ok(())
}

fn usage_model_fallback(agent_config: &Value) -> Option<String> {
    agent_config
        .get("config")
        .and_then(|config| config.get("model"))
        .and_then(Value::as_str)
        .or_else(|| agent_config.get("model").and_then(Value::as_str))
        .filter(|model| !model.trim().is_empty())
        .map(str::to_owned)
}

fn max_turns_from_value(value: &Value) -> Option<u32> {
    value
        .get("max_turns")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

impl TaskService {
    async fn annotate_max_turns_exceeded_block(
        &self,
        execution: &Execution,
        max_turns: Option<u32>,
    ) -> Result<()> {
        let message = match max_turns {
            Some(limit) => format!("Execution stopped after reaching max_turns={limit}"),
            None => "Execution stopped after reaching max_turns".to_owned(),
        };
        self.annotate_execution_block(
            execution,
            api_types::FailureKind::MaxTurnsExceeded,
            message,
            None,
        )
        .await
    }

    async fn resolve_max_turns(&self, task: &Task) -> Result<Option<u32>> {
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        Ok(serde_json::from_str::<Value>(&project.settings)
            .ok()
            .and_then(|value| max_turns_from_value(&value)))
    }

    async fn resolve_execution_logs_path(
        &self,
        execution: &Execution,
        task: &Task,
        execution_id: &str,
    ) -> Result<String> {
        let durable_path = execution_logs_path(
            &self.workspace_root,
            &task.project_id,
            &task.id,
            execution_id,
        );
        let Some(stored_path) = execution.logs_path.as_deref() else {
            return Ok(durable_path);
        };
        if stored_path == durable_path {
            return Ok(durable_path);
        }

        executors::LogWriter::relocate(
            std::path::Path::new(stored_path),
            std::path::Path::new(&durable_path),
        )
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to move execution log: {error}"))
        })?;

        Ok(durable_path)
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn usage_provider_and_model_come_from_execution_snapshot() {
        let snapshot = json!({
            "executor_type": "codex",
            "model": "agent-model",
            "config": {
                "model": "gpt-5.5"
            }
        });

        assert_eq!(super::usage_provider_from_agent_config(&snapshot), "openai");
        assert_eq!(usage_model_fallback(&snapshot).as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn usage_model_falls_back_to_top_level_model() {
        let snapshot = json!({
            "executor_type": "claude_code",
            "model": "claude-haiku-4-5",
            "config": {}
        });

        assert_eq!(
            super::usage_provider_from_agent_config(&snapshot),
            "anthropic"
        );
        assert_eq!(
            usage_model_fallback(&snapshot).as_deref(),
            Some("claude-haiku-4-5")
        );
    }

    #[test]
    fn cursor_usage_provider_maps_to_cursor() {
        let snapshot = json!({
            "executor_type": "cursor",
            "config": {}
        });

        assert_eq!(super::usage_provider_from_agent_config(&snapshot), "cursor");
    }

    #[test]
    fn remote_provider_rejects_snapshot_credentials_before_any_daemon_request() {
        let registry = crate::daemon_transport::DaemonConnectionRegistry::without_handlers();
        let (connection, mut outbound) =
            crate::daemon_transport::DaemonConnection::new("remote-daemon".to_owned());
        registry.register("remote-daemon".to_owned(), connection);
        let provider = crate::daemon_transport::RemoteExecutionProvider::new(
            std::sync::Arc::new(registry),
            "remote-daemon".to_owned(),
        );

        let config = json!({"credential_ref": "credential-a"});
        let error = ensure_snapshot_credential_transport(&provider, &config)
            .expect_err("remote transport cannot resolve a host-only credential");
        assert!(error
            .to_string()
            .contains("exact snapshot credential identity"));
        assert!(outbound.try_recv().is_err());

        let uncredentialed = json!({"credential_ref": null});
        ensure_snapshot_credential_transport(&provider, &uncredentialed)
            .expect("uncredentialed remote execution retains existing transport");
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn execution_git_identity_overrides_agent_configured_identity() {
        let mut snapshot = json!({
            "config": {"env": {"GIT_AUTHOR_EMAIL": "human@example.com"}},
            "runtime_env": {"GIT_COMMITTER_EMAIL": "human@example.com", "OTHER": "kept"},
        });
        let identity = executors::git_identity::AgentGitIdentity::new(
            "implementer",
            "Codex",
            "agent-1234567890",
            "exec-1",
        );
        merge_agent_git_identity(&mut snapshot, &identity).expect("injects");
        let env = &snapshot["runtime_env"];
        assert_eq!(env["GIT_AUTHOR_NAME"], "implementer (Codex)");
        assert_eq!(
            env["GIT_AUTHOR_EMAIL"],
            "agent+implementer.agent123@agents.forge.invalid"
        );
        assert_eq!(env["GIT_COMMITTER_EMAIL"], env["GIT_AUTHOR_EMAIL"]);
        assert_eq!(env["FORGE_AGENT_ROLE"], "implementer");
        assert_eq!(env["FORGE_EXECUTION_ID"], "exec-1");
        assert_eq!(env["OTHER"], "kept");
    }
}
