use super::*;
use crate::{now_rfc3339, AgentExecutionStats, ExecutionPurpose, ExecutionStatus};

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
/// an authoritative Review write. Once the replacement TaskRole exists, the
/// membership is the sole authority; the legacy singleton is used only when
/// that TaskRole has not been created.
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
        "SELECT CASE
            WHEN EXISTS (
                SELECT 1 FROM task_role
                WHERE task_id = ? AND role = ?
            ) THEN EXISTS (
                SELECT 1
                FROM task_role AS role
                JOIN role_membership AS membership
                  ON membership.task_role_id = role.id
                WHERE role.task_id = ? AND role.role = ?
                  AND membership.actor_kind = 'human'
                  AND membership.actor_id = ?
                  AND membership.status = 'active'
            )
            ELSE EXISTS (
                SELECT 1 FROM task_role_assignment
                WHERE task_id = ? AND role_name = ?
                  AND assignee_type = 'user' AND assignee_id = ?
            )
         END",
    )
    .bind(task_id)
    .bind(&canonical_role)
    .bind(task_id)
    .bind(&canonical_role)
    .bind(user_id)
    .bind(task_id)
    .bind(role)
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
            "SELECT * FROM execution
             WHERE status = 'running'
               AND (
                 (last_activity_at IS NULL AND created_at < ?)
                 OR (last_activity_at IS NOT NULL AND last_activity_at < ?)
               )
             ORDER BY COALESCE(last_activity_at, created_at) ASC, id ASC",
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
    let legacy_session_update = input.agent_session_id.clone();
    if matches!(legacy_session_update.as_ref(), Some(None))
        && execution.harness_session_id.is_none()
    {
        push_assignment!("agent_session_id", None::<String>);
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
    if let Some(Some(external_session_id)) = legacy_session_update.as_ref() {
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

    if execution
        .agent_session_id
        .as_deref()
        .is_some_and(|existing| existing != external_session_id)
    {
        return Err(DbError::Check(
            "Execution legacy session projection disagrees with the returned external identity"
                .to_owned(),
        ));
    }

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
        sqlx::query(
            "UPDATE execution
             SET agent_session_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(external_session_id)
        .bind(updated_at)
        .bind(execution_id)
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    if execution.actor_kind.is_none()
        && execution
            .agent_id
            .as_deref()
            .is_some_and(|agent_id| agent_id.eq_ignore_ascii_case("human"))
    {
        // The reserved sentinel is historical compatibility only. Preserve its
        // old projection without turning it into a generic Actor or session.
        sqlx::query("UPDATE execution SET agent_session_id = ?, updated_at = ? WHERE id = ?")
            .bind(external_session_id)
            .bind(updated_at)
            .bind(execution_id)
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    }

    let Some(agent_id) = execution.agent_id.as_deref() else {
        // Historical/system-shaped rows have no generic Agent authority. Keep
        // their bounded legacy projection rather than inventing a principal.
        sqlx::query("UPDATE execution SET agent_session_id = ?, updated_at = ? WHERE id = ?")
            .bind(external_session_id)
            .bind(updated_at)
            .bind(execution_id)
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    };
    if execution.actor_kind != Some(ActorKind::Agent)
        || execution.actor_id.as_deref() != Some(agent_id)
    {
        return Err(DbError::Check(
            "cannot materialize HarnessSession without an Agent ActorRef".to_owned(),
        ));
    }

    let historical_ambiguity: Option<i64> = sqlx::query_scalar(
        "SELECT 1
         FROM execution_session_migration_issue
         WHERE execution_id = ? AND issue_kind = 'historical_session_ambiguous'
         LIMIT 1",
    )
    .bind(execution_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if historical_ambiguity.is_some() {
        // Contradictory historical evidence stays on the bounded legacy
        // projection. Do not silently choose a generic session during a
        // result callback.
        sqlx::query("UPDATE execution SET agent_session_id = ?, updated_at = ? WHERE id = ?")
            .bind(external_session_id)
            .bind(updated_at)
            .bind(execution_id)
            .execute(&mut **transaction)
            .await?;
        return Ok(());
    }

    let snapshot_json = execution
        .executor_config_snapshot_json
        .as_deref()
        .unwrap_or("{}");
    let snapshot = serde_json::from_str::<serde_json::Value>(snapshot_json).unwrap_or_default();
    let harness_kind = snapshot
        .get("executor_type")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("legacy")
        .to_owned();
    if harness_kind != "legacy" {
        harness_session::validate_agent_harness_identity_in_tx(
            transaction,
            agent_id,
            &harness_kind,
        )
        .await?;
    }
    let profile_id =
        harness_session::profile_id_for_snapshot_in_tx(transaction, agent_id, &snapshot).await?;
    let capabilities_snapshot_json = snapshot
        .get("harness_capabilities")
        .map(ToString::to_string)
        .unwrap_or_else(|| r#"{"schema_version":1,"capabilities":{}}"#.to_owned());
    let existing = sqlx::query(
        "SELECT id, status, workspace_id FROM harness_session
         WHERE agent_id = ? AND harness_kind = ? AND external_session_id = ?
         LIMIT 1",
    )
    .bind(agent_id)
    .bind(&harness_kind)
    .bind(external_session_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let harness_session_id = if let Some(existing) = existing {
        let existing_status: HarnessSessionStatus =
            parse_enum(existing.try_get::<String, _>("status")?)?;
        if matches!(
            &existing_status,
            HarnessSessionStatus::Ended | HarnessSessionStatus::Failed
        ) {
            return Err(DbError::Check(
                "external session identity belongs to an ended or failed HarnessSession".to_owned(),
            ));
        }
        let existing_workspace_id: Option<String> = existing.try_get("workspace_id")?;
        if existing_workspace_id.is_some()
            && existing_workspace_id.as_deref() != execution.workspace_id.as_deref()
        {
            return Err(DbError::Check(
                "external session identity is workspace-incompatible".to_owned(),
            ));
        }
        let existing_id = existing.try_get::<String, _>("id")?;
        if matches!(&existing_status, HarnessSessionStatus::Pending)
            && snapshot.get("harness_capabilities").is_some()
        {
            sqlx::query(
                "UPDATE harness_session SET capabilities_snapshot_json = ? WHERE id = ? AND status = 'pending'",
            )
            .bind(&capabilities_snapshot_json)
            .bind(&existing_id)
            .execute(&mut **transaction)
            .await?;
        }
        existing_id
    } else {
        let id = crate::new_uuid_v4();
        sqlx::query(
            "INSERT INTO harness_session (
                 id, agent_id, harness_kind, external_session_id, profile_id,
                 profile_snapshot_json, capabilities_snapshot_json, workspace_id,
                 status, predecessor_session_id, created_at, updated_at, last_activity_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active', NULL, ?, ?, ?)",
        )
        .bind(&id)
        .bind(agent_id)
        .bind(&harness_kind)
        .bind(external_session_id)
        .bind(profile_id.as_deref())
        .bind(snapshot_json)
        .bind(&capabilities_snapshot_json)
        .bind(execution.workspace_id.as_deref())
        .bind(&execution.created_at)
        .bind(updated_at)
        .bind(updated_at)
        .execute(&mut **transaction)
        .await?;
        id
    };
    sqlx::query(
        "UPDATE harness_session
         SET status = 'active', last_activity_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(updated_at)
    .bind(updated_at)
    .bind(&harness_session_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE execution
         SET harness_session_id = ?, agent_session_id = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&harness_session_id)
    .bind(external_session_id)
    .bind(updated_at)
    .bind(execution_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
