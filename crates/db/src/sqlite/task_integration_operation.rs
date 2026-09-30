use super::*;

fn map_operation(row: SqliteRow) -> Result<TaskIntegrationOperation> {
    Ok(TaskIntegrationOperation {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        kind: parse_enum(row.try_get::<String, _>("kind")?)?,
        owner_id: row.try_get("owner_id")?,
        status: parse_enum(row.try_get::<String, _>("status")?)?,
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

async fn insert_operation(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreateTaskIntegrationOperation,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO task_integration_operation (
            id, task_id, kind, owner_id, status, version, created_at, updated_at, finished_at
         ) VALUES (?, ?, ?, ?, 'running', 1, ?, ?, NULL)",
    )
    .bind(&input.id)
    .bind(&input.task_id)
    .bind(input.kind.to_string())
    .bind(&input.owner_id)
    .bind(&input.created_at)
    .bind(&input.created_at)
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
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    updated_at: &str,
) -> Result<Option<TaskIntegrationOperation>> {
    let Some(active) = sqlx::query(
        "SELECT * FROM task_integration_operation
         WHERE task_id = ? AND status = 'running'",
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
    load_operation(tx, &active.id).await.map(Some)
}

#[async_trait]
impl TaskIntegrationOperationRepo for SqliteDb {
    async fn begin(
        &self,
        input: CreateTaskIntegrationOperation,
    ) -> Result<TaskIntegrationOperation> {
        let mut tx = self.pool.begin().await?;
        insert_operation(&mut tx, &input).await?;
        let operation = load_operation(&mut tx, &input.id).await?;
        tx.commit().await?;
        Ok(operation)
    }

    async fn get_active_for_task(&self, task_id: &str) -> Result<Option<TaskIntegrationOperation>> {
        sqlx::query(
            "SELECT * FROM task_integration_operation
             WHERE task_id = ? AND status = 'running'",
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
        let abandoned = abandon_running_in_tx(&mut tx, task_id, updated_at).await?;
        tx.commit().await?;
        Ok(abandoned)
    }

    async fn recover_stale_and_begin(
        &self,
        input: CreateTaskIntegrationOperation,
        updated_at: &str,
    ) -> Result<TaskIntegrationOperation> {
        let mut tx = self.pool.begin().await?;
        abandon_running_in_tx(&mut tx, &input.task_id, updated_at).await?;
        insert_operation(&mut tx, &input).await?;
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
        let updated = sqlx::query(
            "UPDATE task_integration_operation
             SET status = ?, version = version + 1, updated_at = ?, finished_at = ?
             WHERE id = ? AND version = ? AND status = 'running'",
        )
        .bind(input.status.to_string())
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
        tx.commit().await?;
        Ok(operation)
    }
}
