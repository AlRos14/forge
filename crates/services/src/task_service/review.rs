use super::*;
use db::{CollaborationRepo, TaskLifecycleRepo, WorkspaceRepo};

impl TaskService {
    /// Reuse one exact durable ReviewReport and repair the separate Execution
    /// terminal write after a crash. The Artifact's historical producer,
    /// ActorRef, and frozen subject are validated without consulting current
    /// membership or live Workspace state.
    pub(crate) async fn reconcile_existing_review_report(
        &self,
        execution: &Execution,
        expected_report: Option<&db::Artifact>,
        expected_assistant_output: Option<&str>,
    ) -> Result<Option<(Execution, db::Artifact)>> {
        let collaboration =
            crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let Some(report) = collaboration
            .exact_review_report_for_execution(execution)
            .await?
        else {
            return Ok(None);
        };
        if let Some(expected) = expected_report {
            if expected.id != report.id
                || expected.task_id != report.task_id
                || expected.digest != report.digest
                || expected.content != report.content
                || expected.execution_producer() != report.execution_producer()
            {
                return Err(ServiceError::invalid_operation(
                    "Human Review retry does not match its exact persisted ReviewReport",
                ));
            }
        }
        if let Some(output) = expected_assistant_output {
            let replayed = collaboration
                .create_review_report_from_execution(&execution.id, output)
                .await?;
            if replayed.id != report.id
                || replayed.digest != report.digest
                || replayed.content != report.content
                || replayed.execution_producer() != report.execution_producer()
            {
                return Err(ServiceError::invalid_operation(
                    "Agent Review retry conflicts with its exact persisted ReviewReport",
                ));
            }
        }

        let updated = match execution.status {
            ExecutionStatus::Running => {
                let content = report.content.as_deref().ok_or_else(|| {
                    ServiceError::invalid_operation("ReviewReport content is unavailable")
                })?;
                let value: serde_json::Value = serde_json::from_str(content).map_err(|error| {
                    ServiceError::invalid_operation(format!(
                        "ReviewReport JSON is invalid: {error}"
                    ))
                })?;
                let summary = value
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Review completed")
                    .to_owned();
                let timestamp = now_rfc3339();
                let event = execution_status_domain_event(
                    execution,
                    &ExecutionStatus::Completed,
                    &timestamp,
                );
                let (updated, committed_event) = ExecutionRepo::update_with_event(
                    &*self.db,
                    db::UpdateExecution {
                        id: execution.id.clone(),
                        status: Some(ExecutionStatus::Completed),
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: Some(Some(timestamp.clone())),
                        summary: Some(Some(summary)),
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: Some(None),
                        executor_config_snapshot_json: None,
                        updated_at: timestamp,
                    },
                    event,
                )
                .await?;
                self.publish_committed_domain_event(&committed_event);
                updated
            }
            ExecutionStatus::Completed => execution.clone(),
            _ => {
                return Err(ServiceError::invalid_operation(
                    "persisted ReviewReport can reconcile only a Running or Completed Execution",
                ));
            }
        };
        self.maybe_cascade_executor_completion(&updated.id).await?;
        Ok(Some((updated, report)))
    }

