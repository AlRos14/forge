use super::*;
use crate::{
    now_rfc3339, AgentExecutionStats, ExecutionPurpose, ExecutionStatus,
    PendingProviderStartFailure, RunningExecutionFailure,
};

fn map_review_execution_subject(row: SqliteRow) -> Result<ReviewExecutionSubject> {
    Ok(ReviewExecutionSubject {
        execution_id: row.try_get("execution_id")?,
        task_id: row.try_get("task_id")?,
        workspace_id: row.try_get("workspace_id")?,
        base_commit_sha: row.try_get("base_commit_sha")?,
        head_commit_sha: row.try_get("head_commit_sha")?,
        workspace_snapshot_digest: row.try_get("workspace_snapshot_digest")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn insert_review_execution_subject_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    subject: &CreateReviewExecutionSubject,
) -> Result<ReviewExecutionSubject> {
    sqlx::query(
        "INSERT INTO review_execution_subject (
            execution_id, task_id, workspace_id, base_commit_sha, head_commit_sha,
            workspace_snapshot_digest, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&subject.execution_id)
    .bind(&subject.task_id)
    .bind(&subject.workspace_id)
    .bind(&subject.base_commit_sha)
    .bind(&subject.head_commit_sha)
    .bind(&subject.workspace_snapshot_digest)
    .bind(&subject.created_at)
    .execute(&mut **tx)
    .await?;
    let row = sqlx::query("SELECT * FROM review_execution_subject WHERE execution_id = ?")
        .bind(&subject.execution_id)
        .fetch_one(&mut **tx)
        .await?;
    map_review_execution_subject(row)
}

fn review_subject_identity_matches(
    current: &ReviewExecutionSubject,
    proposed: &CreateReviewExecutionSubject,
) -> bool {
    current.execution_id == proposed.execution_id
        && current.task_id == proposed.task_id
        && current.workspace_id == proposed.workspace_id
        && current.base_commit_sha == proposed.base_commit_sha
        && current.head_commit_sha == proposed.head_commit_sha
        && current.workspace_snapshot_digest == proposed.workspace_snapshot_digest
}

fn provider_start_event_matches_execution(
    event: &CreateDomainEvent,
    execution: &Execution,
    event_type: &str,
) -> bool {
    let actor = execution.actor_ref();
    let expected_dedupe_key = format!("{event_type}:{}", execution.id);
    event.event_type == event_type
        && event.entity_type == "execution"
        && event.entity_id == execution.id
        && event.actor_type
            == actor
                .as_ref()
                .map(|actor| actor.kind().to_string())
                .unwrap_or_default()
        && event.actor_id.as_deref() == actor.as_ref().map(|actor| actor.id())
        && event.scope_type == "task"
        && event.scope_id == execution.task_id
        && event.correlation_id == execution.id
        && event.causation_id == execution.parent_execution_id
        && event.causation_depth == i64::from(execution.parent_execution_id.is_some())
        && event.dedupe_key.as_deref() == Some(expected_dedupe_key.as_str())
}

fn provider_start_event_semantics_match(
    row: &SqliteRow,
    input: &CreateDomainEvent,
) -> Result<bool> {
    Ok(row.try_get::<String, _>("event_type")? == input.event_type
        && row.try_get::<String, _>("entity_type")? == input.entity_type
        && row.try_get::<String, _>("entity_id")? == input.entity_id
        && row.try_get::<String, _>("actor_type")? == input.actor_type
        && row.try_get::<Option<String>, _>("actor_id")? == input.actor_id
        && row.try_get::<String, _>("scope_type")? == input.scope_type
        && row.try_get::<String, _>("scope_id")? == input.scope_id
        && row.try_get::<String, _>("correlation_id")? == input.correlation_id
        && row.try_get::<Option<String>, _>("causation_id")? == input.causation_id
        && row.try_get::<i64, _>("causation_depth")? == input.causation_depth
        && row.try_get::<Option<String>, _>("dedupe_key")? == input.dedupe_key
        && row.try_get::<String, _>("payload_json")? == input.payload_json)
}

async fn create_human_review_execution(
    db: &SqliteDb,
    input: CreateExecution,
    subject: Option<CreateReviewExecutionSubject>,
    event: CreateDomainEvent,
) -> Result<(Execution, Option<ReviewExecutionSubject>, DomainEvent)> {
    let human_actor_id = match input.actor_ref.as_ref() {
        Some(ActorRef::Human(user_id)) => Some(user_id.as_str()),
        _ => None,
    };
    let subject_matches = match subject.as_ref() {
        Some(subject) => {
            input.workspace_id.as_deref() == Some(subject.workspace_id.as_str())
                && input.before_sha.as_deref() == Some(subject.base_commit_sha.as_str())
                && input.after_sha.as_deref() == Some(subject.head_commit_sha.as_str())
                && input.id == subject.execution_id
                && input.task_id == subject.task_id
        }
        None => {
            input.workspace_id.is_none() && input.before_sha.is_none() && input.after_sha.is_none()
        }
    };
    if input.status != ExecutionStatus::Running
        || input.role != "reviewer"
        || input.purpose != Some(ExecutionPurpose::Review)
        || human_actor_id.is_none()
        || !subject_matches
        || event.event_type != "execution.started"
        || event.entity_type != "execution"
        || event.entity_id != input.id
        || event.scope_type != "task"
        || event.scope_id != input.task_id
        || event.actor_type != "human"
        || event.actor_id.as_deref() != human_actor_id
    {
        return Err(DbError::Check(
            "Human Review Execution, optional exact subject, and start event must share one identity"
                .to_owned(),
        ));
    }

    let mut tx = db.pool.begin().await?;
    if !human_has_current_role_authority_in_tx(
        &mut tx,
        &input.task_id,
        "reviewer",
        human_actor_id.expect("shape validation requires a Human Actor"),
    )
    .await?
    {
        return Err(DbError::Check(
            "new Human Review Execution requires current authoritative reviewer membership"
                .to_owned(),
        ));
    }
    let execution = SqliteDb::create_execution_in_tx(&mut tx, &input, None).await?;
    let subject = match subject.as_ref() {
        Some(subject) => Some(insert_review_execution_subject_in_tx(&mut tx, subject).await?),
        None => None,
    };
    let event = DomainEventRepo::append_event_in_tx(db, &mut tx, &event).await?;
    tx.commit().await?;
    Ok((execution, subject, event))
}

/// Recheck current Human role authority inside the same SQLite transaction as
/// an authoritative Review write. Only a current target TaskRole membership
/// can authorize it.
pub(super) async fn human_has_current_role_authority_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    role: &str,
    user_id: &str,
) -> Result<bool> {
    let Some(canonical_role) = crate::canonical_task_role_name(role) else {
        return Ok(false);
    };
    let authorized: i64 = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
            FROM task_role AS role
            JOIN role_membership AS membership
              ON membership.task_role_id = role.id
            WHERE role.task_id = ? AND role.role = ?
              AND membership.actor_kind = 'human'
              AND membership.actor_id = ?
              AND membership.status = 'active'
         )",
    )
    .bind(task_id)
    .bind(&canonical_role)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(authorized != 0)
}

