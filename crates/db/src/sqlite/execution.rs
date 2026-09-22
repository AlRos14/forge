use super::*;
use crate::{now_rfc3339, AgentExecutionStats};

#[async_trait]
impl ExecutionRepo for SqliteDb {
    async fn create(&self, input: CreateExecution) -> Result<Execution> {
        let mut transaction = self.pool.begin().await?;
        let execution = Self::create_execution_in_tx(&mut transaction, &input).await?;
        transaction.commit().await?;
        Ok(execution)
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
                WHERE task_id IN (",
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
        let execution = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(&input.id)
            .fetch_optional(&mut *transaction)
            .await?
            .map(super::map_execution)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if let Some(status) = input.status.as_ref() {
            if !execution_transition_allowed(&execution.status, status) {
                return Err(DbError::InvalidTransition);
            }
        }

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
            // An explicit HarnessSession remains resumable after a failed or
            // cancelled Execution. Preserve the immutable executor snapshot
            // needed to reconstruct that continuity; cleanup remains valid
            // for rows that have no generic session authority.
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
        query.build().execute(&mut *transaction).await?;
        if let Some(Some(external_session_id)) = legacy_session_update.as_ref() {
            bind_external_session_in_tx(
                &mut transaction,
                &input.id,
                external_session_id,
                &now_rfc3339(),
            )
            .await?;
        }
        let updated = sqlx::query("SELECT * FROM execution WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *transaction)
            .await
            .map(super::map_execution)??;
        transaction.commit().await?;
        Ok(updated)
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
                 INNER JOIN agent_current a ON a.id = e.agent_id
                 WHERE e.status = 'running'
                   AND a.daemon_id = ?
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
                 INNER JOIN agent_current a ON a.id = e.agent_id
                 WHERE e.status = 'running'
                   AND a.daemon_id = ?
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
    let profile_id =
        harness_session::profile_id_for_snapshot_in_tx(transaction, agent_id, &snapshot).await?;
    let capabilities_snapshot_json = snapshot
        .get("capabilities")
        .map(ToString::to_string)
        .unwrap_or_else(|| "{}".to_owned());
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
            existing_status,
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
        existing.try_get::<String, _>("id")?
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
