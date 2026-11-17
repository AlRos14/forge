//! Node RT (phase 2): the `triaged` receipt of an `external_feedback`
//! coordination request.
//!
//! Unlike validation/review triage, this request is created while the Task is
//! `Merging` (an open PR admission). The exact coordinator authority check is
//! therefore the validation one with `merging` admitted (only `cancelled` and
//! `done` end the right to triage). The per-item rows (V173 triage + V175
//! disposition) are written by `apply_external_feedback_triage` before this
//! receipt; the V172 `triaged` guard refuses the state change unless every set
//! item has its triage row for this exact Execution.

use super::*;

fn authority_sql() -> String {
    validation_triage::AUTHORITY_SQL.replace(
        "lifecycle.state NOT IN ('cancelled','done','merging')",
        "lifecycle.state NOT IN ('cancelled','done')",
    )
}

impl TaskReworkService {
    /// `running|uncertain -> triaged` for the exact coordinator Execution.
    /// Idempotent: an already triaged request of the same Execution is a
    /// replay. A paused Task or Project batch is `coordination.paused` (the
    /// completed response is retained and retried after resume).
    pub(crate) async fn mark_external_feedback_triaged(
        &self,
        execution_id: &str,
        request_id: &str,
    ) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_coordination_request
             WHERE id=? AND kind='external_feedback' AND state='triaged' AND execution_id=?)",
        )
        .bind(request_id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if already {
            tx.commit().await?;
            return Ok(());
        }
        let row = sqlx::query(&authority_sql())
            .bind(execution_id)
            .bind(request_id)
            .bind("external_feedback")
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            let paused: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM task_coordination_request request
                    JOIN task_dispatch dispatch ON dispatch.task_id=request.task_id
                    WHERE request.id=? AND dispatch.state='paused'
                    UNION ALL
                    SELECT 1 FROM project_plan_batch_coordination_request scope
                    JOIN project_plan_batch batch ON batch.id=scope.batch_id
                    WHERE scope.request_id=? AND batch.state='paused')",
            )
            .bind(request_id)
            .bind(request_id)
            .fetch_one(&mut *tx)
            .await?;
            if paused {
                return Err(ServiceError::Coordination {
                    code: "coordination.paused",
                    message: "ExternalFeedback triage waits until the same Task or Project batch is resumed"
                        .to_owned(),
                });
            }
            return Err(ServiceError::invalid_operation(
                "The exact external feedback request, current coordinator, wake, team or Task authority is no longer valid",
            ));
        };
        let task_id: String = row.try_get("task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let actor_id: String = row.try_get("actor_id")?;
        let now = now_rfc3339();
        let changed = sqlx::query(
            "UPDATE task_coordination_request
             SET state='triaged',version=version+1,updated_at=?,completed_at=?
             WHERE id=? AND task_id=? AND kind='external_feedback'
               AND state IN ('running','uncertain') AND execution_id=? AND authorization_version=?",
        )
        .bind(&now)
        .bind(&now)
        .bind(request_id)
        .bind(&task_id)
        .bind(execution_id)
        .bind(authorization_version)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(ServiceError::conflict(
                "ExternalFeedback request changed before its triage receipt was committed",
            ));
        }
        let state_event = DomainEventRepo::append_event_in_tx(
            &*self.db,
            &mut tx,
            &CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.coordination_request_state_changed".to_owned(),
                entity_type: "task_coordination_request".to_owned(),
                entity_id: request_id.to_owned(),
                actor_type: "agent".to_owned(),
                actor_id: Some(actor_id),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: task_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("external-feedback-triaged:{request_id}")),
                payload_json: serde_json::json!({
                    "task_id": task_id,
                    "request_id": request_id,
                    "state": "triaged",
                })
                .to_string(),
                created_at: now,
            },
        )
        .await?;
        tx.commit().await?;
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &state_event,
        );
        Ok(())
    }
}

#[cfg(test)]
mod rt_tests {
    use super::*;

    #[test]
    fn rt_external_feedback_authority_admits_merging_and_nothing_else() {
        let sql = authority_sql();
        assert!(sql.contains("lifecycle.state NOT IN ('cancelled','done')"));
        assert!(!sql.contains("'cancelled','done','merging'"));
        // The validation authority still fences a merging Task.
        assert!(validation_triage::AUTHORITY_SQL.contains("'cancelled','done','merging'"));
    }
}
