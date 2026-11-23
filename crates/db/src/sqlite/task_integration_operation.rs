use super::*;
use std::future::Future;

fn map_operation(row: SqliteRow) -> Result<TaskIntegrationOperation> {
    Ok(TaskIntegrationOperation {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        repo_id: crate::sqlite::try_get_optional_text(&row, "repo_id")?,
        repository_merge_batch_id: crate::sqlite::try_get_optional_text(
            &row,
            "repository_merge_batch_id",
        )?,
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
        repo_id: crate::sqlite::try_get_optional_text(&row, "repo_id")?,
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
        rework_outcome: crate::sqlite::try_get_v173_text(&row, "rework_outcome")?,
        supersession_id: crate::sqlite::try_get_v173_text(&row, "supersession_id")?,
        readmits_supersession_id: crate::sqlite::try_get_v173_text(
            &row,
            "readmits_supersession_id",
        )?,
        credential_handle_id: crate::sqlite::try_get_v173_text(&row, "credential_handle_id")?,
        credential_owner_user_id: crate::sqlite::try_get_v173_text(
            &row,
            "credential_owner_user_id",
        )?,
        feedback_policy_json: crate::sqlite::try_get_v173_text(&row, "feedback_policy_json")?,
        feedback_policy_digest: crate::sqlite::try_get_v173_text(&row, "feedback_policy_digest")?,
        state: row.try_get("state")?,
        provider_status: crate::sqlite::try_get_optional_text(&row, "provider_status")?,
        result_classification: crate::sqlite::try_get_optional_text(&row, "result_classification")?,
        reconciliation_reason: row.try_get("reconciliation_reason")?,
        provider_event_id: row.try_get("provider_event_id")?,
        observed_head_sha: row.try_get("observed_head_sha")?,
        merged_commit_sha: row.try_get("merged_commit_sha")?,
        result_event_id: row.try_get("result_event_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn table_has_column(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    column: &str,
) -> Result<bool> {
    // Table names at call sites are static. This also lets migration-history
    // fixtures use the current repository against their historical schema.
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(&mut **tx)
        .await?;
    for row in rows {
        if row.try_get::<String, _>("name")? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn task_is_batchless_single_repository(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    task_repo_id: &str,
) -> Result<bool> {
    if !table_has_column(tx, "task_repository_set_revision", "revision").await? {
        // Before V183 every Task had one repository identity on task.repo_id.
        return Ok(true);
    }
    let single_primary: i64 = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM task_repository_set_revision revision
             JOIN task_repository_set_revision_repo primary_repo
               ON primary_repo.task_id = revision.task_id
              AND primary_repo.revision = revision.revision
             WHERE revision.task_id = ? AND revision.state = 'current'
               AND primary_repo.repo_id = ? AND primary_repo.role = 'primary'
               AND (SELECT COUNT(*) FROM task_repository_set_revision_repo writable
                    WHERE writable.task_id = revision.task_id
                      AND writable.revision = revision.revision
                      AND writable.role IN ('primary', 'affected')) = 1
               AND NOT EXISTS (
                   SELECT 1 FROM task_requires_repository_merge_batch required
                   WHERE required.task_id = revision.task_id
               )
         )",
    )
    .bind(task_id)
    .bind(task_repo_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(single_primary != 0)
}

async fn resolve_batchless_admission_repo(
    tx: &mut Transaction<'_, Sqlite>,
    admission: &mut RemotePrAdmission,
) -> Result<()> {
    let operation =
        sqlx::query("SELECT * FROM task_integration_operation WHERE id = ? AND task_id = ?")
            .bind(&admission.task_merge_operation_id)
            .bind(&admission.task_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(operation) = operation else {
        admission.repo_id = None;
        return Ok(());
    };
    let operation_repo_id = crate::sqlite::try_get_optional_text(&operation, "repo_id")?;
    let batch_id = crate::sqlite::try_get_optional_text(&operation, "repository_merge_batch_id")?;
    if batch_id.is_some() {
        // Batch admissions must carry the repo identity on the admission row;
        // an incomplete multirepo admission is never repaired by guessing.
        if admission.repo_id.as_deref() != operation_repo_id.as_deref() {
            admission.repo_id = None;
        }
        return Ok(());
    }

    let task_repo_id: Option<String> =
        sqlx::query_scalar("SELECT repo_id FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&admission.task_id)
            .fetch_optional(&mut **tx)
            .await?
            .flatten();
    let Some(task_repo_id) = task_repo_id else {
        admission.repo_id = None;
        return Ok(());
    };
    let resolved_repo_id = admission
        .repo_id
        .as_deref()
        .or(operation_repo_id.as_deref())
        .unwrap_or(&task_repo_id);
    if resolved_repo_id != task_repo_id.as_str()
        || operation_repo_id
            .as_deref()
            .is_some_and(|repo_id| repo_id != task_repo_id.as_str())
        || !task_is_batchless_single_repository(tx, &admission.task_id, &task_repo_id).await?
    {
        admission.repo_id = None;
    } else {
        admission.repo_id = Some(task_repo_id);
    }
    Ok(())
}

async fn load_remote_pr_admission(
    tx: &mut Transaction<'_, Sqlite>,
    task_merge_operation_id: &str,
) -> Result<RemotePrAdmission> {
    let row = sqlx::query("SELECT * FROM remote_pr_admission WHERE task_merge_operation_id = ?")
        .bind(task_merge_operation_id)
        .fetch_one(&mut **tx)
        .await?;
    let mut admission = map_remote_pr_admission(row)?;
    resolve_batchless_admission_repo(tx, &mut admission).await?;
    Ok(admission)
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
) -> Result<Option<DomainEvent>> {
    let gate_evaluation_id = input
        .gate_evaluation_id
        .as_deref()
        .ok_or(DbError::InvalidTransition)?;
    let repo_id = input.repo_id.as_deref().ok_or(DbError::InvalidTransition)?;
    let batch_id = input.repository_merge_batch_id.as_deref();
    let (writable_repository_count, repo_is_writable) = if let Some(batch_id) = batch_id {
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT batch.repository_set_revision
                 FROM task_repository_merge_batch batch
                 JOIN task_repository_merge_batch_repo repo
                   ON repo.batch_id = batch.id AND repo.task_id = batch.task_id
                 WHERE batch.id = ? AND batch.task_id = ?
                   AND batch.gate_evaluation_id = ? AND repo.repo_id = ?",
        )
        .bind(batch_id)
        .bind(&input.task_id)
        .bind(gate_evaluation_id)
        .bind(repo_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some(revision) = revision else {
            return Err(DbError::InvalidTransition);
        };
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_repository_set_revision_repo
                 WHERE task_id = ? AND revision = ? AND role IN ('primary', 'affected')",
        )
        .bind(&input.task_id)
        .bind(revision)
        .fetch_one(&mut **tx)
        .await?;
        let repo_is_writable: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                    SELECT 1 FROM task_repository_merge_batch_repo
                    WHERE batch_id = ? AND task_id = ? AND repo_id = ?
                 )",
        )
        .bind(batch_id)
        .bind(&input.task_id)
        .bind(repo_id)
        .fetch_one(&mut **tx)
        .await?;
        (count, repo_is_writable)
    } else {
        if table_has_column(tx, "task_repository_set_revision", "revision").await? {
            let revision: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM task_repository_set_revision
                     WHERE task_id = ? AND state = 'current'",
            )
            .bind(&input.task_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some(revision) = revision else {
                return Err(DbError::InvalidTransition);
            };
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM task_repository_set_revision_repo
                     WHERE task_id = ? AND revision = ? AND role IN ('primary', 'affected')",
            )
            .bind(&input.task_id)
            .bind(revision)
            .fetch_one(&mut **tx)
            .await?;
            let repo_is_primary: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                        SELECT 1 FROM task_repository_set_revision_repo
                        WHERE task_id = ? AND revision = ? AND repo_id = ? AND role = 'primary'
                     )",
            )
            .bind(&input.task_id)
            .bind(revision)
            .bind(repo_id)
            .fetch_one(&mut **tx)
            .await?;
            if count != 1 || !repo_is_primary {
                return Err(DbError::InvalidTransition);
            }
            (count, repo_is_primary)
        } else {
            // V113 and earlier represent the single writable repository only
            // through task.repo_id. Keep that historical path exact and do not
            // infer any multirepo authority from a TaskMerge input.
            let task_repo_id: Option<String> =
                sqlx::query_scalar("SELECT repo_id FROM task WHERE id = ? AND deleted_at IS NULL")
                    .bind(&input.task_id)
                    .fetch_optional(&mut **tx)
                    .await?
                    .flatten();
            let repo_is_primary = task_repo_id.as_deref() == Some(repo_id);
            (if repo_is_primary { 1 } else { 0 }, repo_is_primary)
        }
    };
    if !repo_is_writable || writable_repository_count == 0 {
        return Err(DbError::InvalidTransition);
    }
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
    let lifecycle_reason_ref = batch_id.unwrap_or(&input.id);
    let lifecycle_reason_kind = if batch_id.is_some() {
        "task_repository_merge_batch"
    } else {
        "task_merge"
    };
    if lifecycle.state == TaskLifecycleState::Merging
        && lifecycle.reason_kind.as_deref() == Some(lifecycle_reason_kind)
        && lifecycle.reason_ref.as_deref() == Some(lifecycle_reason_ref)
    {
        return Ok(None);
    }
    let active_gate_is_current: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM gate_evaluation evaluation
            JOIN gate ON gate.id = evaluation.gate_id
            WHERE evaluation.id = ? AND evaluation.task_id = ?
              AND evaluation.outcome = 'satisfied'
              AND gate.gate_kind = 'merge_readiness'
              AND gate.scope_kind = 'task' AND gate.scope_id = evaluation.task_id
              AND gate.active_policy_revision = evaluation.policy_revision
        )",
    )
    .bind(gate_evaluation_id)
    .bind(&input.task_id)
    .fetch_one(&mut **tx)
    .await?;
    let lifecycle_admission_allowed = lifecycle.state == TaskLifecycleState::ReadyToMerge
        || (batch_id.is_some()
            && lifecycle.state == TaskLifecycleState::Active
            && writable_repository_count > 1
            && active_gate_is_current);
    if !lifecycle_admission_allowed {
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
            "repository_merge_batch_id": batch_id,
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
            reason_kind: Some(lifecycle_reason_kind.to_owned()),
            reason_ref: Some(lifecycle_reason_ref.to_owned()),
            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
            idempotency_key: key,
            updated_at: input.created_at.clone(),
            event: event.clone(),
        },
    )
    .await?;
    let _ = updated_lifecycle;
    let changed = sqlx::query(
        "UPDATE task SET version = version + 1, updated_at = ?
         WHERE id = ? AND version = ? AND deleted_at IS NULL",
    )
    .bind(&input.created_at)
    .bind(&input.task_id)
    .bind(task_version)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(DbError::VersionConflict);
    }
    Ok(Some(event))
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
    let expected_reason_ref = lifecycle_operation
        .repository_merge_batch_id
        .as_deref()
        .unwrap_or(lifecycle_operation.id.as_str());
    let expected_reason_kind = if lifecycle_operation.repository_merge_batch_id.is_some() {
        "task_repository_merge_batch"
    } else {
        "task_merge"
    };
    if lifecycle.state != TaskLifecycleState::Merging
        || lifecycle.reason_kind.as_deref() != Some(expected_reason_kind)
        || lifecycle.reason_ref.as_deref() != Some(expected_reason_ref)
    {
        return Err(DbError::InvalidTransition);
    }
    if lifecycle_operation.status == TaskIntegrationOperationStatus::Succeeded
        && !repository_merge_batch_complete_in_tx(
            tx,
            lifecycle_operation.repository_merge_batch_id.as_deref(),
        )
        .await?
    {
        // A successful repository operation is only one child result. Keep
        // TaskLifecycle in Merging until every changed repository succeeds.
        return Ok(None);
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
        "UPDATE task SET version = version + 1, updated_at = ?
         WHERE id = ? AND version = ? AND deleted_at IS NULL",
    )
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

