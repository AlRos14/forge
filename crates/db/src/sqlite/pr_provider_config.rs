use super::*;

#[async_trait]
impl PrProviderConfigRepo for SqliteDb {
    async fn create(&self, input: CreatePrProviderConfig) -> Result<PrProviderConfig> {
        sqlx::query("INSERT INTO pr_provider_config (id, repo_id, provider_type, base_url, polling_interval_seconds, token_secret_ref, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&input.id)
            .bind(&input.repo_id)
            .bind(&input.provider_type)
            .bind(&input.base_url)
            .bind(input.polling_interval_seconds)
            .bind(&input.token_secret_ref)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await
            .map_err(check_error)?;
        PrProviderConfigRepo::get_by_repo_id(self, &input.repo_id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_repo_id(&self, repo_id: &str) -> Result<Option<PrProviderConfig>> {
        sqlx::query("SELECT * FROM pr_provider_config WHERE repo_id = ?")
            .bind(repo_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_provider_config)
            .transpose()
    }

    async fn update(&self, input: UpdatePrProviderConfig) -> Result<PrProviderConfig> {
        let mut config = sqlx::query("SELECT * FROM pr_provider_config WHERE id = ?")
            .bind(&input.id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pr_provider_config)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if let Some(provider_type) = input.provider_type {
            config.provider_type = provider_type;
        }
        if let Some(base_url) = input.base_url {
            config.base_url = base_url;
        }
        if let Some(polling_interval_seconds) = input.polling_interval_seconds {
            config.polling_interval_seconds = polling_interval_seconds;
        }
        if let Some(token_secret_ref) = input.token_secret_ref {
            config.token_secret_ref = token_secret_ref;
        }
        config.updated_at = input.updated_at;
        sqlx::query("UPDATE pr_provider_config SET provider_type = ?, base_url = ?, polling_interval_seconds = ?, token_secret_ref = ?, updated_at = ? WHERE id = ?")
            .bind(&config.provider_type)
            .bind(config.base_url.as_deref())
            .bind(config.polling_interval_seconds)
            .bind(config.token_secret_ref.as_deref())
            .bind(&config.updated_at)
            .bind(&config.id)
            .execute(&self.pool)
            .await
            .map_err(check_error)?;
        Ok(config)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM pr_provider_config WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }
}
