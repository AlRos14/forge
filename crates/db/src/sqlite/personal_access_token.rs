use super::*;
use crate::{CreatePersonalAccessToken, PersonalAccessToken, PersonalAccessTokenRepo};

#[async_trait]
impl PersonalAccessTokenRepo for SqliteDb {
    async fn create_pat(&self, input: CreatePersonalAccessToken) -> Result<PersonalAccessToken> {
        sqlx::query(
            "INSERT INTO personal_access_token (id, user_id, name, token_hash, prefix, scopes, expires_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.user_id)
        .bind(&input.name)
        .bind(&input.token_hash)
        .bind(&input.prefix)
        .bind(&input.scopes)
        .bind(input.expires_at.as_deref())
        .bind(&input.created_at)
        .execute(&self.pool)
        .await?;

        Ok(PersonalAccessToken {
            id: input.id,
            user_id: input.user_id,
            name: input.name,
            token_hash: input.token_hash,
            prefix: input.prefix,
            scopes: input.scopes,
            expires_at: input.expires_at,
            last_used_at: None,
            created_at: input.created_at,
        })
    }

    async fn get_pat_by_token_hash(&self, token_hash: &str) -> Result<Option<PersonalAccessToken>> {
        sqlx::query("SELECT * FROM personal_access_token WHERE token_hash = ?")
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?
            .map(map_pat)
            .transpose()
    }

    async fn list_pats_by_user(&self, user_id: &str) -> Result<Vec<PersonalAccessToken>> {
        let rows = sqlx::query(
            "SELECT * FROM personal_access_token WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_pat).collect()
    }

    async fn delete_pat(&self, id: &str, user_id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM personal_access_token WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn update_last_used(&self, id: &str, last_used_at: &str) -> Result<()> {
        sqlx::query("UPDATE personal_access_token SET last_used_at = ? WHERE id = ?")
            .bind(last_used_at)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn map_pat(row: SqliteRow) -> Result<PersonalAccessToken> {
    Ok(PersonalAccessToken {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        name: row.try_get("name")?,
        token_hash: row.try_get("token_hash")?,
        prefix: row.try_get("prefix")?,
        scopes: row.try_get("scopes")?,
        expires_at: row.try_get("expires_at")?,
        last_used_at: row.try_get("last_used_at")?,
        created_at: row.try_get("created_at")?,
    })
}
