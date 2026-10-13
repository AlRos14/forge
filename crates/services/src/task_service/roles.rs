use super::*;
use sqlx::{Row, Sqlite, Transaction};

impl TaskService {
    pub async fn on_agent_deleted(&self, agent_id: &str) -> Result<()> {
        validate_required("agent_id", agent_id)?;
        let mut transaction = self.db.pool().begin().await?;
        let events = self
            .on_agent_deleted_in_tx(&mut transaction, agent_id)
            .await?;
        transaction.commit().await?;
        self.publish_role_sweep_events(events);
        Ok(())
    }

    pub(crate) async fn on_agent_deleted_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        agent_id: &str,
    ) -> Result<Vec<RoleSweepEvent>> {
        validate_required("agent_id", agent_id)?;
        let rows = sqlx::query(
            "SELECT DISTINCT t.id AS task_id, t.project_id
             FROM task t
             JOIN task_role tr ON tr.task_id = t.id
             JOIN role_membership rm ON rm.task_role_id = tr.id
             WHERE rm.actor_kind = 'agent' AND rm.actor_id = ?
               AND rm.status IN ('active', 'suspended')
             ORDER BY t.id",
        )
        .bind(agent_id)
        .fetch_all(&mut **transaction)
        .await?;
        let events = rows
            .iter()
            .map(|row| {
                Ok(RoleSweepEvent {
                    task_id: row.try_get("task_id")?,
                    project_id: row.try_get("project_id")?,
                })
            })
            .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
        sqlx::query(
            "UPDATE role_membership
             SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
             WHERE actor_kind = 'agent' AND actor_id = ? AND status IN ('active', 'suspended')",
        )
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .bind(agent_id)
        .execute(&mut **transaction)
        .await?;

        Ok(events)
    }

    pub(crate) fn publish_role_sweep_events(&self, events: Vec<RoleSweepEvent>) {
        for event in events {
            self.publish(ForgeEvent {
                event_type: "task.updated".to_owned(),
                entity_id: event.task_id,
                timestamp: event_timestamp(),
                context: EventContext::TaskUpdated {
                    project_id: event.project_id,
                },
            });
        }
    }

    pub(super) async fn cancel_active_execution(
        &self,
        execution: &Execution,
        reason: &str,
        stop_reason: db::StopReason,
        actor: &api_types::Actor,
        resume_policy: db::ResumePolicy,
    ) -> Result<()> {
        let reconciliation_reason = match &stop_reason {
            db::StopReason::RoleReassigned => "stopped because task moved".to_owned(),
            _ => reason.to_owned(),
        };
        let preserve_resume_context = resume_policy == db::ResumePolicy::Manual;
        // An explicit generic HarnessSession remains resumable even when the
        // cancellation was not manual. Preserve the execution snapshot so a
        // later recovery can reconstruct the same harness invocation.
        let preserve_harness_context =
            preserve_resume_context || execution.harness_session_id.is_some();
        if self.daemon_connections.is_some() || self.task_executor.is_some() {
            if let Err(error) = self.cancel_execution_with_provider(execution, reason).await {
                if matches!(error, ServiceError::DaemonUnavailable { .. }) {
                    return Err(error);
                }
                tracing::warn!(
                    execution_id = %execution.id,
                    %error,
                    "executor cancellation failed; marking execution cancelled"
                );
            }
        }
        let now = now_rfc3339();
        let event =
            super::execution_status_domain_event(execution, &ExecutionStatus::Cancelled, &now);
        let (_, committed_event) = ExecutionRepo::update_with_event(
            &*self.db,
            db::UpdateExecution {
                id: execution.id.clone(),
                status: Some(ExecutionStatus::Cancelled),
                stop_reason: Some(Some(stop_reason)),
                stopped_by: Some(Some(actor.display())),
                resume_policy: Some(Some(resume_policy)),
                stopped_at: Some(Some(now.clone())),
                // Manual user stops retain the exact HarnessSession and executor
                // snapshot so the task façade can resume the same worker thread.
                external_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: Some(Some(reason.to_owned())),
                executor_config_snapshot_json: (!preserve_harness_context).then_some(None),
                updated_at: now,
            },
            event,
        )
        .await
        .map_err(|error| ServiceError::invalid_operation(format!("cancel failed: {error}")))?;
        self.publish_committed_domain_event(&committed_event);
        self.revoke_active_workspace_lease_for_execution(&execution.task_id, &execution.id)
            .await;
        self.publish(ForgeEvent {
            event_type: "task.execution_cancelled".to_owned(),
            entity_id: execution.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskExecutionCancelled {
                task_id: execution.task_id.clone(),
                execution_id: execution.id.clone(),
                reason: reason.to_owned(),
            },
        });
        self.publish(ForgeEvent {
            event_type: "reconciliation.event".to_owned(),
            entity_id: execution.task_id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::ReconciliationEvent {
                task_id: Some(execution.task_id.clone()),
                execution_id: Some(execution.id.clone()),
                reason: reconciliation_reason,
            },
        });
        Ok(())
    }
}

pub(crate) struct RoleSweepEvent {
    pub(crate) task_id: String,
    pub(crate) project_id: String,
}
