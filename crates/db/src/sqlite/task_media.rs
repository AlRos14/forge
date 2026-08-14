use super::*;

#[async_trait]
impl TaskMediaRepo for SqliteDb {
    async fn create_media(&self, input: CreateTaskMedia) -> Result<TaskMedia> {
        sqlx::query(
            "INSERT INTO task_media (id, task_id, display_filename, content_type, byte_size, storage_key, author_type, author_id, author_name, created_at, deleted_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.display_filename)
        .bind(&input.content_type)
        .bind(input.byte_size)
        .bind(&input.storage_key)
        .bind(input.author_type.to_string())
        .bind(input.author_id.as_deref())
        .bind(&input.author_name)
        .bind(&input.created_at)
        .execute(&self.pool)
        .await?;

        Self::get_media_by_id(self, &input.id, true)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn list_media(&self, task_id: &str, page: PageRequest) -> Result<Page<TaskMedia>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM task_media WHERE task_id = ? AND deleted_at IS NULL ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(task_id)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_task_media)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM task_media WHERE task_id = ? AND deleted_at IS NULL",
                )
                .bind(task_id)
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn list_active_media_for_task(&self, task_id: &str) -> Result<Vec<TaskMedia>> {
        sqlx::query(
            "SELECT * FROM task_media WHERE task_id = ? AND deleted_at IS NULL ORDER BY created_at ASC, id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_task_media)
        .collect()
    }

    async fn get_media_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<TaskMedia>> {
        let sql = if include_deleted {
            "SELECT * FROM task_media WHERE id = ?"
        } else {
            "SELECT * FROM task_media WHERE id = ? AND deleted_at IS NULL"
        };

        sqlx::query(sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_task_media)
            .transpose()
    }

    async fn soft_delete_media(&self, id: &str, deleted_at: &str) -> Result<TaskMedia> {
        let result =
            sqlx::query("UPDATE task_media SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL")
                .bind(deleted_at)
                .bind(id)
                .execute(&self.pool)
                .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }

        // V076's legacy trigger marks the additive asset as a candidate, but
        // it cannot see Project evidence attachments.  Reconcile through the
        // shared-media repository so a Task delete never clears bytes still
        // referenced by evidence or an immutable release pin.
        if let Some(asset_id) =
            sqlx::query_scalar::<_, String>("SELECT asset_id FROM task_media WHERE id = ?")
                .bind(id)
                .fetch_optional(self.pool())
                .await?
        {
            SharedMediaRepo::reconcile_media_asset(self, &asset_id, deleted_at).await?;
        }

        Self::get_media_by_id(self, id, true)
            .await?
            .ok_or(DbError::NotFound)
    }
}
