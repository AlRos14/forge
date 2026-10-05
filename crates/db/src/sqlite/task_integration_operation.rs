use super::*;

fn map_operation(row: SqliteRow) -> Result<TaskIntegrationOperation> {
    Ok(TaskIntegrationOperation {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        kind: parse_enum(row.try_get::<String, _>("kind")?)?,
        owner_id: row.try_get("owner_id")?,
        status: parse_enum(row.try_get::<String, _>("status")?)?,
        gate_evaluation_id: row.try_get("gate_evaluation_id")?,
        remote_waiting: row.try_get::<i64, _>("remote_waiting")? != 0,
        parent_operation_id: row.try_get("parent_operation_id")?,
        result_event_id: row.try_get("result_event_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

fn operation_write_error(error: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(database_error) = &error {
        let message = database_error.message().to_ascii_lowercase();
        if message.contains("task_integration_operation.task_id")
            || message.contains("active terminal session")
        {
            return DbError::TaskIntegrationOperationBusy;
        }
        if message.contains("task integration operation") && message.contains("immutable") {
            return DbError::VersionConflict;
        }
    }
    check_error(error)
}

async fn admit_merge_lifecycle_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateTaskIntegrationOperation,
) -> Result<DomainEvent> {
    let gate_evaluation_id = input
        .gate_evaluation_id
        .as_deref()
        .ok_or(DbError::InvalidTransition)?;
    let task_version: i64 =
        sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&input.task_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(DbError::NotFound)?;
    let row = sqlx::query(
        "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         FROM task_lifecycle WHERE task_id = ?",
    )
    .bind(&input.task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let lifecycle = super::gate_lifecycle::map_task_lifecycle(&row)?;
    if lifecycle.state != TaskLifecycleState::ReadyToMerge {
        return Err(DbError::InvalidTransition);
    }
    let key = format!("task-merge-admission:{}", input.id);
    let event = CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "task.lifecycle_changed".to_owned(),
        entity_type: "task".to_owned(),
        entity_id: input.task_id.clone(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: input.task_id.clone(),
        correlation_id: key.clone(),
        causation_id: Some(input.id.clone()),
        causation_depth: 1,
        dedupe_key: Some(key.clone()),
        payload_json: serde_json::json!({
            "task_id": input.task_id,
            "from_state": lifecycle.state,
            "to_state": TaskLifecycleState::Merging,
            "cause_kind": "merge_operation",
            "cause_ref": input.id,
            "gate_evaluation_id": gate_evaluation_id,
            "task_merge_operation_id": input.id,
        })
        .to_string(),
        created_at: input.created_at.clone(),
    };
    let (updated_lifecycle, event) = super::gate_lifecycle::record_lifecycle_transition_in_tx(
        db,
        tx,
        &TransitionTaskLifecycle {
            id: new_uuid_v4(),
            task_id: input.task_id.clone(),
            expected_task_version: task_version,
            expected_lifecycle_version: lifecycle.version,
            expected_state: lifecycle.state,
            to_state: TaskLifecycleState::Merging,
            cause_kind: "merge_operation".to_owned(),
            cause_ref: Some(input.id.clone()),
            reason_kind: Some("task_merge_admitted".to_owned()),
            reason_ref: Some(input.id.clone()),
            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
            idempotency_key: key,
            updated_at: input.created_at.clone(),
            event: event.clone(),
        },
    )
    .await?;
    let _ = updated_lifecycle;
    let changed = sqlx::query(
        "UPDATE task SET status = ?, entry_barrier_json = NULL,
             version = version + 1, updated_at = ?
         WHERE id = ? AND version = ? AND deleted_at IS NULL",
    )
    .bind(TaskLifecycleState::Merging.legacy_projection())
    .bind(&input.created_at)
    .bind(&input.task_id)
    .bind(task_version)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(DbError::VersionConflict);
    }
    Ok(event)
}

async fn finish_merge_lifecycle_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    operation: &TaskIntegrationOperation,
    status: TaskIntegrationOperationStatus,
    updated_at: &str,
) -> Result<Option<DomainEvent>> {
    let mut lifecycle_operation = operation.clone();
    let publication_operation_id = if operation.kind == TaskIntegrationOperationKind::PublishPr
        && status != TaskIntegrationOperationStatus::Succeeded
    {
        let parent_id = operation
            .parent_operation_id
            .as_deref()
            .ok_or(DbError::InvalidTransition)?;
        let parent = load_operation(tx, parent_id).await?;
        if parent.kind != TaskIntegrationOperationKind::TaskMerge
            || !parent.remote_waiting
            || parent.status != TaskIntegrationOperationStatus::Running
            || parent.gate_evaluation_id != operation.gate_evaluation_id
        {
            return Err(DbError::InvalidTransition);
        }
        let changed = sqlx::query(
            "UPDATE task_integration_operation
             SET status = 'failed', result_event_id = ?,
                 version = version + 1, updated_at = ?, finished_at = ?
             WHERE id = ? AND version = ? AND status = 'running' AND remote_waiting = 1",
        )
        .bind(operation.result_event_id.as_deref())
        .bind(updated_at)
        .bind(updated_at)
        .bind(&parent.id)
        .bind(parent.version)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if changed != 1 {
            return Err(DbError::VersionConflict);
        }
        lifecycle_operation = load_operation(tx, &parent.id).await?;
        Some(operation.id.as_str())
    } else {
        None
    };
    if lifecycle_operation.kind != TaskIntegrationOperationKind::TaskMerge {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         FROM task_lifecycle WHERE task_id = ?",
    )
    .bind(&lifecycle_operation.task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let lifecycle = super::gate_lifecycle::map_task_lifecycle(&row)?;
    if lifecycle.state != TaskLifecycleState::Merging {
        return Err(DbError::InvalidTransition);
    }
    let task_version: i64 =
        sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&lifecycle_operation.task_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(DbError::NotFound)?;
    let terminal_status = lifecycle_operation.status;
    let provider_status: Option<String> =
        if let Some(event_id) = lifecycle_operation.result_event_id.as_deref() {
            sqlx::query_scalar(
                "SELECT json_extract(payload_json, '$.status') FROM domain_event WHERE id = ?",
            )
            .bind(event_id)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            None
        };
    let target = if terminal_status == TaskIntegrationOperationStatus::Succeeded {
        TaskLifecycleState::Done
    } else {
        TaskLifecycleState::Blocked
    };
    let key = format!("task-merge-terminal:{}", lifecycle_operation.id);
    let event = CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "task.lifecycle_changed".to_owned(),
        entity_type: "task".to_owned(),
        entity_id: lifecycle_operation.task_id.clone(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: lifecycle_operation.task_id.clone(),
        correlation_id: key.clone(),
        causation_id: Some(lifecycle_operation.id.clone()),
        causation_depth: 1,
        dedupe_key: Some(key.clone()),
        payload_json: serde_json::json!({
            "task_id": lifecycle_operation.task_id,
            "from_state": lifecycle.state,
            "to_state": target,
            "cause_kind": "merge_operation",
            "cause_ref": lifecycle_operation.id,
            "task_merge_status": terminal_status.to_string(),
            "publish_operation_id": publication_operation_id,
            "provider_result_event_id": lifecycle_operation.result_event_id,
        })
        .to_string(),
        created_at: updated_at.to_owned(),
    };
    let (_updated_lifecycle, transition_event) =
        super::gate_lifecycle::record_lifecycle_transition_in_tx(
            db,
            tx,
            &TransitionTaskLifecycle {
                id: new_uuid_v4(),
                task_id: lifecycle_operation.task_id.clone(),
                expected_task_version: task_version,
                expected_lifecycle_version: lifecycle.version,
                expected_state: lifecycle.state,
                to_state: target,
                cause_kind: "merge_operation".to_owned(),
                cause_ref: Some(lifecycle_operation.id.clone()),
                reason_kind: Some(if target == TaskLifecycleState::Done {
                    "task_merge_succeeded".to_owned()
                } else if provider_status.as_deref() == Some("closed") {
                    "pr_closed_without_merge".to_owned()
                } else {
                    "task_merge_failed".to_owned()
                }),
                reason_ref: Some(if provider_status.as_deref() == Some("closed") {
                    lifecycle_operation
                        .result_event_id
                        .clone()
                        .expect("closed PR result is exact")
                } else {
                    lifecycle_operation.id.clone()
                }),
                gate_evaluation_id: None,
                idempotency_key: key,
                updated_at: updated_at.to_owned(),
                event: event.clone(),
            },
        )
        .await?;
    let changed = sqlx::query(
        "UPDATE task SET status = ?, version = version + 1, updated_at = ?
         WHERE id = ? AND version = ? AND deleted_at IS NULL",
    )
    .bind(target.legacy_projection())
    .bind(updated_at)
    .bind(&lifecycle_operation.task_id)
    .bind(task_version)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(DbError::VersionConflict);
    }
    Ok(Some(transition_event))
}

async fn insert_operation(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateTaskIntegrationOperation,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO task_integration_operation (
                id, task_id, kind, owner_id, status, version, created_at, updated_at, finished_at,
            gate_evaluation_id, remote_waiting, parent_operation_id, result_event_id
         ) VALUES (?, ?, ?, ?, 'running', 1, ?, ?, NULL, ?, ?, ?, NULL)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(input.kind.to_string())
    .bind(&input.owner_id)
    .bind(&input.created_at)
    .bind(&input.created_at)
    .bind(input.gate_evaluation_id.as_deref())
    .bind(if input.remote_waiting { 1_i64 } else { 0_i64 })
    .bind(input.parent_operation_id.as_deref())
    .execute(&mut **tx)
    .await
    .map_err(operation_write_error)?;
    Ok(())
}

async fn load_operation(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<TaskIntegrationOperation> {
    let row = sqlx::query("SELECT * FROM task_integration_operation WHERE id = ?")
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    map_operation(row)
}

async fn abandon_running_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    updated_at: &str,
) -> Result<Option<TaskIntegrationOperation>> {
    let Some(active) = sqlx::query(
        "SELECT * FROM task_integration_operation
         WHERE task_id = ? AND status = 'running' AND remote_waiting = 0",
    )
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    let active = map_operation(active)?;
    let updated = sqlx::query(
        "UPDATE task_integration_operation
         SET status = 'abandoned', version = version + 1,
             updated_at = ?, finished_at = ?
         WHERE id = ? AND version = ? AND status = 'running'",
    )
    .bind(updated_at)
    .bind(updated_at)
    .bind(&active.id)
    .bind(active.version)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::VersionConflict);
    }
    let abandoned = load_operation(tx, &active.id).await?;
    finish_merge_lifecycle_in_tx(
        db,
        tx,
        &abandoned,
        TaskIntegrationOperationStatus::Abandoned,
        updated_at,
    )
    .await?;
    Ok(Some(abandoned))
}

