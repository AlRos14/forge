use db::{AgentRepo, DbError, ExecutionRepo, ExecutionStatus, ResumePolicy, StopReason};

use crate::{Result, ServiceError};

use super::TaskDispatcher;

impl TaskDispatcher {
    /// Resume only exact WorkUnit-bound Executions explicitly marked for
    /// automatic recovery. Candidate discovery is by Execution identity and
    /// never depends on Task status or latest-per-Task selection.
    pub(super) async fn recover_auto_cancelled_work_unit_executions(&self) -> Result<u64> {
        let candidates = ExecutionRepo::list_work_unit_auto_recovery_candidates(&*self.db).await?;
        let mut resumed = 0;
        for execution in candidates {
            if self.is_stopped() {
                break;
            }
            if !is_exact_auto_recovery_candidate(&execution) {
                continue;
            }
            let Some(agent_id) = execution.agent_id.as_deref() else {
                continue;
            };
            let Some(agent) = AgentRepo::get_by_id(&*self.db, agent_id).await? else {
                continue;
            };
            if agent.paused {
                tracing::debug!(
                    task_id = %execution.task_id,
                    execution_id = %execution.id,
                    agent_id = %agent_id,
                    "Auto recovery remains pending while its Agent is paused"
                );
                continue;
            }
            if !crate::task_service::execution::has_exact_work_unit_target(
                &self.db,
                &execution.task_id,
                &execution,
            )
            .await?
                || crate::task_service::resumable_external_session(
                    &self.db,
                    &execution,
                    execution.agent_id.as_deref(),
                    execution.workspace_id.as_deref(),
                )
                .await?
                .is_none()
            {
                tracing::debug!(
                    task_id = %execution.task_id,
                    execution_id = %execution.id,
                    "Auto recovery awaits an exact reusable HarnessSession or Human decision"
                );
                continue;
            }

            let result = async {
                let launch = self
                    .task_service
                    .re_execute_automatic_recovery_execution(execution.id.clone())
                    .await?;
                if launch.idempotent_replay {
                    Ok(false)
                } else {
                    self.task_service
                        .start_execution(launch.execution.id.clone())
                        .await
                        .map(|start| start.accepted)
                }
            }
            .await;
            match result {
                Ok(true) => resumed += 1,
                Ok(false) => {}
                Err(ServiceError::Db(DbError::VersionConflict))
                | Err(ServiceError::Db(DbError::TaskVersionConflict { .. })) => {
                    tracing::debug!(
                        task_id = %execution.task_id,
                        execution_id = %execution.id,
                        "automatic execution recovery lost a version race"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        task_id = %execution.task_id,
                        execution_id = %execution.id,
                        %error,
                        "automatic execution recovery failed"
                    );
                }
            }
        }
        Ok(resumed)
    }
}

fn is_exact_auto_recovery_candidate(execution: &db::Execution) -> bool {
    execution.work_unit_id.is_some()
        && execution.status == ExecutionStatus::Cancelled
        && execution.resume_policy == Some(ResumePolicy::Auto)
        && matches!(
            execution.stop_reason,
            Some(StopReason::CrashRecovery | StopReason::GracefulShutdown)
        )
}

#[cfg(test)]
mod tests {
    use super::is_exact_auto_recovery_candidate;
    use db::{ExecutionStatus, ResumePolicy, StopReason};

    #[test]
    fn automatic_recovery_requires_exact_work_unit_execution_and_authorized_stop_reason() {
        let mut execution = db::Execution {
            id: "execution".to_owned(),
            task_id: "task".to_owned(),
            agent_id: Some("agent".to_owned()),
            actor_kind: None,
            actor_id: None,
            role: "implementer".to_owned(),
            purpose: None,
            status: ExecutionStatus::Cancelled,
            stop_reason: Some(StopReason::CrashRecovery),
            stopped_by: Some("system:recovery".to_owned()),
            resume_policy: Some(ResumePolicy::Auto),
            stopped_at: Some("2026-01-01T00:00:00Z".to_owned()),
            parent_execution_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            prompt: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            work_unit_id: Some("work-unit".to_owned()),
            work_unit_version: Some(1),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        assert!(is_exact_auto_recovery_candidate(&execution));

        execution.stop_reason = Some(StopReason::ExecutorFailed);
        assert!(!is_exact_auto_recovery_candidate(&execution));
        execution.stop_reason = Some(StopReason::CrashRecovery);
        execution.status = ExecutionStatus::Failed;
        assert!(!is_exact_auto_recovery_candidate(&execution));
        execution.status = ExecutionStatus::Cancelled;
        execution.work_unit_id = None;
        assert!(!is_exact_auto_recovery_candidate(&execution));
    }
}
