//! Read-only historical access to Product Genesis records retained until PR12.

use std::sync::Arc;

use api_types::{ProductGenesisLifecycle, ProductGenesisSession, ProductMaturity};
use db::{DbError, SqliteDb};
use sqlx::Row;

use crate::{Result, ServiceError};

/// Read-only access to historical Product Genesis sessions.
#[derive(Clone)]
pub struct HistoricalProductGenesisReader {
    db: Arc<SqliteDb>,
}

impl HistoricalProductGenesisReader {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    pub async fn get(&self, id: &str) -> Result<ProductGenesisSession> {
        let row = sqlx::query(SESSION_SELECT_SQL)
            .bind(id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(db_error)?
            .ok_or_else(|| ServiceError::not_found("product_genesis_session", id.to_owned()))?;
        map_genesis_row(row)
    }

    pub async fn active(&self, account_id: &str) -> Result<Option<ProductGenesisSession>> {
        let row = sqlx::query(&format!(
            "{SESSION_SELECT_SQL} WHERE account_id = ? \
                 AND lifecycle IN ('discovering', 'ready_for_project') \
                 ORDER BY created_at DESC, id DESC LIMIT 1"
        ))
        .bind(account_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(map_genesis_row).transpose()
    }
}

const SESSION_SELECT_SQL: &str = "SELECT id, account_id, main_chat_id, prompt_revision, maturity,
    initial_idea, lifecycle, source_message_ids_json,
    preferred_project_agent_identity_id, charter_id, charter_revision_id,
    charter_approval_id, charter_version, project_id, handoff_id,
    failure_reason, version, created_at, updated_at FROM product_genesis_session";

fn db_error(error: sqlx::Error) -> ServiceError {
    ServiceError::Db(DbError::from(error))
}

fn map_genesis_row(row: sqlx::sqlite::SqliteRow) -> Result<ProductGenesisSession> {
    let maturity = match row
        .try_get::<String, _>("maturity")
        .map_err(db_error)?
        .as_str()
    {
        "prototype" => ProductMaturity::Prototype,
        "mvp" => ProductMaturity::Mvp,
        "production" => ProductMaturity::Production,
        "critical" => ProductMaturity::Critical,
        value => {
            return Err(ServiceError::InvalidOperation {
                message: format!("invalid persisted Genesis maturity `{value}`"),
            });
        }
    };
    let lifecycle = match row
        .try_get::<String, _>("lifecycle")
        .map_err(db_error)?
        .as_str()
    {
        "discovering" => ProductGenesisLifecycle::Discovering,
        "ready_for_project" => ProductGenesisLifecycle::ReadyForProject,
        "handed_off" => ProductGenesisLifecycle::HandedOff,
        "cancelled" => ProductGenesisLifecycle::Cancelled,
        value => {
            return Err(ServiceError::InvalidOperation {
                message: format!("invalid persisted Genesis lifecycle `{value}`"),
            });
        }
    };
    let source_json = row
        .try_get::<String, _>("source_message_ids_json")
        .map_err(db_error)?;
    let source_message_ids =
        serde_json::from_str(&source_json).map_err(|error| ServiceError::InvalidOperation {
            message: format!("invalid persisted Genesis source references: {error}"),
        })?;
    Ok(ProductGenesisSession {
        id: row.try_get("id").map_err(db_error)?,
        account_id: row.try_get("account_id").map_err(db_error)?,
        main_chat_id: row.try_get("main_chat_id").map_err(db_error)?,
        prompt_revision: row.try_get("prompt_revision").map_err(db_error)?,
        maturity,
        initial_idea: row.try_get("initial_idea").map_err(db_error)?,
        lifecycle,
        source_message_ids,
        preferred_project_agent_identity_id: row
            .try_get("preferred_project_agent_identity_id")
            .map_err(db_error)?,
        charter_id: row.try_get("charter_id").map_err(db_error)?,
        charter_revision_id: row.try_get("charter_revision_id").map_err(db_error)?,
        charter_approval_id: row.try_get("charter_approval_id").map_err(db_error)?,
        charter_version: row.try_get("charter_version").map_err(db_error)?,
        project_id: row.try_get("project_id").map_err(db_error)?,
        handoff_id: row.try_get("handoff_id").map_err(db_error)?,
        failure_reason: row.try_get("failure_reason").map_err(db_error)?,
        version: row.try_get("version").map_err(db_error)?,
        created_at: row.try_get("created_at").map_err(db_error)?,
        updated_at: row.try_get("updated_at").map_err(db_error)?,
    })
}
