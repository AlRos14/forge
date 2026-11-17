use super::*;
use crate::PrProviderCredentialRef;

fn next_config_updated_at(current: &str, proposed: &str) -> String {
    let (Ok(current_time), Ok(proposed_time)) = (
        chrono::DateTime::parse_from_rfc3339(current),
        chrono::DateTime::parse_from_rfc3339(proposed),
    ) else {
        return if current == proposed {
            chrono::Utc::now().to_rfc3339()
        } else {
            proposed.to_owned()
        };
    };
    if proposed_time > current_time {
        proposed.to_owned()
    } else {
        current_time
            .checked_add_signed(chrono::Duration::nanoseconds(1))
            .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
            .unwrap_or_else(|| proposed.to_owned())
    }
}

async fn load_config(pool: &SqlitePool, id: &str) -> Result<PrProviderConfig> {
    sqlx::query("SELECT * FROM pr_provider_config WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(map_pr_provider_config)
        .transpose()?
        .ok_or(DbError::NotFound)
}

#[async_trait]
impl PrProviderConfigRepo for SqliteDb {
    async fn create(&self, input: CreatePrProviderConfig) -> Result<PrProviderConfig> {
        // Without a credential reference keep the pre-V173 column list, so
        // historical-schema migration tests can still seed provider configs.
        let has_credential =
            input.credential_handle_id.is_some() || input.credential_owner_user_id.is_some();
        let sql = if has_credential {
            "INSERT INTO pr_provider_config (id, repo_id, provider_type, base_url, polling_interval_seconds, token_secret_ref, created_at, updated_at, credential_handle_id, credential_owner_user_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        } else {
            "INSERT INTO pr_provider_config (id, repo_id, provider_type, base_url, polling_interval_seconds, token_secret_ref, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        };
        let mut query = sqlx::query(sql)
            .bind(&input.id)
            .bind(&input.repo_id)
            .bind(&input.provider_type)
            .bind(&input.base_url)
            .bind(input.polling_interval_seconds)
            .bind(&input.token_secret_ref)
            .bind(&input.created_at)
            .bind(&input.updated_at);
        if has_credential {
            query = query
                .bind(&input.credential_handle_id)
                .bind(&input.credential_owner_user_id);
        }
        query.execute(&self.pool).await.map_err(check_error)?;
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

    async fn set_credential(
        &self,
        id: &str,
        credential: Option<PrProviderCredentialRef>,
        updated_at: &str,
    ) -> Result<PrProviderConfig> {
        let updated = sqlx::query(
            "UPDATE pr_provider_config
             SET credential_handle_id = ?, credential_owner_user_id = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(credential.as_ref().map(|value| value.handle_id.as_str()))
        .bind(
            credential
                .as_ref()
                .map(|value| value.owner_user_id.as_str()),
        )
        .bind(updated_at)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            return Err(DbError::NotFound);
        }
        load_config(&self.pool, id).await
    }

    async fn set_feedback_policy(
        &self,
        id: &str,
        expected_updated_at: Option<&str>,
        policy_json: &str,
        updated_at: &str,
    ) -> Result<PrProviderConfig> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current_updated_at = sqlx::query_scalar::<_, String>(
            "SELECT updated_at FROM pr_provider_config WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        if expected_updated_at.is_some_and(|expected| expected != current_updated_at) {
            return Err(DbError::VersionConflict);
        }
        let next_updated_at = next_config_updated_at(&current_updated_at, updated_at);
        let updated = sqlx::query(
            "UPDATE pr_provider_config
             SET feedback_policy_json = ?, updated_at = ?
             WHERE id = ? AND updated_at = ?",
        )
        .bind(policy_json)
        .bind(&next_updated_at)
        .bind(id)
        .bind(&current_updated_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pr_provider_config WHERE id = ?)")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
            return Err(if exists {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        let config = sqlx::query("SELECT * FROM pr_provider_config WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_pr_provider_config)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
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
