use super::*;

impl WorkUnitService {
    /// Submission owns the technical Human attempt, so retrying an uncertain
    /// response reuses its exact report instead of creating a second review.
    pub async fn submit_human_review(
        &self,
        task_service: &crate::TaskService,
        work_unit_id: &str,
        user_id: &str,
        expected_version: i64,
        mut report: api_types::SubmitReviewReportRequest,
    ) -> Result<(Execution, db::Artifact)> {
        let _guard = self
            .integration_locks
            .acquire(&format!("human-work-unit-review:{work_unit_id}"))
            .await;
        let source = CollaborationActorSource::Human(user_id.to_owned());
        self.authorize_work_unit(&source, work_unit_id).await?;
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))?;
        let formal_review = crate::role_capabilities::task_role_name_has_capability(
            &self.db,
            &unit.task_id,
            &unit.role,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        if !formal_review
            || unit.requires_integration
            || unit.assigned_actor != Some(ActorRef::Human(user_id.to_owned()))
        {
            return Err(invalid(
                "Review submission requires the exact assigned Human in a formal_review TaskRole",
            ));
        }
        let digest = Sha256::digest(format!(
            "forge.human-work-unit-review.v1:{work_unit_id}:{expected_version}:{user_id}"
        ));
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let execution_id = Uuid::from_bytes(bytes).to_string();
        if let Some(frozen) = range_review::frozen_range(&self.db, work_unit_id).await? {
            crate::require_range_report_fields(&report)?;
            if report.reviewed_work_unit_id.is_some() || report.reviewed_work_unit_version.is_some()
            {
                return Err(range_review::mismatch(
                    "a commit-range review has no reviewed WorkUnit result",
                ));
            }
            if !range_review::range_allocation_is_independent(
                &self.db,
                work_unit_id,
                &ActorRef::Human(user_id.to_owned()),
            )
            .await?
            {
                return Err(invalid("Review requires an independent reviewer"));
            }
            // A replay finds the checkpoint already reviewed; only a new
            // attempt prepares the read-only group.
            if ExecutionRepo::get_by_id(&*self.db, &execution_id)
                .await?
                .is_none()
            {
                if !range_review::range_review_ready(&self.db, &frozen).await? {
                    return Err(range_review::mismatch(
                        "the review checkpoint is no longer active",
                    ));
                }
                if unit.status == WorkUnitStatus::Open {
                    self.prepare_workspace_group(source.clone(), work_unit_id)
                        .await?;
                }
            }
            return self
                .complete_human_review_attempt(
                    task_service,
                    source,
                    &unit,
                    user_id,
                    expected_version,
                    execution_id,
                    report,
                )
                .await;
        }
        crate::reject_range_report_fields(&report)?;
        let independent = crate::task_rework::reviewer_is_independent(
            &self.db,
            work_unit_id,
            &api_types::ActorRef::Human(user_id.to_owned()),
        )
        .await?;
        if !independent {
            return Err(invalid(
                "Review requires an available exact result and an independent reviewer",
            ));
        }
        let subject = crate::task_rework::get_review_subject(&self.db, work_unit_id)
            .await?
            .ok_or_else(|| invalid("Reviewer WorkUnit has no exact subject"))?;
        if report
            .reviewed_work_unit_id
            .as_deref()
            .is_some_and(|id| id != subject.reviewed_work_unit_id)
        {
            return Err(invalid("Review report cannot replace its WorkUnit subject"));
        }
        let frozen_version = serde_json::to_value(subject.reviewed_work_unit_version)
            .ok()
            .and_then(|value| value.as_i64());
        if report.reviewed_work_unit_version.is_some()
            && report.reviewed_work_unit_version != frozen_version
        {
            return Err(invalid(
                "Review report cannot replace its WorkUnit subject version",
            ));
        }
        // Version is derived by the report producer from the durable subject.
        // Do not turn a client supplied version into authority.
        report.reviewed_work_unit_version = None;
        report.reviewed_work_unit_id = None;

        // A replay of an already submitted review finds the WorkUnit completed;
        // its subject rows and group were frozen and prepared by the first call.
        if unit.status == WorkUnitStatus::Open && subject.result_execution_id.is_some() {
            let repositories = self
                .freeze_review_subject_repositories(work_unit_id)
                .await?;
            if !repositories.is_empty() {
                // Human review is a workspace-free Execution, but repository
                // subjects still require a prepared, read-only WorkspaceGroup
                // before the database admission guard accepts that attempt.
                self.prepare_workspace_group(source.clone(), work_unit_id)
                    .await?;
            }
        }
        if !crate::task_rework::review_subject_ready(&self.db, work_unit_id).await? {
            return Err(invalid(
                "Review requires an available exact result and an independent reviewer",
            ));
        }

        self.complete_human_review_attempt(
            task_service,
            source,
            &unit,
            user_id,
            expected_version,
            execution_id,
            report,
        )
        .await
    }

    /// Admit (or replay) the deterministic Human attempt, persist its exact
    /// report and complete the reviewer WorkUnit.
    #[allow(clippy::too_many_arguments)]
    async fn complete_human_review_attempt(
        &self,
        task_service: &crate::TaskService,
        source: CollaborationActorSource,
        unit: &WorkUnit,
        user_id: &str,
        expected_version: i64,
        execution_id: String,
        report: api_types::SubmitReviewReportRequest,
    ) -> Result<(Execution, db::Artifact)> {
        let work_unit_id = unit.id.as_str();
        if ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .is_none()
        {
            if unit.version != expected_version || unit.status != WorkUnitStatus::Open {
                return Err(db::DbError::VersionConflict.into());
            }
            let now = now_rfc3339();
            let admission = self
                .bind_execution(
                    source.clone(),
                    work_unit_id,
                    expected_version,
                    CreateExecution {
                        id: execution_id.clone(),
                        task_id: unit.task_id.clone(),
                        agent_id: None,
                        actor_ref: Some(ActorRef::Human(user_id.to_owned())),
                        role: unit.role.clone(),
                        status: ExecutionStatus::Running,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        parent_execution_id: None,
                        harness_session_id: None,
                        agent_message_id: None,
                        last_activity_at: Some(now.clone()),
                        summary: None,
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: None,
                        workspace_id: None,
                        created_at: now.clone(),
                        updated_at: now,
                    },
                )
                .await;
            if let Err(error) = admission {
                // A concurrent server may have admitted this same deterministic
                // attempt. It is safe to continue only with that exact identity.
                if ExecutionRepo::get_by_id(&*self.db, &execution_id)
                    .await?
                    .is_none()
                {
                    return Err(error);
                }
            }
        }
        let result = task_service
            .submit_human_review_report(&execution_id, user_id, report)
            .await?;
        let current = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id))?;
        if current.status == WorkUnitStatus::Open {
            self.transition(
                source,
                work_unit_id,
                expected_version,
                WorkUnitStatus::Completed,
            )
            .await?;
        } else if current.status != WorkUnitStatus::Completed
            || current.version != expected_version + 1
        {
            return Err(db::DbError::VersionConflict.into());
        }
        Ok(result)
    }
}
