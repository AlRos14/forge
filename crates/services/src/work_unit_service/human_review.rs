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
        if unit.role != "reviewer"
            || unit.requires_integration
            || unit.assigned_actor != Some(ActorRef::Human(user_id.to_owned()))
        {
            return Err(invalid(
                "Review submission requires the exact assigned Human reviewer",
            ));
        }
        if !crate::task_rework::review_subject_ready(&self.db, work_unit_id).await?
            || !crate::task_rework::reviewer_is_independent(
                &self.db,
                work_unit_id,
                &api_types::ActorRef::Human(user_id.to_owned()),
            )
            .await?
        {
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

        let digest = Sha256::digest(format!(
            "forge.human-work-unit-review.v1:{work_unit_id}:{expected_version}:{user_id}"
        ));
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let execution_id = Uuid::from_bytes(bytes).to_string();
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
                        role: "reviewer".to_owned(),
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
