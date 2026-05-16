use super::*;
use crate::{CreateOAuthClient, OAuthClient, OAuthClientRepo};

#[async_trait]
impl OAuthClientRepo for SqliteDb {
    async fn create_client(&self, input: CreateOAuthClient) -> Result<OAuthClient> {
        sqlx::query(
            "INSERT INTO oauth_client (id, client_id, client_name, redirect_uris_json, token_endpoint_auth_method, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.client_id)
        .bind(input.client_name.as_deref())
        .bind(&input.redirect_uris_json)
        .bind(&input.token_endpoint_auth_method)
        .bind(&input.created_at)
        .execute(&self.pool)
        .await?;

        sqlx::query("SELECT * FROM oauth_client WHERE client_id = ?")
            .bind(&input.client_id)
            .fetch_one(&self.pool)
            .await
            .map(map_oauth_client)?
    }

    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>> {
        sqlx::query("SELECT * FROM oauth_client WHERE client_id = ?")
            .bind(client_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_oauth_client)
            .transpose()
    }

    async fn touch_last_used(&self, client_id: &str, last_used_at: &str) -> Result<()> {
        sqlx::query("UPDATE oauth_client SET last_used_at = ? WHERE client_id = ?")
            .bind(last_used_at)
            .bind(client_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn count_clients_created_since(&self, created_after_rfc3339: &str) -> Result<i64> {
        Ok(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM oauth_client WHERE created_at >= ?")
                .bind(created_after_rfc3339)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

fn map_oauth_client(row: SqliteRow) -> Result<OAuthClient> {
    Ok(OAuthClient {
        id: row.get("id"),
        client_id: row.get("client_id"),
        client_name: row.get("client_name"),
        redirect_uris_json: row.get("redirect_uris_json"),
        token_endpoint_auth_method: row.get("token_endpoint_auth_method"),
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
    })
}