#[async_trait]
impl TaskIntegrationOperationRepo for SqliteDb {
    async fn get_by_id(&self, id: &str) -> Result<Option<TaskIntegrationOperation>> {
        sqlx::query("SELECT * FROM task_integration_operation WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?
            .map(map_operation)
            .transpose()
    }

    async fn begin(
        &self,
        input: CreateTaskIntegrationOperation,
    ) -> Result<TaskIntegrationOperation> {
        let mut tx = self.pool.begin().await?;
        insert_operation(&mut tx, &input).await?;
        if input.kind == TaskIntegrationOperationKind::TaskMerge {
            admit_merge_lifecycle_in_tx(self, &mut tx, &input).await?;
        }
        let operation = load_operation(&mut tx, &input.id).await?;
        tx.commit().await?;
        Ok(operation)
    }

    async fn begin_pull_request_publication(
        &self,
        merge: CreateTaskIntegrationOperation,
        publish: CreateTaskIntegrationOperation,
        metadata: CreatePrMetadata,
    ) -> Result<(TaskIntegrationOperation, TaskIntegrationOperation)> {
        if merge.kind != TaskIntegrationOperationKind::TaskMerge
            || !merge.remote_waiting
            || merge.parent_operation_id.is_some()
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.remote_waiting
            || publish.parent_operation_id.as_deref() != Some(merge.id.as_str())
            || publish.task_id != merge.task_id
            || publish.gate_evaluation_id != merge.gate_evaluation_id
            || metadata.task_id != merge.task_id
            || metadata.task_merge_operation_id != merge.id
            || metadata.publish_operation_id != publish.id
            || metadata.provider_pr_id.is_some()
            || metadata.merge_status != "pending"
        {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin().await?;
        insert_operation(&mut tx, &merge).await?;
        admit_merge_lifecycle_in_tx(self, &mut tx, &merge).await?;
        insert_operation(&mut tx, &publish).await?;
        let existing_pr = sqlx::query("SELECT id, merge_status FROM pr_metadata WHERE task_id = ?")
            .bind(&metadata.task_id)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(existing_pr) = existing_pr {
            let merge_status: String = existing_pr.try_get("merge_status")?;
            if matches!(merge_status.as_str(), "pending" | "legacy_unadmitted") {
                return Err(DbError::InvalidTransition);
            }
            let metadata_id: String = existing_pr.try_get("id")?;
            sqlx::query(
                "UPDATE pr_metadata
                 SET provider_type = ?, provider_pr_id = NULL, pr_url = NULL,
                     source_branch = ?, target_branch = ?, pr_state = 'publishing',
                     merge_status = 'pending', task_merge_operation_id = ?,
                     publish_operation_id = ?, last_synced_at = NULL, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&metadata.provider_type)
            .bind(&metadata.source_branch)
            .bind(&metadata.target_branch)
            .bind(&metadata.task_merge_operation_id)
            .bind(&metadata.publish_operation_id)
            .bind(&metadata.updated_at)
            .bind(metadata_id)
            .execute(&mut *tx)
            .await
            .map_err(check_error)?;
        } else {
            super::pr_metadata::insert_pr_metadata_in_tx(&mut tx, &metadata).await?;
        }
        let merge_operation = load_operation(&mut tx, &merge.id).await?;
        let publish_operation = load_operation(&mut tx, &publish.id).await?;
        tx.commit().await?;
        Ok((merge_operation, publish_operation))
    }

    async fn get_active_for_task(&self, task_id: &str) -> Result<Option<TaskIntegrationOperation>> {
        sqlx::query(
            "SELECT * FROM task_integration_operation
             WHERE task_id = ? AND status = 'running' AND remote_waiting = 0",
        )
        .bind(task_id)
        .fetch_optional(self.pool())
        .await?
        .map(map_operation)
        .transpose()
    }

    async fn abandon_stale(
        &self,
        task_id: &str,
        updated_at: &str,
    ) -> Result<Option<TaskIntegrationOperation>> {
        let mut tx = self.pool.begin().await?;
        let abandoned = abandon_running_in_tx(self, &mut tx, task_id, updated_at).await?;
        tx.commit().await?;
        Ok(abandoned)
    }

    async fn recover_stale_and_begin(
        &self,
        input: CreateTaskIntegrationOperation,
        updated_at: &str,
    ) -> Result<TaskIntegrationOperation> {
        let mut tx = self.pool.begin().await?;
        let abandoned = abandon_running_in_tx(self, &mut tx, &input.task_id, updated_at).await?;
        if abandoned
            .as_ref()
            .is_some_and(|operation| operation.kind == TaskIntegrationOperationKind::TaskMerge)
            && input.kind == TaskIntegrationOperationKind::TaskMerge
        {
            // The old merge may have changed Git before its process died.
            // Persist its exact abandoned failure and block the Task; never
            // start a second merge from the stale admission.
            tx.commit().await?;
            return Err(DbError::InvalidTransition);
        }
        insert_operation(&mut tx, &input).await?;
        if input.kind == TaskIntegrationOperationKind::TaskMerge {
            admit_merge_lifecycle_in_tx(self, &mut tx, &input).await?;
        }
        let operation = load_operation(&mut tx, &input.id).await?;
        tx.commit().await?;
        Ok(operation)
    }

    async fn finish(
        &self,
        input: FinishTaskIntegrationOperation,
    ) -> Result<TaskIntegrationOperation> {
        if input.status == TaskIntegrationOperationStatus::Running {
            return Err(DbError::Check(
                "a Task integration operation can only finish with a terminal status".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let current = load_operation(&mut tx, &input.id).await?;
        if current.status != TaskIntegrationOperationStatus::Running {
            if current.status == input.status && current.result_event_id == input.result_event_id {
                tx.commit().await?;
                return Ok(current);
            }
            return Err(DbError::VersionConflict);
        }
        let updated = sqlx::query(
            "UPDATE task_integration_operation
             SET status = ?, result_event_id = ?, version = version + 1,
                 updated_at = ?, finished_at = ?
             WHERE id = ? AND version = ? AND status = 'running'",
        )
        .bind(input.status.to_string())
        .bind(input.result_event_id.as_deref())
        .bind(&input.updated_at)
        .bind(&input.finished_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(operation_write_error)?;
        if updated.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let operation = load_operation(&mut tx, &input.id).await?;
        finish_merge_lifecycle_in_tx(self, &mut tx, &operation, input.status, &input.updated_at)
            .await?;
        tx.commit().await?;
        Ok(operation)
    }
}
