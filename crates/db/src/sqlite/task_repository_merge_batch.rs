use super::*;

fn map_batch(row: SqliteRow) -> Result<TaskRepositoryMergeBatch> {
    Ok(TaskRepositoryMergeBatch {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        gate_evaluation_id: row.try_get("gate_evaluation_id")?,
        repository_set_revision: row.try_get("repository_set_revision")?,
        repository_set_digest: row.try_get("repository_set_digest")?,
        state: parse_enum(row.try_get::<String, _>("state")?)?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_batch_repo(row: SqliteRow) -> Result<TaskRepositoryMergeBatchRepoRow> {
    Ok(TaskRepositoryMergeBatchRepoRow {
        batch_id: row.try_get("batch_id")?,
        task_id: row.try_get("task_id")?,
        repository_set_revision: row.try_get("repository_set_revision")?,
        repo_id: row.try_get("repo_id")?,
        position: row.try_get("position")?,
        state: parse_enum(row.try_get::<String, _>("state")?)?,
        work_unit_repository_integration_id: row.try_get("work_unit_repository_integration_id")?,
        task_merge_operation_id: row.try_get("task_merge_operation_id")?,
        pr_metadata_id: row.try_get("pr_metadata_id")?,
        before_sha: row.try_get("before_sha")?,
        after_sha: row.try_get("after_sha")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn get_batch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<TaskRepositoryMergeBatch>> {
    sqlx::query("SELECT * FROM task_repository_merge_batch WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_batch)
        .transpose()
}

async fn list_batch_repositories_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch_id: &str,
) -> Result<Vec<TaskRepositoryMergeBatchRepoRow>> {
    sqlx::query(
        "SELECT * FROM task_repository_merge_batch_repo
         WHERE batch_id = ? ORDER BY position, repo_id",
    )
    .bind(batch_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(map_batch_repo)
    .collect()
}

#[async_trait]
impl TaskRepositoryMergeBatchRepo for SqliteDb {
    async fn create(
        &self,
        batch: &TaskRepositoryMergeBatch,
        repos: &[TaskRepositoryMergeBatchRepoRow],
    ) -> Result<TaskRepositoryMergeBatch> {
        if batch.version != 1
            || repos.iter().any(|repo| {
                repo.batch_id != batch.id
                    || repo.task_id != batch.task_id
                    || repo.repository_set_revision != batch.repository_set_revision
            })
        {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = get_batch_in_tx(&mut tx, &batch.id).await? {
            let existing_repos = list_batch_repositories_in_tx(&mut tx, &batch.id).await?;
            let mut requested_repos = repos.to_vec();
            requested_repos.sort_by(|left, right| {
                left.position
                    .cmp(&right.position)
                    .then_with(|| left.repo_id.cmp(&right.repo_id))
            });
            let same_batch_identity = existing.id == batch.id
                && existing.task_id == batch.task_id
                && existing.gate_evaluation_id == batch.gate_evaluation_id
                && existing.repository_set_revision == batch.repository_set_revision
                && existing.repository_set_digest.as_deref()
                    == batch.repository_set_digest.as_deref()
                && existing.created_at == batch.created_at;
            let same_repo_identities = existing_repos.len() == requested_repos.len()
                && existing_repos
                    .iter()
                    .zip(&requested_repos)
                    .all(|(existing, requested)| {
                        existing.batch_id == requested.batch_id
                            && existing.task_id == requested.task_id
                            && existing.repository_set_revision == requested.repository_set_revision
                            && existing.repo_id == requested.repo_id
                            && existing.position == requested.position
                            && existing.created_at == requested.created_at
                    });
            if !same_batch_identity || !same_repo_identities {
                return Err(DbError::IdempotencyConflict);
            }
            tx.commit().await?;
            return Ok(existing);
        }
        sqlx::query(
            "INSERT INTO task_repository_merge_batch (
                 id, task_id, gate_evaluation_id, repository_set_revision,
                 repository_set_digest, state, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&batch.id)
        .bind(&batch.task_id)
        .bind(&batch.gate_evaluation_id)
        .bind(batch.repository_set_revision)
        .bind(batch.repository_set_digest.as_deref())
        .bind(batch.state.to_string())
        .bind(batch.version)
        .bind(&batch.created_at)
        .bind(&batch.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        for repo in repos {
            sqlx::query(
                "INSERT INTO task_repository_merge_batch_repo (
                     batch_id, task_id, repository_set_revision, repo_id, position,
                     state, work_unit_repository_integration_id,
                     task_merge_operation_id, pr_metadata_id, before_sha, after_sha,
                     created_at, updated_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&repo.batch_id)
            .bind(&repo.task_id)
            .bind(repo.repository_set_revision)
            .bind(&repo.repo_id)
            .bind(repo.position)
            .bind(repo.state.to_string())
            .bind(repo.work_unit_repository_integration_id.as_deref())
            .bind(repo.task_merge_operation_id.as_deref())
            .bind(repo.pr_metadata_id.as_deref())
            .bind(repo.before_sha.as_deref())
            .bind(repo.after_sha.as_deref())
            .bind(&repo.created_at)
            .bind(&repo.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(check_error)?;
        }
        let result = get_batch_in_tx(&mut tx, &batch.id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(result)
    }

    async fn get_by_id(&self, batch_id: &str) -> Result<Option<TaskRepositoryMergeBatch>> {
        sqlx::query("SELECT * FROM task_repository_merge_batch WHERE id = ?")
            .bind(batch_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_batch)
            .transpose()
    }

    async fn list_by_task(&self, task_id: &str) -> Result<Vec<TaskRepositoryMergeBatch>> {
        sqlx::query(
            "SELECT * FROM task_repository_merge_batch
             WHERE task_id = ? ORDER BY created_at, id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_batch)
        .collect()
    }

    async fn list_repositories(
        &self,
        batch_id: &str,
    ) -> Result<Vec<TaskRepositoryMergeBatchRepoRow>> {
        sqlx::query(
            "SELECT * FROM task_repository_merge_batch_repo
             WHERE batch_id = ? ORDER BY position, repo_id",
        )
        .bind(batch_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_batch_repo)
        .collect()
    }

    async fn transition_batch(
        &self,
        batch_id: &str,
        expected_version: i64,
        state: TaskRepositoryMergeBatchState,
        updated_at: &str,
    ) -> Result<TaskRepositoryMergeBatch> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = get_batch_in_tx(&mut tx, batch_id)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        if current.state == state {
            tx.commit().await?;
            return Ok(current);
        }
        let result = sqlx::query(
            "UPDATE task_repository_merge_batch
             SET state = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(state.to_string())
        .bind(updated_at)
        .bind(batch_id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let updated = get_batch_in_tx(&mut tx, batch_id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(updated)
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_repo(
        &self,
        batch_id: &str,
        repo_id: &str,
        expected_version: i64,
        state: TaskRepositoryMergeBatchRepoState,
        work_unit_repository_integration_id: Option<&str>,
        task_merge_operation_id: Option<&str>,
        pr_metadata_id: Option<&str>,
        before_sha: Option<&str>,
        after_sha: Option<&str>,
    ) -> Result<TaskRepositoryMergeBatchRepoRow> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let Some(row) = sqlx::query(
            "SELECT * FROM task_repository_merge_batch_repo
             WHERE batch_id = ? AND repo_id = ?",
        )
        .bind(batch_id)
        .bind(repo_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Err(DbError::NotFound);
        };
        let current = map_batch_repo(row)?;
        let batch = get_batch_in_tx(&mut tx, batch_id)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.state == state
            && current.work_unit_repository_integration_id.as_deref()
                == work_unit_repository_integration_id
            && current.task_merge_operation_id.as_deref() == task_merge_operation_id
            && current.pr_metadata_id.as_deref() == pr_metadata_id
            && current.before_sha.as_deref() == before_sha
            && current.after_sha.as_deref() == after_sha
        {
            tx.commit().await?;
            return Ok(current);
        }
        if batch.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        let updated_at = crate::now_rfc3339();
        let result = sqlx::query(
            "UPDATE task_repository_merge_batch_repo
             SET state = ?, work_unit_repository_integration_id = ?,
                 task_merge_operation_id = ?, pr_metadata_id = ?, before_sha = ?,
                 after_sha = ?, updated_at = ?
             WHERE batch_id = ? AND repo_id = ?",
        )
        .bind(state.to_string())
        .bind(work_unit_repository_integration_id)
        .bind(task_merge_operation_id)
        .bind(pr_metadata_id)
        .bind(before_sha)
        .bind(after_sha)
        .bind(&updated_at)
        .bind(batch_id)
        .bind(repo_id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let bumped = sqlx::query(
            "UPDATE task_repository_merge_batch
             SET version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&updated_at)
        .bind(batch_id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if bumped.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let updated = sqlx::query(
            "SELECT * FROM task_repository_merge_batch_repo
             WHERE batch_id = ? AND repo_id = ?",
        )
        .bind(batch_id)
        .bind(repo_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        map_batch_repo(updated)
    }
}