    pub async fn start_human_review_execution(
        &self,
        task_id: &str,
        user_id: &str,
        workspace_id: Option<&str>,
    ) -> Result<Execution> {
        validate_required("task_id", task_id)?;
        validate_required("user_id", user_id)?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        if lifecycle.state != db::TaskLifecycleState::Active {
            return Err(ServiceError::invalid_operation(
                "Human Review Execution requires an active aggregate Task lifecycle",
            ));
        }
        if !human_is_active_role_member_authoritative(
            &self.db,
            task_id,
            crate::workflow::default_roles::REVIEWER,
            user_id,
        )
        .await?
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "Human reviewer is not an active member of the reviewer TaskRole"
                    .to_owned(),
            });
        }

        let _workspace_review_guard = if let (Some(workspace_id), Some(locks)) =
            (workspace_id, self.workspace_exec_locks.as_ref())
        {
            Some(locks.acquire(workspace_id).await)
        } else {
            None
        };
        if workspace_id.is_some() {
            let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
            if lifecycle.state != db::TaskLifecycleState::Active {
                return Err(ServiceError::invalid_operation(
                    "Human Review Execution requires an active aggregate Task lifecycle",
                ));
            }
            if !human_is_active_role_member_authoritative(
                &self.db,
                task_id,
                crate::workflow::default_roles::REVIEWER,
                user_id,
            )
            .await?
            {
                return Err(ServiceError::AuthorizationDenied {
                    message: "Human reviewer is not an active member of the reviewer TaskRole"
                        .to_owned(),
                });
            }
        }

        if let Some(existing) = ExecutionRepo::find_running_human_review_execution(
            &*self.db,
            task_id,
            user_id,
            workspace_id,
        )
        .await?
        {
            if workspace_id.is_some() {
                crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                    .ensure_review_subject_current(&existing)
                    .await?;
            }
            return Ok(existing);
        }

        let workspace_subject = if let Some(workspace_id) = workspace_id {
            let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            if workspace.task_id != task_id || workspace.status != WorkspaceStatus::Ready {
                return Err(ServiceError::invalid_operation(
                    "Human Review Execution Workspace must be Ready and belong to the exact Task",
                ));
            }
            let diff = crate::DiffService::new(Arc::clone(&self.db))
                .workspace_diff(&workspace.id)
                .await?;
            if diff.head_sha.is_empty() {
                return Err(ServiceError::invalid_operation(
                    "Human Review Execution Workspace has no exact HEAD commit",
                ));
            }
            let snapshot_digest =
                crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
            Some((workspace.id, diff.base_sha, diff.head_sha, snapshot_digest))
        } else {
            None
        };
        let now = now_rfc3339();
        let id = new_uuid_v4();
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "execution.started".to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(user_id.to_owned()),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("human-review-execution-started:{id}")),
            payload_json: json!({
                "execution_id": id,
                "task_id": task_id,
                "project_id": task.project_id,
                "role": crate::workflow::default_roles::REVIEWER,
                "purpose": "review",
                "actor_kind": "human",
                "actor_id": user_id,
                "workspace_id": workspace_subject.as_ref().map(|subject| &subject.0),
                "base_commit_sha": workspace_subject.as_ref().map(|subject| &subject.1),
                "head_commit_sha": workspace_subject.as_ref().map(|subject| &subject.2),
                "workspace_snapshot_digest": workspace_subject.as_ref().map(|subject| &subject.3),
            })
            .to_string(),
            created_at: now.clone(),
        };
        let execution_input = db::CreateExecution {
            id: id.clone(),
            task_id: task_id.to_owned(),
            agent_id: None,
            actor_ref: Some(db::ActorRef::Human(user_id.to_owned())),
            role: crate::workflow::default_roles::REVIEWER.to_owned(),
            purpose: Some(ExecutionPurpose::Review),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: Some(now.clone()),
            summary: None,
            logs_path: None,
            before_sha: workspace_subject.as_ref().map(|subject| subject.1.clone()),
            after_sha: workspace_subject.as_ref().map(|subject| subject.2.clone()),
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: workspace_subject.as_ref().map(|subject| subject.0.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let (execution, committed_event) =
            if let Some((workspace_id, base_sha, head_sha, digest)) = workspace_subject {
                let write = ExecutionRepo::create_human_review_execution_with_subject(
                    &*self.db,
                    execution_input,
                    db::CreateReviewExecutionSubject {
                        execution_id: id,
                        task_id: task_id.to_owned(),
                        workspace_id,
                        base_commit_sha: base_sha,
                        head_commit_sha: head_sha,
                        workspace_snapshot_digest: digest,
                        created_at: now,
                    },
                    event,
                )
                .await?;
                let committed_event = write.event.ok_or_else(|| {
                    ServiceError::invalid_operation("new Human Review subject has no start event")
                })?;
                (write.execution, committed_event)
            } else {
                ExecutionRepo::create_human_review_execution_without_subject(
                    &*self.db,
                    execution_input,
                    event,
                )
                .await?
            };
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &committed_event);
        Ok(execution)
    }

    pub async fn submit_human_review_report(
        &self,
        execution_id: &str,
        user_id: &str,
        request: api_types::SubmitReviewReportRequest,
    ) -> Result<(Execution, db::Artifact)> {
        validate_required("execution_id", execution_id)?;
        validate_required("user_id", user_id)?;
        let mut execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.to_owned()))?;
        let _workspace_review_guard = if execution.status == ExecutionStatus::Running {
            if let (Some(workspace_id), Some(locks)) = (
                execution.workspace_id.as_deref(),
                self.workspace_exec_locks.as_ref(),
            ) {
                Some(locks.acquire(workspace_id).await)
            } else {
                None
            }
        } else {
            None
        };
        execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.to_owned()))?;
        if execution.role != crate::workflow::default_roles::REVIEWER
            || execution.purpose != Some(ExecutionPurpose::Review)
            || execution.actor_ref() != Some(db::ActorRef::Human(user_id.to_owned()))
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "ReviewReport submission must target the exact Human reviewer Execution"
                    .to_owned(),
            });
        }
        if !matches!(
            execution.status,
            ExecutionStatus::Running | ExecutionStatus::Completed
        ) {
            return Err(ServiceError::invalid_operation(
                "ReviewReport can only complete a running Human Review Execution",
            ));
        }
        let summary = request.summary.trim();
        if summary.is_empty() || summary.len() > 8192 {
            return Err(ServiceError::invalid_operation(
                "ReviewReport summary must contain between 1 and 8192 bytes",
            ));
        }
        let mut unique_evidence = std::collections::HashSet::new();
        if request
            .evidence_ids
            .iter()
            .any(|id| !unique_evidence.insert(id.as_str()))
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport contains a duplicate Evidence reference",
            ));
        }
        let mut unique_artifacts = std::collections::HashSet::new();
        if request
            .artifact_ids
            .iter()
            .any(|id| !unique_artifacts.insert(id.as_str()))
        {
            return Err(ServiceError::invalid_operation(
                "ReviewReport contains a duplicate Artifact reference",
            ));
        }

        let verdict = match request.verdict {
            api_types::ReviewReportVerdict::Pass => "pass",
            api_types::ReviewReportVerdict::RequestChanges => "fail",
            api_types::ReviewReportVerdict::Questions => "needs_human",
        };
        let evidence_considered = request
            .evidence_ids
            .iter()
            .map(|id| serde_json::json!({ "evidence_id": id }))
            .chain(
                request
                    .artifact_ids
                    .iter()
                    .map(|id| serde_json::json!({ "artifact_id": id })),
            )
            .collect::<Vec<_>>();
        let assistant_output = format!(
            "FORGE_RESULT: {}",
            serde_json::json!({
                "schema_version": 1,
                "kind": "review",
                "verdict": verdict,
                "summary": summary,
                "criteria": request.criteria,
                "findings": request.findings,
                "questions": request.questions,
                "evidence_considered": evidence_considered,
            })
        );
        let existing_report = CollaborationRepo::get_execution_artifact_output(
            &*self.db,
            execution_id,
            db::ArtifactKind::ReviewReport,
        )
        .await?;
        if execution.status == ExecutionStatus::Completed && existing_report.is_none() {
            return Err(ServiceError::invalid_operation(
                "completed Human Review Execution has no exact ReviewReport to replay",
            ));
        }
        let report =
            crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                .create_human_review_report_from_execution(
                    execution_id,
                    &assistant_output,
                    request.evidence_ids,
                    request.artifact_ids,
                )
                .await?;
        let (updated, report) = self
            .reconcile_existing_review_report(&execution, Some(&report), None)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Human Review submission did not persist its exact ReviewReport",
                )
            })?;
        Ok((updated, report))
    }
}
