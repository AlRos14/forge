use std::sync::Arc;

use db::{ContextManifest, ContextManifestSource, ScopedMemoryRepository, SqliteDb};

use crate::{Result, ServiceError};

/// Read-only access to cognitive context manifests retained from legacy runs.
#[derive(Clone)]
pub struct HistoricalContextManifestReader<R = SqliteDb> {
    db: Arc<R>,
}

impl<R> HistoricalContextManifestReader<R>
where
    R: ScopedMemoryRepository + Send + Sync,
{
    pub fn new(db: Arc<R>) -> Self {
        Self { db }
    }

    pub async fn get_authorized(
        &self,
        id: uuid::Uuid,
        identity_id: uuid::Uuid,
        context_scope_id: uuid::Uuid,
    ) -> Result<Option<ContextManifest>> {
        self.db
            .get_context_manifest_scoped(
                &id.to_string(),
                &identity_id.to_string(),
                &context_scope_id.to_string(),
            )
            .await
            .map_err(Into::into)
    }

    pub async fn list_authorized(
        &self,
        identity_id: uuid::Uuid,
        context_scope_id: Option<uuid::Uuid>,
        limit: u32,
    ) -> Result<Vec<ContextManifest>> {
        let context_scope_id = context_scope_id.map(|id| id.to_string());
        self.db
            .list_context_manifests_scoped(
                &identity_id.to_string(),
                context_scope_id.as_deref(),
                i64::from(limit.clamp(1, 100)),
            )
            .await
            .map_err(Into::into)
    }

    pub async fn sources(
        &self,
        id: uuid::Uuid,
        identity_id: uuid::Uuid,
        context_scope_id: uuid::Uuid,
    ) -> Result<Vec<ContextManifestSource>> {
        // Scope the parent before listing source metadata so ids and ordering
        // cannot be used as a cross-scope existence oracle.
        self.get_authorized(id, identity_id, context_scope_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("context_manifest", id.to_string()))?;
        self.db
            .list_context_manifest_sources(&id.to_string())
            .await
            .map_err(Into::into)
    }
}
