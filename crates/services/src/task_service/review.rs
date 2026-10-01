use super::*;
use db::WorkspaceRepo;

impl TaskService {
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
        if task.status != crate::workflow::default_states::REVIEW {
            return Err(ServiceError::invalid_operation(
                "Human Review Execution requires a Task in review state",
            ));
        }
        let assignment = TaskRoleAssignmentRepo::get_by_task_and_role(
            &*self.db,
            task_id,
            crate::workflow::default_roles::REVIEWER,
        )
        .await?;
        if !assignment.is_some_and(|assignment| {
            assignment.assignee_type == Some(db::AssigneeKind::User)
                && assignment.assignee_id.as_deref() == Some(user_id)
        }) {
            return Err(ServiceError::AuthorizationDenied {
                message: "Human reviewer is not the assigned reviewer for this Task".to_owned(),
            });
        }
        let page = ExecutionRepo::list_by_task_and_role(
            &*self.db,
            task_id,
            crate::workflow::default_roles::REVIEWER,
            PageRequest {
                cursor: None,
                limit: 100,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        if let Some(existing) = page.items.into_iter().find(|execution| {
            execution.purpose == Some(ExecutionPurpose::Review)
                && execution.actor_ref() == Some(db::ActorRef::Human(user_id.to_owned()))
                && execution.status == ExecutionStatus::Running
                && execution.workspace_id.as_deref() == workspace_id
        }) {
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
                .workspace_diff(workspace_id)
                .await?;
            Some((diff.base_sha, diff.head_sha))
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
                "workspace_id": workspace_id,
                "base_commit_sha": workspace_subject.as_ref().map(|(base, _)| base),
                "head_commit_sha": workspace_subject.as_ref().map(|(_, head)| head),
            })
            .to_string(),
            created_at: now.clone(),
        };
        let (execution, event) = ExecutionRepo::create_with_event(
            &*self.db,
            CreateExecution {
                id,
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
                before_sha: workspace_subject.as_ref().map(|(base, _)| base.clone()),
                after_sha: workspace_subject.as_ref().map(|(_, head)| head.clone()),
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: workspace_id.map(str::to_owned),
                created_at: now.clone(),
                updated_at: now,
            },
            event,
        )
        .await?;
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &event);
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
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
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
        if execution.status == ExecutionStatus::Running {
            if let Some(workspace_id) = execution.workspace_id.as_deref() {
                let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
                if workspace.task_id != execution.task_id
                    || workspace.status != WorkspaceStatus::Ready
                {
                    return Err(ServiceError::invalid_operation(
                        "Human Review Execution Workspace is no longer Ready for its exact Task",
                    ));
                }
                let diff = crate::DiffService::new(Arc::clone(&self.db))
                    .workspace_diff(workspace_id)
                    .await?;
                if execution.before_sha.as_deref() != Some(diff.base_sha.as_str())
                    || execution.after_sha.as_deref() != Some(diff.head_sha.as_str())
                {
                    return Err(ServiceError::invalid_operation(
                    "Human Review Execution subject commit changed; start a new exact Review Execution",
                ));
                }
            }
        }
        let assignment = TaskRoleAssignmentRepo::get_by_task_and_role(
            &*self.db,
            &execution.task_id,
            crate::workflow::default_roles::REVIEWER,
        )
        .await?;
        if !assignment.is_some_and(|assignment| {
            assignment.assignee_type == Some(db::AssigneeKind::User)
                && assignment.assignee_id.as_deref() == Some(user_id)
        }) {
            return Err(ServiceError::AuthorizationDenied {
                message: "Human Actor no longer holds the reviewer TaskRole".to_owned(),
            });
        }
        let summary = request.summary.trim();
        if summary.is_empty() || summary.len() > 8192 {
            return Err(ServiceError::invalid_operation(
                "ReviewReport summary must contain between 1 and 8192 bytes",
            ));
        }
        let evidence_ids = request.evidence_ids;
        let artifact_ids = request.artifact_ids;
        if execution.status == ExecutionStatus::Running {
            let mut unique_evidence = evidence_ids.clone();
            unique_evidence.sort();
            unique_evidence.dedup();
            for id in &unique_evidence {
                let evidence = db::ValidationRunRepo::get_evidence(&*self.db, id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("evidence", id.clone()))?;
                if evidence.task_id != execution.task_id {
                    return Err(ServiceError::invalid_operation(
                        "Review Evidence input must belong to the exact Review Task",
                    ));
                }
            }
            db::ValidationRunRepo::pin_execution_evidence_inputs(
                &*self.db,
                execution_id,
                &unique_evidence,
                &now_rfc3339(),
            )
            .await?;
            for id in &artifact_ids {
                let artifact = db::CollaborationRepo::get_artifact(&*self.db, id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("artifact", id.clone()))?;
                if artifact.task_id != execution.task_id {
                    return Err(ServiceError::invalid_operation(
                        "Review Artifact input must belong to the exact Review Task",
                    ));
                }
                db::CollaborationRepo::pin_execution_artifact_input(
                    &*self.db,
                    execution_id,
                    id,
                    &now_rfc3339(),
                )
                .await?;
            }
        }
        let verdict = match request.verdict {
            api_types::ReviewReportVerdict::Pass => "pass",
            api_types::ReviewReportVerdict::RequestChanges => "fail",
            api_types::ReviewReportVerdict::Questions => "needs_human",
        };
        let evidence_considered = evidence_ids
            .iter()
            .map(|id| serde_json::json!({ "evidence_id": id }))
            .chain(
                artifact_ids
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
        let report =
            crate::CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                .create_review_report_from_execution(execution_id, &assistant_output)
                .await?;

        let updated = if execution.status == ExecutionStatus::Completed {
            execution
        } else {
            let timestamp = now_rfc3339();
            let event =
                execution_status_domain_event(&execution, &ExecutionStatus::Completed, &timestamp);
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
                    summary: Some(Some(summary.to_owned())),
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
        };
        self.maybe_cascade_executor_completion(&updated.id).await?;
        Ok((updated, report))
    }
}
