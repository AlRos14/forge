use super::*;
use crate::{models::*, pagination::*, repository::*};

fn map_human_decision_request(row: SqliteRow) -> Result<HumanDecisionRequest> {
    Ok(HumanDecisionRequest {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        source_kind: parse_enum(row.try_get("source_kind")?)?,
        source_id: row.try_get("source_id")?,
        raised_by_execution_id: row.try_get("raised_by_execution_id")?,
        authorization_version: row.try_get("authorization_version")?,
        question: row.try_get("question")?,
        options_json: row.try_get("options_json")?,
        recommended_option_id: row.try_get("recommended_option_id")?,
        recommendation_rationale: row.try_get("recommendation_rationale")?,
        evidence_links_json: row.try_get("evidence_links_json")?,
        max_authority_effect: parse_enum(row.try_get("max_authority_effect")?)?,
        state: parse_enum(row.try_get("state")?)?,
        answered_by_user_id: row.try_get("answered_by_user_id")?,
        answered_option_id: row.try_get("answered_option_id")?,
        answer_note: row.try_get("answer_note")?,
        answered_at: row.try_get("answered_at")?,
        resume_request_id: row.try_get("resume_request_id")?,
        resume_binding_json: row.try_get("resume_binding_json")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn get_decision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<HumanDecisionRequest>> {
    sqlx::query("SELECT * FROM human_decision_request WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_human_decision_request)
        .transpose()
}

async fn append_optional_event(
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

/// `0` rows from a version-guarded update: distinguish a missing row from a
/// stale version or a state that no longer permits the transition.
async fn stale_or_missing(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<DbError> {
    Ok(if get_decision_in_tx(tx, id).await?.is_some() {
        DbError::VersionConflict
    } else {
        DbError::NotFound
    })
}

#[async_trait]
impl HumanDecisionRepo for SqliteDb {
    async fn create_human_decision_request(
        &self,
        input: CreateHumanDecisionRequest,
        event: Option<CreateDomainEvent>,
    ) -> Result<HumanDecisionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing = sqlx::query(
            "SELECT * FROM human_decision_request
             WHERE source_kind = ? AND source_id = ?
               AND COALESCE(authorization_version, 0) = COALESCE(?, 0)",
        )
        .bind(input.source_kind.to_string())
        .bind(&input.source_id)
        .bind(input.authorization_version)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_human_decision_request)
        .transpose()?;
        if let Some(existing) = existing {
            if existing.task_id != input.task_id
                || existing.question != input.question
                || existing.options_json != input.options_json
                || existing.recommended_option_id != input.recommended_option_id
                || existing.evidence_links_json != input.evidence_links_json
                || existing.max_authority_effect != input.max_authority_effect
                || existing.resume_binding_json != input.resume_binding_json
            {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(HumanDecisionWrite {
                decision: existing,
                created: false,
                event: None,
            });
        }
        sqlx::query(
            "INSERT INTO human_decision_request (
                id, task_id, source_kind, source_id, raised_by_execution_id,
                authorization_version, question, options_json, recommended_option_id,
                recommendation_rationale, evidence_links_json, max_authority_effect,
                state, resume_binding_json, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.source_kind.to_string())
        .bind(&input.source_id)
        .bind(&input.raised_by_execution_id)
        .bind(input.authorization_version)
        .bind(&input.question)
        .bind(&input.options_json)
        .bind(&input.recommended_option_id)
        .bind(&input.recommendation_rationale)
        .bind(&input.evidence_links_json)
        .bind(input.max_authority_effect.to_string())
        .bind(&input.resume_binding_json)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let decision = get_decision_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(HumanDecisionWrite {
            decision,
            created: true,
            event,
        })
    }

    async fn answer_human_decision(
        &self,
        input: AnswerHumanDecision,
        event: Option<CreateDomainEvent>,
    ) -> Result<HumanDecisionWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE human_decision_request SET
                state = 'answered', answered_option_id = ?, answered_by_user_id = ?,
                answer_note = ?, answered_at = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND state = 'open'",
        )
        .bind(&input.option_id)
        .bind(&input.answered_by_user_id)
        .bind(&input.note)
        .bind(&input.answered_at)
        .bind(&input.answered_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(stale_or_missing(&mut tx, &input.id).await?);
        }
        let decision = get_decision_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(HumanDecisionWrite {
            decision,
            created: false,
            event,
        })
    }

    async fn close_human_decision(
        &self,
        id: &str,
        expected_version: i64,
        to_state: HumanDecisionState,
        updated_at: &str,
        event: Option<CreateDomainEvent>,
    ) -> Result<HumanDecisionWrite> {
        if !matches!(
            to_state,
            HumanDecisionState::Superseded | HumanDecisionState::Cancelled
        ) {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE human_decision_request SET state = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND state = 'open'",
        )
        .bind(to_state.to_string())
        .bind(updated_at)
        .bind(id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(stale_or_missing(&mut tx, id).await?);
        }
        let decision = get_decision_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(HumanDecisionWrite {
            decision,
            created: false,
            event,
        })
    }

    async fn bind_human_decision_resume_request(
        &self,
        id: &str,
        expected_version: i64,
        resume_request_id: &str,
        updated_at: &str,
    ) -> Result<HumanDecisionRequest> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE human_decision_request SET resume_request_id = ?,
                version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND state = 'answered' AND resume_request_id IS NULL",
        )
        .bind(resume_request_id)
        .bind(updated_at)
        .bind(id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(stale_or_missing(&mut tx, id).await?);
        }
        let decision = get_decision_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(decision)
    }

    async fn get_human_decision_request(&self, id: &str) -> Result<Option<HumanDecisionRequest>> {
        sqlx::query("SELECT * FROM human_decision_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_human_decision_request)
            .transpose()
    }

    async fn get_human_decision_request_by_source(
        &self,
        source_kind: HumanDecisionSourceKind,
        source_id: &str,
        authorization_version: Option<i64>,
    ) -> Result<Option<HumanDecisionRequest>> {
        sqlx::query(
            "SELECT * FROM human_decision_request
             WHERE source_kind = ? AND source_id = ?
               AND COALESCE(authorization_version, 0) = COALESCE(?, 0)",
        )
        .bind(source_kind.to_string())
        .bind(source_id)
        .bind(authorization_version)
        .fetch_optional(&self.pool)
        .await?
        .map(map_human_decision_request)
        .transpose()
    }

    async fn list_human_decision_requests_by_task(
        &self,
        task_id: &str,
        state: Option<HumanDecisionState>,
        page: &PageRequest,
    ) -> Result<Page<HumanDecisionRequest>> {
        let offset = decode_offset(&page.cursor)?;
        let state = state.map(|state| state.to_string());
        let rows = sqlx::query(
            "SELECT * FROM human_decision_request
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
            .map(map_human_decision_request)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM human_decision_request
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
}
