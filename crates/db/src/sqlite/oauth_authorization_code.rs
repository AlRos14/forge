use super::*;
use crate::{CreateOAuthAuthorizationCode, OAuthAuthorizationCode, OAuthAuthorizationCodeRepo};

#[async_trait]
impl OAuthAuthorizationCodeRepo for SqliteDb {
    async fn create_code(
        &self,
        input: CreateOAuthAuthorizationCode,
    ) -> Result<OAuthAuthorizationCode> {
        sqlx::query(
            "INSERT INTO oauth_authorization_code (id, code_hash, user_id, client_id, redirect_uri, code_challenge, code_challenge_method, resource, scopes, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.code_hash)
        .bind(&input.user_id)
        .bind(&input.client_id)
        .bind(&input.redirect_uri)
        .bind(&input.code_challenge)
        .bind(&input.code_challenge_method)
        .bind(&input.resource)
        .bind(&input.scopes)
        .bind(&input.expires_at)
        .bind(&input.created_at)
        .execute(&self.pool)
        .await?;

        sqlx::query("SELECT * FROM oauth_authorization_code WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&self.pool)
            .await
            .map(map_oauth_authorization_code)?
    }

    async fn get_code_by_hash(&self, code_hash: &str) -> Result<Option<OAuthAuthorizationCode>> {
        sqlx::query("SELECT * FROM oauth_authorization_code WHERE code_hash = ?")
            .bind(code_hash)
            .fetch_optional(&self.pool)
            .await?
            .map(map_oauth_authorization_code)
            .transpose()
    }

    async fn mark_code_consumed(&self, id: &str, consumed_at: &str) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE oauth_authorization_code SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(consumed_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_expired_codes(&self, now_rfc3339: &str) -> Result<u64> {
        let result = sqlx::query("DELETE FROM oauth_authorization_code WHERE expires_at < ?")
            .bind(now_rfc3339)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

fn map_oauth_authorization_code(row: SqliteRow) -> Result<OAuthAuthorizationCode> {
    Ok(OAuthAuthorizationCode {
        id: row.get("id"),
        code_hash: row.get("code_hash"),
        user_id: row.get("user_id"),
        client_id: row.get("client_id"),
        redirect_uri: row.get("redirect_uri"),
        code_challenge: row.get("code_challenge"),
        code_challenge_method: row.get("code_challenge_method"),
        resource: row.get("resource"),
        scopes: row.get("scopes"),
        expires_at: row.get("expires_at"),
        consumed_at: row.get("consumed_at"),
        created_at: row.get("created_at"),
    })
}
