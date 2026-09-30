use super::*;

#[async_trait]
impl OrchestratorWakeRepo for SqliteDb {
    async fn admit_orchestrator_wake(&self, input: CreateOrchestratorWake) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO orchestrator_wake (
                id, event_id, event_sequence, task_id, task_role_id,
                coordination_mode, actor_kind, actor_id, work_unit_id, correlation_id, causation_id,
                causation_depth, policy_ref, policy_version, policy_digest, state,
                available_at, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)
             ON CONFLICT(event_id, task_id, actor_kind, actor_id) DO NOTHING",
        )
        .bind(&input.id)
        .bind(&input.event_id)
        .bind(input.event_sequence)
        .bind(&input.task_id)
        .bind(&input.task_role_id)
        .bind(input.coordination_mode.map(|mode| mode.to_string()))
        .bind(input.actor_kind.to_string())
        .bind(&input.actor_id)
        .bind(input.work_unit_id.as_deref())
        .bind(&input.correlation_id)
        .bind(input.causation_id.as_deref())
        .bind(input.causation_depth)
        .bind(&input.policy_ref)
        .bind(input.policy_version)
        .bind(&input.policy_digest)
        .bind(&input.available_at)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn claim_orchestrator_wake(
        &self,
        input: ClaimOrchestratorWake,
    ) -> Result<Option<OrchestratorWake>> {
        // Claim in one SQLite write statement. A read-then-write transaction
        // can leave two WAL readers racing to upgrade their snapshots; RETURNING
        // makes the conditional claim itself the cross-process authority.
        let row = sqlx::query(
            "UPDATE orchestrator_wake
             SET state = 'leased', lease_owner = ?, lease_until = ?,
                 version = version + 1, updated_at = ?
             WHERE id = (
                 SELECT candidate.id
                 FROM orchestrator_wake candidate
                 WHERE candidate.available_at <= ?
                   AND (candidate.state = 'pending'
                        OR (candidate.state = 'leased' AND candidate.lease_until <= ?))
                   AND NOT EXISTS (
                        SELECT 1 FROM orchestrator_wake active
                        WHERE active.actor_kind = candidate.actor_kind
                          AND active.actor_id = candidate.actor_id
                          AND active.id <> candidate.id
                          AND (active.state IN ('running', 'uncertain')
                               OR (active.state = 'leased' AND active.lease_until > ?))
                   )
                 ORDER BY candidate.event_sequence ASC, candidate.id ASC
                 LIMIT 1
             )
             RETURNING *",
        )
        .bind(&input.lease_owner)
        .bind(&input.leased_until)
        .bind(&input.now)
        .bind(&input.now)
        .bind(&input.now)
        .bind(&input.now)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| map_orchestrator_wake(&row))
            .transpose()
            .map_err(Into::into)
    }

    async fn get_orchestrator_wake(&self, id: &str) -> Result<Option<OrchestratorWake>> {
        sqlx::query("SELECT * FROM orchestrator_wake WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| map_orchestrator_wake(&row))
            .transpose()
            .map_err(Into::into)
    }

    async fn get_orchestrator_wake_by_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<(OrchestratorWake, OrchestratorWakeExecution)>> {
        let row = sqlx::query(
            "SELECT w.*, x.attempt_number AS x_attempt_number,
                    x.execution_id AS x_execution_id, x.state AS x_state,
                    x.last_error AS x_last_error, x.created_at AS x_created_at,
                    x.updated_at AS x_updated_at
             FROM orchestrator_wake w
             JOIN orchestrator_wake_execution x ON x.wake_id = w.id
             WHERE x.execution_id = ?",
        )
        .bind(execution_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let wake = map_orchestrator_wake(&row)?;
            let execution = OrchestratorWakeExecution {
                wake_id: row.try_get("id")?,
                attempt_number: row.try_get("x_attempt_number")?,
                execution_id: row.try_get("x_execution_id")?,
                state: row.try_get("x_state")?,
                last_error: row.try_get("x_last_error")?,
                created_at: row.try_get("x_created_at")?,
                updated_at: row.try_get("x_updated_at")?,
            };
            Ok::<_, DbError>((wake, execution))
        })
        .transpose()
        .map_err(Into::into)
    }

    async fn reserve_orchestrator_wake_execution(
        &self,
        input: ReserveOrchestratorWakeExecution,
    ) -> Result<OrchestratorWakeExecution> {
        let mut transaction = self.pool.begin().await?;
        let wake = sqlx::query("SELECT * FROM orchestrator_wake WHERE id = ?")
            .bind(&input.wake_id)
            .fetch_optional(&mut *transaction)
            .await?
            .map(|row| map_orchestrator_wake(&row))
            .transpose()?;
        let Some(wake) = wake else {
            return Err(DbError::NotFound);
        };
        if wake.state != OrchestratorWakeState::Leased
            || wake.lease_owner.as_deref() != Some(input.lease_owner.as_str())
        {
            return Err(DbError::VersionConflict);
        }

        if let Some(attempt_number) = wake.current_attempt {
            let current = sqlx::query(
                "SELECT * FROM orchestrator_wake_execution
                 WHERE wake_id = ? AND attempt_number = ?",
            )
            .bind(&input.wake_id)
            .bind(attempt_number)
            .fetch_optional(&mut *transaction)
            .await?
            .map(|row| map_orchestrator_wake_execution(&row))
            .transpose()?;
            if let Some(current) = current {
                if matches!(
                    current.state.as_str(),
                    "reserved" | "start_requested" | "running" | "uncertain"
                ) {
                    transaction.commit().await?;
                    return Ok(current);
                }
            }
        }

        let attempt_number = wake.attempt_count + 1;
        sqlx::query(
            "INSERT INTO orchestrator_wake_execution (
                wake_id, attempt_number, execution_id, state, created_at, updated_at
             ) VALUES (?, ?, ?, 'reserved', ?, ?)",
        )
        .bind(&input.wake_id)
        .bind(attempt_number)
        .bind(&input.execution_id)
        .bind(&input.now)
        .bind(&input.now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE orchestrator_wake
             SET attempt_count = ?, current_attempt = ?, last_error = NULL,
                 version = version + 1, updated_at = ?
             WHERE id = ? AND state = 'leased' AND lease_owner = ?",
        )
        .bind(attempt_number)
        .bind(attempt_number)
        .bind(&input.now)
        .bind(&input.wake_id)
        .bind(&input.lease_owner)
        .execute(&mut *transaction)
        .await?;
        let attempt = sqlx::query(
            "SELECT * FROM orchestrator_wake_execution
             WHERE wake_id = ? AND attempt_number = ?",
        )
        .bind(&input.wake_id)
        .bind(attempt_number)
        .fetch_one(&mut *transaction)
        .await
        .and_then(|row| map_orchestrator_wake_execution(&row))
        .map_err(DbError::from)?;
        transaction.commit().await?;
        Ok(attempt)
    }

    async fn transition_orchestrator_wake(
        &self,
        input: TransitionOrchestratorWake,
    ) -> Result<bool> {
        let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE orchestrator_wake SET state = ");
        query.push_bind(input.state.to_string());
        query
            .push(", lease_owner = NULL, lease_until = NULL, version = version + 1, updated_at = ");
        query.push_bind(&input.updated_at);
        if let Some(available_at) = input.available_at.as_deref() {
            query.push(", available_at = ").push_bind(available_at);
        }
        if let Some(current_attempt) = input.current_attempt {
            query
                .push(", current_attempt = ")
                .push_bind(current_attempt);
        }
        if let Some(last_error) = input.last_error.as_ref() {
            query
                .push(", last_error = ")
                .push_bind(last_error.as_deref());
        }
        query.push(" WHERE id = ").push_bind(&input.id);
        if let Some(expected_state) = input.expected_state {
            query
                .push(" AND state = ")
                .push_bind(expected_state.to_string());
        }
        if let Some(lease_owner) = input.lease_owner.as_deref() {
            query
                .push(" AND state = 'leased' AND lease_owner = ")
                .push_bind(lease_owner);
        }
        let result = query.build().execute(&self.pool).await?;
        Ok(result.rows_affected() == 1)
    }

    async fn transition_orchestrator_wake_execution(
        &self,
        input: TransitionOrchestratorWakeExecution,
    ) -> Result<bool> {
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE orchestrator_wake_execution SET state = ");
        query.push_bind(&input.state);
        query.push(", updated_at = ").push_bind(&input.updated_at);
        if let Some(last_error) = input.last_error.as_ref() {
            query
                .push(", last_error = ")
                .push_bind(last_error.as_deref());
        }
        query
            .push(" WHERE wake_id = ")
            .push_bind(&input.wake_id)
            .push(" AND attempt_number = ")
            .push_bind(input.attempt_number)
            .push(" AND execution_id = ")
            .push_bind(&input.execution_id);
        if let Some(expected_state) = input.expected_state.as_deref() {
            query.push(" AND state = ").push_bind(expected_state);
        }
        if let Some(lease_owner) = input.lease_owner.as_deref() {
            query
                .push(
                    " AND EXISTS (SELECT 1 FROM orchestrator_wake w
                 WHERE w.id = orchestrator_wake_execution.wake_id
                   AND w.state = 'leased' AND w.lease_owner = ",
                )
                .push_bind(lease_owner)
                .push(")");
        }
        let result = query.build().execute(&self.pool).await?;
        Ok(result.rows_affected() == 1)
    }

    async fn reserve_orchestrator_action(
        &self,
        input: ReserveOrchestratorAction,
    ) -> Result<OrchestratorActionRecord> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO orchestrator_action (
                execution_id, action_index, action_type, action_digest, result_id,
                state, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 'reserved', ?, ?)
             ON CONFLICT(execution_id, action_index) DO NOTHING",
        )
        .bind(&input.execution_id)
        .bind(input.action_index)
        .bind(&input.action_type)
        .bind(&input.action_digest)
        .bind(&input.result_id)
        .bind(&input.now)
        .bind(&input.now)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT execution_id, action_index, action_type, action_digest,
                    result_id, state, created_at, updated_at
             FROM orchestrator_action WHERE execution_id = ? AND action_index = ?",
        )
        .bind(&input.execution_id)
        .bind(input.action_index)
        .fetch_one(&mut *transaction)
        .await?;
        let record = OrchestratorActionRecord {
            execution_id: row.try_get("execution_id")?,
            action_index: row.try_get("action_index")?,
            action_type: row.try_get("action_type")?,
            action_digest: row.try_get("action_digest")?,
            result_id: row.try_get("result_id")?,
            state: row.try_get("state")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        };
        if record.action_type != input.action_type || record.action_digest != input.action_digest {
            return Err(DbError::IdempotencyConflict);
        }
        transaction.commit().await?;
        Ok(record)
    }

    async fn complete_orchestrator_action(
        &self,
        execution_id: &str,
        action_index: i64,
        updated_at: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE orchestrator_action
             SET state = 'completed', updated_at = ?
             WHERE execution_id = ? AND action_index = ? AND state = 'reserved'",
        )
        .bind(updated_at)
        .bind(execution_id)
        .bind(action_index)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }
}

