use std::{str::FromStr, sync::Arc};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use db::{
    MemoryAccessQuery, MemoryGetQuery, MemoryItem, MemoryKind, MemoryScopeGrant,
    ScopedMemoryRepository, SqliteDb,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{Result, ServiceError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCreator {
    pub creator_type: String,
    pub creator_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReferences {
    pub source_ref: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearchResult {
    pub id: Uuid,
    pub kind: MemoryKind,
    pub title: String,
    pub source_type: db::MemorySourceType,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub references: Option<MemoryReferences>,
    pub confidence: Option<db::MemoryConfidence>,
    pub created_at: Option<String>,
    pub creator: Option<MemoryCreator>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryAccessContext {
    pub identity_id: Option<String>,
    pub grants: Vec<MemoryScopeGrant>,
}

impl MemoryAccessContext {
    pub fn for_scope(
        identity_id: Option<String>,
        scope_type: impl Into<String>,
        scope_id: impl Into<String>,
        visibility: Vec<String>,
    ) -> Self {
        Self {
            identity_id: identity_id.clone(),
            grants: vec![MemoryScopeGrant {
                scope_type: scope_type.into(),
                scope_id: scope_id.into(),
                visibility,
                identity_id,
            }],
        }
    }
}

/// Read-only access to historical semantic-memory records for compatibility
/// views. It cannot index, publish, change lifecycle, or bind a cognition source.
#[derive(Clone)]
pub struct HistoricalMemoryReader<R = SqliteDb> {
    db: Arc<R>,
}

impl<R> HistoricalMemoryReader<R>
where
    R: ScopedMemoryRepository + Send + Sync,
{
    pub fn new(db: Arc<R>) -> Self {
        Self { db }
    }

    pub async fn search_scoped(
        &self,
        access: &MemoryAccessContext,
        query: String,
        layer: Option<u8>,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<(Vec<MemorySearchResult>, bool, Option<String>)> {
        let (items, has_more) = self
            .db
            .search_memory_items_scoped(MemoryAccessQuery {
                identity_id: access.identity_id.clone(),
                grants: access.grants.clone(),
                query,
                limit: i64::from(limit),
                cursor,
                include_retracted: false,
            })
            .await?;
        let next_cursor = if has_more {
            items
                .last()
                .map(scoped_memory_cursor_for_item)
                .transpose()?
        } else {
            None
        };
        let layer = resolve_layer(layer)?;
        let results = items
            .into_iter()
            .map(|item| shape_item(item, layer))
            .collect::<Result<Vec<_>>>()?;
        Ok((results, has_more, next_cursor))
    }

    pub async fn get_scoped(
        &self,
        access: &MemoryAccessContext,
        id: Uuid,
        layer: Option<u8>,
    ) -> Result<MemorySearchResult> {
        let item = self
            .db
            .get_memory_item_scoped(MemoryGetQuery {
                id: id.to_string(),
                identity_id: access.identity_id.clone(),
                grants: access.grants.clone(),
                include_retracted: false,
            })
            .await?
            .ok_or_else(|| ServiceError::not_found("memory_item", id.to_string()))?;
        shape_item(item, resolve_layer(layer)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryLayer {
    One,
    Two,
    Three,
}

fn resolve_layer(layer: Option<u8>) -> Result<MemoryLayer> {
    match layer {
        Some(1) => Ok(MemoryLayer::One),
        Some(2) => Ok(MemoryLayer::Two),
        Some(3) | None => Ok(MemoryLayer::Three),
        Some(other) => Err(ServiceError::invalid_operation(format!(
            "invalid memory layer {other}; expected 1, 2, or 3"
        ))),
    }
}

fn shape_item(item: MemoryItem, layer: MemoryLayer) -> Result<MemorySearchResult> {
    let id = Uuid::parse_str(&item.id).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid memory item id: {error}"))
    })?;
    let kind = MemoryKind::from_str(&item.kind).map_err(ServiceError::invalid_operation)?;
    let source_type = db::MemorySourceType::from_str(&item.source_type)
        .map_err(ServiceError::invalid_operation)?;
    let references = MemoryReferences {
        source_ref: source_ref_from_metadata(&item.metadata_json)
            .unwrap_or_else(|| item.id.clone()),
        project_id: item.project_id.clone(),
        task_id: item.task_id.clone(),
        execution_id: item.execution_id.clone(),
    };
    let confidence = item
        .confidence
        .as_deref()
        .map(db::MemoryConfidence::from_str)
        .transpose()
        .map_err(ServiceError::invalid_operation)?;
    let creator = item
        .created_by_type
        .clone()
        .map(|creator_type| MemoryCreator {
            creator_type,
            creator_id: item.created_by_id.clone(),
        });
    let metadata = serde_json::from_str::<Value>(&item.metadata_json).ok();

    Ok(match layer {
        MemoryLayer::One => MemorySearchResult {
            id,
            kind,
            title: item.title,
            source_type,
            summary: None,
            body: None,
            references: None,
            confidence: None,
            created_at: None,
            creator: None,
            metadata: None,
        },
        MemoryLayer::Two => MemorySearchResult {
            id,
            kind,
            title: item.title,
            source_type,
            summary: item.summary,
            body: None,
            references: Some(references),
            confidence,
            created_at: Some(item.created_at),
            creator,
            metadata: None,
        },
        MemoryLayer::Three => MemorySearchResult {
            id,
            kind,
            title: item.title,
            source_type,
            summary: item.summary,
            body: Some(item.body),
            references: Some(references),
            confidence,
            created_at: Some(item.created_at),
            creator,
            metadata,
        },
    })
}

fn scoped_memory_cursor_for_item(item: &MemoryItem) -> Result<String> {
    let rank = match item.authority.as_str() {
        "decision" => 600,
        "procedure" => 500,
        "verified_fact" => 450,
        "proposal" => 300,
        "hypothesis" => 200,
        _ => 100,
    } + item.retention_priority;
    let bytes = serde_json::to_vec(&json!({
        "rank": rank,
        "created_at": item.created_at,
        "id": item.id,
    }))
    .map_err(|error| ServiceError::invalid_operation(format!("invalid memory cursor: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn source_ref_from_metadata(metadata_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(metadata_json)
        .ok()
        .and_then(|value| {
            value
                .get("source_ref")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}
