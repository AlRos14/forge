use super::*;

pub(super) fn map_repository_integration(row: SqliteRow) -> Result<WorkUnitRepositoryIntegration> {
    Ok(WorkUnitRepositoryIntegration {
        id: row.try_get("id")?,
        work_unit_integration_id: row.try_get("work_unit_integration_id")?,
        task_id: row.try_get("task_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        execution_id: row.try_get("execution_id")?,
        repository_set_revision: row.try_get("repository_set_revision")?,
        repo_id: row.try_get("repo_id")?,
        source_workspace_id: row.try_get("source_workspace_id")?,
        source_branch: row.try_get("source_branch")?,
        source_sha: row.try_get("source_sha")?,
        target_workspace_id: row.try_get("target_workspace_id")?,
        target_branch: row.try_get("target_branch")?,
        target_before_sha: row.try_get("target_before_sha")?,
        target_after_sha: row.try_get("target_after_sha")?,
        outcome: parse_enum(row.try_get::<String, _>("outcome")?)?,
        conflict_metadata_json: row.try_get("conflict_metadata_json")?,
        version: row.try_get("version")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn get_by_id_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<WorkUnitRepositoryIntegration>> {
    sqlx::query("SELECT * FROM work_unit_repository_integration WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_repository_integration)
        .transpose()
}

#[async_trait]
impl WorkUnitRepositoryIntegrationRepo for SqliteDb {
    async fn create(
        &self,
        input: CreateWorkUnitRepositoryIntegration,
    ) -> Result<WorkUnitRepositoryIntegration> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM work_unit_repository_integration
             WHERE work_unit_integration_id = ? AND repo_id = ?",
        )
        .bind(&input.work_unit_integration_id)
        .bind(&input.repo_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = map_repository_integration(existing)?;
            if existing.id != input.id
                || existing.task_id != input.task_id
                || existing.work_unit_id != input.work_unit_id
                || existing.execution_id != input.execution_id
                || existing.repository_set_revision != input.repository_set_revision
                || existing.repo_id != input.repo_id
                || existing.source_workspace_id != input.source_workspace_id
                || existing.source_branch != input.source_branch
                || existing.source_sha != input.source_sha
                || existing.target_workspace_id != input.target_workspace_id
                || existing.target_branch != input.target_branch
                || existing.target_before_sha != input.target_before_sha
                || existing.started_at != input.started_at
                || existing.created_at != input.created_at
            {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(existing);
        }
        sqlx::query(
            "INSERT INTO work_unit_repository_integration (
                 id, work_unit_integration_id, task_id, work_unit_id, execution_id,
                 repository_set_revision, repo_id, source_workspace_id, source_branch,
                 source_sha, target_workspace_id, target_branch, target_before_sha,
                 target_after_sha, outcome, conflict_metadata_json, version, started_at,
                 finished_at, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 'running', NULL, 1, ?, NULL, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.work_unit_integration_id)
        .bind(&input.task_id)
        .bind(&input.work_unit_id)
        .bind(&input.execution_id)
        .bind(input.repository_set_revision)
        .bind(&input.repo_id)
        .bind(&input.source_workspace_id)
        .bind(&input.source_branch)
        .bind(&input.source_sha)
        .bind(&input.target_workspace_id)
        .bind(&input.target_branch)
        .bind(&input.target_before_sha)
        .bind(&input.started_at)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let record = get_by_id_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(record)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<WorkUnitRepositoryIntegration>> {
        sqlx::query("SELECT * FROM work_unit_repository_integration WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_repository_integration)
            .transpose()
    }

    async fn get_by_parent_and_repo(
        &self,
        work_unit_integration_id: &str,
        repo_id: &str,
    ) -> Result<Option<WorkUnitRepositoryIntegration>> {
        sqlx::query(
            "SELECT * FROM work_unit_repository_integration
             WHERE work_unit_integration_id = ? AND repo_id = ?",
        )
        .bind(work_unit_integration_id)
        .bind(repo_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_repository_integration)
        .transpose()
    }

    async fn list_by_work_unit(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<WorkUnitRepositoryIntegration>> {
        sqlx::query(
            "SELECT integration.*
             FROM work_unit_repository_integration integration
             LEFT JOIN task_repository_set_revision_repo repository
               ON repository.task_id = integration.task_id
              AND repository.revision = integration.repository_set_revision
              AND repository.repo_id = integration.repo_id
             WHERE integration.work_unit_id = ?
             ORDER BY repository.position, integration.repo_id, integration.started_at, integration.id",
        )
        .bind(work_unit_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_repository_integration)
        .collect()
    }

    async fn transition(
        &self,
        id: &str,
        expected_version: i64,
        outcome: WorkUnitRepositoryIntegrationOutcome,
        target_after_sha: Option<&str>,
        conflict_metadata_json: Option<&str>,
        finished_at: Option<&str>,
        updated_at: &str,
    ) -> Result<WorkUnitRepositoryIntegration> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = get_by_id_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        let valid_transition = match (current.outcome, outcome) {
            (WorkUnitRepositoryIntegrationOutcome::Running, next)
                if next != WorkUnitRepositoryIntegrationOutcome::Running =>
            {
                true
            }
            (
                WorkUnitRepositoryIntegrationOutcome::Conflict,
                WorkUnitRepositoryIntegrationOutcome::Running,
            ) => true,
            _ => false,
        };
        if !valid_transition {
            return Err(DbError::InvalidTransition);
        }
        let terminal = outcome != WorkUnitRepositoryIntegrationOutcome::Running;
        if terminal != finished_at.is_some()
            || (matches!(
                outcome,
                WorkUnitRepositoryIntegrationOutcome::Success
                    | WorkUnitRepositoryIntegrationOutcome::Unchanged
            )) != target_after_sha.is_some()
        {
            return Err(DbError::Check(
                "repository integration outcome fields are inconsistent".to_owned(),
            ));
        }
        let result = sqlx::query(
            "UPDATE work_unit_repository_integration
             SET outcome = ?, target_after_sha = ?, conflict_metadata_json = ?,
                 version = version + 1, finished_at = ?, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(outcome.to_string())
        .bind(target_after_sha)
        .bind(conflict_metadata_json)
        .bind(finished_at)
        .bind(updated_at)
        .bind(id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let updated = get_by_id_in_tx(&mut tx, id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(updated)
    }
}