fn map_orchestrator_wake(row: &SqliteRow) -> std::result::Result<OrchestratorWake, sqlx::Error> {
    Ok(OrchestratorWake {
        id: row.try_get("id")?,
        event_id: row.try_get("event_id")?,
        event_sequence: row.try_get("event_sequence")?,
        task_id: row.try_get("task_id")?,
        task_role_id: row.try_get("task_role_id")?,
        coordination_mode: row
            .try_get::<Option<String>, _>("coordination_mode")?
            .map(|value| {
                CoordinationMode::from_str(&value).map_err(|_| sqlx::Error::ColumnDecode {
                    index: "coordination_mode".to_owned(),
                    source: Box::new(std::io::Error::other("invalid coordination mode")),
                })
            })
            .transpose()?,
        actor_kind: row
            .try_get::<String, _>("actor_kind")?
            .parse()
            .map_err(|_| sqlx::Error::ColumnDecode {
                index: "actor_kind".to_owned(),
                source: Box::new(std::io::Error::other("invalid actor kind")),
            })?,
        actor_id: row.try_get("actor_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        correlation_id: row.try_get("correlation_id")?,
        causation_id: row.try_get("causation_id")?,
        causation_depth: row.try_get("causation_depth")?,
        policy_ref: row.try_get("policy_ref")?,
        policy_version: row.try_get("policy_version")?,
        policy_digest: row.try_get("policy_digest")?,
        state: row.try_get::<String, _>("state")?.parse().map_err(|_| {
            sqlx::Error::ColumnDecode {
                index: "state".to_owned(),
                source: Box::new(std::io::Error::other("invalid wake state")),
            }
        })?,
        available_at: row.try_get("available_at")?,
        lease_owner: row.try_get("lease_owner")?,
        lease_until: row.try_get("lease_until")?,
        attempt_count: row.try_get("attempt_count")?,
        current_attempt: row.try_get("current_attempt")?,
        last_error: row.try_get("last_error")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_orchestrator_wake_execution(
    row: &SqliteRow,
) -> std::result::Result<OrchestratorWakeExecution, sqlx::Error> {
    Ok(OrchestratorWakeExecution {
        wake_id: row.try_get("wake_id")?,
        attempt_number: row.try_get("attempt_number")?,
        execution_id: row.try_get("execution_id")?,
        state: row.try_get("state")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
