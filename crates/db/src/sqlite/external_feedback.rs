use super::*;
use crate::{models::*, pagination::*, repository::*};

fn map_external_feedback_item(row: SqliteRow) -> Result<ExternalFeedbackItem> {
    let review_state: Option<String> = row.try_get("review_state")?;
    Ok(ExternalFeedbackItem {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        admission_id: row.try_get("admission_id")?,
        provider_item_id: row.try_get("provider_item_id")?,
        kind: parse_enum(row.try_get("kind")?)?,
        author_login: row.try_get("author_login")?,
        author_association: row.try_get("author_association")?,
        review_state: review_state.map(parse_enum).transpose()?,
        path: row.try_get("path")?,
        line: row.try_get("line")?,
        commit_sha: row.try_get("commit_sha")?,
        body: row.try_get("body")?,
        body_truncated: row.try_get::<i64, _>("body_truncated")? != 0,
        body_digest: row.try_get("body_digest")?,
        provider_url: row.try_get("provider_url")?,
        trust_class: parse_enum(row.try_get("trust_class")?)?,
        head_state: parse_enum(row.try_get("head_state")?)?,
        ingested_at: row.try_get("ingested_at")?,
    })
}

fn map_external_feedback_triage(row: SqliteRow) -> Result<ExternalFeedbackTriage> {
    Ok(ExternalFeedbackTriage {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        feedback_item_id: row.try_get("feedback_item_id")?,
        body_digest: row.try_get("body_digest")?,
        disposition: parse_enum(row.try_get("disposition")?)?,
        rationale: row.try_get("rationale")?,
        instructions: row.try_get("instructions")?,
        decision_digest: row.try_get("decision_digest")?,
        execution_id: row.try_get("execution_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_external_feedback_disposition(row: SqliteRow) -> Result<ExternalFeedbackDispositionRecord> {
    let scope: Option<String> = row.try_get("scope_classification")?;
    let dismiss: Option<String> = row.try_get("dismiss_reason")?;
    Ok(ExternalFeedbackDispositionRecord {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        triage_id: row.try_get("triage_id")?,
        request_id: row.try_get("request_id")?,
        feedback_item_id: row.try_get("feedback_item_id")?,
        body_digest: row.try_get("body_digest")?,
        disposition: parse_enum(row.try_get::<String, _>("disposition")?)?,
        legacy_disposition: parse_enum(row.try_get::<String, _>("legacy_disposition")?)?,
        scope_classification: scope.map(parse_enum).transpose()?,
        classification_facts_json: row.try_get("classification_facts_json")?,
        dismiss_reason: dismiss.map(parse_enum).transpose()?,
        proposal_json: row.try_get("proposal_json")?,
        human_decision_id: row.try_get("human_decision_id")?,
        followup_task_id: row.try_get("followup_task_id")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn record_disposition(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateExternalFeedbackDisposition,
) -> Result<ExternalFeedbackDispositionRecord> {
    if let Some(existing) = sqlx::query(
        "SELECT * FROM task_external_feedback_disposition
         WHERE request_id = ? AND feedback_item_id = ?",
    )
    .bind(&input.request_id)
    .bind(&input.feedback_item_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(map_external_feedback_disposition)
    .transpose()?
    {
        if existing.body_digest != input.body_digest
            || existing.disposition != input.disposition
            || existing.triage_id != input.triage_id
            || existing.scope_classification != input.scope_classification
            || existing.dismiss_reason != input.dismiss_reason
            || existing.proposal_json != input.proposal_json
        {
            return Err(DbError::IdempotencyConflict);
        }
        return Ok(existing);
    }
    sqlx::query(
        "INSERT INTO task_external_feedback_disposition (
            id, task_id, triage_id, request_id, feedback_item_id, body_digest,
            disposition, legacy_disposition, scope_classification,
            classification_facts_json, dismiss_reason, proposal_json, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(&input.triage_id)
    .bind(&input.request_id)
    .bind(&input.feedback_item_id)
    .bind(&input.body_digest)
    .bind(input.disposition.to_string())
    .bind(input.disposition.legacy().to_string())
    .bind(input.scope_classification.map(|value| value.to_string()))
    .bind(&input.classification_facts_json)
    .bind(input.dismiss_reason.map(|value| value.to_string()))
    .bind(&input.proposal_json)
    .bind(&input.created_at)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    sqlx::query("SELECT * FROM task_external_feedback_disposition WHERE id = ?")
        .bind(&input.id)
        .fetch_one(&mut **tx)
        .await
        .map_err(DbError::from)
        .and_then(map_external_feedback_disposition)
}

async fn bind_disposition_link(
    pool: &SqlitePool,
    disposition_id: &str,
    column: &'static str,
    value: &str,
) -> Result<ExternalFeedbackDispositionRecord> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let existing = sqlx::query("SELECT * FROM task_external_feedback_disposition WHERE id = ?")
        .bind(disposition_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_external_feedback_disposition)
        .transpose()?
        .ok_or(DbError::NotFound)?;
    let current = if column == "human_decision_id" {
        existing.human_decision_id.clone()
    } else {
        existing.followup_task_id.clone()
    };
    match current.as_deref() {
        Some(current) if current == value => return Ok(existing),
        Some(_) => return Err(DbError::IdempotencyConflict),
        None => {}
    }
    // `column` is one of two compile-time literals.
    sqlx::query(&format!(
        "UPDATE task_external_feedback_disposition SET {column} = ? WHERE id = ?"
    ))
    .bind(value)
    .bind(disposition_id)
    .execute(&mut *tx)
    .await
    .map_err(check_error)?;
    let updated = sqlx::query("SELECT * FROM task_external_feedback_disposition WHERE id = ?")
        .bind(disposition_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(DbError::from)
        .and_then(map_external_feedback_disposition)?;
    tx.commit().await?;
    Ok(updated)
}

async fn add_set_items(
    tx: &mut Transaction<'_, Sqlite>,
    request_id: &str,
    task_id: &str,
    entries: &[ExternalFeedbackSetEntry],
) -> Result<()> {
    if entries.is_empty() || entries.len() > MAX_EXTERNAL_FEEDBACK_SET_ITEMS {
        return Err(DbError::Check(format!(
            "an external_feedback set holds 1..={MAX_EXTERNAL_FEEDBACK_SET_ITEMS} items"
        )));
    }
    let existing = sqlx::query(
        "SELECT feedback_item_id, body_digest FROM task_external_feedback_set_item
         WHERE request_id = ? ORDER BY position",
    )
    .bind(request_id)
    .fetch_all(&mut **tx)
    .await?;
    if !existing.is_empty() {
        let same = existing.len() == entries.len()
            && existing.iter().zip(entries).all(|(row, entry)| {
                row.try_get::<String, _>("feedback_item_id")
                    .map(|id| id == entry.feedback_item_id)
                    .unwrap_or(false)
                    && row
                        .try_get::<String, _>("body_digest")
                        .map(|digest| digest == entry.body_digest)
                        .unwrap_or(false)
            });
        return if same {
            Ok(())
        } else {
            Err(DbError::IdempotencyConflict)
        };
    }
    for (position, entry) in entries.iter().enumerate() {
        sqlx::query(
            "INSERT INTO task_external_feedback_set_item
                (request_id, task_id, feedback_item_id, body_digest, position)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(request_id)
        .bind(task_id)
        .bind(&entry.feedback_item_id)
        .bind(&entry.body_digest)
        .bind(position as i64)
        .execute(&mut **tx)
        .await
        .map_err(check_error)?;
    }
    Ok(())
}

async fn record_triage(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateExternalFeedbackTriage,
) -> Result<ExternalFeedbackTriage> {
    let existing = sqlx::query(
        "SELECT * FROM task_external_feedback_triage
         WHERE request_id = ? AND feedback_item_id = ?",
    )
    .bind(&input.request_id)
    .bind(&input.feedback_item_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(map_external_feedback_triage)
    .transpose()?;
    if let Some(existing) = existing {
        if existing.decision_digest != input.decision_digest
            || existing.body_digest != input.body_digest
            || existing.disposition != input.disposition
            || existing.execution_id != input.execution_id
        {
            return Err(DbError::IdempotencyConflict);
        }
        return Ok(existing);
    }
    sqlx::query(
        "INSERT INTO task_external_feedback_triage (
            id, task_id, request_id, feedback_item_id, body_digest, disposition,
            rationale, instructions, decision_digest, execution_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(&input.request_id)
    .bind(&input.feedback_item_id)
    .bind(&input.body_digest)
    .bind(input.disposition.to_string())
    .bind(&input.rationale)
    .bind(&input.instructions)
    .bind(&input.decision_digest)
    .bind(&input.execution_id)
    .bind(&input.created_at)
    .execute(&mut **tx)
    .await
    .map_err(check_error)?;
    sqlx::query("SELECT * FROM task_external_feedback_triage WHERE id = ?")
        .bind(&input.id)
        .fetch_one(&mut **tx)
        .await
        .map_err(DbError::from)
        .and_then(map_external_feedback_triage)
}

#[async_trait]
impl ExternalFeedbackRepo for SqliteDb {
    async fn ingest_external_feedback(
        &self,
        items: Vec<CreateExternalFeedbackItem>,
    ) -> Result<Vec<(ExternalFeedbackItem, bool)>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut out = Vec::with_capacity(items.len());
        for input in &items {
            let inserted = sqlx::query(
                "INSERT INTO external_feedback_item (
                    id, task_id, admission_id, provider_item_id, kind, author_login,
                    author_association, review_state, path, line, commit_sha, body,
                    body_truncated, body_digest, provider_url, trust_class, head_state,
                    ingested_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(admission_id, provider_item_id, body_digest) DO NOTHING",
            )
            .bind(&input.id)
            .bind(&input.task_id)
            .bind(&input.admission_id)
            .bind(&input.provider_item_id)
            .bind(input.kind.to_string())
            .bind(&input.author_login)
            .bind(&input.author_association)
            .bind(input.review_state.map(|state| state.to_string()))
            .bind(&input.path)
            .bind(input.line)
            .bind(&input.commit_sha)
            .bind(&input.body)
            .bind(i64::from(input.body_truncated))
            .bind(&input.body_digest)
            .bind(&input.provider_url)
            .bind(input.trust_class.to_string())
            .bind(input.head_state.to_string())
            .bind(&input.ingested_at)
            .execute(&mut *tx)
            .await
            .map_err(check_error)?
            .rows_affected();
            let row = sqlx::query(
                "SELECT * FROM external_feedback_item
                 WHERE admission_id = ? AND provider_item_id = ? AND body_digest = ?",
            )
            .bind(&input.admission_id)
            .bind(&input.provider_item_id)
            .bind(&input.body_digest)
            .fetch_one(&mut *tx)
            .await?;
            let item = map_external_feedback_item(row)?;
            if inserted == 0 && item.task_id != input.task_id {
                return Err(DbError::IdempotencyConflict);
            }
            out.push((item, inserted == 1));
        }
        tx.commit().await?;
        Ok(out)
    }

    async fn get_external_feedback_item(&self, id: &str) -> Result<Option<ExternalFeedbackItem>> {
        sqlx::query("SELECT * FROM external_feedback_item WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_external_feedback_item)
            .transpose()
    }

    async fn list_external_feedback_items(
        &self,
        admission_id: &str,
        trust_class: Option<ExternalFeedbackTrustClass>,
        head_state: Option<ExternalFeedbackHeadState>,
        page: &PageRequest,
    ) -> Result<Page<ExternalFeedbackItem>> {
        let offset = decode_offset(&page.cursor)?;
        let trust_class = trust_class.map(|value| value.to_string());
        let head_state = head_state.map(|value| value.to_string());
        let rows = sqlx::query(
            "SELECT * FROM external_feedback_item
             WHERE admission_id = ? AND (? IS NULL OR trust_class = ?)
               AND (? IS NULL OR head_state = ?)
             ORDER BY ingested_at, id LIMIT ? OFFSET ?",
        )
        .bind(admission_id)
        .bind(trust_class.as_deref())
        .bind(trust_class.as_deref())
        .bind(head_state.as_deref())
        .bind(head_state.as_deref())
        .bind(limit(page) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .into_iter()
            .map(map_external_feedback_item)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM external_feedback_item
                     WHERE admission_id = ? AND (? IS NULL OR trust_class = ?)
                       AND (? IS NULL OR head_state = ?)",
                )
                .bind(admission_id)
                .bind(trust_class.as_deref())
                .bind(trust_class.as_deref())
                .bind(head_state.as_deref())
                .bind(head_state.as_deref())
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, page, offset, total)
    }

    async fn list_unconsumed_trusted_feedback(
        &self,
        admission_id: &str,
        limit: i64,
    ) -> Result<Vec<ExternalFeedbackItem>> {
        sqlx::query(
            "SELECT i.* FROM external_feedback_item i
             WHERE i.admission_id = ? AND i.trust_class = 'trusted'
               AND i.head_state = 'current'
               AND EXISTS (SELECT 1 FROM remote_pr_admission a
                           WHERE a.task_merge_operation_id = i.admission_id
                             AND a.rework_outcome IS NULL)
               AND NOT EXISTS (SELECT 1 FROM task_external_feedback_set_item s
                               WHERE s.feedback_item_id = i.id)
             ORDER BY i.ingested_at, i.id LIMIT ?",
        )
        .bind(admission_id)
        .bind(limit.clamp(1, MAX_EXTERNAL_FEEDBACK_SET_ITEMS as i64))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_external_feedback_item)
        .collect()
    }

    async fn mark_external_feedback_stale_head(
        &self,
        admission_id: &str,
        current_head_sha: &str,
    ) -> Result<u64> {
        // A stored SHA may be an abbreviation (>= 7 chars) of the full head.
        let result = sqlx::query(
            "UPDATE external_feedback_item SET head_state = 'stale_head'
             WHERE admission_id = ? AND head_state = 'current'
               AND commit_sha IS NOT NULL
               AND substr(?, 1, length(commit_sha)) != commit_sha",
        )
        .bind(admission_id)
        .bind(current_head_sha)
        .execute(&self.pool)
        .await
        .map_err(check_error)?;
        Ok(result.rows_affected())
    }

    async fn add_external_feedback_set_items(
        &self,
        request_id: &str,
        task_id: &str,
        entries: &[ExternalFeedbackSetEntry],
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        add_set_items(&mut tx, request_id, task_id, entries).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn add_external_feedback_set_items_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        request_id: &str,
        task_id: &str,
        entries: &[ExternalFeedbackSetEntry],
    ) -> Result<()> {
        add_set_items(tx, request_id, task_id, entries).await
    }

    async fn list_external_feedback_set(
        &self,
        request_id: &str,
    ) -> Result<Vec<ExternalFeedbackItem>> {
        sqlx::query(
            "SELECT i.* FROM task_external_feedback_set_item s
             JOIN external_feedback_item i ON i.id = s.feedback_item_id
             WHERE s.request_id = ? ORDER BY s.position",
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_external_feedback_item)
        .collect()
    }

    async fn record_external_feedback_triage(
        &self,
        input: CreateExternalFeedbackTriage,
    ) -> Result<ExternalFeedbackTriage> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let triage = record_triage(&mut tx, &input).await?;
        tx.commit().await?;
        Ok(triage)
    }

    async fn record_external_feedback_triage_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        input: CreateExternalFeedbackTriage,
    ) -> Result<ExternalFeedbackTriage> {
        record_triage(tx, &input).await
    }

    async fn list_external_feedback_triage(
        &self,
        request_id: &str,
    ) -> Result<Vec<ExternalFeedbackTriage>> {
        sqlx::query(
            "SELECT * FROM task_external_feedback_triage
             WHERE request_id = ? ORDER BY created_at, id",
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_external_feedback_triage)
        .collect()
    }

    async fn latest_external_feedback_triage_for_item(
        &self,
        feedback_item_id: &str,
    ) -> Result<Option<ExternalFeedbackTriage>> {
        sqlx::query(
            "SELECT * FROM task_external_feedback_triage
             WHERE feedback_item_id = ? ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(feedback_item_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_external_feedback_triage)
        .transpose()
    }

    async fn record_external_feedback_disposition(
        &self,
        input: CreateExternalFeedbackDisposition,
    ) -> Result<ExternalFeedbackDispositionRecord> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let record = record_disposition(&mut tx, &input).await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn record_external_feedback_disposition_in_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        input: CreateExternalFeedbackDisposition,
    ) -> Result<ExternalFeedbackDispositionRecord> {
        record_disposition(tx, &input).await
    }

    async fn list_external_feedback_dispositions(
        &self,
        request_id: &str,
    ) -> Result<Vec<ExternalFeedbackDispositionRecord>> {
        sqlx::query(
            "SELECT d.* FROM task_external_feedback_disposition d
             LEFT JOIN task_external_feedback_set_item s
               ON s.request_id = d.request_id AND s.feedback_item_id = d.feedback_item_id
             WHERE d.request_id = ? ORDER BY s.position, d.created_at, d.id",
        )
        .bind(request_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_external_feedback_disposition)
        .collect()
    }

    async fn bind_external_feedback_disposition_decision(
        &self,
        disposition_id: &str,
        human_decision_id: &str,
    ) -> Result<ExternalFeedbackDispositionRecord> {
        bind_disposition_link(
            &self.pool,
            disposition_id,
            "human_decision_id",
            human_decision_id,
        )
        .await
    }

    async fn bind_external_feedback_disposition_followup(
        &self,
        disposition_id: &str,
        followup_task_id: &str,
    ) -> Result<ExternalFeedbackDispositionRecord> {
        bind_disposition_link(
            &self.pool,
            disposition_id,
            "followup_task_id",
            followup_task_id,
        )
        .await
    }
}
