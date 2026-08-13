use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use db::{
    now_rfc3339, CreateForgeMemorySourceBinding, MemoryAccessQuery, MemoryItem,
    ScopedMemoryRepository, SqliteDb,
};
use serde_json::Value;
use uuid::Uuid;

use crate::{MemoryAccessContext, Result, ServiceError};

/// The immutable admission identity used to construct a ForgeMemorySource.
/// The source never accepts a caller-supplied scope after construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySourceBindingInput {
    pub binding_id: Uuid,
    pub identity_id: Uuid,
    pub context_scope_id: Uuid,
    pub scope_type: String,
    pub scope_id: String,
    pub account_id: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub policy_revision: String,
    pub access: MemoryAccessContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeMemoryQuery {
    pub query: String,
    pub limit: u32,
    /// Stable source ids already represented by recent canonical Agent Chat
    /// history or an admitted LCM timeline. Legacy transcript ids are treated
    /// as migration provenance and are suppressed after ACL filtering.
    pub represented_source_ids: Vec<String>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeMemoryRecord {
    pub id: Uuid,
    pub revision: String,
    pub authority: String,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub sensitivity: String,
    pub retention_priority: i64,
    pub source_type: String,
    pub source_ref: Option<String>,
    pub provenance_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeMemorySearch {
    pub records: Vec<ForgeMemoryRecord>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub deduplicated_source_ids: Vec<String>,
}

#[derive(Clone)]
pub struct ForgeMemorySource<R = SqliteDb> {
    db: Arc<R>,
    binding_id: String,
    identity_id: String,
    context_scope_id: String,
    scope_type: String,
    scope_id: String,
    access: MemoryAccessContext,
    allow_restricted: bool,
    max_results: u32,
}

impl<R> ForgeMemorySource<R>
where
    R: ScopedMemoryRepository + Send + Sync + 'static,
{
    pub async fn bind(db: Arc<R>, input: MemorySourceBindingInput) -> Result<Self> {
        let identity_id = input.identity_id.to_string();
        let context_scope_id = input.context_scope_id.to_string();
        if input.access.identity_id.as_deref() != Some(identity_id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "memory source identity must match the admitted access identity",
            ));
        }
        if !matches!(
            input.scope_type.as_str(),
            "account" | "project" | "agent_chat" | "task"
        ) {
            return Err(ServiceError::invalid_operation(
                "memory source requires an admitted canonical scope",
            ));
        }
        let Some(canonical_grant) = input
            .access
            .grants
            .iter()
            .find(|grant| grant.scope_type == input.scope_type && grant.scope_id == input.scope_id)
            .cloned()
        else {
            return Err(ServiceError::invalid_operation(
                "memory source requires an admitted canonical-scope grant",
            ));
        };
        let create = db
            .create_memory_source_binding(CreateForgeMemorySourceBinding {
                id: input.binding_id.to_string(),
                identity_id: identity_id.clone(),
                context_scope_id: context_scope_id.clone(),
                scope_type: input.scope_type.clone(),
                scope_id: input.scope_id.clone(),
                account_id: input.account_id,
                project_id: input.project_id,
                task_id: input.task_id,
                policy_revision: input.policy_revision,
                created_at: now_rfc3339(),
            })
            .await;
        let binding = match create {
            Ok(binding) => binding,
            Err(db::DbError::Sqlx(_)) => db
                .get_memory_source_binding(&identity_id, &context_scope_id)
                .await?
                .ok_or(db::DbError::NotFound)?,
            Err(error) => return Err(error.into()),
        };
        if binding.identity_id != identity_id
            || binding.context_scope_id != context_scope_id
            || binding.scope_type != input.scope_type
            || binding.scope_id != input.scope_id
        {
            return Err(ServiceError::Conflict(
                "memory source binding is immutable and cannot be retargeted".to_owned(),
            ));
        }
        Ok(Self {
            db,
            binding_id: binding.id,
            identity_id: binding.identity_id,
            context_scope_id: binding.context_scope_id,
            scope_type: binding.scope_type,
            scope_id: binding.scope_id,
            // A source is permanently canonical-scope bound. Additional
            // grants in the admission context cannot widen it after bind.
            access: MemoryAccessContext {
                identity_id: input.access.identity_id,
                grants: vec![canonical_grant],
            },
            allow_restricted: false,
            max_results: 50,
        })
    }

    pub fn binding_id(&self) -> &str {
        &self.binding_id
    }

    pub fn identity_id(&self) -> &str {
        &self.identity_id
    }

    pub fn context_scope_id(&self) -> &str {
        &self.context_scope_id
    }

    pub fn scope_type(&self) -> &str {
        &self.scope_type
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn with_max_results(mut self, max_results: u32) -> Self {
        self.max_results = max_results.clamp(1, 500);
        self
    }

    /// Restricted records remain host-owned by default. Secret records are
    /// filtered in the repository before this method can observe them.
    pub fn allow_restricted(mut self, allow: bool) -> Self {
        self.allow_restricted = allow;
        self
    }

    pub async fn search(&self, query: ForgeMemoryQuery) -> Result<ForgeMemorySearch> {
        let requested = query.limit.min(self.max_results).max(1);
        let (items, has_more) = self
            .db
            .search_memory_items_scoped(MemoryAccessQuery {
                identity_id: self.access.identity_id.clone(),
                grants: self.access.grants.clone(),
                query: query.query,
                limit: i64::from(requested),
                cursor: query.cursor,
                include_retracted: false,
            })
            .await?;
        let raw_cursor = items.last().map(scoped_cursor_for_item).transpose()?;
        let mut deduplicated_source_ids = Vec::new();
        let mut records = Vec::with_capacity(items.len());
        for item in items {
            if query.represented_source_ids.iter().any(|source_id| {
                source_id == &item.id || source_id == &source_ref(&item).unwrap_or_default()
            }) {
                if let Some(source_id) = source_ref(&item) {
                    deduplicated_source_ids.push(source_id);
                } else {
                    deduplicated_source_ids.push(item.id.clone());
                }
                continue;
            }
            if item.sensitivity == "secret"
                || (!self.allow_restricted && item.sensitivity == "restricted")
            {
                continue;
            }
            records.push(record_from_item(item)?);
        }
        Ok(ForgeMemorySearch {
            records,
            has_more,
            next_cursor: if has_more { raw_cursor } else { None },
            deduplicated_source_ids,
        })
    }
}

fn record_from_item(item: MemoryItem) -> Result<ForgeMemoryRecord> {
    let source_ref = source_ref(&item);
    Ok(ForgeMemoryRecord {
        id: Uuid::parse_str(&item.id).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid memory id: {error}"))
        })?,
        revision: item
            .source_revision
            .clone()
            .unwrap_or_else(|| item.created_at.clone()),
        authority: item.authority,
        title: item.title,
        summary: item.summary,
        body: item.body,
        sensitivity: item.sensitivity,
        retention_priority: item.retention_priority,
        source_type: item.source_type,
        source_ref,
        provenance_json: item.provenance_json,
        created_at: item.created_at,
    })
}

fn scoped_cursor_for_item(item: &MemoryItem) -> Result<String> {
    let rank = match item.authority.as_str() {
        "decision" => 600,
        "procedure" => 500,
        "verified_fact" => 450,
        "proposal" => 300,
        "hypothesis" => 200,
        _ => 100,
    } + item.retention_priority;
    let value = serde_json::json!({
        "rank": rank,
        "created_at": item.created_at,
        "id": item.id,
    });
    let bytes = serde_json::to_vec(&value).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid memory cursor: {error}"))
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn source_ref(item: &MemoryItem) -> Option<String> {
    serde_json::from_str::<Value>(&item.metadata_json)
        .ok()
        .and_then(|value| {
            value
                .get("source_ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}
