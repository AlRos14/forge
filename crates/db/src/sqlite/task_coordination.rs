use super::*;

fn coordination_actor_columns(actor: Option<&ActorRef>) -> (Option<String>, Option<String>) {
    actor
        .map(|actor| (Some(actor.kind().to_string()), Some(actor.id().to_owned())))
        .unwrap_or((None, None))
}

fn map_task_coordination(row: SqliteRow) -> Result<TaskCoordination> {
    let actor_kind: Option<String> = row.try_get("coordinator_actor_kind")?;
    let actor_id: Option<String> = row.try_get("coordinator_actor_id")?;
    let coordinator_actor = match (actor_kind, actor_id) {
        (None, None) => None,
        (Some(kind), Some(id)) => Some(match kind.as_str() {
            "human" => ActorRef::Human(id),
            "agent" => ActorRef::Agent(id),
            _ => {
                return Err(DbError::Check(
                    "coordination Actor kind is invalid".to_owned(),
                ))
            }
        }),
        _ => {
            return Err(DbError::Check(
                "coordination Actor columns are incomplete".to_owned(),
            ))
        }
    };
    Ok(TaskCoordination {
        task_id: row.try_get("task_id")?,
        mode: parse_enum(row.try_get("mode")?)?,
        coordinator_task_role_id: row.try_get("coordinator_task_role_id")?,
        coordinator_actor,
        authorization_version: row.try_get("authorization_version")?,
        team_snapshot_json: row.try_get("team_snapshot_json")?,
        team_digest: row.try_get("team_digest")?,
        envelope_json: row.try_get("envelope_json")?,
        envelope_digest: row.try_get("envelope_digest")?,
        requested_by: row.try_get("requested_by")?,
        turn_count: row.try_get("turn_count")?,
        quality_request_count: row.try_get("quality_request_count")?,
        max_turns: row.try_get("max_turns")?,
        max_quality_requests: row.try_get("max_quality_requests")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        revoked_at: row.try_get("revoked_at")?,
        revoke_reason: row.try_get("revoke_reason")?,
    })
}