#[async_trait]
impl ExecutionRepo for SqliteDb {
    async fn create(&self, input: CreateExecution) -> Result<Execution> {
        let mut transaction = self.pool.begin().await?;
        let execution = Self::create_execution_in_tx(&mut transaction, &input, None).await?;
        transaction.commit().await?;
        Ok(execution)
    }

    async fn create_with_event(
        &self,
        input: CreateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)> {
        let mut transaction = self.pool.begin().await?;
        let execution = Self::create_execution_in_tx(&mut transaction, &input, None).await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut transaction, &event).await?;
        transaction.commit().await?;
        Ok((execution, event))
    }

    async fn create_human_review_execution_with_subject(
        &self,
        input: CreateExecution,
        subject: CreateReviewExecutionSubject,
        event: CreateDomainEvent,
    ) -> Result<ReviewExecutionSubjectWrite> {
        let (execution, subject, event) =
            create_human_review_execution(self, input, Some(subject), event).await?;
        Ok(ReviewExecutionSubjectWrite {
            execution,
            subject: subject.expect("workspace-bound method supplies a subject"),
            event: Some(event),
        })
    }

    async fn create_human_review_execution_without_subject(
        &self,
        input: CreateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)> {
        let (execution, subject, event) =
            create_human_review_execution(self, input, None, event).await?;
        debug_assert!(subject.is_none());
        Ok((execution, event))
    }

    async fn freeze_review_execution_subject(
        &self,
        subject: CreateReviewExecutionSubject,
        updated_at: &str,
        event: CreateDomainEvent,
    ) -> Result<ReviewExecutionSubjectWrite> {
        if event.event_type != "execution.review_subject_frozen"
            || event.entity_type != "execution"
            || event.entity_id != subject.execution_id
            || event.scope_type != "task"
            || event.scope_id != subject.task_id
        {
            return Err(DbError::Check(
                "Review subject event must identify its exact Execution and Task".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(&subject.execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if execution.task_id != subject.task_id
            || execution.role != "reviewer"
            || execution.purpose != Some(ExecutionPurpose::Review)
            || execution.status != ExecutionStatus::Running
            || execution.workspace_id.as_deref() != Some(subject.workspace_id.as_str())
        {
            return Err(DbError::Check(
                "Review subject requires its exact Running reviewer Execution".to_owned(),
            ));
        }
        let actor = execution.actor_ref().ok_or_else(|| {
            DbError::Check("Review subject requires a persisted Human or Agent ActorRef".to_owned())
        })?;
        if event.actor_type != actor.kind().to_string()
            || event.actor_id.as_deref() != Some(actor.id())
        {
            return Err(DbError::Check(
                "Review subject event ActorRef must match its exact Execution".to_owned(),
            ));
        }

        let existing = sqlx::query("SELECT * FROM review_execution_subject WHERE execution_id = ?")
            .bind(&subject.execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_review_execution_subject)
            .transpose()?;
        if let Some(existing) = existing {
            if !review_subject_identity_matches(&existing, &subject)
                || execution.before_sha.as_deref() != Some(existing.base_commit_sha.as_str())
                || execution.after_sha.as_deref() != Some(existing.head_commit_sha.as_str())
            {
                return Err(DbError::Check(
                    "Review Execution subject is already frozen to a different identity".to_owned(),
                ));
            }
            tx.commit().await?;
            return Ok(ReviewExecutionSubjectWrite {
                execution,
                subject: existing,
                event: None,
            });
        }

        if execution
            .before_sha
            .as_deref()
            .is_some_and(|value| value != subject.base_commit_sha)
            || execution
                .after_sha
                .as_deref()
                .is_some_and(|value| value != subject.head_commit_sha)
        {
            return Err(DbError::Check(
                "Review Execution commit identity conflicts with its frozen subject".to_owned(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE execution SET before_sha = ?, after_sha = ?, updated_at = ?
             WHERE id = ? AND task_id = ? AND status = 'running'",
        )
        .bind(&subject.base_commit_sha)
        .bind(&subject.head_commit_sha)
        .bind(updated_at)
        .bind(&subject.execution_id)
        .bind(&subject.task_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(DbError::VersionConflict);
        }
        let row = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(&subject.execution_id)
            .fetch_one(&mut *tx)
            .await?;
        let execution = map_execution(row)?;
        let subject = insert_review_execution_subject_in_tx(&mut tx, &subject).await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        Ok(ReviewExecutionSubjectWrite {
            execution,
            subject,
            event: Some(event),
        })
    }

    async fn get_review_execution_subject(
        &self,
        execution_id: &str,
    ) -> Result<Option<ReviewExecutionSubject>> {
        sqlx::query("SELECT * FROM review_execution_subject WHERE execution_id = ?")
            .bind(execution_id)
            .fetch_optional(self.pool())
            .await?
            .map(map_review_execution_subject)
            .transpose()
    }

    async fn find_running_human_review_execution(
        &self,
        task_id: &str,
        user_id: &str,
        workspace_id: Option<&str>,
    ) -> Result<Option<Execution>> {
        sqlx::query(
            "SELECT * FROM execution
             WHERE task_id = ? AND role = 'reviewer' AND purpose = 'review'
               AND actor_kind = 'human' AND actor_id = ? AND status = 'running'
               AND workspace_id IS ?
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(user_id)
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?
        .map(map_execution)
        .transpose()
    }

    async fn create_with_artifact_inputs_and_event(
        &self,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)> {
        if input.status != ExecutionStatus::Running
            || event.event_type != "execution.started"
            || event.entity_type != "execution"
            || event.entity_id != input.id
            || event.scope_type != "task"
            || event.scope_id != input.task_id
        {
            return Err(DbError::Check(
                "Artifact inputs require the matching Running Execution start event".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        let execution = Self::create_execution_in_tx(&mut transaction, &input, None).await?;
        for artifact_id in artifact_input_ids {
            super::collaboration::pin_execution_artifact_input_in_tx(
                &mut transaction,
                &input.id,
                &artifact_id,
                &input.created_at,
            )
            .await?;
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut transaction, &event).await?;
        transaction.commit().await?;
        Ok((execution, event))
    }

    async fn create_orchestrator_execution(
        &self,
        input: CreateExecution,
        wake_id: &str,
        attempt_number: i64,
        lease_owner: &str,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)> {
        let Some(ActorRef::Agent(agent_id)) = input.actor_ref.as_ref() else {
            return Err(DbError::Check(
                "automatic orchestrator Execution requires an Agent ActorRef".to_owned(),
            ));
        };
        if input.role != "orchestrator"
            || input.purpose != Some(ExecutionPurpose::Orchestrate)
            || input.workspace_id.is_some()
            || event.event_type != "execution.started"
            || event.entity_type != "execution"
            || event.entity_id != input.id
            || event.scope_type != "task"
            || event.scope_id != input.task_id
            || event.actor_type != "agent"
            || event.actor_id.as_deref() != Some(agent_id.as_str())
        {
            return Err(DbError::Check(
                "orchestrator Execution must be workspace-free with exact role and purpose"
                    .to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let admitted: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM orchestrator_wake w
                JOIN orchestrator_wake_execution x ON x.wake_id = w.id
                WHERE w.id = ? AND w.task_id = ? AND w.actor_kind = 'agent'
                  AND w.actor_id = ? AND w.task_role_id = (
                      SELECT id FROM task_role WHERE task_id = w.task_id
                        AND role = 'orchestrator'
                  )
                  AND w.state = 'leased' AND w.lease_owner = ?
                  AND EXISTS (
                      SELECT 1 FROM role_membership rm
                      WHERE rm.task_role_id = w.task_role_id
                        AND rm.actor_kind = 'agent' AND rm.actor_id = w.actor_id
                        AND rm.status = 'active'
                  )
                  AND x.attempt_number = ? AND x.execution_id = ?
                  AND x.state = 'reserved'
            )",
        )
        .bind(wake_id)
        .bind(&input.task_id)
        .bind(agent_id)
        .bind(lease_owner)
        .bind(attempt_number)
        .bind(&input.id)
        .fetch_one(&mut *transaction)
        .await?;
        if admitted == 0 {
            return Err(DbError::VersionConflict);
        }
        let execution = Self::create_execution_in_tx(&mut transaction, &input, None).await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut transaction, &event).await?;
        transaction.commit().await?;
        Ok((execution, event))
    }

    async fn get_task_id(&self, id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT task_id FROM execution WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Execution>> {
        sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_execution)
            .transpose()
    }

    async fn has_historical_session_ambiguity(&self, execution_id: &str) -> Result<bool> {
        let marked: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1
                 FROM execution_session_migration_issue
                 WHERE execution_id = ?
                   AND issue_kind = 'historical_session_ambiguous'
             )",
        )
        .bind(execution_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(marked != 0)
    }

    async fn stats_by_agent(&self, agent_id: &str) -> Result<AgentExecutionStats> {
        let row = sqlx::query(
            "SELECT \
                COUNT(*) AS total_runs, \
                COALESCE(SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END), 0) AS completed_runs, \
                AVG(CASE \
                    WHEN status != 'running' \
                    THEN (JULIANDAY(updated_at) - JULIANDAY(created_at)) * 86400000 \
                    ELSE NULL \
                END) AS avg_duration_ms \
             FROM execution \
             WHERE agent_id = ?",
        )
        .bind(agent_id)
        .fetch_one(&self.pool)
        .await?;

        let total_runs: i64 = row.try_get("total_runs")?;
        let completed_runs: i64 = row.try_get("completed_runs")?;
        let avg_duration_ms = row
            .try_get::<Option<f64>, _>("avg_duration_ms")?
            .map(|duration| duration.round() as i64);
        let success_rate = if total_runs > 0 {
            Some(completed_runs as f64 / total_runs as f64)
        } else {
            None
        };

        Ok(AgentExecutionStats {
            total_runs,
            avg_duration_ms,
            success_rate,
        })
    }

    async fn list_by_task(&self, task_id: &str, page: PageRequest) -> Result<Page<Execution>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM execution WHERE task_id = ? ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(task_id)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_execution)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                    .bind(task_id)
                    .fetch_one(&self.pool)
                    .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn list_latest_executions_for_tasks(&self, task_ids: &[&str]) -> Result<Vec<Execution>> {
        if task_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut query = sqlx::QueryBuilder::<Sqlite>::new(
            "SELECT * FROM (
                SELECT execution.*,
                       ROW_NUMBER() OVER (
                           PARTITION BY task_id
                           ORDER BY created_at DESC, id DESC
                       ) AS rn
                FROM execution
                WHERE work_unit_id IS NULL AND task_id IN (",
        );
        let mut separated = query.separated(", ");
        for task_id in task_ids {
            separated.push_bind(*task_id);
        }
        separated.push_unseparated(
            ")
            ) ranked
            WHERE rn = 1
            ORDER BY task_id ASC",
        );
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(map_execution).collect()
    }

    async fn list_work_unit_auto_recovery_candidates(&self) -> Result<Vec<Execution>> {
        let rows = sqlx::query(
            "SELECT * FROM execution
             WHERE work_unit_id IS NOT NULL
               AND status = 'cancelled'
               AND resume_policy = 'auto'
               AND stop_reason IN ('crash_recovery', 'graceful_shutdown')
               AND NOT EXISTS (
                   SELECT 1 FROM execution child
                   WHERE child.parent_execution_id = execution.id
               )
             ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_execution).collect()
    }

    async fn list_pending_provider_starts(&self) -> Result<Vec<Execution>> {
        let rows = sqlx::query(
            "SELECT e.* FROM execution e
             JOIN domain_event pending
               ON pending.entity_type = 'execution'
              AND pending.entity_id = e.id
              AND pending.event_type = 'execution.provider_start_pending'
              AND pending.dedupe_key = 'execution.provider_start_pending:' || e.id
             WHERE e.work_unit_id IS NOT NULL
               AND e.actor_kind = 'agent'
               AND e.agent_id = e.actor_id
               AND e.status = 'running'
               AND NOT EXISTS (
                   SELECT 1 FROM domain_event attempt
                   WHERE attempt.entity_type = 'execution'
                     AND attempt.entity_id = e.id
                     AND attempt.event_type = 'execution.provider_start_attempted'
                     AND attempt.dedupe_key = 'execution.provider_start_attempted:' || e.id
               )
             ORDER BY pending.sequence ASC, e.id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_execution).collect()
    }

    async fn is_provider_start_pending(&self, execution_id: &str) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM execution e
                WHERE e.id = ? AND e.work_unit_id IS NOT NULL
                  AND e.actor_kind = 'agent' AND e.agent_id = e.actor_id
                  AND e.status = 'running'
                  AND EXISTS (
                      SELECT 1 FROM domain_event pending
                      WHERE pending.entity_type = 'execution'
                        AND pending.entity_id = e.id
                        AND pending.event_type = 'execution.provider_start_pending'
                        AND pending.dedupe_key = 'execution.provider_start_pending:' || e.id
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM domain_event attempt
                      WHERE attempt.entity_type = 'execution'
                        AND attempt.entity_id = e.id
                        AND attempt.event_type = 'execution.provider_start_attempted'
                        AND attempt.dedupe_key = 'execution.provider_start_attempted:' || e.id
                  )
            )",
        )
        .bind(execution_id)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn fail_pending_provider_start(
        &self,
        execution_id: &str,
        update: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<PendingProviderStartFailure> {
        if update.id != execution_id
            || update.status != Some(ExecutionStatus::Failed)
            || event.event_type != "execution.failed"
            || event.entity_type != "execution"
            || event.entity_id != execution_id
        {
            return Err(DbError::Check(
                "pending provider Start failure must target its exact Execution".to_owned(),
            ));
        }

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if execution.status != ExecutionStatus::Running {
            tx.commit().await?;
            return Ok(PendingProviderStartFailure::NotRunning(execution));
        }

        let attempt_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM domain_event
                WHERE entity_type = 'execution'
                  AND entity_id = ?
                  AND event_type = 'execution.provider_start_attempted'
                  AND dedupe_key = 'execution.provider_start_attempted:' || ?
            )",
        )
        .bind(execution_id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if attempt_exists {
            let rejected_outcome_is_exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM domain_event AS outcome
                    WHERE outcome.dedupe_key = 'execution.provider_start_rejected:' || ?
                      AND outcome.event_type = 'execution.provider_start_rejected'
                      AND outcome.entity_type = 'execution'
                      AND outcome.entity_id = ?
                      AND outcome.actor_type = ?
                      AND outcome.actor_id IS ?
                      AND outcome.scope_type = 'task'
                      AND outcome.scope_id = ?
                      AND outcome.correlation_id = ?
                      AND outcome.causation_id IS ?
                      AND outcome.causation_depth = ?
                      AND NOT EXISTS (
                          SELECT 1 FROM domain_event AS competing
                          WHERE competing.entity_type = 'execution'
                            AND competing.entity_id = outcome.entity_id
                            AND competing.event_type IN (
                                'execution.provider_start_accepted',
                                'execution.provider_start_uncertain'
                            )
                      )
                )",
            )
            .bind(execution_id)
            .bind(execution_id)
            .bind(
                execution
                    .actor_ref()
                    .as_ref()
                    .map(|actor| actor.kind().to_string())
                    .unwrap_or_default(),
            )
            .bind(
                execution
                    .actor_ref()
                    .as_ref()
                    .map(|actor| actor.id().to_owned()),
            )
            .bind(&execution.task_id)
            .bind(&execution.id)
            .bind(&execution.parent_execution_id)
            .bind(i64::from(execution.parent_execution_id.is_some()))
            .fetch_one(&mut *tx)
            .await?;
            if !rejected_outcome_is_exact {
                tx.commit().await?;
                return Ok(PendingProviderStartFailure::Attempted(execution));
            }
        }

        let pending_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM domain_event
                WHERE entity_type = 'execution'
                  AND entity_id = ?
                  AND event_type = 'execution.provider_start_pending'
                  AND dedupe_key = 'execution.provider_start_pending:' || ?
            )",
        )
        .bind(execution_id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !pending_exists {
            tx.commit().await?;
            return Ok(PendingProviderStartFailure::NotPending(execution));
        }

        let updated = update_execution_in_tx(&mut tx, update).await?;
        let committed_event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        Ok(PendingProviderStartFailure::Resolved(
            Box::new(updated),
            committed_event,
        ))
    }

    async fn fail_running_unless_provider_start_pending(
        &self,
        execution_id: &str,
        update: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<RunningExecutionFailure> {
        if update.id != execution_id
            || update.status != Some(ExecutionStatus::Failed)
            || event.event_type != "execution.failed"
            || event.entity_type != "execution"
            || event.entity_id != execution_id
        {
            return Err(DbError::Check(
                "running Execution failure must target its exact Execution".to_owned(),
            ));
        }

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if execution.status != ExecutionStatus::Running {
            tx.commit().await?;
            return Ok(RunningExecutionFailure::NotRunning(execution));
        }

        let provider_start_pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM domain_event pending
                WHERE pending.entity_type = 'execution'
                  AND pending.entity_id = ?
                  AND pending.event_type = 'execution.provider_start_pending'
                  AND pending.dedupe_key = 'execution.provider_start_pending:' || ?
                  AND NOT EXISTS (
                      SELECT 1 FROM domain_event attempt
                      WHERE attempt.entity_type = 'execution'
                        AND attempt.entity_id = pending.entity_id
                        AND attempt.event_type = 'execution.provider_start_attempted'
                        AND attempt.dedupe_key = 'execution.provider_start_attempted:' || pending.entity_id
                  )
            )",
        )
        .bind(execution_id)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if provider_start_pending {
            tx.commit().await?;
            return Ok(RunningExecutionFailure::Pending(execution));
        }

        let updated = update_execution_in_tx(&mut tx, update).await?;
        let committed_event = DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?;
        tx.commit().await?;
        Ok(RunningExecutionFailure::Failed(
            Box::new(updated),
            committed_event,
        ))
    }

    async fn claim_provider_start(
        &self,
        execution_id: &str,
        attempt_event: CreateDomainEvent,
    ) -> Result<ProviderStartClaim> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if execution.status != ExecutionStatus::Running
            || !matches!(execution.actor_ref(), Some(ActorRef::Agent(_)))
            || execution.agent_id.as_deref() != execution.actor_ref().as_ref().map(ActorRef::id)
            || (execution.role == "orchestrator")
                != (execution.purpose == Some(ExecutionPurpose::Orchestrate))
            || !provider_start_event_matches_execution(
                &attempt_event,
                &execution,
                "execution.provider_start_attempted",
            )
        {
            return Err(DbError::Check(
                "provider start claim requires the exact Running Agent WorkUnit Execution"
                    .to_owned(),
            ));
        }

        let attempt_key = format!("execution.provider_start_attempted:{execution_id}");
        if let Some(existing_attempt) =
            sqlx::query("SELECT * FROM domain_event WHERE dedupe_key = ?")
                .bind(&attempt_key)
                .fetch_optional(&mut *tx)
                .await?
        {
            if !provider_start_event_semantics_match(&existing_attempt, &attempt_event)? {
                return Err(DbError::Check(
                    "provider start attempt receipt conflicts with its Execution".to_owned(),
                ));
            }
            let mut disposition = None;
            for (event_type, claim) in [
                (
                    "execution.provider_start_accepted",
                    ProviderStartClaim::Accepted,
                ),
                (
                    "execution.provider_start_rejected",
                    ProviderStartClaim::Rejected,
                ),
                (
                    "execution.provider_start_uncertain",
                    ProviderStartClaim::Uncertain,
                ),
            ] {
                let outcome_key = format!("{event_type}:{execution_id}");
                if let Some(outcome) = sqlx::query(
                    "SELECT event_type, entity_type, entity_id, actor_type, actor_id,
                            scope_type, scope_id, correlation_id, causation_id,
                            causation_depth, dedupe_key
                     FROM domain_event WHERE dedupe_key = ?",
                )
                .bind(&outcome_key)
                .fetch_optional(&mut *tx)
                .await?
                {
                    let actor = execution.actor_ref();
                    let identity_matches = outcome.try_get::<String, _>("event_type")?
                        == event_type
                        && outcome.try_get::<String, _>("entity_type")? == "execution"
                        && outcome.try_get::<String, _>("entity_id")? == execution.id
                        && outcome.try_get::<String, _>("actor_type")?
                            == actor
                                .as_ref()
                                .map(|actor| actor.kind().to_string())
                                .unwrap_or_default()
                        && outcome.try_get::<Option<String>, _>("actor_id")?
                            == actor.as_ref().map(|actor| actor.id().to_owned())
                        && outcome.try_get::<String, _>("scope_type")? == "task"
                        && outcome.try_get::<String, _>("scope_id")? == execution.task_id
                        && outcome.try_get::<String, _>("correlation_id")? == execution.id
                        && outcome.try_get::<Option<String>, _>("causation_id")?
                            == execution.parent_execution_id
                        && outcome.try_get::<i64, _>("causation_depth")?
                            == i64::from(execution.parent_execution_id.is_some())
                        && outcome.try_get::<Option<String>, _>("dedupe_key")? == Some(outcome_key);
                    if !identity_matches || disposition.is_some() {
                        return Err(DbError::Check(
                            "provider start outcome receipt conflicts with its Execution"
                                .to_owned(),
                        ));
                    }
                    disposition = Some(claim);
                }
            }
            tx.commit().await?;
            return Ok(disposition.unwrap_or(ProviderStartClaim::Uncertain));
        }

        let task_is_cancelled: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_lifecycle
                WHERE task_id = ? AND state = 'cancelled'
            )",
        )
        .bind(&execution.task_id)
        .fetch_one(&mut *tx)
        .await?;
        if task_is_cancelled {
            tx.commit().await?;
            return Ok(ProviderStartClaim::TaskCancelled);
        }

        let is_orchestrator = execution.role == "orchestrator"
            && execution.purpose == Some(ExecutionPurpose::Orchestrate);
        if is_orchestrator {
            if execution.work_unit_id.is_some() || execution.workspace_id.is_some() {
                tx.commit().await?;
                return Ok(ProviderStartClaim::AuthorityLost);
            }
            let actor = execution.actor_ref().ok_or_else(|| {
                DbError::Check("Orchestrator claim has no exact Agent Actor".to_owned())
            })?;
            let actor_kind = actor.kind().to_string();
            let actor_id = actor.id().to_owned();
            // Orchestrator has no WorkUnit. Its exact durable Wake attempt is
            // the start authority, so bind the Wake, TaskRole snapshot,
            // membership, lease, and Execution under this same write lock.
            let wake_authority_is_current: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1
                    FROM orchestrator_wake AS wake
                    JOIN orchestrator_wake_execution AS attempt
                      ON attempt.wake_id = wake.id
                     AND attempt.execution_id = ?
                     AND attempt.attempt_number = wake.current_attempt
                    JOIN execution AS current_execution
                      ON current_execution.id = attempt.execution_id
                    JOIN task AS current_task
                      ON current_task.id = wake.task_id
                    JOIN agent_current AS current_agent
                      ON current_agent.id = wake.actor_id
                    JOIN task_role AS role
                      ON role.id = wake.task_role_id
                    JOIN role_membership AS member
                      ON member.task_role_id = wake.task_role_id
                     AND member.actor_kind = wake.actor_kind
                     AND member.actor_id = wake.actor_id
                     AND member.status = 'active'
                    WHERE current_execution.id = ?
                      AND current_execution.task_id = wake.task_id
                      AND current_execution.role = 'orchestrator'
                      AND current_execution.purpose = 'orchestrate'
                      AND current_execution.actor_kind = ?
                      AND current_execution.actor_id = ?
                      AND current_execution.agent_id = wake.actor_id
                      AND current_execution.work_unit_id IS NULL
                      AND current_execution.workspace_id IS NULL
                      AND current_task.deleted_at IS NULL
                      AND current_agent.paused = 0
                      AND wake.actor_kind = ?
                      AND wake.actor_id = ?
                      AND wake.state = 'leased'
                      AND wake.lease_owner IS NOT NULL
                      AND trim(wake.lease_owner) <> ''
                      AND wake.lease_until > ?
                      AND wake.current_attempt = attempt.attempt_number
                      AND wake.attempt_count >= attempt.attempt_number
                      AND attempt.state = 'start_requested'
                      AND role.task_id = wake.task_id
                      AND role.role = 'orchestrator'
                      AND role.version = wake.task_role_version
                      AND role.policy_json = wake.task_role_policy_json
                      AND role.coordination_mode IS wake.coordination_mode
                      AND json_valid(role.policy_json)
                      AND json_type(role.policy_json) = 'object'
                      AND COALESCE(json_extract(role.policy_json, '$.schema_version'), 1) = 1
                      AND COALESCE(json_extract(role.policy_json, '$.automatic_orchestration'), 1) = 1
                      AND COALESCE(json_extract(role.policy_json, '$.max_actions_per_execution'), 16)
                            BETWEEN 0 AND 16
                      AND COALESCE(json_extract(role.policy_json, '$.max_work_unit_creations_per_execution'), 4)
                            BETWEEN 0 AND 4
                      AND (
                            json_type(role.policy_json, '$.allowed_actions') IS NULL
                            OR (
                                json_type(role.policy_json, '$.allowed_actions') = 'array'
                                AND NOT EXISTS (
                                    SELECT 1 FROM json_each(role.policy_json, '$.allowed_actions') AS action
                                    WHERE action.type <> 'text'
                                       OR action.value NOT IN ('message', 'handoff', 'create_work_unit', 'proposal')
                                )
                                AND (SELECT COUNT(*) FROM json_each(role.policy_json, '$.allowed_actions'))
                                    = (SELECT COUNT(DISTINCT action.value)
                                       FROM json_each(role.policy_json, '$.allowed_actions') AS action)
                            )
                      )
                      AND json_valid(current_execution.executor_config_snapshot_json)
                      AND json_extract(current_execution.executor_config_snapshot_json,
                                       '$.pr6_orchestrator_wake.wake_id') = wake.id
                      AND json_extract(current_execution.executor_config_snapshot_json,
                                       '$.pr6_orchestrator_wake.task_role_version') = wake.task_role_version
                      AND json_extract(current_execution.executor_config_snapshot_json,
                                       '$.pr6_orchestrator_wake.task_role_policy_json') = wake.task_role_policy_json
                )",
            )
            .bind(&execution.id)
            .bind(&execution.id)
            .bind(&actor_kind)
            .bind(&actor_id)
            .bind(&actor_kind)
            .bind(&actor_id)
            .bind(crate::now_rfc3339())
            .fetch_one(&mut *tx)
            .await?;
            if !wake_authority_is_current {
                tx.commit().await?;
                return Ok(ProviderStartClaim::AuthorityLost);
            }
        }

        let agent_id = execution.agent_id.as_deref().ok_or(DbError::NotFound)?;
        if !super::daemon::execution_daemon_authorized_in_tx(
            &mut tx,
            agent_id,
            execution.executor_config_snapshot_json.as_deref(),
        )
        .await?
        {
            tx.commit().await?;
            return Ok(ProviderStartClaim::AuthorityLost);
        }

        let paused: Option<bool> =
            sqlx::query_scalar("SELECT paused FROM agent_current WHERE id = ?")
                .bind(agent_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(paused) = paused else {
            return Err(DbError::NotFound);
        };
        if paused {
            return Err(DbError::AgentPaused {
                agent_id: agent_id.to_owned(),
            });
        }

        if !is_orchestrator && execution.workspace_id.is_some() && execution.work_unit_id.is_none()
        {
            tx.commit().await?;
            return Ok(ProviderStartClaim::AuthorityLost);
        }

        if let Some(work_unit_id) = execution.work_unit_id.as_deref() {
            let Some(work_unit_version) = execution.work_unit_version else {
                tx.commit().await?;
                return Ok(ProviderStartClaim::AuthorityLost);
            };
            let actor = execution.actor_ref().ok_or_else(|| {
                DbError::Check("provider start claim has no exact WorkUnit Actor".to_owned())
            })?;
            let actor_kind = actor.kind().to_string();
            let actor_id = actor.id().to_owned();
            // Authority may change after service preflight and while provider
            // parameters are prepared. Recheck the same exact WorkUnit and
            // TaskRole bindings under the claim's write lock.
            let work_unit_authority_is_current: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM work_unit w
                    JOIN task_role tr ON tr.task_id = w.task_id AND tr.role = w.role
                    JOIN role_membership rm ON rm.task_role_id = tr.id
                    WHERE w.id = ? AND w.task_id = ?
                      AND w.status = 'open' AND w.version = ? AND w.role = ?
                      AND w.assigned_actor_kind = ? AND w.assigned_actor_id = ?
                      AND rm.actor_kind = ? AND rm.actor_id = ? AND rm.status = 'active'
                      AND ((w.requires_integration = 1 AND ? IS NOT NULL)
                           OR (w.requires_integration = 0 AND ? IS NULL))
                )",
            )
            .bind(work_unit_id)
            .bind(&execution.task_id)
            .bind(work_unit_version)
            .bind(&execution.role)
            .bind(&actor_kind)
            .bind(&actor_id)
            .bind(&actor_kind)
            .bind(&actor_id)
            .bind(execution.workspace_id.as_deref())
            .bind(execution.workspace_id.as_deref())
            .fetch_one(&mut *tx)
            .await?;
            if !work_unit_authority_is_current {
                tx.commit().await?;
                return Ok(ProviderStartClaim::AuthorityLost);
            }
        }

        if let (Some(work_unit_id), Some(workspace_id)) = (
            execution.work_unit_id.as_deref(),
            execution.workspace_id.as_deref(),
        ) {
            let actor_id = execution
                .actor_ref()
                .map(|actor| actor.id().to_owned())
                .ok_or_else(|| {
                    DbError::Check("provider start claim has no exact Workspace Actor".to_owned())
                })?;
            let lease_is_current: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM workspace_lease
                    WHERE task_id = ? AND work_unit_id = ? AND workspace_id = ?
                      AND execution_id = ? AND operation_idempotency_key = ?
                      AND role = CASE WHEN trim(?) = 'reviewer' THEN 'reviewer' ELSE 'worker' END
                      AND assigned_principal_type = 'agent' AND assigned_principal_id = ?
                      AND status = 'active' AND expires_at > ?
                )",
            )
            .bind(&execution.task_id)
            .bind(work_unit_id)
            .bind(workspace_id)
            .bind(&execution.id)
            .bind(&execution.id)
            .bind(&execution.role)
            .bind(&actor_id)
            .bind(crate::now_rfc3339())
            .fetch_one(&mut *tx)
            .await?;
            if !lease_is_current {
                tx.commit().await?;
                return Ok(ProviderStartClaim::AuthorityLost);
            }
        }

        DomainEventRepo::append_event_in_tx(self, &mut tx, &attempt_event).await?;
        tx.commit().await?;
        Ok(ProviderStartClaim::Claimed)
    }

    async fn record_provider_start_outcome(
        &self,
        execution_id: &str,
        outcome_event: CreateDomainEvent,
    ) -> Result<DomainEvent> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(execution_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        let allowed = matches!(
            outcome_event.event_type.as_str(),
            "execution.provider_start_accepted"
                | "execution.provider_start_rejected"
                | "execution.provider_start_uncertain"
        );
        if !allowed
            || !provider_start_event_matches_execution(
                &outcome_event,
                &execution,
                &outcome_event.event_type,
            )
        {
            return Err(DbError::Check(
                "provider start outcome requires its exact Execution identity".to_owned(),
            ));
        }
        let attempt_key = format!("execution.provider_start_attempted:{execution_id}");
        let has_attempt: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM domain_event WHERE dedupe_key = ?
                 AND event_type = 'execution.provider_start_attempted'
                 AND entity_type = 'execution' AND entity_id = ?)",
        )
        .bind(&attempt_key)
        .bind(execution_id)
        .fetch_one(&mut *tx)
        .await?;
        if !has_attempt {
            return Err(DbError::Check(
                "provider start outcome has no durable attempt receipt".to_owned(),
            ));
        }
        for event_type in [
            "execution.provider_start_accepted",
            "execution.provider_start_rejected",
            "execution.provider_start_uncertain",
        ] {
            let outcome_key = format!("{event_type}:{execution_id}");
            if event_type != outcome_event.event_type
                && sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM domain_event WHERE dedupe_key = ?)",
                )
                .bind(outcome_key)
                .fetch_one(&mut *tx)
                .await?
            {
                return Err(DbError::VersionConflict);
            }
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &outcome_event).await?;
        tx.commit().await?;
        Ok(event)
    }

    async fn list_by_task_and_role(
        &self,
        task_id: &str,
        role: &str,
        page: PageRequest,
    ) -> Result<Page<Execution>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM execution WHERE task_id = ? AND role = ? ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(task_id)
            .bind(role)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_execution)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = ?",
                )
                .bind(task_id)
                .bind(role)
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn count_by_task_and_role(&self, task_id: &str, role: &str) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = ?",
        )
        .bind(task_id)
        .bind(role)
        .fetch_one(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn update(&self, input: UpdateExecution) -> Result<Execution> {
        let mut transaction = self.pool.begin().await?;
        let updated = update_execution_in_tx(&mut transaction, input).await?;
        transaction.commit().await?;
        Ok(updated)
    }

    async fn update_with_event(
        &self,
        input: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)> {
        let mut transaction = self.pool.begin().await?;
        let updated = update_execution_in_tx(&mut transaction, input).await?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut transaction, &event).await?;
        transaction.commit().await?;
        Ok((updated, event))
    }

    async fn update_last_activity_at(&self, id: &str, timestamp: &str) -> Result<()> {
        sqlx::query("UPDATE execution SET last_activity_at = ?, updated_at = ? WHERE id = ?")
            .bind(timestamp)
            .bind(timestamp)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_stalled_running(&self, stale_before: &str) -> Result<Vec<Execution>> {
        let rows = sqlx::query(
            "SELECT e.* FROM execution e
             WHERE e.status = 'running'
               AND (
                 (e.last_activity_at IS NULL AND e.created_at < ?)
                 OR (e.last_activity_at IS NOT NULL AND e.last_activity_at < ?)
               )
               AND NOT EXISTS (
                   SELECT 1 FROM domain_event pending
                   WHERE pending.entity_type = 'execution'
                     AND pending.entity_id = e.id
                     AND pending.event_type = 'execution.provider_start_pending'
                     AND pending.dedupe_key = 'execution.provider_start_pending:' || e.id
                     AND NOT EXISTS (
                         SELECT 1 FROM domain_event attempt
                         WHERE attempt.entity_type = 'execution'
                           AND attempt.entity_id = e.id
                           AND attempt.event_type = 'execution.provider_start_attempted'
                           AND attempt.dedupe_key = 'execution.provider_start_attempted:' || e.id
                     )
               )
             ORDER BY COALESCE(e.last_activity_at, e.created_at) ASC, e.id ASC",
        )
        .bind(stale_before)
        .bind(stale_before)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(map_execution).collect()
    }

    async fn list_running(&self) -> Result<Vec<Execution>> {
        let rows = sqlx::query(
            "SELECT * FROM execution
             WHERE status = 'running'
             ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(map_execution).collect()
    }

    async fn list_running_for_daemon_not_in(
        &self,
        daemon_id: &str,
        created_before: &str,
        exclude_ids: &[String],
    ) -> Result<Vec<Execution>> {
        let rows = if exclude_ids.is_empty() {
            sqlx::query(
                "SELECT e.* FROM execution e
                 WHERE e.status = 'running'
                   AND NOT EXISTS (
                       SELECT 1 FROM domain_event pending
                       WHERE pending.entity_type = 'execution'
                         AND pending.entity_id = e.id
                         AND pending.event_type = 'execution.provider_start_pending'
                         AND pending.dedupe_key = 'execution.provider_start_pending:' || e.id
                         AND NOT EXISTS (
                             SELECT 1 FROM domain_event attempt
                             WHERE attempt.entity_type = 'execution'
                               AND attempt.entity_id = e.id
                               AND attempt.event_type = 'execution.provider_start_attempted'
                               AND attempt.dedupe_key = 'execution.provider_start_attempted:' || e.id
                         )
                   )
                   AND CASE
                         WHEN json_valid(e.executor_config_snapshot_json)
                         THEN json_extract(e.executor_config_snapshot_json, '$.resolved_daemon_id')
                       END = ?
                   AND e.created_at < ?
                 ORDER BY e.created_at ASC, e.id ASC",
            )
            .bind(daemon_id)
            .bind(created_before)
            .fetch_all(&self.pool)
            .await?
        } else {
            let placeholders = exclude_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT e.* FROM execution e
                 WHERE e.status = 'running'
                   AND NOT EXISTS (
                       SELECT 1 FROM domain_event pending
                       WHERE pending.entity_type = 'execution'
                         AND pending.entity_id = e.id
                         AND pending.event_type = 'execution.provider_start_pending'
                         AND pending.dedupe_key = 'execution.provider_start_pending:' || e.id
                         AND NOT EXISTS (
                             SELECT 1 FROM domain_event attempt
                             WHERE attempt.entity_type = 'execution'
                               AND attempt.entity_id = e.id
                               AND attempt.event_type = 'execution.provider_start_attempted'
                               AND attempt.dedupe_key = 'execution.provider_start_attempted:' || e.id
                         )
                   )
                   AND CASE
                         WHEN json_valid(e.executor_config_snapshot_json)
                         THEN json_extract(e.executor_config_snapshot_json, '$.resolved_daemon_id')
                       END = ?
                   AND e.created_at < ?
                   AND e.id NOT IN ({placeholders})
                 ORDER BY e.created_at ASC, e.id ASC"
            );
            let mut query = sqlx::query(&query).bind(daemon_id).bind(created_before);
            for execution_id in exclude_ids {
                query = query.bind(execution_id);
            }
            query.fetch_all(&self.pool).await?
        };

        rows.into_iter().map(map_execution).collect()
    }

    async fn get_logs_path(&self, id: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT logs_path FROM execution WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Into::into)
    }

    async fn record_harness_session_result(
        &self,
        execution_id: &str,
        external_session_id: &str,
        updated_at: &str,
    ) -> Result<Execution> {
        let mut transaction = self.pool.begin().await?;
        bind_external_session_in_tx(
            &mut transaction,
            execution_id,
            external_session_id,
            updated_at,
        )
        .await?;
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(execution_id)
            .fetch_one(&mut *transaction)
            .await
            .map(super::map_execution)??;
        transaction.commit().await?;
        Ok(execution)
    }
}

