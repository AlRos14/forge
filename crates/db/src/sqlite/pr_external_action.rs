use super::*;
use crate::{models::*, pagination::*, repository::*};

fn map_pr_external_action(row: SqliteRow) -> Result<PrExternalAction> {
    Ok(PrExternalAction {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        admission_id: row.try_get("admission_id")?,
        kind: parse_enum(row.try_get("kind")?)?,
        state: parse_enum(row.try_get("state")?)?,
        authorization_mode: parse_enum(row.try_get("authorization_mode")?)?,
        gate_evaluation_id: row.try_get("gate_evaluation_id")?,
        subject_commit_sha: row.try_get("subject_commit_sha")?,
        expected_remote_head_sha: row.try_get("expected_remote_head_sha")?,
        summary_artifact_id: row.try_get("summary_artifact_id")?,
        body_digest: row.try_get("body_digest")?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        idempotency_key: row.try_get("idempotency_key")?,
        provider_event_id: row.try_get("provider_event_id")?,
        provider_result_json: row.try_get("provider_result_json")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

async fn get_action_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<PrExternalAction>> {
    sqlx::query("SELECT * FROM pr_external_action WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_pr_external_action)
        .transpose()
}

async fn append_action_event(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    event: Option<&CreateDomainEvent>,
) -> Result<Option<DomainEvent>> {
    match event {
        Some(event) => Ok(Some(
            DomainEventRepo::append_event_in_tx(db, tx, event).await?,
        )),
        None => Ok(None),
    }
}

/// `0` rows from a version-guarded update: a missing row is `NotFound`, any
/// other cause (stale version) is `VersionConflict`.
async fn action_stale_or_missing(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<DbError> {
    Ok(if get_action_in_tx(tx, id).await?.is_some() {
        DbError::VersionConflict
    } else {
        DbError::NotFound
    })
}

#[async_trait]
impl PrExternalActionRepo for SqliteDb {
    async fn create_pr_external_action(
        &self,
        input: CreatePrExternalAction,
        event: Option<CreateDomainEvent>,
    ) -> Result<PrExternalActionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query("SELECT * FROM pr_external_action WHERE idempotency_key = ?")
            .bind(&input.idempotency_key)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_pr_external_action)
            .transpose()?;
        if let Some(existing) = existing {
            if existing.task_id != input.task_id
                || existing.admission_id != input.admission_id
                || existing.kind != input.kind
                || existing.authorization_mode != input.authorization_mode
                || existing.gate_evaluation_id != input.gate_evaluation_id
                || existing.subject_commit_sha != input.subject_commit_sha
                || existing.expected_remote_head_sha != input.expected_remote_head_sha
                || existing.summary_artifact_id != input.summary_artifact_id
                || existing.body_digest != input.body_digest
            {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(PrExternalActionWrite {
                action: existing,
                created: false,
                event: None,
            });
        }
        let state = match input.authorization_mode {
            PrFeedbackAuthorization::Automatic => PrExternalActionState::Approved,
            PrFeedbackAuthorization::HumanApproval => PrExternalActionState::Proposed,
        };
        sqlx::query(
            "INSERT INTO pr_external_action (
                id, task_id, admission_id, kind, state, authorization_mode,
                gate_evaluation_id, subject_commit_sha, expected_remote_head_sha,
                summary_artifact_id, body_digest, idempotency_key, version,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.admission_id)
        .bind(input.kind.to_string())
        .bind(state.to_string())
        .bind(input.authorization_mode.to_string())
        .bind(&input.gate_evaluation_id)
        .bind(&input.subject_commit_sha)
        .bind(&input.expected_remote_head_sha)
        .bind(&input.summary_artifact_id)
        .bind(&input.body_digest)
        .bind(&input.idempotency_key)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let action = get_action_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_action_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(PrExternalActionWrite {
            action,
            created: true,
            event,
        })
    }

    async fn attach_pr_external_action_proposal(
        &self,
        id: &str,
        expected_version: i64,
        proposal_id: &str,
        updated_at: &str,
    ) -> Result<PrExternalAction> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE pr_external_action
             SET proposal_id = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND state = 'proposed' AND proposal_id IS NULL",
        )
        .bind(proposal_id)
        .bind(updated_at)
        .bind(id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(action_stale_or_missing(&mut tx, id).await?);
        }
        let action = get_action_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(action)
    }

    async fn transition_pr_external_action(
        &self,
        input: TransitionPrExternalAction,
        event: Option<CreateDomainEvent>,
    ) -> Result<PrExternalActionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let terminal = input.to_state.is_terminal();
        let updated = sqlx::query(
            "UPDATE pr_external_action SET
                state = ?,
                decision_id = COALESCE(?, decision_id),
                provider_event_id = COALESCE(?, provider_event_id),
                provider_result_json = COALESCE(?, provider_result_json),
                version = version + 1,
                updated_at = ?,
                finished_at = CASE WHEN ? THEN ? ELSE NULL END
             WHERE id = ? AND version = ?",
        )
        .bind(input.to_state.to_string())
        .bind(&input.decision_id)
        .bind(&input.provider_event_id)
        .bind(&input.provider_result_json)
        .bind(&input.updated_at)
        .bind(terminal)
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(action_stale_or_missing(&mut tx, &input.id).await?);
        }
        let action = get_action_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_action_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(PrExternalActionWrite {
            action,
            created: false,
            event,
        })
    }

    async fn get_pr_external_action(&self, id: &str) -> Result<Option<PrExternalAction>> {
        sqlx::query("SELECT * FROM pr_external_action WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_external_action)
            .transpose()
    }

    async fn get_pr_external_action_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<PrExternalAction>> {
        sqlx::query("SELECT * FROM pr_external_action WHERE idempotency_key = ?")
            .bind(idempotency_key)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_external_action)
            .transpose()
    }

    async fn get_open_pr_external_action(&self, task_id: &str) -> Result<Option<PrExternalAction>> {
        sqlx::query(
            "SELECT * FROM pr_external_action
             WHERE task_id = ? AND state IN ('proposed', 'approved', 'running',
                                             'outcome_unknown', 'reconciliation_required')",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_pr_external_action)
        .transpose()
    }

    async fn list_pr_external_actions_by_task(
        &self,
        task_id: &str,
        state: Option<PrExternalActionState>,
        page: &PageRequest,
    ) -> Result<Page<PrExternalAction>> {
        let offset = decode_offset(&page.cursor)?;
        let state = state.map(|state| state.to_string());
        let rows = sqlx::query(
            "SELECT * FROM pr_external_action
             WHERE task_id = ? AND (? IS NULL OR state = ?)
             ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id)
        .bind(state.as_deref())
        .bind(state.as_deref())
        .bind(limit(page) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .into_iter()
            .map(map_pr_external_action)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM pr_external_action
                     WHERE task_id = ? AND (? IS NULL OR state = ?)",
                )
                .bind(task_id)
                .bind(state.as_deref())
                .bind(state.as_deref())
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, page, offset, total)
    }

    async fn list_pr_external_actions_in_states(
        &self,
        states: &[PrExternalActionState],
        limit: i64,
    ) -> Result<Vec<PrExternalAction>> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; states.len()].join(", ");
        let sql = format!(
            "SELECT * FROM pr_external_action WHERE state IN ({placeholders})
             ORDER BY updated_at, id LIMIT ?"
        );
        let mut query = sqlx::query(&sql);
        for state in states {
            query = query.bind(state.to_string());
        }
        query
            .bind(limit.clamp(1, 500))
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_pr_external_action)
            .collect()
    }
}
