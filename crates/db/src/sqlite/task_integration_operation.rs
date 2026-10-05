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

fn map_remote_pr_admission(row: SqliteRow) -> Result<RemotePrAdmission> {
    Ok(RemotePrAdmission {
        task_merge_operation_id: row.try_get("task_merge_operation_id")?,
        publish_operation_id: row.try_get("publish_operation_id")?,
        metadata_id: row.try_get("metadata_id")?,
        task_id: row.try_get("task_id")?,
        provider_config_id: row.try_get("provider_config_id")?,
        provider_type: row.try_get("provider_type")?,
        provider_config_revision: row.try_get("provider_config_revision")?,
        provider_config_digest: row.try_get("provider_config_digest")?,
        provider_base_url: row.try_get("provider_base_url")?,
        token_secret_ref: row.try_get("token_secret_ref")?,
        remote_repo_identity: row.try_get("remote_repo_identity")?,
        source_branch: row.try_get("source_branch")?,
        target_branch: row.try_get("target_branch")?,
        admitted_source_sha: row.try_get("admitted_source_sha")?,
        state: row.try_get("state")?,
        provider_status: row.try_get("provider_status")?,
        result_classification: row.try_get("result_classification")?,
        reconciliation_reason: row.try_get("reconciliation_reason")?,
        provider_event_id: row.try_get("provider_event_id")?,
        observed_head_sha: row.try_get("observed_head_sha")?,
        merged_commit_sha: row.try_get("merged_commit_sha")?,
        result_event_id: row.try_get("result_event_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_remote_pr_admission(
    tx: &mut Transaction<'_, Sqlite>,
    task_merge_operation_id: &str,
) -> Result<RemotePrAdmission> {
    let row = sqlx::query("SELECT * FROM remote_pr_admission WHERE task_merge_operation_id = ?")
        .bind(task_merge_operation_id)
        .fetch_one(&mut **tx)
        .await?;
    map_remote_pr_admission(row)
}

async fn snapshot_terminal_remote_pr_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    metadata_id: &str,
    archived_at: &str,
) -> Result<()> {
    let current = sqlx::query(
        "SELECT task_id, task_merge_operation_id, publish_operation_id
         FROM pr_metadata WHERE id = ?",
    )
    .bind(metadata_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let current_task_id: String = current.try_get("task_id")?;
    let previous_merge_id: Option<String> = current.try_get("task_merge_operation_id")?;
    let previous_publish_id: Option<String> = current.try_get("publish_operation_id")?;
    let previous_merge_id = previous_merge_id.ok_or(DbError::InvalidTransition)?;
    let previous_publish_id = previous_publish_id.ok_or(DbError::InvalidTransition)?;
    if current_task_id != task_id {
        return Err(DbError::InvalidTransition);
    }
    let admission = load_remote_pr_admission(tx, &previous_merge_id).await?;
    if admission.task_id != task_id
        || admission.metadata_id != metadata_id
        || admission.publish_operation_id != previous_publish_id
        || !matches!(
            admission.state.as_str(),
            "merged" | "closed" | "head_mismatch" | "publication_failed"
        )
    {
        return Err(DbError::InvalidTransition);
    }

    sqlx::query(
        "INSERT INTO remote_pr_history (
            history_id, task_id, original_metadata_id, history_origin,
            task_merge_operation_id, publish_operation_id,
            provider_config_id, provider_type, provider_config_revision,
            provider_config_digest, provider_base_url, token_secret_ref,
            provider_pr_id, pr_url, remote_repo_identity, source_branch,
            target_branch, admitted_source_sha, admission_state, provider_status,
            result_classification, observed_head_sha, merged_commit_sha,
            pr_state, merge_status, admission_status, provider_event_id,
            result_event_id, admission_created_at, result_created_at, archived_at
         )
         SELECT ?, admission.task_id, admission.metadata_id, 'terminal_snapshot',
                admission.task_merge_operation_id, admission.publish_operation_id,
                admission.provider_config_id, admission.provider_type,
                admission.provider_config_revision, admission.provider_config_digest,
                admission.provider_base_url, admission.token_secret_ref,
                metadata.provider_pr_id, metadata.pr_url,
                admission.remote_repo_identity, admission.source_branch,
                admission.target_branch, admission.admitted_source_sha,
                admission.state, admission.provider_status,
                admission.result_classification, admission.observed_head_sha,
                admission.merged_commit_sha, metadata.pr_state, metadata.merge_status,
                metadata.admission_status, admission.provider_event_id,
                admission.result_event_id, admission.created_at, event.created_at, ?
         FROM remote_pr_admission admission
         JOIN pr_metadata metadata
           ON metadata.id = admission.metadata_id
          AND metadata.task_id = admission.task_id
          AND metadata.task_merge_operation_id = admission.task_merge_operation_id
          AND metadata.publish_operation_id = admission.publish_operation_id
         JOIN domain_event event ON event.id = admission.result_event_id
         WHERE admission.task_merge_operation_id = ?
           AND admission.task_id = ?
           AND admission.state IN ('merged', 'closed', 'head_mismatch', 'publication_failed')
           AND NOT EXISTS (
               SELECT 1 FROM remote_pr_history history
               WHERE history.task_merge_operation_id = admission.task_merge_operation_id
           )",
    )
    .bind(new_uuid_v4())
    .bind(archived_at)
    .bind(&previous_merge_id)
    .bind(task_id)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;

    let exact_snapshot: i64 = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM remote_pr_history history
             JOIN remote_pr_admission admission
               ON admission.task_merge_operation_id = history.task_merge_operation_id
             JOIN domain_event result_event
               ON result_event.id = admission.result_event_id
             JOIN pr_metadata metadata
               ON metadata.id = history.original_metadata_id
              AND metadata.task_id = history.task_id
             WHERE history.task_merge_operation_id = ?
               AND history.task_id = ?
               AND history.original_metadata_id = ?
               AND history.publish_operation_id = ?
               AND history.provider_config_id = admission.provider_config_id
               AND history.provider_type = admission.provider_type
               AND history.provider_config_revision = admission.provider_config_revision
               AND history.provider_config_digest = admission.provider_config_digest
               AND history.provider_base_url IS admission.provider_base_url
               AND history.token_secret_ref IS admission.token_secret_ref
               AND history.remote_repo_identity = admission.remote_repo_identity
               AND history.source_branch = admission.source_branch
               AND history.target_branch = admission.target_branch
               AND history.admitted_source_sha = admission.admitted_source_sha
               AND history.admission_state = admission.state
               AND history.provider_status = admission.provider_status
               AND history.result_classification IS admission.result_classification
               AND history.observed_head_sha IS admission.observed_head_sha
               AND history.merged_commit_sha IS admission.merged_commit_sha
               AND history.provider_event_id IS admission.provider_event_id
               AND history.result_event_id = admission.result_event_id
               AND history.admission_created_at = admission.created_at
               AND history.result_created_at = result_event.created_at
               AND history.provider_pr_id IS metadata.provider_pr_id
               AND history.pr_url IS metadata.pr_url
               AND history.provider_type = metadata.provider_type
               AND history.source_branch = metadata.source_branch
               AND history.target_branch = metadata.target_branch
               AND history.pr_state = metadata.pr_state
               AND history.merge_status = metadata.merge_status
               AND history.admission_status = metadata.admission_status
               AND metadata.task_merge_operation_id = admission.task_merge_operation_id
               AND metadata.publish_operation_id = admission.publish_operation_id
         )",
    )
    .bind(&previous_merge_id)
    .bind(task_id)
    .bind(metadata_id)
    .bind(&previous_publish_id)
    .fetch_one(&mut **tx)
    .await?;
    if exact_snapshot != 1 {
        return Err(DbError::InvalidTransition);
    }
    Ok(())
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
    let provider_result: Option<(String, Option<String>)> =
        if let Some(event_id) = lifecycle_operation.result_event_id.as_deref() {
            sqlx::query_as(
                "SELECT json_extract(payload_json, '$.status'),
                        json_extract(payload_json, '$.result_classification')
                 FROM domain_event WHERE id = ?",
            )
            .bind(event_id)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            None
        };
    let provider_status = provider_result.as_ref().map(|result| result.0.as_str());
    let result_classification = provider_result
        .as_ref()
        .and_then(|result| result.1.as_deref());
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
            "provider_status": provider_status,
            "result_classification": result_classification,
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
                } else if result_classification == Some("head_mismatch") {
                    "remote_pr_head_mismatch".to_owned()
                } else if provider_status == Some("closed") {
                    "pr_closed_without_merge".to_owned()
                } else {
                    "task_merge_failed".to_owned()
                }),
                reason_ref: Some(
                    if provider_status == Some("closed")
                        || result_classification == Some("head_mismatch")
                    {
                        lifecycle_operation
                            .result_event_id
                            .clone()
                            .expect("terminal provider result is exact")
                    } else {
                        lifecycle_operation.id.clone()
                    },
                ),
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
         WHERE task_id = ? AND status = 'running' AND remote_waiting = 0
           AND NOT (
               kind = 'publish_pr' AND EXISTS (
                   SELECT 1 FROM task_integration_operation remote_merge
                   WHERE remote_merge.id = task_integration_operation.parent_operation_id
                     AND remote_merge.task_id = task_integration_operation.task_id
                     AND remote_merge.kind = 'task_merge'
                     AND remote_merge.remote_waiting = 1
                     AND remote_merge.status = 'running'
               )
           )",
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
        if input.kind == TaskIntegrationOperationKind::TaskMerge && input.remote_waiting {
            return Err(DbError::InvalidTransition);
        }
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
        mut remote_admission: CreateRemotePrAdmission,
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
            || remote_admission.task_merge_operation_id != merge.id
            || remote_admission.publish_operation_id != publish.id
            || remote_admission.task_id != merge.task_id
            || remote_admission.metadata_id != metadata.id
            || remote_admission.provider_type != metadata.provider_type
            || remote_admission.source_branch != metadata.source_branch
            || remote_admission.target_branch != metadata.target_branch
        {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin().await?;
        insert_operation(&mut tx, &merge).await?;
        admit_merge_lifecycle_in_tx(self, &mut tx, &merge).await?;
        insert_operation(&mut tx, &publish).await?;
        let existing_pr = sqlx::query(
            "SELECT id, merge_status, admission_status, task_merge_operation_id,
                    publish_operation_id
             FROM pr_metadata WHERE task_id = ?",
        )
        .bind(&metadata.task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let metadata_id = if let Some(existing_pr) = existing_pr {
            let merge_status: String = existing_pr.try_get("merge_status")?;
            let admission_status: String = existing_pr.try_get("admission_status")?;
            let metadata_id: String = existing_pr.try_get("id")?;
            if admission_status == "legacy_unadmitted" {
                if !matches!(
                    merge_status.as_str(),
                    "merged" | "closed_without_merge" | "publication_failed"
                ) {
                    return Err(DbError::InvalidTransition);
                }
                let archived_at = metadata.updated_at.clone();
                let archived = sqlx::query(
                    "INSERT INTO legacy_pr_history (
                         history_id, task_id, legacy_metadata_id, provider_type,
                         provider_pr_id, pr_url, source_branch, target_branch,
                         pr_state, merge_status, admission_status,
                         task_merge_operation_id, publish_operation_id,
                         last_synced_at, metadata_created_at, metadata_updated_at,
                         archived_at
                     )
                     SELECT ?, task_id, id, provider_type, provider_pr_id, pr_url,
                            source_branch, target_branch, pr_state, merge_status,
                            admission_status, task_merge_operation_id,
                            publish_operation_id, last_synced_at, created_at,
                            updated_at, ?
                     FROM pr_metadata
                     WHERE id = ? AND task_id = ?
                       AND admission_status = 'legacy_unadmitted'
                       AND merge_status IN ('merged', 'closed_without_merge',
                                            'publication_failed')
                       AND NOT EXISTS (
                           SELECT 1 FROM remote_pr_admission admission
                           WHERE admission.metadata_id = pr_metadata.id
                       )",
                )
                .bind(new_uuid_v4())
                .bind(&archived_at)
                .bind(&metadata_id)
                .bind(&metadata.task_id)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?
                .rows_affected();
                if archived != 1 {
                    return Err(DbError::InvalidTransition);
                }
                let deleted = sqlx::query(
                    "DELETE FROM pr_metadata
                     WHERE id = ? AND task_id = ? AND admission_status = 'legacy_unadmitted'",
                )
                .bind(&metadata_id)
                .bind(&metadata.task_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if deleted != 1 {
                    return Err(DbError::VersionConflict);
                }
                super::pr_metadata::insert_pr_metadata_in_tx(&mut tx, &metadata).await?;
                metadata.id.clone()
            } else {
                let previous_merge_id: Option<String> =
                    existing_pr.try_get("task_merge_operation_id")?;
                let previous_publish_id: Option<String> =
                    existing_pr.try_get("publish_operation_id")?;
                let Some(previous_merge_id) = previous_merge_id else {
                    return Err(DbError::InvalidTransition);
                };
                let Some(previous_publish_id) = previous_publish_id else {
                    return Err(DbError::InvalidTransition);
                };
                let previous_admission =
                    load_remote_pr_admission(&mut tx, &previous_merge_id).await?;
                if previous_admission.task_id != metadata.task_id
                    || previous_admission.metadata_id != metadata_id
                    || previous_admission.publish_operation_id != previous_publish_id
                    || !matches!(
                        previous_admission.state.as_str(),
                        "merged" | "closed" | "head_mismatch" | "publication_failed"
                    )
                {
                    return Err(DbError::InvalidTransition);
                }
                snapshot_terminal_remote_pr_in_tx(
                    &mut tx,
                    &metadata.task_id,
                    &metadata_id,
                    &metadata.updated_at,
                )
                .await?;
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
                .bind(&metadata_id)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
                metadata_id
            }
        } else {
            super::pr_metadata::insert_pr_metadata_in_tx(&mut tx, &metadata).await?;
            metadata.id.clone()
        };
        remote_admission.metadata_id = metadata_id.clone();
        sqlx::query(
            "INSERT INTO remote_pr_admission (
                task_merge_operation_id, publish_operation_id, metadata_id, task_id,
                provider_config_id, provider_type, provider_config_revision,
                provider_config_digest, provider_base_url, token_secret_ref,
                remote_repo_identity, source_branch, target_branch, admitted_source_sha,
                state, reconciliation_reason, provider_event_id, observed_head_sha,
                merged_commit_sha, result_event_id, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'admitted', NULL,
                       NULL, NULL, NULL, NULL, ?, ?)",
        )
        .bind(&remote_admission.task_merge_operation_id)
        .bind(&remote_admission.publish_operation_id)
        .bind(&remote_admission.metadata_id)
        .bind(&remote_admission.task_id)
        .bind(&remote_admission.provider_config_id)
        .bind(&remote_admission.provider_type)
        .bind(&remote_admission.provider_config_revision)
        .bind(&remote_admission.provider_config_digest)
        .bind(remote_admission.provider_base_url.as_deref())
        .bind(remote_admission.token_secret_ref.as_deref())
        .bind(&remote_admission.remote_repo_identity)
        .bind(&remote_admission.source_branch)
        .bind(&remote_admission.target_branch)
        .bind(&remote_admission.admitted_source_sha)
        .bind(&remote_admission.created_at)
        .bind(&remote_admission.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        sqlx::query("UPDATE pr_metadata SET admission_status = 'admitted' WHERE id = ?")
            .bind(&metadata_id)
            .execute(&mut *tx)
            .await
            .map_err(check_error)?;
        let merge_operation = load_operation(&mut tx, &merge.id).await?;
        let publish_operation = load_operation(&mut tx, &publish.id).await?;
        tx.commit().await?;
        Ok((merge_operation, publish_operation))
    }

    async fn get_remote_pr_admission(
        &self,
        task_merge_operation_id: &str,
    ) -> Result<Option<RemotePrAdmission>> {
        sqlx::query("SELECT * FROM remote_pr_admission WHERE task_merge_operation_id = ?")
            .bind(task_merge_operation_id)
            .fetch_optional(self.pool())
            .await?
            .map(map_remote_pr_admission)
            .transpose()
    }

    async fn record_remote_pr_outcome(
        &self,
        input: RecordRemotePrOutcome,
    ) -> Result<Option<DomainEvent>> {
        const ACCEPTED: &[&str] = &[
            "open",
            "merged",
            "closed",
            "publication_failed",
            "reconciliation_required",
        ];
        if !ACCEPTED.contains(&input.status.as_str()) {
            return Err(DbError::Check("unknown remote PR outcome".to_owned()));
        }

        let mut tx = self.pool.begin().await?;
        let admission = load_remote_pr_admission(&mut tx, &input.task_merge_operation_id).await?;
        if admission.publish_operation_id != input.publish_operation_id
            || admission.task_id != input.expected_task_id
            || admission.metadata_id != input.metadata_id
            || admission.provider_config_id != input.provider_config_id
            || admission.provider_config_digest != input.provider_config_digest
            || admission.remote_repo_identity != input.remote_repo_identity
            || admission.source_branch != input.source_branch
            || admission.target_branch != input.target_branch
        {
            return Err(DbError::InvalidTransition);
        }
        let mut merge = load_operation(&mut tx, &admission.task_merge_operation_id).await?;
        let mut publish = load_operation(&mut tx, &admission.publish_operation_id).await?;
        if merge.task_id != admission.task_id
            || merge.kind != TaskIntegrationOperationKind::TaskMerge
            || !merge.remote_waiting
            || merge.gate_evaluation_id != publish.gate_evaluation_id
            || publish.task_id != admission.task_id
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.parent_operation_id.as_deref() != Some(merge.id.as_str())
        {
            return Err(DbError::InvalidTransition);
        }

        let terminal_admission = matches!(
            admission.state.as_str(),
            "merged" | "closed" | "head_mismatch" | "publication_failed"
        );
        if terminal_admission || merge.status != TaskIntegrationOperationStatus::Running {
            if input.status == "reconciliation_required" {
                return Err(DbError::InvalidTransition);
            }
            if input.status == "merged"
                && input.merged_commit_sha.as_deref().is_none_or(str::is_empty)
            {
                return Err(DbError::Check(
                    "merged PR result requires the provider merged commit SHA".to_owned(),
                ));
            }
            if input.status != "publication_failed"
                && (input.provider_event_id.as_deref().is_none_or(str::is_empty)
                    || input.provider_pr_id.as_deref().is_none_or(str::is_empty))
            {
                return Err(DbError::Check(
                    "provider PR observations require exact provider event and PR identities"
                        .to_owned(),
                ));
            }
            if input.remote_repo_identity != admission.remote_repo_identity {
                return Err(DbError::InvalidTransition);
            }
            let result_classification = if input.status == "merged"
                && input.observed_head_sha.as_deref()
                    != Some(admission.admitted_source_sha.as_str())
            {
                Some("head_mismatch")
            } else {
                None
            };
            let provider_event_key = input
                .provider_event_id
                .as_deref()
                .unwrap_or("definitive-publication-rejection");
            let dedupe_key = format!(
                "remote-pr-result:{}:{}",
                admission.task_merge_operation_id, provider_event_key
            );
            let mut incoming_payload = serde_json::json!({
                "task_id": admission.task_id,
                "pr_metadata_id": admission.metadata_id,
                "provider_pr_id": input.provider_pr_id,
                "provider_type": admission.provider_type,
                "provider_config_id": admission.provider_config_id,
                "provider_config_revision": admission.provider_config_revision,
                "provider_config_digest": admission.provider_config_digest,
                "remote_repo_identity": admission.remote_repo_identity,
                "source_branch": admission.source_branch,
                "target_branch": admission.target_branch,
                "admitted_source_sha": admission.admitted_source_sha,
                "head_sha": input.observed_head_sha,
                "merged_commit_sha": input.merged_commit_sha,
                "provider_event_id": input.provider_event_id,
                "pr_url": input.pr_url,
                "reconciliation_reason": input.reconciliation_reason,
                "status": input.status,
                "task_merge_operation_id": admission.task_merge_operation_id,
                "publish_operation_id": admission.publish_operation_id,
            });
            if let Some(classification) = result_classification {
                incoming_payload["result_classification"] =
                    serde_json::Value::String(classification.to_owned());
            }
            let prior_event =
                sqlx::query("SELECT id, payload_json FROM domain_event WHERE dedupe_key = ?")
                    .bind(&dedupe_key)
                    .fetch_optional(&mut *tx)
                    .await?;
            let Some(prior_event) = prior_event else {
                return Err(DbError::InvalidTransition);
            };
            let prior_event_id: String = prior_event.try_get("id")?;
            let prior_payload: String = prior_event.try_get("payload_json")?;
            let mut prior_payload: serde_json::Value = serde_json::from_str(&prior_payload)
                .map_err(|error| DbError::Check(format!("invalid stored PR result: {error}")))?;
            if admission.result_event_id.as_deref() == Some(prior_event_id.as_str())
                && admission.state == "head_mismatch"
                && admission.provider_status.as_deref() == Some("merged")
                && admission.result_classification.as_deref() == Some("head_mismatch")
                && input.status == "merged"
                && input.observed_head_sha.as_deref()
                    != Some(admission.admitted_source_sha.as_str())
                && prior_payload
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    == Some("head_mismatch")
            {
                prior_payload["status"] = serde_json::Value::String("merged".to_owned());
                prior_payload["result_classification"] =
                    serde_json::Value::String("head_mismatch".to_owned());
            }
            if prior_payload != incoming_payload {
                return Err(DbError::InvalidTransition);
            }
            tx.commit().await?;
            return DomainEventRepo::get_event(self, &prior_event_id).await;
        }

        let metadata_row = sqlx::query(
            "SELECT task_id, task_merge_operation_id, publish_operation_id,
                    source_branch, target_branch, pr_state
             FROM pr_metadata WHERE id = ?",
        )
        .bind(&admission.metadata_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        if metadata_row.try_get::<String, _>("task_id")? != admission.task_id
            || metadata_row
                .try_get::<Option<String>, _>("task_merge_operation_id")?
                .as_deref()
                != Some(admission.task_merge_operation_id.as_str())
            || metadata_row
                .try_get::<Option<String>, _>("publish_operation_id")?
                .as_deref()
                != Some(admission.publish_operation_id.as_str())
            || metadata_row.try_get::<String, _>("source_branch")? != admission.source_branch
            || metadata_row.try_get::<String, _>("target_branch")? != admission.target_branch
        {
            return Err(DbError::InvalidTransition);
        }
        let current_pr_state: String = metadata_row.try_get("pr_state")?;

        let provider_status = input.status.clone();
        let mut resulting_state = input.status.clone();
        let mut resulting_reason = input.reconciliation_reason.clone();
        let result_classification = if input.status == "merged"
            && input.observed_head_sha.as_deref() != Some(admission.admitted_source_sha.as_str())
        {
            Some("head_mismatch")
        } else {
            None
        };
        if input.status == "merged"
            && input.observed_head_sha.as_deref() != Some(admission.admitted_source_sha.as_str())
        {
            resulting_state = "head_mismatch".to_owned();
            resulting_reason =
                Some("provider merged a head other than the admitted source SHA".to_owned());
        } else if input.status == "open"
            && input.observed_head_sha.as_deref() != Some(admission.admitted_source_sha.as_str())
        {
            resulting_state = "reconciliation_required".to_owned();
            resulting_reason = Some("open PR head differs from the admitted source SHA".to_owned());
        }
        if input.status == "merged" && input.merged_commit_sha.as_deref().is_none_or(str::is_empty)
        {
            return Err(DbError::Check(
                "merged PR result requires the provider merged commit SHA".to_owned(),
            ));
        }
        if input.status != "reconciliation_required" {
            if input.status != "publication_failed"
                && (input.provider_event_id.as_deref().is_none_or(str::is_empty)
                    || input.provider_pr_id.as_deref().is_none_or(str::is_empty))
            {
                return Err(DbError::Check(
                    "provider PR observations require exact provider event and PR identities"
                        .to_owned(),
                ));
            }
            if input.remote_repo_identity != admission.remote_repo_identity {
                return Err(DbError::InvalidTransition);
            }
        }

        let event_id = if input.status == "reconciliation_required" {
            None
        } else {
            let provider_event_key = input
                .provider_event_id
                .as_deref()
                .unwrap_or("definitive-publication-rejection");
            let dedupe_key = format!(
                "remote-pr-result:{}:{}",
                admission.task_merge_operation_id, provider_event_key
            );
            let mut result_payload = serde_json::json!({
                "task_id": admission.task_id,
                "pr_metadata_id": admission.metadata_id,
                "provider_pr_id": input.provider_pr_id,
                "provider_type": admission.provider_type,
                "provider_config_id": admission.provider_config_id,
                "provider_config_revision": admission.provider_config_revision,
                "provider_config_digest": admission.provider_config_digest,
                "remote_repo_identity": admission.remote_repo_identity,
                "source_branch": admission.source_branch,
                "target_branch": admission.target_branch,
                "admitted_source_sha": admission.admitted_source_sha,
                "head_sha": input.observed_head_sha,
                "merged_commit_sha": input.merged_commit_sha,
                "provider_event_id": input.provider_event_id,
                "pr_url": input.pr_url,
                "reconciliation_reason": input.reconciliation_reason,
                "status": provider_status,
                "task_merge_operation_id": admission.task_merge_operation_id,
                "publish_operation_id": admission.publish_operation_id,
            });
            if let Some(classification) = result_classification {
                result_payload["result_classification"] =
                    serde_json::Value::String(classification.to_owned());
            }
            let create_event = CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "pr.status_changed".to_owned(),
                entity_type: "pr_metadata".to_owned(),
                entity_id: admission.metadata_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: admission.task_id.clone(),
                correlation_id: admission.task_merge_operation_id.clone(),
                causation_id: Some(admission.publish_operation_id.clone()),
                causation_depth: 1,
                dedupe_key: Some(dedupe_key.clone()),
                payload_json: result_payload.to_string(),
                created_at: input.updated_at.clone(),
            };
            let prior_event =
                sqlx::query("SELECT id, payload_json FROM domain_event WHERE dedupe_key = ?")
                    .bind(&dedupe_key)
                    .fetch_optional(&mut *tx)
                    .await?;
            if let Some(prior_event) = prior_event {
                let prior_event_id: String = prior_event.try_get("id")?;
                let prior_payload: String = prior_event.try_get("payload_json")?;
                let prior_payload: serde_json::Value = serde_json::from_str(&prior_payload)
                    .map_err(|error| {
                        DbError::Check(format!("invalid stored PR result: {error}"))
                    })?;
                let incoming_payload: serde_json::Value =
                    serde_json::from_str(&create_event.payload_json).map_err(|error| {
                        DbError::Check(format!("invalid incoming PR result: {error}"))
                    })?;
                let mut comparable_prior = prior_payload.clone();
                if admission.result_event_id.as_deref() == Some(prior_event_id.as_str())
                    && admission.state == "head_mismatch"
                    && admission.provider_status.as_deref() == Some("merged")
                    && admission.result_classification.as_deref() == Some("head_mismatch")
                    && input.status == "merged"
                    && input.observed_head_sha.as_deref()
                        != Some(admission.admitted_source_sha.as_str())
                    && comparable_prior
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        == Some("head_mismatch")
                {
                    comparable_prior["status"] = serde_json::Value::String("merged".to_owned());
                    comparable_prior["result_classification"] =
                        serde_json::Value::String("head_mismatch".to_owned());
                }
                if comparable_prior != incoming_payload {
                    return Err(DbError::InvalidTransition);
                }
                tx.commit().await?;
                return DomainEventRepo::get_event(self, &prior_event_id).await;
            }
            let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &create_event).await?;
            Some(event.id)
        };

        let already_terminal = merge.status != TaskIntegrationOperationStatus::Running;
        if already_terminal
            || admission.state == "merged"
            || admission.state == "closed"
            || admission.state == "head_mismatch"
            || admission.state == "publication_failed"
        {
            return Err(DbError::InvalidTransition);
        }

        sqlx::query(
            "UPDATE remote_pr_admission
             SET state = ?, reconciliation_reason = ?,
                 provider_status = COALESCE(?, provider_status),
                 result_classification = COALESCE(?, result_classification),
                 provider_event_id = COALESCE(?, provider_event_id),
                 observed_head_sha = COALESCE(?, observed_head_sha),
                 merged_commit_sha = COALESCE(?, merged_commit_sha),
                 result_event_id = COALESCE(?, result_event_id), updated_at = ?
             WHERE task_merge_operation_id = ?",
        )
        .bind(&resulting_state)
        .bind(resulting_reason.as_deref())
        .bind(if input.status == "reconciliation_required" {
            None
        } else {
            Some(provider_status.as_str())
        })
        .bind(result_classification)
        .bind(input.provider_event_id.as_deref())
        .bind(input.observed_head_sha.as_deref())
        .bind(input.merged_commit_sha.as_deref())
        .bind(event_id.as_deref())
        .bind(&input.updated_at)
        .bind(&admission.task_merge_operation_id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;

        let metadata_admission_status = match resulting_state.as_str() {
            "admitted" => "admitted",
            "reconciliation_required" => "reconciliation_required",
            "open" => "open",
            "merged" => "merged",
            "closed" => "closed",
            "head_mismatch" | "publication_failed" => "failed",
            _ => return Err(DbError::InvalidTransition),
        };
        let merge_status = match provider_status.as_str() {
            "open" => "pending",
            "merged" => "merged",
            "closed" => "closed_without_merge",
            "publication_failed" => "publication_failed",
            _ if resulting_state == "reconciliation_required" => "pending",
            _ => return Err(DbError::InvalidTransition),
        };
        let pr_state = if resulting_state == "reconciliation_required" {
            current_pr_state.as_str()
        } else if provider_status == "publication_failed" {
            "failed"
        } else {
            provider_status.as_str()
        };
        sqlx::query(
            "UPDATE pr_metadata
             SET provider_pr_id = COALESCE(?, provider_pr_id),
                 pr_url = COALESCE(?, pr_url), pr_state = ?, merge_status = ?,
                 admission_status = ?, last_synced_at = ?, updated_at = ?
             WHERE id = ? AND task_merge_operation_id = ?
               AND publish_operation_id = ?",
        )
        .bind(input.provider_pr_id.as_deref())
        .bind(input.pr_url.as_deref())
        .bind(pr_state)
        .bind(merge_status)
        .bind(metadata_admission_status)
        .bind(&input.updated_at)
        .bind(&input.updated_at)
        .bind(&admission.metadata_id)
        .bind(&admission.task_merge_operation_id)
        .bind(&admission.publish_operation_id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;

        if provider_status == "publication_failed" {
            if publish.status != TaskIntegrationOperationStatus::Running {
                return Err(DbError::InvalidTransition);
            }
            let changed = sqlx::query(
                "UPDATE task_integration_operation
                 SET status = 'failed', result_event_id = ?, version = version + 1,
                     updated_at = ?, finished_at = ?
                 WHERE id = ? AND version = ? AND status = 'running'",
            )
            .bind(event_id.as_deref())
            .bind(&input.updated_at)
            .bind(&input.updated_at)
            .bind(&publish.id)
            .bind(publish.version)
            .execute(&mut *tx)
            .await
            .map_err(operation_write_error)?
            .rows_affected();
            if changed != 1 {
                return Err(DbError::VersionConflict);
            }
            publish = load_operation(&mut tx, &publish.id).await?;
            finish_merge_lifecycle_in_tx(
                self,
                &mut tx,
                &publish,
                TaskIntegrationOperationStatus::Failed,
                &input.updated_at,
            )
            .await?;
        } else if provider_status == "reconciliation_required" {
            // An unavailable or ambiguous provider outcome updates only the
            // reconciliation marker. PublishPr and TaskMerge stay running.
        } else {
            if publish.status == TaskIntegrationOperationStatus::Running {
                let changed = sqlx::query(
                    "UPDATE task_integration_operation
                     SET status = 'succeeded', version = version + 1,
                         updated_at = ?, finished_at = ?
                     WHERE id = ? AND version = ? AND status = 'running'",
                )
                .bind(&input.updated_at)
                .bind(&input.updated_at)
                .bind(&publish.id)
                .bind(publish.version)
                .execute(&mut *tx)
                .await?
                .rows_affected();
                if changed != 1 {
                    return Err(DbError::VersionConflict);
                }
            } else if publish.status != TaskIntegrationOperationStatus::Succeeded {
                return Err(DbError::InvalidTransition);
            }

            if matches!(
                resulting_state.as_str(),
                "merged" | "closed" | "head_mismatch"
            ) {
                let terminal_status = if resulting_state == "merged" {
                    TaskIntegrationOperationStatus::Succeeded
                } else {
                    TaskIntegrationOperationStatus::Failed
                };
                let changed = sqlx::query(
                    "UPDATE task_integration_operation
                     SET status = ?, result_event_id = ?, version = version + 1,
                         updated_at = ?, finished_at = ?
                     WHERE id = ? AND version = ? AND status = 'running'
                       AND remote_waiting = 1",
                )
                .bind(terminal_status.to_string())
                .bind(event_id.as_deref())
                .bind(&input.updated_at)
                .bind(&input.updated_at)
                .bind(&merge.id)
                .bind(merge.version)
                .execute(&mut *tx)
                .await
                .map_err(operation_write_error)?
                .rows_affected();
                if changed != 1 {
                    return Err(DbError::VersionConflict);
                }
                merge = load_operation(&mut tx, &merge.id).await?;
                finish_merge_lifecycle_in_tx(
                    self,
                    &mut tx,
                    &merge,
                    terminal_status,
                    &input.updated_at,
                )
                .await?;
            }
        }

        tx.commit().await?;
        match event_id {
            Some(event_id) => DomainEventRepo::get_event(self, &event_id).await,
            None => Ok(None),
        }
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
        if input.kind == TaskIntegrationOperationKind::TaskMerge && input.remote_waiting {
            return Err(DbError::InvalidTransition);
        }
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
        if current.kind == TaskIntegrationOperationKind::TaskMerge && current.remote_waiting {
            return Err(DbError::InvalidTransition);
        }
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
