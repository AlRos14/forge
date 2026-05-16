use super::*;

#[async_trait]
impl ExecutionUsageRepo for SqliteDb {
    async fn upsert(&self, input: UpsertExecutionUsage) -> Result<ExecutionUsage> {
        let id = crate::new_uuid_v4();
        let now = crate::now_rfc3339();
        sqlx::query(
            "INSERT INTO execution_usage (id, execution_id, provider, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, cost_usd, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(execution_id, provider, model) DO UPDATE SET
                input_tokens = input_tokens + excluded.input_tokens,
                output_tokens = output_tokens + excluded.output_tokens,
                cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
                cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
                cost_usd = CASE
                    WHEN excluded.cost_usd IS NOT NULL THEN COALESCE(cost_usd, 0) + excluded.cost_usd
                    ELSE cost_usd
                END",
        )
        .bind(&id)
        .bind(&input.execution_id)
        .bind(&input.provider)
        .bind(&input.model)
        .bind(input.input_tokens)
        .bind(input.output_tokens)
        .bind(input.cache_read_tokens)
        .bind(input.cache_write_tokens)
        .bind(input.cost_usd)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        let row = sqlx::query(
            "SELECT * FROM execution_usage WHERE execution_id = ? AND provider = ? AND model = ?",
        )
        .bind(&input.execution_id)
        .bind(&input.provider)
        .bind(&input.model)
        .fetch_one(&self.pool)
        .await?;
        map_execution_usage(row)
    }

    async fn list_by_execution(&self, execution_id: &str) -> Result<Vec<ExecutionUsage>> {
        let rows = sqlx::query(
            "SELECT * FROM execution_usage WHERE execution_id = ? ORDER BY created_at ASC",
        )
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_execution_usage).collect()
    }

    async fn get_task_usage_summary(&self, task_id: &str) -> Result<TaskUsageSummary> {
        let row = sqlx::query(
            "SELECT
                COALESCE(SUM(eu.input_tokens), 0) as total_input_tokens,
                COALESCE(SUM(eu.output_tokens), 0) as total_output_tokens,
                COALESCE(SUM(eu.cache_read_tokens), 0) as total_cache_read_tokens,
                COALESCE(SUM(eu.cache_write_tokens), 0) as total_cache_write_tokens,
                SUM(eu.cost_usd) as total_cost_usd,
                COUNT(DISTINCT eu.execution_id) as execution_count
             FROM execution_usage eu
             JOIN execution e ON e.id = eu.execution_id
             WHERE e.task_id = ?",
        )
        .bind(task_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(TaskUsageSummary {
            total_input_tokens: row.try_get("total_input_tokens")?,
            total_output_tokens: row.try_get("total_output_tokens")?,
            total_cache_read_tokens: row.try_get("total_cache_read_tokens")?,
            total_cache_write_tokens: row.try_get("total_cache_write_tokens")?,
            total_cost_usd: row.try_get("total_cost_usd")?,
            execution_count: row.try_get("execution_count")?,
        })
    }
}