async fn repository_merge_batch_complete_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch_id: Option<&str>,
) -> Result<bool> {
    let Some(batch_id) = batch_id else {
        return Ok(true);
    };
    let row_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_repository_merge_batch_repo WHERE batch_id = ?",
    )
    .bind(batch_id)
    .fetch_one(&mut **tx)
    .await?;
    if row_count == 0 {
        return Ok(false);
    }
    let incomplete_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)
         FROM task_repository_merge_batch_repo row
         JOIN repo ON repo.id = row.repo_id
         LEFT JOIN task_integration_operation operation
           ON operation.id = row.task_merge_operation_id
         LEFT JOIN remote_pr_admission admission
           ON admission.task_merge_operation_id = operation.id
         WHERE row.batch_id = ?
           AND NOT (
             (repo.work_mode = 'direct_merge'
              AND (row.state = 'integrated'
                   OR operation.status = 'succeeded'))
             OR (repo.work_mode = 'pull_request'
                 AND (row.state = 'merged' OR admission.state = 'merged'))
           )",
    )
    .bind(batch_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(incomplete_count == 0)
}

async fn reset_pr_metadata_for_new_pr(
    tx: &mut Transaction<'_, Sqlite>,
    metadata: &CreatePrMetadata,
    metadata_id: &str,
) -> Result<()> {
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
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    Ok(())
}