async fn update_execution_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    input: UpdateExecution,
) -> Result<Execution> {
    let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
        .bind(&input.id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(super::map_execution)
        .transpose()?
        .ok_or(DbError::NotFound)?;
    if let Some(status) = input.status.as_ref() {
        if !execution_transition_allowed(&execution.status, status) {
            return Err(DbError::InvalidTransition);
        }
    }
    let requested_status = input.status.clone();
    let updated_at = input.updated_at.clone();

    let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE execution SET ");
    let mut needs_comma = false;
    macro_rules! push_assignment {
        ($column:literal, $value:expr) => {{
            if needs_comma {
                query.push(", ");
            }
            needs_comma = true;
            query.push($column).push(" = ").push_bind($value);
        }};
    }
    if let Some(status) = input.status {
        push_assignment!("status", status.to_string());
    }
    let external_session_update = input.external_session_id.clone();
    let resolved_harness_session_id = if external_session_update
        .as_ref()
        .is_some_and(Option::is_some)
    {
        if let Some(Some(snapshot_json)) = input.executor_config_snapshot_json.as_ref() {
            super::harness_session::create_resolved_harness_session_in_tx(
                transaction,
                &execution,
                snapshot_json,
                &updated_at,
            )
            .await?
        } else {
            None
        }
    } else {
        None
    };
    if let Some(harness_session_id) = resolved_harness_session_id {
        push_assignment!("harness_session_id", harness_session_id);
    }
    if let Some(agent_message_id) = input.agent_message_id {
        push_assignment!("agent_message_id", agent_message_id);
    }
    if let Some(last_activity_at) = input.last_activity_at {
        push_assignment!("last_activity_at", last_activity_at);
    }
    if let Some(summary) = input.summary {
        push_assignment!("summary", summary);
    }
    if let Some(logs_path) = input.logs_path {
        push_assignment!("logs_path", logs_path);
    }
    if let Some(before_sha) = input.before_sha {
        push_assignment!("before_sha", before_sha);
    }
    if let Some(after_sha) = input.after_sha {
        push_assignment!("after_sha", after_sha);
    }
    if let Some(error) = input.error {
        push_assignment!("error", error);
    }
    if let Some(executor_config_snapshot_json) = input.executor_config_snapshot_json {
        // Keep the immutable snapshot whenever an explicit HarnessSession
        // may be used by recovery or a future explicit Resume operation.
        if executor_config_snapshot_json.is_some() || execution.harness_session_id.is_none() {
            push_assignment!(
                "executor_config_snapshot_json",
                executor_config_snapshot_json
            );
        }
    }
    if let Some(stop_reason) = input.stop_reason {
        push_assignment!("stop_reason", stop_reason.map(|value| value.to_string()));
    }
    if let Some(stopped_by) = input.stopped_by {
        push_assignment!("stopped_by", stopped_by);
    }
    if let Some(resume_policy) = input.resume_policy {
        push_assignment!(
            "resume_policy",
            resume_policy.map(|value| value.to_string())
        );
    }
    if let Some(stopped_at) = input.stopped_at {
        push_assignment!("stopped_at", stopped_at);
    }
    if needs_comma {
        query.push(", ");
    }
    query.push("updated_at = ").push_bind(input.updated_at);
    query.push(" WHERE id = ").push_bind(&input.id);
    query.build().execute(&mut **transaction).await?;
    if let Some(Some(external_session_id)) = external_session_update.as_ref() {
        bind_external_session_in_tx(transaction, &input.id, external_session_id, &now_rfc3339())
            .await?;
    }
    if execution.work_unit_id.is_some()
        && execution.status == ExecutionStatus::Running
        && requested_status.as_ref().is_some_and(|status| {
            matches!(
                status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
            )
        })
    {
        sqlx::query(
            "UPDATE workspace_lease
             SET status = 'revoked', revoked_at = ?, version = version + 1,
                 updated_at = ?
             WHERE execution_id = ? AND work_unit_id = ? AND status = 'active'",
        )
        .bind(&updated_at)
        .bind(&updated_at)
        .bind(&execution.id)
        .bind(execution.work_unit_id.as_deref())
        .execute(&mut **transaction)
        .await?;
    }
    let updated = sqlx::query("SELECT * FROM execution WHERE id = ?")
        .bind(&input.id)
        .fetch_one(&mut **transaction)
        .await
        .map(super::map_execution)??;
    Ok(updated)
}

async fn bind_external_session_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    execution_id: &str,
    external_session_id: &str,
    updated_at: &str,
) -> Result<()> {
    if external_session_id.trim().is_empty() {
        return Err(DbError::Check(
            "external HarnessSession identity cannot be empty".to_owned(),
        ));
    }
    let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
        .bind(execution_id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(super::map_execution)
        .transpose()?
        .ok_or(DbError::NotFound)?;

    if execution.actor_kind == Some(ActorKind::Human) {
        return Err(DbError::Check(
            "Human Executions cannot receive an external HarnessSession identity".to_owned(),
        ));
    }

    if execution.actor_kind == Some(ActorKind::Agent) {
        let snapshot = execution
            .executor_config_snapshot_json
            .as_deref()
            .and_then(|snapshot| serde_json::from_str::<serde_json::Value>(snapshot).ok())
            .unwrap_or_default();
        if let Some(routing) = snapshot.get("routing") {
            let selected_candidate_key = routing
                .as_object()
                .and_then(|routing| routing.get("selected_candidate_key"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if selected_candidate_key.is_none() {
                return Err(DbError::Check(
                    "cannot bind external session identity before executor route resolution"
                        .to_owned(),
                ));
            }
        }
    }

    if let Some(harness_session_id) = execution.harness_session_id.as_deref() {
        let row = sqlx::query(
            "SELECT agent_id, harness_kind, external_session_id, status, workspace_id
             FROM harness_session WHERE id = ?",
        )
        .bind(harness_session_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(DbError::NotFound)?;
        let session_agent_id: String = row.try_get("agent_id")?;
        let session_harness_kind: String = row.try_get("harness_kind")?;
        let existing_external: Option<String> = row.try_get("external_session_id")?;
        let status: HarnessSessionStatus = parse_enum(row.try_get::<String, _>("status")?)?;
        let session_workspace_id: Option<String> = row.try_get("workspace_id")?;
        if session_agent_id != execution.actor_id.clone().unwrap_or_default()
            || execution.actor_kind != Some(ActorKind::Agent)
        {
            return Err(DbError::Check(
                "HarnessSession does not belong to the Execution Agent".to_owned(),
            ));
        }
        if let Some(snapshot_json) = execution.executor_config_snapshot_json.as_deref() {
            let snapshot =
                serde_json::from_str::<serde_json::Value>(snapshot_json).unwrap_or_default();
            if let Some(executor_type) = snapshot
                .get("executor_type")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if executor_type != session_harness_kind {
                    return Err(DbError::Check(
                        "HarnessSession result uses a different harness kind".to_owned(),
                    ));
                }
            }
        }
        if let Some(existing_external) = existing_external {
            if existing_external != external_session_id {
                return Err(DbError::Check(
                    "HarnessSession external identity cannot diverge".to_owned(),
                ));
            }
        }
        if !matches!(
            status,
            HarnessSessionStatus::Pending | HarnessSessionStatus::Active
        ) {
            return Err(DbError::Check(
                "HarnessSession is not usable for an executor result".to_owned(),
            ));
        }
        if session_workspace_id.is_some()
            && session_workspace_id.as_deref() != execution.workspace_id.as_deref()
        {
            return Err(DbError::Check(
                "HarnessSession workspace is incompatible with the Execution".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE harness_session
             SET external_session_id = ?, status = 'active', last_activity_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(external_session_id)
        .bind(updated_at)
        .bind(updated_at)
        .bind(harness_session_id)
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    Err(DbError::Check(
        "Execution has no exact HarnessSession authority for this external session result"
            .to_owned(),
    ))
}
