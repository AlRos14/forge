//! V175 non-destructive PR pause (D3'): the only writers of
//! `pr_rework_supersession` and of the admission pause marker.

use super::*;
use crate::{models::*, pagination::*, repository::*};

fn map_supersession(row: SqliteRow) -> Result<PrReworkSupersession> {
    Ok(PrReworkSupersession {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        task_merge_operation_id: row.try_get("task_merge_operation_id")?,
        publish_operation_id: row.try_get("publish_operation_id")?,
        metadata_id: row.try_get("metadata_id")?,
        request_id: row.try_get("request_id")?,
        triage_set_digest: row.try_get("triage_set_digest")?,
        provider_pr_id: row.try_get("provider_pr_id")?,
        superseded_head_sha: row.try_get("superseded_head_sha")?,
        result_event_id: row.try_get("result_event_id")?,
        state: parse_enum(row.try_get::<String, _>("state")?)?,
        readmission_merge_operation_id: row.try_get("readmission_merge_operation_id")?,
        convergence_decision_id: row.try_get("convergence_decision_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn get_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<PrReworkSupersession>> {
    sqlx::query("SELECT * FROM pr_rework_supersession WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_supersession)
        .transpose()
}

async fn supersede_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    input: &SupersedeAdmission,
) -> Result<SupersedeAdmissionWrite> {
    if let Some(existing) =
        sqlx::query("SELECT * FROM pr_rework_supersession WHERE task_merge_operation_id = ?")
            .bind(&input.task_merge_operation_id)
            .fetch_optional(&mut **tx)
            .await?
            .map(map_supersession)
            .transpose()?
    {
        if existing.request_id != input.request_id {
            return Err(DbError::IdempotencyConflict);
        }
        return Ok(SupersedeAdmissionWrite {
            supersession: existing,
            created: false,
            events: Vec::new(),
        });
    }

    let row = sqlx::query(
        "SELECT a.task_id, a.publish_operation_id, a.metadata_id, a.state, a.rework_outcome,
                a.admitted_source_sha, m.provider_pr_id,
                r.task_id AS request_task_id, r.kind AS request_kind,
                r.state AS request_state, r.source_subject_id, r.source_subject_digest
         FROM remote_pr_admission a
         JOIN pr_metadata m ON m.id = a.metadata_id
         JOIN task_coordination_request r ON r.id = ?
         WHERE a.task_merge_operation_id = ?",
    )
    .bind(&input.request_id)
    .bind(&input.task_merge_operation_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let task_id: String = row.try_get("task_id")?;
    let publish_operation_id: String = row.try_get("publish_operation_id")?;
    let metadata_id: String = row.try_get("metadata_id")?;
    let admission_state: String = row.try_get("state")?;
    let rework_outcome: Option<String> = row.try_get("rework_outcome")?;
    let head_sha: String = row.try_get("admitted_source_sha")?;
    let provider_pr_id: Option<String> = row.try_get("provider_pr_id")?;
    let request_task_id: String = row.try_get("request_task_id")?;
    let request_kind: String = row.try_get("request_kind")?;
    let request_state: String = row.try_get("request_state")?;
    let request_source: Option<String> = row.try_get("source_subject_id")?;
    let set_digest: Option<String> = row.try_get("source_subject_digest")?;
    let (Some(provider_pr_id), Some(set_digest)) = (provider_pr_id, set_digest) else {
        return Err(DbError::PrReworkNotAuthorized);
    };
    if request_task_id != task_id
        || request_kind != "external_feedback"
        || request_state != "triaged"
        || request_source.as_deref() != Some(input.task_merge_operation_id.as_str())
        || admission_state != "open"
        || rework_outcome.is_some()
    {
        return Err(DbError::PrReworkNotAuthorized);
    }

    let lifecycle_row = sqlx::query(
        "SELECT task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         FROM task_lifecycle WHERE task_id = ?",
    )
    .bind(&task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let lifecycle = super::gate_lifecycle::map_task_lifecycle(&lifecycle_row)?;
    let task_version: i64 =
        sqlx::query_scalar("SELECT version FROM task WHERE id = ? AND deleted_at IS NULL")
            .bind(&task_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or(DbError::NotFound)?;
    let operation = sqlx::query(
        "SELECT kind, status, remote_waiting, version, repository_merge_batch_id
         FROM task_integration_operation
         WHERE id = ? AND task_id = ?",
    )
    .bind(&input.task_merge_operation_id)
    .bind(&task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?;
    let operation_kind: String = operation.try_get("kind")?;
    let operation_status: String = operation.try_get("status")?;
    let operation_remote: i64 = operation.try_get("remote_waiting")?;
    let operation_version: i64 = operation.try_get("version")?;
    let repository_merge_batch_id: Option<String> =
        operation.try_get("repository_merge_batch_id")?;
    if operation_kind != "task_merge" || operation_status != "running" || operation_remote != 1 {
        return Err(DbError::PrReworkNotAuthorized);
    }
    if lifecycle.state != TaskLifecycleState::Merging
        || lifecycle.reason_kind.as_deref() != Some("task_repository_merge_batch")
        || lifecycle.reason_ref != repository_merge_batch_id
    {
        return Err(DbError::PrReworkNotAuthorized);
    }

    let superseded = superseded_event_input(
        &task_id,
        &metadata_id,
        input,
        &publish_operation_id,
        &provider_pr_id,
        &head_sha,
    );
    let superseded_event = DomainEventRepo::append_event_in_tx(db, tx, &superseded).await?;
    sqlx::query(
        "INSERT INTO pr_rework_supersession (
            id, task_id, task_merge_operation_id, publish_operation_id, metadata_id,
            request_id, triage_set_digest, provider_pr_id, superseded_head_sha,
            result_event_id, state, version, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', 1, ?, ?)",
    )
    .bind(&input.id)
    .bind(&task_id)
    .bind(&input.task_merge_operation_id)
    .bind(&publish_operation_id)
    .bind(&metadata_id)
    .bind(&input.request_id)
    .bind(&set_digest)
    .bind(&provider_pr_id)
    .bind(&head_sha)
    .bind(&superseded_event.id)
    .bind(&input.now)
    .bind(&input.now)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    let marked = sqlx::query(
        "UPDATE remote_pr_admission
         SET rework_outcome = 'superseded_for_rework', supersession_id = ?
         WHERE task_merge_operation_id = ? AND state = 'open' AND rework_outcome IS NULL",
    )
    .bind(&input.id)
    .bind(&input.task_merge_operation_id)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?
    .rows_affected();
    if marked != 1 {
        return Err(DbError::VersionConflict);
    }
    let abandoned = sqlx::query(
        "UPDATE task_integration_operation
         SET status = 'abandoned', result_event_id = ?, version = version + 1,
             updated_at = ?, finished_at = ?
         WHERE id = ? AND version = ? AND status = 'running' AND remote_waiting = 1",
    )
    .bind(&superseded_event.id)
    .bind(&input.now)
    .bind(&input.now)
    .bind(&input.task_merge_operation_id)
    .bind(operation_version)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?
    .rows_affected();
    if abandoned != 1 {
        return Err(DbError::VersionConflict);
    }

    // The payload deliberately carries no `task_merge_status`: this is a pause,
    // not a merge failure, and must never create a TaskMerge retry receipt.
    let key = format!("pr-rework-supersede:{}", input.task_merge_operation_id);
    let lifecycle_event = CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "task.lifecycle_changed".to_owned(),
        entity_type: "task".to_owned(),
        entity_id: task_id.clone(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.clone(),
        correlation_id: key.clone(),
        causation_id: Some(input.task_merge_operation_id.clone()),
        causation_depth: 1,
        dedupe_key: Some(format!("task-lifecycle:{task_id}:{key}")),
        payload_json: serde_json::json!({
            "task_id": task_id,
            "from_state": lifecycle.state,
            "to_state": TaskLifecycleState::Blocked,
            "from_version": lifecycle.version,
            "to_version": lifecycle.version + 1,
            "cause_kind": "merge_operation",
            "cause_ref": input.task_merge_operation_id,
            "gate_evaluation_id": serde_json::Value::Null,
            "reason_kind": PR_FEEDBACK_REWORK_REASON_KIND,
            "reason_ref": input.id,
        })
        .to_string(),
        created_at: input.now.clone(),
    };
    let (_updated, lifecycle_event) = super::gate_lifecycle::record_lifecycle_transition_in_tx(
        db,
        tx,
        &TransitionTaskLifecycle {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            expected_task_version: task_version,
            expected_lifecycle_version: lifecycle.version,
            expected_state: lifecycle.state,
            to_state: TaskLifecycleState::Blocked,
            cause_kind: "merge_operation".to_owned(),
            cause_ref: Some(input.task_merge_operation_id.clone()),
            reason_kind: Some(PR_FEEDBACK_REWORK_REASON_KIND.to_owned()),
            reason_ref: Some(input.id.clone()),
            gate_evaluation_id: None,
            idempotency_key: key,
            updated_at: input.now.clone(),
            event: lifecycle_event,
        },
    )
    .await?;
    let bumped = sqlx::query(
        "UPDATE task SET version = version + 1, updated_at = ?
         WHERE id = ? AND version = ? AND deleted_at IS NULL",
    )
    .bind(&input.now)
    .bind(&task_id)
    .bind(task_version)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if bumped != 1 {
        return Err(DbError::VersionConflict);
    }
    let supersession = get_in_tx(tx, &input.id).await?.ok_or(DbError::NotFound)?;
    Ok(SupersedeAdmissionWrite {
        supersession,
        created: true,
        events: vec![superseded_event, lifecycle_event],
    })
}

fn superseded_event_input(
    task_id: &str,
    metadata_id: &str,
    input: &SupersedeAdmission,
    publish_operation_id: &str,
    provider_pr_id: &str,
    head_sha: &str,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: PR_ADMISSION_SUPERSEDED_EVENT.to_owned(),
        entity_type: "pr_metadata".to_owned(),
        entity_id: metadata_id.to_owned(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: format!("pr-rework:{}", input.id),
        causation_id: Some(input.request_id.clone()),
        causation_depth: 1,
        dedupe_key: Some(format!(
            "pr-admission-superseded:{}",
            input.task_merge_operation_id
        )),
        payload_json: serde_json::json!({
            "task_id": task_id,
            "pr_metadata_id": metadata_id,
            "task_merge_operation_id": input.task_merge_operation_id,
            "publish_operation_id": publish_operation_id,
            "supersession_id": input.id,
            "provider_pr_id": provider_pr_id,
            "superseded_head_sha": head_sha,
        })
        .to_string(),
        created_at: input.now.clone(),
    }
}

async fn transition_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    input: &TransitionPrReworkSupersession,
) -> Result<PrReworkSupersession> {
    let current = get_in_tx(tx, &input.id).await?.ok_or(DbError::NotFound)?;
    if current.state == input.to_state
        && (input.readmission_merge_operation_id.is_none()
            || input.readmission_merge_operation_id == current.readmission_merge_operation_id)
        && (input.convergence_decision_id.is_none()
            || input.convergence_decision_id == current.convergence_decision_id)
    {
        return Ok(current);
    }
    if current.version != input.expected_version {
        return Err(DbError::VersionConflict);
    }
    let updated = sqlx::query(
        "UPDATE pr_rework_supersession
         SET state = ?, version = version + 1, updated_at = ?,
             readmission_merge_operation_id =
                 COALESCE(?, readmission_merge_operation_id),
             convergence_decision_id = COALESCE(?, convergence_decision_id)
         WHERE id = ? AND version = ?",
    )
    .bind(input.to_state.to_string())
    .bind(&input.updated_at)
    .bind(input.readmission_merge_operation_id.as_deref())
    .bind(input.convergence_decision_id.as_deref())
    .bind(&input.id)
    .bind(input.expected_version)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?
    .rows_affected();
    if updated != 1 {
        return Err(DbError::VersionConflict);
    }
    get_in_tx(tx, &input.id).await?.ok_or(DbError::NotFound)
}

#[async_trait]
impl PrReworkRepo for SqliteDb {
    async fn supersede_admission_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        input: SupersedeAdmission,
    ) -> Result<SupersedeAdmissionWrite> {
        supersede_in_tx(self, tx, &input).await
    }

    async fn create_supersession(
        &self,
        input: SupersedeAdmission,
    ) -> Result<SupersedeAdmissionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let write = supersede_in_tx(self, &mut tx, &input).await?;
        tx.commit().await?;
        Ok(write)
    }

    async fn get_supersession(&self, id: &str) -> Result<Option<PrReworkSupersession>> {
        sqlx::query("SELECT * FROM pr_rework_supersession WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_supersession)
            .transpose()
    }

    async fn get_supersession_by_task_merge(
        &self,
        task_merge_operation_id: &str,
    ) -> Result<Option<PrReworkSupersession>> {
        sqlx::query("SELECT * FROM pr_rework_supersession WHERE task_merge_operation_id = ?")
            .bind(task_merge_operation_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_supersession)
            .transpose()
    }

    async fn get_open_supersession_by_task(
        &self,
        task_id: &str,
    ) -> Result<Option<PrReworkSupersession>> {
        sqlx::query("SELECT * FROM pr_rework_supersession WHERE task_id = ? AND state = 'open'")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_supersession)
            .transpose()
    }

    async fn get_latest_supersession_by_task(
        &self,
        task_id: &str,
    ) -> Result<Option<PrReworkSupersession>> {
        sqlx::query(
            "SELECT * FROM pr_rework_supersession WHERE task_id = ?
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_supersession)
        .transpose()
    }

    async fn list_supersessions_by_task(
        &self,
        task_id: &str,
        page: &PageRequest,
    ) -> Result<Page<PrReworkSupersession>> {
        let offset = decode_offset(&page.cursor)?;
        let rows = sqlx::query(
            "SELECT * FROM pr_rework_supersession WHERE task_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id)
        .bind(limit(page) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .into_iter()
            .map(map_supersession)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM pr_rework_supersession WHERE task_id = ?",
                )
                .bind(task_id)
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, page, offset, total)
    }

    async fn list_supersessions_in_state(
        &self,
        state: PrReworkState,
        limit: i64,
    ) -> Result<Vec<PrReworkSupersession>> {
        sqlx::query(
            "SELECT * FROM pr_rework_supersession WHERE state = ?
             ORDER BY created_at, id LIMIT ?",
        )
        .bind(state.to_string())
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_supersession)
        .collect()
    }

    async fn transition_supersession(
        &self,
        input: TransitionPrReworkSupersession,
    ) -> Result<PrReworkSupersession> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = transition_in_tx(&mut tx, &input).await?;
        tx.commit().await?;
        Ok(updated)
    }

    async fn transition_supersession_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        input: TransitionPrReworkSupersession,
    ) -> Result<PrReworkSupersession> {
        transition_in_tx(tx, &input).await
    }
}
