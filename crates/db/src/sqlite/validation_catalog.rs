use super::*;
use crate::{models::*, pagination::*, repository::*};

fn map_validation_check_definition(row: SqliteRow) -> Result<ValidationCheckDefinition> {
    Ok(ValidationCheckDefinition {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        repo_id: row.try_get("repo_id")?,
        check_key: row.try_get("check_key")?,
        version: row.try_get("version")?,
        command: row.try_get("command")?,
        timeout_seconds: row.try_get("timeout_seconds")?,
        env_policy: row.try_get("env_policy")?,
        definition_digest: row.try_get("definition_digest")?,
        state: parse_enum(row.try_get("state")?)?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        retired_at: row.try_get("retired_at")?,
    })
}

fn map_validation_check_request(row: SqliteRow) -> Result<ValidationCheckRequest> {
    let baseline: i64 = row.try_get("baseline")?;
    Ok(ValidationCheckRequest {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        authorization_version: row.try_get("authorization_version")?,
        check_definition_id: row.try_get("check_definition_id")?,
        definition_digest: row.try_get("definition_digest")?,
        trigger_kind: parse_enum(row.try_get("trigger_kind")?)?,
        subject_kind: parse_enum(row.try_get("subject_kind")?)?,
        subject_id: row.try_get("subject_id")?,
        subject_version: row.try_get("subject_version")?,
        workspace_id: row.try_get("workspace_id")?,
        commit_sha: row.try_get("commit_sha")?,
        workspace_snapshot_digest: row.try_get("workspace_snapshot_digest")?,
        baseline: baseline != 0,
        baseline_commit_sha: row.try_get("baseline_commit_sha")?,
        requested_by_request_id: row.try_get("requested_by_request_id")?,
        requested_by_execution_id: row.try_get("requested_by_execution_id")?,
        reason: row.try_get("reason")?,
        idempotency_key: row.try_get("idempotency_key")?,
        state: parse_enum(row.try_get("state")?)?,
        candidate_run_id: row.try_get("candidate_run_id")?,
        failure_code: row.try_get("failure_code")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn map_validation_comparison(row: SqliteRow) -> Result<ValidationComparison> {
    let same_output_digest: i64 = row.try_get("same_output_digest")?;
    Ok(ValidationComparison {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        check_request_id: row.try_get("check_request_id")?,
        candidate_run_id: row.try_get("candidate_run_id")?,
        candidate_status: parse_enum(row.try_get("candidate_status")?)?,
        candidate_output_digest: row.try_get("candidate_output_digest")?,
        baseline_commit_sha: row.try_get("baseline_commit_sha")?,
        baseline_snapshot_digest: row.try_get("baseline_snapshot_digest")?,
        baseline_provenance_json: row.try_get("baseline_provenance_json")?,
        baseline_status: parse_enum(row.try_get("baseline_status")?)?,
        baseline_exit_code: row.try_get("baseline_exit_code")?,
        baseline_output_digest: row.try_get("baseline_output_digest")?,
        classification: parse_enum(row.try_get("classification")?)?,
        same_output_digest: same_output_digest != 0,
        created_at: row.try_get("created_at")?,
    })
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

async fn get_definition_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<ValidationCheckDefinition>> {
    sqlx::query("SELECT * FROM validation_check_definition WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_validation_check_definition)
        .transpose()
}

async fn get_request_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<ValidationCheckRequest>> {
    sqlx::query("SELECT * FROM validation_check_request WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_validation_check_request)
        .transpose()
}

fn same_request_identity(
    existing: &ValidationCheckRequest,
    input: &CreateValidationCheckRequest,
) -> bool {
    existing.task_id == input.task_id
        && existing.authorization_version == input.authorization_version
        && existing.check_definition_id == input.check_definition_id
        && existing.definition_digest == input.definition_digest
        && existing.trigger_kind == input.trigger_kind
        && existing.subject_kind == input.subject_kind
        && existing.subject_id == input.subject_id
        && existing.subject_version == input.subject_version
        && existing.workspace_id == input.workspace_id
        && existing.commit_sha == input.commit_sha
        && existing.workspace_snapshot_digest == input.workspace_snapshot_digest
        && existing.baseline == input.baseline
}

#[async_trait]
impl ValidationCatalogRepo for SqliteDb {
    async fn publish_validation_check_definition(
        &self,
        input: PublishValidationCheckDefinition,
    ) -> Result<ValidationCheckDefinition> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous_version: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(version) FROM validation_check_definition
             WHERE project_id = ? AND COALESCE(repo_id, '') = COALESCE(?, '') AND check_key = ?",
        )
        .bind(&input.project_id)
        .bind(&input.repo_id)
        .bind(&input.check_key)
        .fetch_one(&mut *tx)
        .await?;
        let version = previous_version.unwrap_or(0) + 1;
        sqlx::query(
            "UPDATE validation_check_definition SET state = 'retired', retired_at = ?
             WHERE project_id = ? AND COALESCE(repo_id, '') = COALESCE(?, '')
               AND check_key = ? AND state = 'active'",
        )
        .bind(&input.created_at)
        .bind(&input.project_id)
        .bind(&input.repo_id)
        .bind(&input.check_key)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let digest = validation_check_definition_digest(&ValidationCheckDigestInput {
            check_key: input.check_key.clone(),
            version,
            command: input.command.clone(),
            timeout_seconds: input.timeout_seconds,
            env_policy: input.env_policy.clone(),
        });
        sqlx::query(
            "INSERT INTO validation_check_definition (
                id, project_id, repo_id, check_key, version, command, timeout_seconds,
                env_policy, definition_digest, state, created_by, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.project_id)
        .bind(&input.repo_id)
        .bind(&input.check_key)
        .bind(version)
        .bind(&input.command)
        .bind(input.timeout_seconds)
        .bind(&input.env_policy)
        .bind(&digest)
        .bind(&input.created_by)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let definition = get_definition_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(definition)
    }

    async fn retire_validation_check_definition(
        &self,
        id: &str,
        expected_state: ValidationCheckState,
        retired_at: &str,
    ) -> Result<ValidationCheckDefinition> {
        if expected_state != ValidationCheckState::Active {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE validation_check_definition SET state = 'retired', retired_at = ?
             WHERE id = ? AND state = 'active'",
        )
        .bind(retired_at)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            let exists = get_definition_in_tx(&mut tx, id).await?.is_some();
            return Err(if exists {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        let definition = get_definition_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(definition)
    }

    async fn get_validation_check_definition(
        &self,
        id: &str,
    ) -> Result<Option<ValidationCheckDefinition>> {
        sqlx::query("SELECT * FROM validation_check_definition WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_check_definition)
            .transpose()
    }

    async fn get_active_validation_check_definition(
        &self,
        project_id: &str,
        repo_id: Option<&str>,
        check_key: &str,
    ) -> Result<Option<ValidationCheckDefinition>> {
        sqlx::query(
            "SELECT * FROM validation_check_definition
             WHERE project_id = ? AND COALESCE(repo_id, '') = COALESCE(?, '')
               AND check_key = ? AND state = 'active'",
        )
        .bind(project_id)
        .bind(repo_id)
        .bind(check_key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_validation_check_definition)
        .transpose()
    }

    async fn list_validation_check_definitions(
        &self,
        project_id: &str,
        repo_id: Option<&str>,
        state: Option<ValidationCheckState>,
        page: &PageRequest,
    ) -> Result<Page<ValidationCheckDefinition>> {
        let offset = decode_offset(&page.cursor)?;
        let state = state.map(|state| state.to_string());
        let rows = sqlx::query(
            "SELECT * FROM validation_check_definition
             WHERE project_id = ? AND (? IS NULL OR repo_id IS ?) AND (? IS NULL OR state = ?)
             ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(project_id)
        .bind(repo_id)
        .bind(repo_id)
        .bind(state.as_deref())
        .bind(state.as_deref())
        .bind(limit(page) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .into_iter()
            .map(map_validation_check_definition)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM validation_check_definition
                     WHERE project_id = ? AND (? IS NULL OR repo_id IS ?)
                       AND (? IS NULL OR state = ?)",
                )
                .bind(project_id)
                .bind(repo_id)
                .bind(repo_id)
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

#[async_trait]
impl ValidationCheckRequestRepo for SqliteDb {
    async fn create_validation_check_request(
        &self,
        input: CreateValidationCheckRequest,
        event: Option<CreateDomainEvent>,
    ) -> Result<ValidationCheckRequestWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing =
            sqlx::query("SELECT * FROM validation_check_request WHERE idempotency_key = ?")
                .bind(&input.idempotency_key)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_validation_check_request)
                .transpose()?;
        if let Some(existing) = existing {
            if !same_request_identity(&existing, &input) {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(ValidationCheckRequestWrite {
                request: existing,
                created: false,
                event: None,
            });
        }
        sqlx::query(
            "INSERT INTO validation_check_request (
                id, task_id, authorization_version, check_definition_id, definition_digest,
                trigger_kind, subject_kind, subject_id, subject_version, workspace_id,
                commit_sha, workspace_snapshot_digest, baseline, requested_by_request_id,
                requested_by_execution_id, reason, idempotency_key, state, version,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.authorization_version)
        .bind(&input.check_definition_id)
        .bind(&input.definition_digest)
        .bind(input.trigger_kind.to_string())
        .bind(input.subject_kind.to_string())
        .bind(&input.subject_id)
        .bind(input.subject_version)
        .bind(&input.workspace_id)
        .bind(&input.commit_sha)
        .bind(&input.workspace_snapshot_digest)
        .bind(i64::from(input.baseline))
        .bind(&input.requested_by_request_id)
        .bind(&input.requested_by_execution_id)
        .bind(&input.reason)
        .bind(&input.idempotency_key)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let request = get_request_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(ValidationCheckRequestWrite {
            request,
            created: true,
            event,
        })
    }

    async fn transition_validation_check_request(
        &self,
        input: TransitionValidationCheckRequest,
        event: Option<CreateDomainEvent>,
    ) -> Result<ValidationCheckRequestWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE validation_check_request SET
                state = ?,
                candidate_run_id = COALESCE(?, candidate_run_id),
                baseline_commit_sha = COALESCE(?, baseline_commit_sha),
                failure_code = ?,
                version = version + 1,
                updated_at = ?,
                completed_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(input.to_state.to_string())
        .bind(&input.candidate_run_id)
        .bind(&input.baseline_commit_sha)
        .bind(&input.failure_code)
        .bind(&input.updated_at)
        .bind(&input.completed_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            let exists = get_request_in_tx(&mut tx, &input.id).await?.is_some();
            return Err(if exists {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        let request = get_request_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(ValidationCheckRequestWrite {
            request,
            created: false,
            event,
        })
    }

    async fn get_validation_check_request(
        &self,
        id: &str,
    ) -> Result<Option<ValidationCheckRequest>> {
        sqlx::query("SELECT * FROM validation_check_request WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_check_request)
            .transpose()
    }

    async fn get_validation_check_request_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ValidationCheckRequest>> {
        sqlx::query("SELECT * FROM validation_check_request WHERE idempotency_key = ?")
            .bind(idempotency_key)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_check_request)
            .transpose()
    }

    async fn list_validation_check_requests_by_task(
        &self,
        task_id: &str,
        state: Option<ValidationCheckRequestState>,
        page: &PageRequest,
    ) -> Result<Page<ValidationCheckRequest>> {
        let offset = decode_offset(&page.cursor)?;
        let state = state.map(|state| state.to_string());
        let rows = sqlx::query(
            "SELECT * FROM validation_check_request
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
            .map(map_validation_check_request)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM validation_check_request
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

    async fn list_validation_check_requests_by_state(
        &self,
        state: ValidationCheckRequestState,
        limit: i64,
    ) -> Result<Vec<ValidationCheckRequest>> {
        sqlx::query(
            "SELECT * FROM validation_check_request WHERE state = ?
             ORDER BY created_at ASC, id ASC LIMIT ?",
        )
        .bind(state.to_string())
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_validation_check_request)
        .collect()
    }
}

#[async_trait]
impl ValidationComparisonRepo for SqliteDb {
    async fn create_validation_comparison(
        &self,
        input: CreateValidationComparison,
        event: Option<CreateDomainEvent>,
    ) -> Result<ValidationComparisonWrite> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing =
            sqlx::query("SELECT * FROM validation_comparison WHERE check_request_id = ?")
                .bind(&input.check_request_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_validation_comparison)
                .transpose()?;
        if let Some(existing) = existing {
            if existing.candidate_run_id != input.candidate_run_id
                || existing.baseline_commit_sha != input.baseline_commit_sha
                || existing.baseline_snapshot_digest != input.baseline_snapshot_digest
                || existing.baseline_status != input.baseline_status
                || existing.baseline_exit_code != input.baseline_exit_code
                || existing.baseline_output_digest != input.baseline_output_digest
                || existing.candidate_output_digest != input.candidate_output_digest
            {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(ValidationComparisonWrite {
                comparison: existing,
                created: false,
                event: None,
            });
        }
        let candidate_status: String =
            sqlx::query_scalar("SELECT status FROM validation_run WHERE id = ? AND task_id = ?")
                .bind(&input.candidate_run_id)
                .bind(&input.task_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
        let candidate_status: ValidationRunStatus = parse_enum(candidate_status)?;
        let classification =
            ValidationComparisonClass::classify(candidate_status, input.baseline_status);
        let same_output_digest = matches!(
            (&input.candidate_output_digest, &input.baseline_output_digest),
            (Some(candidate), Some(baseline)) if candidate == baseline
        );
        sqlx::query(
            "INSERT INTO validation_comparison (
                id, task_id, check_request_id, candidate_run_id, candidate_status,
                candidate_output_digest, baseline_commit_sha, baseline_snapshot_digest,
                baseline_provenance_json, baseline_status, baseline_exit_code,
                baseline_output_digest, classification, same_output_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.check_request_id)
        .bind(&input.candidate_run_id)
        .bind(candidate_status.to_string())
        .bind(&input.candidate_output_digest)
        .bind(&input.baseline_commit_sha)
        .bind(&input.baseline_snapshot_digest)
        .bind(&input.baseline_provenance_json)
        .bind(input.baseline_status.to_string())
        .bind(input.baseline_exit_code)
        .bind(&input.baseline_output_digest)
        .bind(classification.to_string())
        .bind(i64::from(same_output_digest))
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let comparison = sqlx::query("SELECT * FROM validation_comparison WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await
            .map_err(DbError::from)
            .and_then(map_validation_comparison)?;
        let event = append_optional_event(self, &mut tx, event.as_ref()).await?;
        tx.commit().await?;
        Ok(ValidationComparisonWrite {
            comparison,
            created: true,
            event,
        })
    }

    async fn get_validation_comparison(&self, id: &str) -> Result<Option<ValidationComparison>> {
        sqlx::query("SELECT * FROM validation_comparison WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_comparison)
            .transpose()
    }

    async fn get_validation_comparison_by_request(
        &self,
        check_request_id: &str,
    ) -> Result<Option<ValidationComparison>> {
        sqlx::query("SELECT * FROM validation_comparison WHERE check_request_id = ?")
            .bind(check_request_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_comparison)
            .transpose()
    }

    async fn list_validation_comparisons_by_task(
        &self,
        task_id: &str,
        page: &PageRequest,
    ) -> Result<Page<ValidationComparison>> {
        let offset = decode_offset(&page.cursor)?;
        let rows = sqlx::query(
            "SELECT * FROM validation_comparison WHERE task_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id)
        .bind(limit(page) + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let items = rows
            .into_iter()
            .map(map_validation_comparison)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM validation_comparison WHERE task_id = ?",
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
}
