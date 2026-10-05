use super::*;

pub(super) async fn insert_pr_metadata_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    input: &CreatePrMetadata,
) -> Result<()> {
    sqlx::query("INSERT INTO pr_metadata (id, task_id, provider_type, provider_pr_id, pr_url, source_branch, target_branch, pr_state, merge_status, task_merge_operation_id, publish_operation_id, last_synced_at, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.provider_type)
        .bind(&input.provider_pr_id)
        .bind(&input.pr_url)
        .bind(&input.source_branch)
        .bind(&input.target_branch)
        .bind(&input.pr_state)
        .bind(&input.merge_status)
        .bind(&input.task_merge_operation_id)
        .bind(&input.publish_operation_id)
        .bind(&input.last_synced_at)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&mut **tx)
        .await
        .map_err(check_error)?;
    Ok(())
}

#[async_trait]
impl PrMetadataRepo for SqliteDb {
    async fn create(&self, input: CreatePrMetadata) -> Result<PrMetadata> {
        let mut tx = self.pool.begin().await?;
        insert_pr_metadata_in_tx(&mut tx, &input).await?;
        tx.commit().await?;
        PrMetadataRepo::get_by_task_id(self, &input.task_id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<PrMetadata>> {
        sqlx::query("SELECT * FROM pr_metadata WHERE task_id = ?")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_metadata)
            .transpose()
    }

    async fn update(&self, input: UpdatePrMetadata) -> Result<PrMetadata> {
        let mut metadata = sqlx::query("SELECT * FROM pr_metadata WHERE id = ?")
            .bind(&input.id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_metadata)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if let Some(provider_type) = input.provider_type {
            metadata.provider_type = provider_type;
        }
        if let Some(provider_pr_id) = input.provider_pr_id {
            metadata.provider_pr_id = provider_pr_id;
        }
        if let Some(pr_url) = input.pr_url {
            metadata.pr_url = pr_url;
        }
        if let Some(source_branch) = input.source_branch {
            metadata.source_branch = source_branch;
        }
        if let Some(target_branch) = input.target_branch {
            metadata.target_branch = target_branch;
        }
        if let Some(pr_state) = input.pr_state {
            metadata.pr_state = pr_state;
        }
        if let Some(merge_status) = input.merge_status {
            metadata.merge_status = merge_status;
        }
        if let Some(task_merge_operation_id) = input.task_merge_operation_id {
            metadata.task_merge_operation_id = Some(task_merge_operation_id);
        }
        if let Some(publish_operation_id) = input.publish_operation_id {
            metadata.publish_operation_id = Some(publish_operation_id);
        }
        if let Some(last_synced_at) = input.last_synced_at {
            metadata.last_synced_at = last_synced_at;
        }
        metadata.updated_at = input.updated_at;
        sqlx::query("UPDATE pr_metadata SET provider_type = ?, provider_pr_id = ?, pr_url = ?, source_branch = ?, target_branch = ?, pr_state = ?, merge_status = ?, task_merge_operation_id = ?, publish_operation_id = ?, last_synced_at = ?, updated_at = ? WHERE id = ?")
            .bind(&metadata.provider_type)
            .bind(metadata.provider_pr_id.as_deref())
            .bind(metadata.pr_url.as_deref())
            .bind(&metadata.source_branch)
            .bind(&metadata.target_branch)
            .bind(&metadata.pr_state)
            .bind(&metadata.merge_status)
            .bind(metadata.task_merge_operation_id.as_deref())
            .bind(metadata.publish_operation_id.as_deref())
            .bind(metadata.last_synced_at.as_deref())
            .bind(&metadata.updated_at)
            .bind(&metadata.id)
            .execute(&self.pool)
            .await
            .map_err(check_error)?;
        Ok(metadata)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM pr_metadata WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
