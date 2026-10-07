use super::*;

const EXECUTION_LOG_BATCH_MAX_ENTRIES: usize = 50;
const EXECUTION_LOG_BATCH_MAX_WAIT: Duration = Duration::from_millis(500);

impl TaskService {
    pub(crate) async fn freeze_review_subject_and_inputs(
        &self,
        execution: Execution,
    ) -> Result<Execution> {
        if execution.role != crate::workflow::default_roles::REVIEWER
            || execution.purpose != Some(ExecutionPurpose::Review)
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
        if execution.role == crate::workflow::default_roles::REVIEWER
            && execution.purpose == Some(ExecutionPurpose::Review)
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
        let result = async {
            let agent = match execution.agent_id.as_deref() {
                Some(agent_id) => Some(
                    AgentRepo::get_by_id(&*self.db, agent_id)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?,
                ),
                None => None,
            };
            if execution.role == crate::workflow::default_roles::REVIEWER
                && execution.purpose == Some(ExecutionPurpose::Review)
            {
                self.verify_execution_workspace_authority(&execution)
                    .await?;
            }
            let execution = self.freeze_review_subject_and_inputs(execution).await?;
            let provider = self
                .execution_provider_for_agent(agent.as_ref(), &execution.id)
                .await?;
            let params = self.execution_start_params(&execution).await?;
            if snapshot_credential_ref(&params.executor_config)?.is_some() {
                ensure_snapshot_credential_transport(provider.as_ref(), &params.executor_config)?;
                if self.credential_env.is_none() {
                    return Err(ServiceError::invalid_operation(
                        "snapshot credential cannot be resolved by this execution host",
                    ));
                }
            }
            provider.start(params).await
        }
        .await;

        match result {
            Ok(result) => Ok(result),
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
                Err(error)
            }
        }
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
        if execution.role == crate::workflow::default_roles::REVIEWER
            && execution.purpose == Some(ExecutionPurpose::Review)
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
        let orchestrator_execution = execution.role == "orchestrator"
            && execution.purpose == Some(ExecutionPurpose::Orchestrate);
        let workspace = if orchestrator_execution {
            self.validate_orchestrator_execution(&execution).await?;
            let path = self.orchestrator_context_path(&execution.id);
            std::fs::create_dir_all(&path).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "failed to create isolated orchestrator context: {error}"
                ))
            })?;
            Workspace {
                id: format!("orchestrator:{}", execution.id),
                task_id: task.id.clone(),
                repo_id: task.repo_id.clone().unwrap_or_default(),
                worktree_path: path.to_string_lossy().into_owned(),
                branch: "orchestrator-read-only".to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: None,
                cleanup_after: None,
                error: None,
                created_at: execution.created_at.clone(),
                updated_at: execution.updated_at.clone(),
            }
        } else {
            let workspace_id = execution
                .workspace_id
                .as_deref()
                .ok_or_else(|| ServiceError::invalid_operation("execution missing workspace_id"))?;
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            self.verify_active_workspace_lease(
                &task,
                &workspace,
                &execution.role,
                execution.agent_id.as_deref(),
                &execution.id,
            )
            .await?;
            workspace
        };
        let workspace_id = workspace.id.as_str();
        let snapshot = execution
            .executor_config_snapshot_json
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("execution missing executor config snapshot")
            })?;
        let mut agent_config = parse_json_value("executor config snapshot", snapshot)?;
        crate::task_service::config::validate_agent_routing_snapshot(&agent_config)?;
        if execution.role == crate::workflow::default_roles::REVIEWER
            || matches!(
                task.task_type.as_str(),
                "planning" | "discovery" | "review" | "validation"
            )
        {
            executors::mark_worktree_read_only(&mut agent_config);
        }
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
        let max_turns = self.resolve_max_turns(&task).await?;
        let logs_path = self
            .resolve_execution_logs_path(&execution, &task, &workspace, &execution_id)
            .await?;
        if let Some(parent) = std::path::Path::new(&logs_path).parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ServiceError::invalid_operation(format!("failed to create log directory: {error}"))
            })?;
        }

        if let Some(terminal_activity) = self.terminal_activity.as_ref() {
            if terminal_activity
                .workspace_has_active_terminal(workspace_id)
                .await
            {
                return Err(ServiceError::TerminalActiveExecution {
                    workspace_id: workspace_id.to_owned(),
                });
            }
        }
        let _exec_lock_guard = if let Some(locks) = self.workspace_exec_locks.as_ref() {
            if let Some(guard) = locks.try_acquire(workspace_id) {
                if let Some(terminal_activity) = self.terminal_activity.as_ref() {
                    if terminal_activity
                        .workspace_has_active_terminal(workspace_id)
                        .await
                    {
                        return Err(ServiceError::TerminalActiveExecution {
                            workspace_id: workspace_id.to_owned(),
                        });
                    }
                }
                Some(guard)
            } else {
                if let Some(terminal_activity) = self.terminal_activity.as_ref() {
                    if terminal_activity
                        .workspace_has_active_terminal(workspace_id)
                        .await
                    {
                        return Err(ServiceError::TerminalActiveExecution {
                            workspace_id: workspace_id.to_owned(),
                        });
                    }
                }
                self.event_bus.publish(events::ForgeEvent {
                    event_type: "workspace.execution_waiting".to_owned(),
                    entity_id: workspace_id.to_owned(),
                    timestamp: events::event_timestamp(),
                    context: events::EventContext::WorkspaceExecutionWaiting {
                        workspace_id: workspace_id.to_owned(),
                        task_id: task.id.clone(),
                    },
                });
                Some(locks.acquire(workspace_id).await)
            }
        } else {
            None
        };

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

        let is_review_execution = execution_before_launch.role
            == crate::workflow::default_roles::REVIEWER
            && execution_before_launch.purpose == Some(ExecutionPurpose::Review);
        let mut review_worktree_snapshot = None;
        let mut review_snapshot_digest = None;
        if is_review_execution
            && execution_before_launch.workspace_id.is_some()
            && executors::is_worktree_read_only(&agent_config)
        {
            let worktree_path = std::path::Path::new(&workspace.worktree_path);
            let digest_before =
                match crate::ValidationService::snapshot_digest(&workspace.worktree_path).await {
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
                match crate::ValidationService::snapshot_digest(&workspace.worktree_path).await {
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
            .freeze_review_subject_and_inputs(execution_before_launch)
            .await?;
        let review_subject = if let Some(snapshot) = review_worktree_snapshot.as_ref() {
            let subject = ExecutionRepo::get_review_execution_subject(&*self.db, &execution.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "local Review Execution did not persist its exact workspace subject",
                    )
                })?;
            let live_digest =
                crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
            let live_head =
                git::get_current_sha(std::path::Path::new(&workspace.worktree_path)).await?;
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
                agent_session_id: None,
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

        let read_only_head = if review_worktree_snapshot.is_none()
            && execution.workspace_id.is_some()
            && executors::is_worktree_read_only(&agent_config)
        {
            Some(git::get_current_sha(std::path::Path::new(&workspace.worktree_path)).await?)
        } else {
            None
        };
        let existing_plan_output = if execution.purpose == Some(ExecutionPurpose::Plan) {
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
        let usage_probe = if existing_plan_output.is_some() {
            None
        } else {
            self.task_executor.clone().and_then(|executor| {
                super::spawn_account_usage_probe(
                    Arc::clone(&self.db),
                    execution.executor_config_snapshot_json.clone(),
                    execution_id.clone(),
                    executor,
                )
            })
        };
        let prelaunch_review_error = if let Some(subject) = review_subject.as_ref() {
            let current_head =
                git::get_current_sha(std::path::Path::new(&workspace.worktree_path)).await;
            let current_digest =
                crate::ValidationService::snapshot_digest(&workspace.worktree_path).await;
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
                    worktree_path: workspace.worktree_path.clone(),
                    description,
                    agent_config,
                    logs_path: logs_path.clone(),
                    heartbeat_interval_seconds: 30,
                    max_turns,
                    log_sender: Some(log_tx),
                })
                .await
        };
        if let Some(probe) = usage_probe {
            probe.stop().await;
        }
        if let Err(error) = executors::LogWriter::compact(std::path::Path::new(&logs_path)).await {
            tracing::warn!(%execution_id, %error, "failed to compress final execution log segment; plain log retained");
        }
        let mut review_restore_error = None;
        if let (Some(snapshot), Some(subject)) =
            (review_worktree_snapshot.as_ref(), review_subject.as_ref())
        {
            let restore = snapshot
                .restore(std::path::Path::new(&workspace.worktree_path))
                .await;
            match restore {
                Ok(()) => {
                    let restored_head =
                        git::get_current_sha(std::path::Path::new(&workspace.worktree_path)).await;
                    let restored_digest =
                        crate::ValidationService::snapshot_digest(&workspace.worktree_path).await;
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
        let restore_result = if review_worktree_snapshot.is_some() {
            Ok(())
        } else if let Some(head) = read_only_head.as_deref() {
            git::restore_worktree(std::path::Path::new(&workspace.worktree_path), head)
                .await
                .map_err(ServiceError::from)
        } else {
            Ok(())
        };
        let mut result = if let Some(error) = review_restore_error {
            let snapshot = review_worktree_snapshot
                .take()
                .expect("Review restore failure retains its isolated snapshot");
            let backup_path = snapshot.preserve_for_diagnostics();
            tracing::error!(
                execution_id = %execution_id,
                workspace_path = %workspace.worktree_path,
                snapshot_backup_path = %backup_path.display(),
                %error,
                "read-only Review workspace could not be restored to its exact subject"
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
            restore_result?;
            if let Some(subject) = review_subject.as_ref() {
                result.after_sha = Some(subject.head_commit_sha.clone());
            } else if let Some(head) = read_only_head {
                result.after_sha = Some(head);
            }
            result
        };
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

        if current_execution.purpose == Some(ExecutionPurpose::Plan)
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

        if current_execution.role == crate::workflow::default_roles::REVIEWER
            && current_execution.purpose == Some(ExecutionPurpose::Review)
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
                    );
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
                agent_session_id: Some(result.agent_session_id),
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

        if let Err(error) = self
            .memory_service
            .record_execution_summary_if_present(&task.project_id, &updated)
            .await
        {
            tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
        }

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
    pub(in crate::task_service) async fn cancel_execution_with_provider(
        &self,
        execution: &Execution,
        reason: &str,
    ) -> Result<()> {
        let agent = match execution.agent_id.as_deref() {
            Some(agent_id) => Some(
                AgentRepo::get_by_id(&*self.db, agent_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?,
            ),
            None => None,
        };
        let provider = self
            .execution_provider_for_agent(agent.as_ref(), &execution.id)
            .await?;
        provider
            .cancel(api_types::ExecutionCancelParams {
                execution_id: execution.id.clone(),
                reason: Some(reason.to_owned()),
            })
            .await?;
        Ok(())
    }

    async fn execution_provider_for_agent(
        &self,
        agent: Option<&Agent>,
        execution_id: &str,
    ) -> Result<Arc<dyn crate::daemon_transport::ExecutionProvider>> {
        let daemon_id = agent.and_then(|agent| agent.daemon_id.as_deref());
        if let Some(registry) = self.daemon_connections.as_ref() {
            return crate::daemon_transport::select_execution_provider(
                daemon_id, &self.db, registry,
            )
            .await
            .inspect_err(|error| {
                if let ServiceError::DaemonUnavailable { daemon_id } = error {
                    tracing::warn!(
                        execution_id = %execution_id,
                        daemon_id = %daemon_id,
                        agent_id = ?agent.map(|agent| agent.id.as_str()),
                        "remote daemon unavailable for execution dispatch"
                    );
                }
            });
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

    async fn execution_start_params(
        &self,
        execution: &Execution,
    ) -> Result<api_types::ExecutionStartParams> {
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let workspace_path = if execution.role == "orchestrator"
            && execution.purpose == Some(ExecutionPurpose::Orchestrate)
        {
            self.validate_orchestrator_execution(execution).await?;
            let path = self.orchestrator_context_path(&execution.id);
            std::fs::create_dir_all(&path).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "failed to create isolated orchestrator context: {error}"
                ))
            })?;
            path.to_string_lossy().into_owned()
        } else {
            let workspace_id = execution
                .workspace_id
                .as_deref()
                .ok_or_else(|| ServiceError::invalid_operation("execution missing workspace_id"))?;
            WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?
                .worktree_path
        };
        let snapshot = execution
            .executor_config_snapshot_json
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("execution missing executor config snapshot")
            })?;
        let mut executor_config = parse_json_value("executor config snapshot", snapshot)?;
        crate::task_service::config::validate_agent_routing_snapshot(&executor_config)?;
        if execution.role == crate::workflow::default_roles::REVIEWER
            || matches!(
                task.task_type.as_str(),
                "planning" | "discovery" | "review" | "validation"
            )
        {
            executors::mark_worktree_read_only(&mut executor_config);
        }
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
            workspace_path,
            executor_type,
            executor_config,
            prompt: json!({ "description": description }),
            invocation,
            max_turns,
        })
    }
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
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let message = match max_turns {
            Some(limit) => format!("Execution stopped after reaching max_turns={limit}"),
            None => "Execution stopped after reaching max_turns".to_owned(),
        };
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: api_types::FailureKind::MaxTurnsExceeded,
            blocking_reason: "max_turns_exceeded".to_owned(),
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
            message: Some(message.clone()),
            hook: None,
            recovery_actions: vec![
                api_types::RecoveryAction::ResetToInitial,
                api_types::RecoveryAction::CancelTask,
            ],
        };
        let blocked_meta = json!({
            "reason": message,
            "created_at": now_rfc3339(),
            "kind": "max_turns_exceeded",
            "execution_id": execution.id,
        });
        let updated = TaskRepo::update_status(
            &*self.db,
            db::UpdateTaskStatus {
                id: task.id.clone(),
                expected_version: task.version,
                status: task.status,
                assignee_id: None,
                error_annotation: Some(Some(serde_json::to_string(&annotation).map_err(
                    |error| {
                        ServiceError::invalid_operation(format!(
                            "failed to serialize max-turns annotation: {error}"
                        ))
                    },
                )?)),
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
            entity_id: updated.id,
            timestamp: event_timestamp(),
            context: EventContext::TaskBlocked {
                project_id: updated.project_id,
                reason: "max_turns_exceeded".to_owned(),
                kind: Some(api_types::FailureKind::MaxTurnsExceeded),
                source: None,
                execution_id: Some(execution.id.clone()),
            },
        });
        Ok(())
    }

    async fn resolve_max_turns(&self, task: &Task) -> Result<Option<u32>> {
        if let Some(value) = task
            .task_state_config
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|value| max_turns_from_value(&value))
        {
            return Ok(Some(value));
        }

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
        workspace: &Workspace,
        execution_id: &str,
    ) -> Result<String> {
        let durable_path = execution_logs_path(
            &self.workspace_root,
            &task.project_id,
            &workspace.task_id,
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
}