async fn insert_operation(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateTaskIntegrationOperation,
) -> Result<()> {
    if table_has_column(tx, "task_integration_operation", "repo_id").await? {
        sqlx::query(
            "INSERT INTO task_integration_operation (
                    id, task_id, repo_id, repository_merge_batch_id, kind, owner_id,
                    status, version, created_at, updated_at, finished_at,
                    gate_evaluation_id, remote_waiting, parent_operation_id, result_event_id
             ) VALUES (?, ?, ?, ?, ?, ?, 'running', 1, ?, ?, NULL, ?, ?, ?, NULL)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.repo_id.as_deref())
        .bind(input.repository_merge_batch_id.as_deref())
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
    } else {
        sqlx::query(
            "INSERT INTO task_integration_operation (
                    id, task_id, kind, owner_id, status, version,
                    created_at, updated_at, finished_at, gate_evaluation_id,
                    remote_waiting, parent_operation_id, result_event_id
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
    }
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
            || merge.repo_id.is_none()
            || !merge.remote_waiting
            || merge.parent_operation_id.is_some()
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.remote_waiting
            || publish.parent_operation_id.as_deref() != Some(merge.id.as_str())
            || publish.task_id != merge.task_id
            || publish.repo_id != merge.repo_id
            || publish.repository_merge_batch_id != merge.repository_merge_batch_id
            || publish.gate_evaluation_id != merge.gate_evaluation_id
            || metadata.task_id != merge.task_id
            || Some(metadata.repo_id.as_str()) != merge.repo_id.as_deref()
            || metadata.task_merge_operation_id != merge.id
            || metadata.publish_operation_id != publish.id
            || metadata.provider_pr_id.is_some()
            || metadata.merge_status != "pending"
            || remote_admission.task_merge_operation_id != merge.id
            || remote_admission.publish_operation_id != publish.id
            || remote_admission.task_id != merge.task_id
            || remote_admission.repo_id != merge.repo_id.as_deref().unwrap_or_default()
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
        if let Some(batch_id) = merge.repository_merge_batch_id.as_deref() {
            let linked = sqlx::query(
                "UPDATE task_repository_merge_batch_repo
                 SET task_merge_operation_id = ?, updated_at = ?
                 WHERE batch_id = ? AND task_id = ? AND repo_id = ?
                   AND (task_merge_operation_id IS NULL OR task_merge_operation_id = ?)",
            )
            .bind(&merge.id)
            .bind(&merge.created_at)
            .bind(batch_id)
            .bind(&merge.task_id)
            .bind(merge.repo_id.as_deref())
            .bind(&merge.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if linked != 1 {
                return Err(DbError::InvalidTransition);
            }
        }
        insert_operation(&mut tx, &publish).await?;
        let has_metadata_repo_id = table_has_column(&mut tx, "pr_metadata", "repo_id").await?;
        let existing_pr_sql = if has_metadata_repo_id {
            "SELECT id, merge_status, admission_status, task_merge_operation_id,
                    publish_operation_id
             FROM pr_metadata WHERE task_id = ? AND repo_id = ?"
        } else {
            "SELECT id, merge_status, 'admitted' AS admission_status, task_merge_operation_id,
                    publish_operation_id
             FROM pr_metadata WHERE task_id = ?"
        };
        let mut existing_pr_query = sqlx::query(existing_pr_sql).bind(&metadata.task_id);
        if has_metadata_repo_id {
            existing_pr_query = existing_pr_query.bind(&metadata.repo_id);
        }
        let existing_pr = existing_pr_query.fetch_optional(&mut *tx).await?;
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
                {
                    return Err(DbError::InvalidTransition);
                }
                // V175: an admission paused for rework has no remote result and
                // no history. While the pause is open the SAME PR stays bound
                // (provider PR id, URL and state are kept); after the PR was
                // closed remotely the projection is reset for a new PR.
                let pause_state: Option<String> = if previous_admission.rework_outcome.is_some() {
                    match previous_admission.supersession_id.as_deref() {
                        Some(supersession_id) => sqlx::query_scalar(
                            "SELECT state FROM pr_rework_supersession WHERE id = ? AND task_id = ?",
                        )
                        .bind(supersession_id)
                        .bind(&metadata.task_id)
                        .fetch_optional(&mut *tx)
                        .await?,
                        None => return Err(DbError::InvalidTransition),
                    }
                } else {
                    None
                };
                match pause_state.as_deref() {
                    Some("open") => {
                        sqlx::query(
                            "UPDATE pr_metadata
                             SET task_merge_operation_id = ?, publish_operation_id = ?,
                                 updated_at = ?
                             WHERE id = ?",
                        )
                        .bind(&metadata.task_merge_operation_id)
                        .bind(&metadata.publish_operation_id)
                        .bind(&metadata.updated_at)
                        .bind(&metadata_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(check_error)?;
                    }
                    Some("remote_closed") => {
                        reset_pr_metadata_for_new_pr(&mut tx, &metadata, &metadata_id).await?;
                    }
                    Some(_) => return Err(DbError::InvalidTransition),
                    None => {
                        if !matches!(
                            previous_admission.state.as_str(),
                            "merged" | "closed" | "head_mismatch" | "publication_failed"
                        ) || previous_admission.rework_outcome.is_some()
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
                        reset_pr_metadata_for_new_pr(&mut tx, &metadata, &metadata_id).await?;
                    }
                }
                metadata_id
            }
        } else {
            super::pr_metadata::insert_pr_metadata_in_tx(&mut tx, &metadata).await?;
            metadata.id.clone()
        };
        remote_admission.metadata_id = metadata_id.clone();
        // V173: freeze the credential reference and feedback policy that the
        // repo config carries in this same transaction; the admission insert
        // guard verifies the copy is exactly the current config.
        let frozen = sqlx::query("SELECT * FROM pr_provider_config WHERE id = ?")
            .bind(&remote_admission.provider_config_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::InvalidTransition)?;
        let frozen_credential_handle_id =
            crate::sqlite::try_get_v173_text(&frozen, "credential_handle_id")?;
        let frozen_credential_owner_user_id =
            crate::sqlite::try_get_v173_text(&frozen, "credential_owner_user_id")?;
        let frozen_policy_json = crate::sqlite::try_get_v173_text(&frozen, "feedback_policy_json")?
            .unwrap_or_else(|| crate::DEFAULT_PR_FEEDBACK_POLICY_JSON.to_owned());
        let frozen_policy_digest = crate::pr_feedback_policy_digest(&frozen_policy_json);
        let has_v173_columns =
            table_has_column(&mut tx, "remote_pr_admission", "credential_handle_id").await?;
        let has_v175_column =
            table_has_column(&mut tx, "remote_pr_admission", "readmits_supersession_id").await?;
        // V175: while a pause is open the admission MUST re-admit it (the insert
        // guard rejects any other admission); otherwise the column stays NULL.
        let readmits_supersession_id: Option<String> = if has_v175_column {
            sqlx::query_scalar(
                "SELECT id FROM pr_rework_supersession WHERE task_id = ? AND state = 'open'",
            )
            .bind(&remote_admission.task_id)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        };
        let has_admission_repo_id =
            table_has_column(&mut tx, "remote_pr_admission", "repo_id").await?;
        let mut columns = vec![
            "task_merge_operation_id",
            "publish_operation_id",
            "metadata_id",
            "task_id",
        ];
        if has_admission_repo_id {
            columns.push("repo_id");
        }
        columns.extend([
            "provider_config_id",
            "provider_type",
            "provider_config_revision",
            "provider_config_digest",
            "provider_base_url",
            "token_secret_ref",
            "remote_repo_identity",
            "source_branch",
            "target_branch",
            "admitted_source_sha",
            "state",
            "reconciliation_reason",
            "provider_event_id",
            "observed_head_sha",
            "merged_commit_sha",
            "result_event_id",
            "created_at",
            "updated_at",
        ]);
        if has_v173_columns {
            columns.extend([
                "credential_handle_id",
                "credential_owner_user_id",
                "feedback_policy_json",
                "feedback_policy_digest",
            ]);
        }
        if has_v175_column {
            columns.push("readmits_supersession_id");
        }
        let placeholders = vec!["?"; columns.len()].join(", ");
        let insert_sql = format!(
            "INSERT INTO remote_pr_admission ({}) VALUES ({placeholders})",
            columns.join(", ")
        );
        let mut insert = sqlx::query(&insert_sql)
            .bind(&remote_admission.task_merge_operation_id)
            .bind(&remote_admission.publish_operation_id)
            .bind(&remote_admission.metadata_id)
            .bind(&remote_admission.task_id);
        if has_admission_repo_id {
            insert = insert.bind(&remote_admission.repo_id);
        }
        let mut insert = insert
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
            .bind("admitted")
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(&remote_admission.created_at)
            .bind(&remote_admission.updated_at);
        if has_v173_columns {
            insert = insert
                .bind(frozen_credential_handle_id.as_deref())
                .bind(frozen_credential_owner_user_id.as_deref())
                .bind(&frozen_policy_json)
                .bind(&frozen_policy_digest);
        }
        if has_v175_column {
            insert = insert.bind(readmits_supersession_id.as_deref());
        }
        insert.execute(&mut *tx).await.map_err(check_error)?;
        if table_has_column(&mut tx, "pr_metadata", "admission_status").await? {
            sqlx::query("UPDATE pr_metadata SET admission_status = 'admitted' WHERE id = ?")
                .bind(&metadata_id)
                .execute(&mut *tx)
                .await
                .map_err(check_error)?;
        }
        let merge_operation = load_operation(&mut tx, &merge.id).await?;
        let publish_operation = load_operation(&mut tx, &publish.id).await?;
        tx.commit().await?;
        Ok((merge_operation, publish_operation))
    }

    async fn get_remote_pr_admission(
        &self,
        task_merge_operation_id: &str,
    ) -> Result<Option<RemotePrAdmission>> {
        let mut tx = self.pool.begin().await?;
        let row =
            sqlx::query("SELECT * FROM remote_pr_admission WHERE task_merge_operation_id = ?")
                .bind(task_merge_operation_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let mut admission = map_remote_pr_admission(row)?;
        resolve_batchless_admission_repo(&mut tx, &mut admission).await?;
        tx.commit().await?;
        Ok(Some(admission))
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
        let tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        finish_operation_in_tx(self, input, tx, std::future::ready(())).await
    }
}

