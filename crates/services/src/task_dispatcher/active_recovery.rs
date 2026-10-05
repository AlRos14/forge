use db::{
    DbError, ExecutionRepo, ExecutionStatus, Project, ResumePolicy, StopReason, TaskLifecycleRepo,
    TaskLifecycleState,
};

use crate::{Result, ServiceError};

use super::TaskDispatcher;

impl TaskDispatcher {
    /// Resume only an exact Task-scoped Execution that the shutdown/recovery
    /// authority explicitly marked for automatic recovery. Task lifecycle,
    /// TaskType, workflow state, and retry receipts never direct rework here.
    pub(super) async fn recover_auto_cancelled_tasks(&self, project: &Project) -> Result<u64> {
        let tasks = self
            .list_tasks(&project.id, vec!["in_progress".to_owned()])
            .await?;
        let task_ids = tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>();
        let latest_executions =
            ExecutionRepo::list_latest_executions_for_tasks(&*self.db, &task_ids).await?;
        let latest_by_task = latest_executions
            .into_iter()
            .map(|execution| (execution.task_id.clone(), execution))
            .collect::<std::collections::HashMap<_, _>>();

        let mut resumed = 0;
        for task in tasks {
            if self.is_stopped() {
                break;
            }
            let Some(lifecycle) =
                TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id).await?
            else {
                continue;
            };
            if lifecycle.state != TaskLifecycleState::Active {
                continue;
            }
            let Some(execution) = latest_by_task.get(&task.id) else {
                continue;
            };
            if !is_exact_auto_recovery_candidate(execution) {
                continue;
            }

            let result = async {
                let launch = self
                    .task_service
                    .re_execute_execution(execution.id.clone())
                    .await?;
                self.task_service
                    .start_execution(launch.execution.id.clone())
                    .await
            }
            .await;
            match result {
                Ok(start) if start.accepted => resumed += 1,
                Ok(_) => {}
                Err(ServiceError::Db(DbError::VersionConflict))
                | Err(ServiceError::Db(DbError::TaskVersionConflict { .. })) => {
                    tracing::debug!(
                        task_id = %task.id,
                        execution_id = %execution.id,
                        "automatic execution recovery lost a version race"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        task_id = %task.id,
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
    execution.work_unit_id.is_none()
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
    fn automatic_recovery_requires_exact_cancelled_task_execution_and_authorized_stop_reason() {
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
            agent_session_id: None,
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
            work_unit_id: None,
            work_unit_version: None,
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
        execution.work_unit_id = Some("work-unit".to_owned());
        assert!(!is_exact_auto_recovery_candidate(&execution));
    }
}
