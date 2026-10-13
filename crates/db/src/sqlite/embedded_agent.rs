use crate::{CredentialHandle, CredentialHandleRepo, CredentialUsage, DbError, Result, SqliteDb};
use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, Row};

#[async_trait]
impl CredentialHandleRepo for SqliteDb {
    async fn get_credential_handle(&self, id: &str) -> Result<Option<CredentialHandle>> {
        sqlx::query("SELECT * FROM credential_handle WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_credential_handle)
            .transpose()
    }

    async fn list_credential_handles(&self, owner_user_id: &str) -> Result<Vec<CredentialHandle>> {
        sqlx::query(
            "SELECT * FROM credential_handle
             WHERE owner_user_id = ?
             ORDER BY created_at DESC, id DESC",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_credential_handle)
        .collect()
    }

    async fn rename_credential_handle(
        &self,
        id: &str,
        owner_user_id: &str,
        label: &str,
        expected_version: i64,
        updated_at: &str,
    ) -> Result<CredentialHandle> {
        let result = sqlx::query(
            "UPDATE credential_handle
             SET label = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?",
        )
        .bind(label)
        .bind(updated_at)
        .bind(id)
        .bind(owner_user_id)
        .bind(expected_version)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            let exists =
                sqlx::query("SELECT id FROM credential_handle WHERE id = ? AND owner_user_id = ?")
                    .bind(id)
                    .bind(owner_user_id)
                    .fetch_optional(&self.pool)
                    .await?
                    .is_some();
            return Err(if exists {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        self.get_credential_handle(id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn list_credential_usage(&self, owner_user_id: &str) -> Result<Vec<CredentialUsage>> {
        let rows = sqlx::query(
            "SELECT ap.credential_ref AS credential_id,
                    ai.id AS agent_id,
                    ai.name AS agent_name,
                    ap.executor_type AS runtime,
                    (SELECT MAX(s.last_activity_at)
                       FROM agent_session_history s
                      WHERE s.identity_id = ai.id) AS last_used_at
             FROM agent_identity ai
             JOIN agent_profile ap ON ap.id = ai.selected_profile_id
             WHERE ap.credential_ref IS NOT NULL
               AND ai.owner_id = ?
             ORDER BY ai.name ASC, ai.id ASC",
        )
        .bind(owner_user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(CredentialUsage {
                    credential_id: row.try_get("credential_id")?,
                    agent_id: row.try_get("agent_id")?,
                    agent_name: row.try_get("agent_name")?,
                    runtime: row.try_get("runtime")?,
                    last_used_at: row.try_get("last_used_at")?,
                })
            })
            .collect()
    }

    async fn revoke_credential_handle(
        &self,
        id: &str,
        owner_user_id: &str,
        updated_at: &str,
    ) -> Result<CredentialHandle> {
        let result = sqlx::query(
            "UPDATE credential_handle
             SET status = 'revoked', version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(updated_at)
        .bind(id)
        .bind(owner_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        self.get_credential_handle(id)
            .await?
            .ok_or(DbError::NotFound)
    }
}

fn map_credential_handle(row: SqliteRow) -> Result<CredentialHandle> {
    Ok(CredentialHandle {
        id: row.try_get("id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        provider: row.try_get("provider")?,
        label: row.try_get("label")?,
        status: row.try_get("status")?,
        credential_method: row.try_get("credential_method")?,
        metadata_json: row.try_get("metadata_json")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