// Tests pause after the operation read to force a competing commit; production passes `ready()`.
async fn finish_operation_in_tx<F>(
    db: &SqliteDb,
    input: FinishTaskIntegrationOperation,
    mut tx: Transaction<'_, Sqlite>,
    after_load: F,
) -> Result<TaskIntegrationOperation>
where
    F: Future<Output = ()> + Send,
{
    let current = load_operation(&mut tx, &input.id).await?;
    after_load.await;
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
    finish_merge_lifecycle_in_tx(db, &mut tx, &operation, input.status, &input.updated_at).await?;
    tx.commit().await?;
    Ok(operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, ActorRef,
        CreateDomainEvent, CreateProject, CreateRoleMembership, CreateTask,
        CreateTaskIntegrationOperation, CreateTaskRole, CreateWorkUnit, ProjectRepo,
        RoleMembershipRepo, RoleMembershipStatus, TaskIntegrationOperationRepo, TaskLifecycleState,
        TaskRepo, TaskRoleRepo, User, UserRepo, WorkUnitRepo,
    };
    use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
    use std::str::FromStr;
    use tokio::sync::oneshot;

    async fn zero_wait_contention_pool(database_url: &str) -> SqlitePool {
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(database_url)
            .expect("SQLite connection options")
            .create_if_missing(true);
        SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("PRAGMA journal_mode = WAL")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout = 0")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await
            .expect("zero-wait competing connection")
    }

    async fn fixture() -> (
        tempfile::TempDir,
        SqliteDb,
        SqlitePool,
        String,
        String,
        String,
        TaskIntegrationOperation,
    ) {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database_url = format!("sqlite://{}", directory.path().join("forge.db").display());
        let pool = create_sqlite_pool(&database_url)
            .await
            .expect("primary pool");
        run_migrations(&pool).await.expect("migrations");
        let db = SqliteDb::new(pool);
        let concurrent_pool = zero_wait_contention_pool(&database_url).await;

        let user_id = new_uuid_v4();
        let now = now_rfc3339();
        UserRepo::create_user(
            &db,
            &User {
                id: user_id.clone(),
                email: "finish-race@example.invalid".to_owned(),
                password_hash: "test-only".to_owned(),
                display_name: None,
                is_admin: true,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("user");
        let project_id = new_uuid_v4();
        ProjectRepo::create(
            &db,
            CreateProject {
                id: project_id.clone(),
                name: "WorkUnit finish race".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(user_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project");
        let task_id = new_uuid_v4();
        TaskRepo::create(
            &db,
            CreateTask {
                repositories: Vec::new(),
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                subtask_order: None,
                title: "Reproduce WorkUnit create finish".to_owned(),
                description: None,
                lifecycle_state: TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task");
        let task_role = TaskRoleRepo::create(
            &db,
            CreateTaskRole {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                role: "implementer".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("TaskRole");
        RoleMembershipRepo::add(
            &db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: task_role.id,
                actor_kind: ActorKind::Human,
                actor_id: user_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("active creator membership");

        let operation = TaskIntegrationOperationRepo::begin(
            &db,
            CreateTaskIntegrationOperation {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                repo_id: None,
                repository_merge_batch_id: None,
                kind: TaskIntegrationOperationKind::WorkUnitCreate,
                owner_id: new_uuid_v4(),
                gate_evaluation_id: None,
                remote_waiting: false,
                parent_operation_id: None,
                created_at: now.clone(),
            },
        )
        .await
        .expect("durable WorkUnitCreate claim");
        let work_unit_id = new_uuid_v4();
        WorkUnitRepo::create(
            &db,
            CreateWorkUnit {
                repository_ids: Vec::new(),
                id: work_unit_id.clone(),
                task_id: task_id.clone(),
                parent_work_unit_id: None,
                title: "Committed before operation finish".to_owned(),
                scope: "Test the create/finish boundary".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: None,
                requires_integration: false,
                provenance: None,
                artifact_input_ids: Vec::new(),
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by: ActorRef::Human(user_id.clone()),
                created_at: now.clone(),
            },
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: work_unit_id.clone(),
                actor_type: "human".to_owned(),
                actor_id: Some(user_id),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now,
            },
        )
        .await
        .expect("WorkUnit commits before finish");

        (
            directory,
            db,
            concurrent_pool,
            task_id,
            work_unit_id,
            project_id,
            operation,
        )
    }

    fn finish_input(operation: &TaskIntegrationOperation) -> FinishTaskIntegrationOperation {
        let now = now_rfc3339();
        FinishTaskIntegrationOperation {
            id: operation.id.clone(),
            expected_version: operation.version,
            status: TaskIntegrationOperationStatus::Succeeded,
            result_event_id: None,
            updated_at: now.clone(),
            finished_at: now,
        }
    }

    #[tokio::test]
    async fn deferred_finish_reproduces_busy_snapshot_after_work_unit_commit() {
        let (_directory, db, concurrent_pool, _task_id, work_unit_id, project_id, operation) =
            fixture().await;
        assert!(WorkUnitRepo::get_by_id(&db, &work_unit_id)
            .await
            .expect("read committed WorkUnit")
            .is_some());

        let tx = db.pool.begin().await.expect("deferred finish transaction");
        let (loaded_tx, loaded_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        let finish_db = db.clone();
        let finish = tokio::spawn(async move {
            finish_operation_in_tx(&finish_db, finish_input(&operation), tx, async move {
                loaded_tx
                    .send(())
                    .expect("finish read its operation snapshot");
                resume_rx.await.expect("resume after competing commit");
            })
            .await
        });
        loaded_rx.await.expect("finish has loaded its snapshot");

        let mut concurrent = concurrent_pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("competing writer begins");
        sqlx::query("UPDATE project SET updated_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(project_id)
            .execute(&mut *concurrent)
            .await
            .expect("competing write");
        concurrent.commit().await.expect("competing write commits");
        resume_tx.send(()).expect("resume finish");

        let error = finish
            .await
            .expect("finish task joins")
            .expect_err("deferred snapshot cannot upgrade after a competing commit");
        let database_error = match error {
            DbError::Sqlx(sqlx::Error::Database(error)) => error,
            other => panic!("expected SQLite busy snapshot, got {other:?}"),
        };
        assert_eq!(database_error.code().as_deref(), Some("517"));
        assert!(database_error.message().contains("database is locked"));
        assert!(WorkUnitRepo::get_by_id(&db, &work_unit_id)
            .await
            .expect("read WorkUnit after finish error")
            .is_some());
    }

    #[tokio::test]
    async fn immediate_finish_serializes_a_competing_writer() {
        let (_directory, db, concurrent_pool, _task_id, _work_unit_id, project_id, operation) =
            fixture().await;
        let tx = db
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("immediate finish transaction");
        let (loaded_tx, loaded_rx) = oneshot::channel();
        let (resume_tx, resume_rx) = oneshot::channel();
        let finish_db = db.clone();
        let finish = tokio::spawn(async move {
            finish_operation_in_tx(&finish_db, finish_input(&operation), tx, async move {
                loaded_tx.send(()).expect("finish read its operation");
                resume_rx.await.expect("resume finish");
            })
            .await
        });
        loaded_rx.await.expect("finish has read its operation");

        let (writer_attempt_tx, writer_attempt_rx) = oneshot::channel();
        let (retry_tx, retry_rx) = oneshot::channel();
        let writer = tokio::spawn(async move {
            let result = concurrent_pool.begin_with("BEGIN IMMEDIATE").await;
            let outcome = match result {
                Err(sqlx::Error::Database(error)) => {
                    format!("{} {}", error.code().unwrap_or_default(), error.message())
                }
                Err(error) => format!("unexpected error: {error}"),
                Ok(tx) => {
                    tx.rollback()
                        .await
                        .expect("unexpectedly acquired writer rolls back");
                    "writer acquired while finish held its reservation".to_owned()
                }
            };
            writer_attempt_tx
                .send(outcome)
                .expect("report competing writer attempt");
            retry_rx.await.expect("retry after finish commits");

            let mut tx = concurrent_pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .expect("competing writer begins after finish commits");
            sqlx::query("UPDATE project SET updated_at = ? WHERE id = ?")
                .bind(now_rfc3339())
                .bind(project_id)
                .execute(&mut *tx)
                .await
                .expect("competing write");
            tx.commit().await.expect("competing writer commits");
        });
        let blocked_writer = writer_attempt_rx
            .await
            .expect("competing writer attempted while finish holds its reservation");
        assert!(
            blocked_writer.starts_with("5 database is locked"),
            "expected SQLITE_BUSY while finish held its reservation, got {blocked_writer}"
        );
        resume_tx
            .send(())
            .expect("resume finish before writer retry");

        let finished = finish
            .await
            .expect("finish task joins")
            .expect("immediate finish succeeds");
        assert_eq!(finished.status, TaskIntegrationOperationStatus::Succeeded);
        retry_tx.send(()).expect("retry writer after finish commit");
        writer.await.expect("competing writer completes");
    }
}