fn map_task_coordination_receipt(row: SqliteRow) -> Result<TaskCoordinationReceipt> {
    Ok(TaskCoordinationReceipt {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        operation: parse_enum(row.try_get("operation")?)?,
        idempotency_key: row.try_get("idempotency_key")?,
        request_digest: row.try_get("request_digest")?,
        authorization_version: row.try_get("authorization_version")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_task_coordination_request(row: SqliteRow) -> Result<TaskCoordinationRequest> {
    Ok(TaskCoordinationRequest {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        authorization_version: row.try_get("authorization_version")?,
        mode: parse_enum(row.try_get("mode")?)?,
        brief_json: row.try_get("brief_json")?,
        objective_digest: row.try_get("objective_digest")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        state: parse_enum(row.try_get("state")?)?,
        wake_id: row.try_get("wake_id")?,
        execution_id: row.try_get("execution_id")?,
        plan_artifact_id: row.try_get("plan_artifact_id")?,
        failure_code: row.try_get("failure_code")?,
        failure_message: row.try_get("failure_message")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn map_task_coordination_plan(row: SqliteRow) -> Result<TaskCoordinationPlan> {
    Ok(TaskCoordinationPlan {
        artifact_id: row.try_get("artifact_id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        authorization_version: row.try_get("authorization_version")?,
        plan_digest: row.try_get("plan_digest")?,
        state: parse_enum(row.try_get("state")?)?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_task_coordination_materialization(
    row: SqliteRow,
) -> Result<TaskCoordinationMaterialization> {
    Ok(TaskCoordinationMaterialization {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        plan_artifact_id: row.try_get("plan_artifact_id")?,
        plan_digest: row.try_get("plan_digest")?,
        authorization_version: row.try_get("authorization_version")?,
        task_version_before: row.try_get("task_version_before")?,
        task_version_after: row.try_get("task_version_after")?,
        work_unit_ids_json: row.try_get("work_unit_ids_json")?,
        receipt_digest: row.try_get("receipt_digest")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_quality_request(row: SqliteRow) -> Result<TaskQualityRequest> {
    let actor_kind: Option<String> = row.try_get("target_actor_kind")?;
    let actor_id: Option<String> = row.try_get("target_actor_id")?;
    let target_actor = match (actor_kind, actor_id) {
        (None, None) => None,
        (Some(kind), Some(id)) => Some(match kind.as_str() {
            "human" => ActorRef::Human(id),
            "agent" => ActorRef::Agent(id),
            _ => return Err(DbError::Check("quality target kind is invalid".to_owned())),
        }),
        _ => {
            return Err(DbError::Check(
                "quality target Actor columns are incomplete".to_owned(),
            ))
        }
    };
    let created_by_kind: String = row.try_get("created_by_kind")?;
    let created_by_id: String = row.try_get("created_by_id")?;
    let created_by = match created_by_kind.as_str() {
        "human" => ActorRef::Human(created_by_id),
        "agent" => ActorRef::Agent(created_by_id),
        _ => return Err(DbError::Check("quality creator kind is invalid".to_owned())),
    };
    Ok(TaskQualityRequest {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        category: parse_enum(row.try_get("category")?)?,
        target_kind: parse_enum(row.try_get("target_kind")?)?,
        target_task_role_id: row.try_get("target_task_role_id")?,
        target_actor,
        summary: row.try_get("summary")?,
        findings_json: row.try_get("findings_json")?,
        resolution_condition: row.try_get("resolution_condition")?,
        state: parse_enum(row.try_get("state")?)?,
        handoff_id: row.try_get("handoff_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        budget_round: row.try_get("budget_round")?,
        source_digest: row.try_get("source_digest")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        created_by,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        resolved_at: row.try_get("resolved_at")?,
    })
}

fn map_intervention(row: SqliteRow) -> Result<TaskIntervention> {
    Ok(TaskIntervention {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        kind: parse_enum(row.try_get("kind")?)?,
        state: parse_enum(row.try_get("state")?)?,
        source_kind: row.try_get("source_kind")?,
        source_id: row.try_get("source_id")?,
        payload_json: row.try_get("payload_json")?,
        request_digest: row.try_get("request_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        expected_task_version: row.try_get("expected_task_version")?,
        expected_coordination_version: row.try_get("expected_coordination_version")?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        coordination_request_id: row.try_get("coordination_request_id")?,
        failure_code: row.try_get("failure_code")?,
        failure_message: row.try_get("failure_message")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn append_optional_event(
    db: &SqliteDb,
    transaction: &mut Transaction<'_, Sqlite>,
    event: Option<&CreateDomainEvent>,
) -> Result<Option<DomainEvent>> {
    match event {
        Some(event) => Ok(Some(
            DomainEventRepo::append_event_in_tx(db, transaction, event).await?,
        )),
        None => Ok(None),
    }
}

fn check_error(error: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(database_error) = &error {
        let message = database_error.message().to_ascii_lowercase();
        if message.contains("unique constraint failed") {
            return DbError::IdempotencyConflict;
        }
        if message.contains("foreign key constraint failed") {
            return DbError::NotFound;
        }
        if message.contains("constraint failed") || message.contains("invalid") {
            return DbError::Check(database_error.message().to_owned());
        }
    }
    DbError::Sqlx(error)
}

#[async_trait]
impl TaskCoordinationRepo for SqliteDb {
    async fn get_task_coordination(&self, task_id: &str) -> Result<Option<TaskCoordination>> {
        sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination)
            .transpose()
    }

    async fn get_task_coordination_receipt(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskCoordinationReceipt>> {
        sqlx::query(
            "SELECT * FROM task_coordination_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_task_coordination_receipt)
        .transpose()
    }

    async fn authorize_task_coordination(
        &self,
        input: AuthorizeTaskCoordinationInput,
        expected_task_version: i64,
    ) -> Result<TaskCoordinationWrite> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_receipt)
        .transpose()?
        {
            if existing.request_digest != input.request_digest
                || existing.operation != TaskCoordinationOperation::Authorize
            {
                return Err(DbError::IdempotencyConflict);
            }
            let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_task_coordination)
                .transpose()?
                .ok_or(DbError::NotFound)?;
            tx.commit().await?;
            return Ok(TaskCoordinationWrite {
                coordination,
                receipt: existing,
                replayed: true,
                event: None,
            });
        }
        let task = sqlx::query("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let actual_task_version: i64 = task.try_get("version")?;
        if actual_task_version != expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: expected_task_version,
                actual: actual_task_version,
            });
        }
        let current = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if current.authorization_version != input.expected_coordination_version {
            return Err(DbError::VersionConflict);
        }
        let (actor_kind, actor_id) = coordination_actor_columns(input.coordinator_actor.as_ref());
        sqlx::query(
            "INSERT INTO task_coordination_receipt (
                id, task_id, operation, idempotency_key, request_digest,
                authorization_version, created_at
             ) VALUES (?, ?, 'authorize', ?, ?, ?, ?)",
        )
        .bind(&input.receipt_id)
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .bind(&input.request_digest)
        .bind(current.authorization_version + 1)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "UPDATE task_coordination
             SET mode = ?, coordinator_task_role_id = ?, coordinator_actor_kind = ?,
                 coordinator_actor_id = ?, authorization_version = authorization_version + 1,
                 team_snapshot_json = ?, team_digest = ?, envelope_json = ?,
                 envelope_digest = ?, requested_by = ?, updated_at = ?,
                 revoked_at = NULL, revoke_reason = NULL
             WHERE task_id = ? AND authorization_version = ?",
        )
        .bind(input.mode.to_string())
        .bind(input.coordinator_task_role_id.as_deref())
        .bind(actor_kind.as_deref())
        .bind(actor_id.as_deref())
        .bind(&input.team_snapshot_json)
        .bind(&input.team_digest)
        .bind(&input.envelope_json)
        .bind(&input.envelope_digest)
        .bind(&input.requested_by)
        .bind(&input.created_at)
        .bind(&input.task_id)
        .bind(input.expected_coordination_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        tx.commit().await?;
        Ok(TaskCoordinationWrite {
            coordination,
            receipt: TaskCoordinationReceipt {
                id: input.receipt_id,
                task_id: input.task_id,
                operation: TaskCoordinationOperation::Authorize,
                idempotency_key: input.idempotency_key,
                request_digest: input.request_digest,
                authorization_version: input.expected_coordination_version + 1,
                created_at: input.created_at,
            },
            replayed: false,
            event: Some(event),
        })
    }

    async fn revoke_task_coordination(
        &self,
        input: RevokeTaskCoordinationInput,
        expected_task_version: i64,
    ) -> Result<TaskCoordinationWrite> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_receipt
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_receipt)
        .transpose()?
        {
            if existing.request_digest != input.request_digest
                || existing.operation != TaskCoordinationOperation::Revoke
            {
                return Err(DbError::IdempotencyConflict);
            }
            let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_task_coordination)
                .transpose()?
                .ok_or(DbError::NotFound)?;
            tx.commit().await?;
            return Ok(TaskCoordinationWrite {
                coordination,
                receipt: existing,
                replayed: true,
                event: None,
            });
        }
        let task = sqlx::query("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let actual_task_version: i64 = task.try_get("version")?;
        if actual_task_version != expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: expected_task_version,
                actual: actual_task_version,
            });
        }
        let current = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if current.authorization_version != input.expected_coordination_version {
            return Err(DbError::VersionConflict);
        }
        sqlx::query(
            "INSERT INTO task_coordination_receipt (
                id, task_id, operation, idempotency_key, request_digest,
                authorization_version, created_at
             ) VALUES (?, ?, 'revoke', ?, ?, ?, ?)",
        )
        .bind(&input.receipt_id)
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .bind(&input.request_digest)
        .bind(current.authorization_version + 1)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "UPDATE task_coordination
             SET authorization_version = authorization_version + 1,
                 revoked_at = ?, revoke_reason = ?, requested_by = ?, updated_at = ?
             WHERE task_id = ? AND authorization_version = ?",
        )
        .bind(&input.created_at)
        .bind(input.reason.as_deref())
        .bind(&input.requested_by)
        .bind(&input.created_at)
        .bind(&input.task_id)
        .bind(input.expected_coordination_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let coordination = sqlx::query("SELECT * FROM task_coordination WHERE task_id = ?")
            .bind(&input.task_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        tx.commit().await?;
        Ok(TaskCoordinationWrite {
            coordination,
            receipt: TaskCoordinationReceipt {
                id: input.receipt_id,
                task_id: input.task_id,
                operation: TaskCoordinationOperation::Revoke,
                idempotency_key: input.idempotency_key,
                request_digest: input.request_digest,
                authorization_version: input.expected_coordination_version + 1,
                created_at: input.created_at,
            },
            replayed: false,
            event: Some(event),
        })
    }

    async fn consume_coordination_turn_budget(&self, task_id: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE task_coordination
             SET turn_count = turn_count + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE task_id = ? AND turn_count < max_turns",
        )
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn refund_coordination_turn_budget(&self, task_id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE task_coordination
             SET turn_count = max(turn_count - 1, 0),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE task_id = ?",
        )
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn consume_quality_request_budget(&self, task_id: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE task_coordination
             SET quality_request_count = quality_request_count + 1,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE task_id = ? AND quality_request_count < max_quality_requests",
        )
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn create_task_coordination_request(
        &self,
        input: CreateTaskCoordinationRequestInput,
    ) -> Result<(TaskCoordinationRequest, Option<DomainEvent>)> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_request
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_request)
        .transpose()?
        {
            if existing.request_digest != input.request_digest {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok((existing, None));
        }
        sqlx::query(
            "INSERT INTO task_coordination_request (
                id, task_id, authorization_version, mode, brief_json,
                objective_digest, request_digest, idempotency_key, state,
                version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'requested', 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.authorization_version)
        .bind(input.mode.to_string())
        .bind(&input.brief_json)
        .bind(&input.objective_digest)
        .bind(&input.request_digest)
        .bind(&input.idempotency_key)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_request)?;
        tx.commit().await?;
        Ok((request, Some(event)))
    }

    async fn get_task_coordination_request(
        &self,
        id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn get_task_coordination_request_by_key(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query(
            "SELECT * FROM task_coordination_request
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_task_coordination_request)
        .transpose()
    }

    async fn link_task_coordination_request_wake(
        &self,
        request_id: &str,
        task_id: &str,
        wake_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE task_coordination_request
             SET wake_id = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ? AND task_id = ? AND wake_id IS NULL",
        )
        .bind(wake_id)
        .bind(request_id)
        .bind(task_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_task_coordination_requests(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskCoordinationRequest>> {
        sqlx::query(
            "SELECT * FROM task_coordination_request
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_task_coordination_request)
        .collect()
    }

    async fn get_task_coordination_request_by_wake(
        &self,
        wake_id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE wake_id = ?")
            .bind(wake_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn get_task_coordination_request_by_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<TaskCoordinationRequest>> {
        sqlx::query("SELECT * FROM task_coordination_request WHERE execution_id = ?")
            .bind(execution_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_request)
            .transpose()
    }

    async fn update_task_coordination_request(
        &self,
        input: UpdateTaskCoordinationRequestState,
    ) -> Result<TaskCoordinationRequest> {
        let mut tx = self.pool.begin().await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_coordination_request SET state = ");
        query.push_bind(input.state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(&input.updated_at);
        if let Some(wake_id) = input.wake_id.as_deref() {
            query.push(", wake_id = ").push_bind(wake_id);
        }
        if let Some(execution_id) = input.execution_id.as_deref() {
            query.push(", execution_id = ").push_bind(execution_id);
        }
        if let Some(plan_artifact_id) = input.plan_artifact_id.as_deref() {
            query
                .push(", plan_artifact_id = ")
                .push_bind(plan_artifact_id);
        }
        if let Some(failure_code) = input.failure_code.as_ref() {
            query
                .push(", failure_code = ")
                .push_bind(failure_code.as_deref());
        }
        if let Some(failure_message) = input.failure_message.as_ref() {
            query
                .push(", failure_message = ")
                .push_bind(failure_message.as_deref());
        }
        if let Some(completed_at) = input.completed_at.as_deref() {
            query.push(", completed_at = ").push_bind(completed_at);
        }
        query
            .push(" WHERE id = ")
            .push_bind(&input.id)
            .push(" AND version = ")
            .push_bind(input.expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, input.event.as_ref()).await?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_request)?;
        tx.commit().await?;
        Ok(request)
    }

    async fn complete_task_coordination_turn(
        &self,
        input: CompleteTaskCoordinationTurnInput,
    ) -> Result<TaskCoordinationPlanWrite> {
        let mut tx = self.pool.begin().await?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_request)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if request.task_id != input.task_id
            || request.authorization_version != input.authorization_version
        {
            return Err(DbError::VersionConflict);
        }
        if matches!(
            request.state,
            TaskCoordinationRequestState::Cancelled | TaskCoordinationRequestState::Failed
        ) {
            return Err(DbError::InvalidTransition);
        }
        let existing_artifact: Option<String> = sqlx::query_scalar(
            "SELECT artifact_id FROM task_coordination_plan WHERE artifact_id = ?",
        )
        .bind(&input.artifact_id)
        .fetch_optional(&mut *tx)
        .await?;
        if existing_artifact.is_some() {
            return Err(DbError::IdempotencyConflict);
        }
        sqlx::query(
            "INSERT INTO artifact (
                id, task_id, kind, storage_kind, content, content_ref,
                metadata_json, digest, created_at
             ) VALUES (?, ?, 'work_unit_plan', 'inline', ?, NULL, ?, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.artifact_content)
        .bind(&input.artifact_metadata_json)
        .bind(&input.plan_digest)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
             VALUES (?, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.producer_execution_id)
        .bind(&input.task_id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO execution_artifact_output (
                execution_id, artifact_id, task_id, kind, digest, created_at
             ) VALUES (?, ?, ?, 'work_unit_plan', ?, ?)",
        )
        .bind(&input.producer_execution_id)
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.plan_digest)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "INSERT INTO task_coordination_plan (
                artifact_id, task_id, request_id, authorization_version,
                plan_digest, state, proposal_id, decision_id, version,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1, ?, ?)",
        )
        .bind(&input.artifact_id)
        .bind(&input.task_id)
        .bind(&input.request_id)
        .bind(input.authorization_version)
        .bind(&input.plan_digest)
        .bind(input.plan_state.to_string())
        .bind(input.proposal_id.as_deref())
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query(
            "UPDATE task_coordination_request
             SET state = ?, plan_artifact_id = ?, version = version + 1,
                 updated_at = ?, completed_at = ?,
                 failure_code = NULL, failure_message = NULL
             WHERE id = ? AND version = ?",
        )
        .bind(input.request_state.to_string())
        .bind(&input.artifact_id)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .bind(&input.request_id)
        .bind(request.version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        DomainEventRepo::append_event_in_tx(self, &mut tx, &input.artifact_event).await?;
        DomainEventRepo::append_event_in_tx(self, &mut tx, &input.plan_event).await?;
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(&input.artifact_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_plan)?;
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_request)?;
        tx.commit().await?;
        Ok(TaskCoordinationPlanWrite { plan, request })
    }

    async fn get_task_coordination_plan(
        &self,
        artifact_id: &str,
    ) -> Result<Option<TaskCoordinationPlan>> {
        sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(artifact_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_plan)
            .transpose()
    }

    async fn list_task_coordination_plans(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskCoordinationPlan>> {
        sqlx::query(
            "SELECT * FROM task_coordination_plan
             WHERE task_id = ? ORDER BY created_at DESC, artifact_id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_task_coordination_plan)
        .collect()
    }

    async fn update_task_coordination_plan_state(
        &self,
        artifact_id: &str,
        expected_version: i64,
        state: TaskCoordinationPlanState,
        proposal_id: Option<Option<String>>,
        decision_id: Option<Option<String>>,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<TaskCoordinationPlan> {
        let mut tx = self.pool.begin().await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_coordination_plan SET state = ");
        query.push_bind(state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(updated_at);
        if let Some(proposal_id) = proposal_id.as_ref() {
            query
                .push(", proposal_id = ")
                .push_bind(proposal_id.as_deref());
        }
        if let Some(decision_id) = decision_id.as_ref() {
            query
                .push(", decision_id = ")
                .push_bind(decision_id.as_deref());
        }
        query
            .push(" WHERE artifact_id = ")
            .push_bind(artifact_id)
            .push(" AND version = ")
            .push_bind(expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, event.as_ref()).await?;
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(artifact_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_task_coordination_plan)?;
        tx.commit().await?;
        Ok(plan)
    }

    async fn materialize_task_coordination_plan(
        &self,
        input: MaterializeTaskCoordinationPlanInput,
    ) -> Result<TaskCoordinationMaterialization> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?",
        )
        .bind(&input.plan_artifact_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_task_coordination_materialization)
        .transpose()?
        {
            if existing.plan_digest != input.plan_digest {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(existing);
        }
        let plan = sqlx::query("SELECT * FROM task_coordination_plan WHERE artifact_id = ?")
            .bind(&input.plan_artifact_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_plan)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if plan.task_id != input.task_id
            || plan.request_id != input.request_id
            || plan.plan_digest != input.plan_digest
            || plan.authorization_version != input.authorization_version
            || plan.version != input.expected_plan_version
            || plan.state != TaskCoordinationPlanState::Approved
        {
            return Err(DbError::VersionConflict);
        }
        let request = sqlx::query("SELECT * FROM task_coordination_request WHERE id = ?")
            .bind(&input.request_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_task_coordination_request)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if request.task_id != input.task_id
            || request.version != input.expected_request_version
            || request.authorization_version != input.authorization_version
            || request.plan_artifact_id.as_deref() != Some(input.plan_artifact_id.as_str())
            || request.state == TaskCoordinationRequestState::Cancelled
        {
            return Err(DbError::VersionConflict);
        }
        let task_version: i64 =
            sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
        if task_version != input.task_version_before {
            return Err(DbError::TaskVersionConflict {
                expected: input.task_version_before,
                actual: task_version,
            });
        }
        for unit in &input.work_units {
            let (actor_kind, actor_id) = coordination_actor_columns(unit.assigned_actor.as_ref());
            sqlx::query(
                "INSERT INTO work_unit (
                    id, task_id, parent_work_unit_id, title, scope, status, role,
                    assigned_actor_kind, assigned_actor_id, requires_integration,
                    provenance_kind, provenance_id, provenance_actor_kind,
                    review_approval_gate_id, review_approval_line_id,
                    created_by_kind, created_by_id, version, created_at, updated_at,
                    execution_instructions
                 ) VALUES (?, ?, ?, ?, ?, 'open', ?, ?, ?, ?, 'artifact', ?, NULL,
                           NULL, NULL, 'human', ?, 1, ?, ?, ?)",
            )
            .bind(&unit.id)
            .bind(&input.task_id)
            .bind(unit.parent_work_unit_id.as_deref())
            .bind(&unit.title)
            .bind(&unit.scope)
            .bind(&unit.role)
            .bind(actor_kind.as_deref())
            .bind(actor_id.as_deref())
            .bind(i64::from(unit.requires_integration))
            .bind(&input.plan_artifact_id)
            .bind(&input.created_by)
            .bind(&input.created_at)
            .bind(&input.created_at)
            .bind(unit.execution_instructions.as_deref())
            .execute(&mut *tx)
            .await
            .map_err(check_error)?;
            for artifact in &unit.artifact_inputs {
                sqlx::query(
                    "INSERT INTO work_unit_artifact_input (
                        work_unit_id, task_id, artifact_id, artifact_digest,
                        producer_execution_id, created_at
                     ) VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&unit.id)
                .bind(&input.task_id)
                .bind(&artifact.artifact_id)
                .bind(&artifact.digest)
                .bind(&artifact.producer_execution_id)
                .bind(&input.created_at)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
            }
        }
        for unit in &input.work_units {
            for dependency in &unit.depends_on_work_unit_ids {
                sqlx::query(
                    "INSERT INTO work_unit_dependency (
                        task_id, work_unit_id, depends_on_work_unit_id,
                        created_by_kind, created_by_id, created_at
                     ) VALUES (?, ?, ?, 'human', ?, ?)",
                )
                .bind(&input.task_id)
                .bind(&unit.id)
                .bind(dependency)
                .bind(&input.created_by)
                .bind(&input.created_at)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
            }
        }
        for unit in &input.work_units {
            if let Some(reviewed_work_unit_id) = unit.review_subject_work_unit_id.as_deref() {
                sqlx::query(
                    "INSERT INTO work_unit_review_subject (
                        review_work_unit_id, task_id, reviewed_work_unit_id,
                        reviewed_work_unit_version, result_execution_id,
                        result_artifact_id, result_commit_sha, result_workspace_id,
                        workspace_snapshot_digest, created_at
                     ) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, ?)",
                )
                .bind(&unit.id)
                .bind(&input.task_id)
                .bind(reviewed_work_unit_id)
                .bind(&input.created_at)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
            }
        }
        let work_unit_ids = input
            .work_units
            .iter()
            .map(|unit| unit.id.clone())
            .collect::<Vec<_>>();
        let work_unit_ids_json = serde_json::to_string(&work_unit_ids)
            .map_err(|error| DbError::Check(format!("serialize WorkUnit IDs: {error}")))?;
        sqlx::query(
            "INSERT INTO task_coordination_materialization (
                id, task_id, request_id, plan_artifact_id, plan_digest,
                authorization_version, task_version_before, task_version_after,
                work_unit_ids_json, receipt_digest, created_by, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.receipt_id)
        .bind(&input.task_id)
        .bind(&input.request_id)
        .bind(&input.plan_artifact_id)
        .bind(&input.plan_digest)
        .bind(input.authorization_version)
        .bind(input.task_version_before)
        .bind(input.task_version_before)
        .bind(&work_unit_ids_json)
        .bind(&input.receipt_digest)
        .bind(&input.created_by)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let plan_update = sqlx::query(
            "UPDATE task_coordination_plan
             SET state = 'materialized', version = version + 1, updated_at = ?
             WHERE artifact_id = ? AND version = ? AND state = 'approved'",
        )
        .bind(&input.created_at)
        .bind(&input.plan_artifact_id)
        .bind(input.expected_plan_version)
        .execute(&mut *tx)
        .await?;
        if plan_update.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let request_update = sqlx::query(
            "UPDATE task_coordination_request
             SET state = 'materialized', version = version + 1, updated_at = ?,
                 completed_at = ?, failure_code = NULL, failure_message = NULL
             WHERE id = ? AND version = ? AND state != 'cancelled'",
        )
        .bind(&input.created_at)
        .bind(&input.created_at)
        .bind(&input.request_id)
        .bind(input.expected_request_version)
        .execute(&mut *tx)
        .await?;
        if request_update.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        for event in &input.events {
            DomainEventRepo::append_event_in_tx(self, &mut tx, event).await?;
        }
        let receipt = sqlx::query(
            "SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?",
        )
        .bind(&input.plan_artifact_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)
        .and_then(map_task_coordination_materialization)?;
        tx.commit().await?;
        Ok(receipt)
    }

    async fn get_task_coordination_materialization(
        &self,
        plan_artifact_id: &str,
    ) -> Result<Option<TaskCoordinationMaterialization>> {
        sqlx::query("SELECT * FROM task_coordination_materialization WHERE plan_artifact_id = ?")
            .bind(plan_artifact_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_coordination_materialization)
            .transpose()
    }

    async fn create_task_quality_request(
        &self,
        input: CreateTaskQualityRequestInput,
    ) -> Result<TaskQualityWrite> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_quality_request WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&input.task_id)
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_quality_request)
        .transpose()?
        {
            if existing.request_digest != input.request_digest {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(TaskQualityWrite {
                request: existing,
                event: None,
                replayed: true,
            });
        }
        let budget_round: i64 = sqlx::query_scalar(
            "SELECT quality_request_count + 1 FROM task_coordination WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let (actor_kind, actor_id) = coordination_actor_columns(input.target_actor.as_ref());
        sqlx::query(
            "INSERT INTO task_quality_request (
                id, task_id, source_kind, source_id, category, target_kind,
                target_task_role_id, target_actor_kind, target_actor_id, summary,
                findings_json, resolution_condition, state, handoff_id, work_unit_id,
                budget_round, source_digest, request_digest, idempotency_key,
                created_by_kind, created_by_id, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.source_kind)
        .bind(&input.source_id)
        .bind(input.category.to_string())
        .bind(input.target_kind.to_string())
        .bind(input.target_task_role_id.as_deref())
        .bind(actor_kind.as_deref())
        .bind(actor_id.as_deref())
        .bind(&input.summary)
        .bind(&input.findings_json)
        .bind(&input.resolution_condition)
        .bind(input.state.to_string())
        .bind(input.handoff_id.as_deref())
        .bind(input.work_unit_id.as_deref())
        .bind(budget_round)
        .bind(&input.source_digest)
        .bind(&input.request_digest)
        .bind(&input.idempotency_key)
        .bind(input.created_by.kind().to_string())
        .bind(input.created_by.id())
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        let request = sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_quality_request)?;
        tx.commit().await?;
        Ok(TaskQualityWrite {
            request,
            event: Some(event),
            replayed: false,
        })
    }

    async fn get_task_quality_request(&self, id: &str) -> Result<Option<TaskQualityRequest>> {
        sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_quality_request)
            .transpose()
    }

    async fn get_task_quality_request_by_key(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<Option<TaskQualityRequest>> {
        sqlx::query(
            "SELECT * FROM task_quality_request
             WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(task_id)
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_quality_request)
        .transpose()
    }

    async fn list_task_quality_requests(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskQualityRequest>> {
        sqlx::query(
            "SELECT * FROM task_quality_request
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_quality_request)
        .collect()
    }

    async fn update_task_quality_request_state(
        &self,
        id: &str,
        expected_version: i64,
        state: TaskQualityState,
        handoff_id: Option<Option<String>>,
        work_unit_id: Option<Option<String>>,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<TaskQualityRequest> {
        let mut tx = self.pool.begin().await?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_quality_request SET state = ");
        query.push_bind(state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(updated_at);
        if state == TaskQualityState::Resolved {
            query.push(", resolved_at = ").push_bind(updated_at);
        }
        if let Some(handoff_id) = handoff_id.as_ref() {
            query
                .push(", handoff_id = ")
                .push_bind(handoff_id.as_deref());
        }
        if let Some(work_unit_id) = work_unit_id.as_ref() {
            query
                .push(", work_unit_id = ")
                .push_bind(work_unit_id.as_deref());
        }
        query
            .push(" WHERE id = ")
            .push_bind(id)
            .push(" AND version = ")
            .push_bind(expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, event.as_ref()).await?;
        let request = sqlx::query("SELECT * FROM task_quality_request WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_quality_request)?;
        tx.commit().await?;
        Ok(request)
    }

    async fn create_task_intervention(
        &self,
        input: CreateTaskInterventionInput,
    ) -> Result<TaskInterventionWrite> {
        let mut tx = self.pool.begin().await?;
        if let Some(existing) =
            sqlx::query("SELECT * FROM task_intervention WHERE task_id = ? AND idempotency_key = ?")
                .bind(&input.task_id)
                .bind(&input.idempotency_key)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_intervention)
                .transpose()?
        {
            if existing.request_digest != input.request_digest || existing.kind != input.kind {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(TaskInterventionWrite {
                intervention: existing,
                event: None,
                replayed: true,
            });
        }
        sqlx::query(
            "INSERT INTO task_intervention (
                id, task_id, kind, state, source_kind, source_id, payload_json,
                request_digest, idempotency_key, expected_task_version,
                expected_coordination_version, version, created_at, updated_at
             ) VALUES (?, ?, ?, 'recorded', ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.kind.to_string())
        .bind(&input.source_kind)
        .bind(&input.source_id)
        .bind(&input.payload_json)
        .bind(&input.request_digest)
        .bind(&input.idempotency_key)
        .bind(input.expected_task_version)
        .bind(input.expected_coordination_version)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &input.event).await?;
        let intervention = sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_intervention)?;
        tx.commit().await?;
        Ok(TaskInterventionWrite {
            intervention,
            event: Some(event),
            replayed: false,
        })
    }

    async fn get_task_intervention(&self, id: &str) -> Result<Option<TaskIntervention>> {
        sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_intervention)
            .transpose()
    }

    async fn list_task_interventions(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskIntervention>> {
        sqlx::query(
            "SELECT * FROM task_intervention
             WHERE task_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_intervention)
        .collect()
    }

    async fn update_task_intervention_state(
        &self,
        id: &str,
        expected_version: i64,
        state: TaskInterventionState,
        proposal_id: Option<Option<String>>,
        decision_id: Option<Option<String>>,
        coordination_request_id: Option<Option<String>>,
        failure_code: Option<Option<String>>,
        failure_message: Option<Option<String>>,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<TaskIntervention> {
        let mut tx = self.pool.begin().await?;
        let mut query = sqlx::QueryBuilder::<Sqlite>::new("UPDATE task_intervention SET state = ");
        query.push_bind(state.to_string());
        query
            .push(", version = version + 1, updated_at = ")
            .push_bind(updated_at);
        if let Some(proposal_id) = proposal_id.as_ref() {
            query
                .push(", proposal_id = ")
                .push_bind(proposal_id.as_deref());
        }
        if let Some(decision_id) = decision_id.as_ref() {
            query
                .push(", decision_id = ")
                .push_bind(decision_id.as_deref());
        }
        if let Some(request_id) = coordination_request_id.as_ref() {
            query
                .push(", coordination_request_id = ")
                .push_bind(request_id.as_deref());
        }
        if let Some(failure_code) = failure_code.as_ref() {
            query
                .push(", failure_code = ")
                .push_bind(failure_code.as_deref());
        }
        if let Some(failure_message) = failure_message.as_ref() {
            query
                .push(", failure_message = ")
                .push_bind(failure_message.as_deref());
        }
        query
            .push(" WHERE id = ")
            .push_bind(id)
            .push(" AND version = ")
            .push_bind(expected_version);
        let result = query.build().execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_optional_event(self, &mut tx, event.as_ref()).await?;
        let intervention = sqlx::query("SELECT * FROM task_intervention WHERE id = ?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_intervention)?;
        tx.commit().await?;
        Ok(intervention)
    }
}
