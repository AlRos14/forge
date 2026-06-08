use super::*;
use crate::{MemoryItem, MemoryRepository};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryCursor {
    created_at: String,
    id: String,
}

fn decode_memory_cursor(cursor: Option<String>) -> Result<Option<MemoryCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| DbError::InvalidCursor)?;
    let cursor: MemoryCursor =
        serde_json::from_slice(&bytes).map_err(|_| DbError::InvalidCursor)?;
    if cursor.created_at.is_empty() || cursor.id.is_empty() {
        return Err(DbError::InvalidCursor);
    }
    Ok(Some(cursor))
}

fn map_memory_rows(rows: Vec<SqliteRow>) -> Result<Vec<MemoryItem>> {
    rows.into_iter()
        .map(|row| MemoryItem::from_row(&row).map_err(DbError::from))
        .collect()
}

#[async_trait]
impl MemoryRepository for SqliteDb {
    async fn insert_memory_item(&self, item: &MemoryItem) -> Result<()> {
        sqlx::query("INSERT INTO memory_item (id, project_id, task_id, execution_id, conversation_id, source_type, kind, title, summary, body, metadata_json, confidence, quality_score, created_by_type, created_by_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&item.id)
            .bind(&item.project_id)
            .bind(item.task_id.as_deref())
            .bind(item.execution_id.as_deref())
            .bind(item.conversation_id.as_deref())
            .bind(&item.source_type)
            .bind(&item.kind)
            .bind(&item.title)
            .bind(item.summary.as_deref())
            .bind(&item.body)
            .bind(&item.metadata_json)
            .bind(item.confidence.as_deref())
            .bind(item.quality_score)
            .bind(item.created_by_type.as_deref())
            .bind(item.created_by_id.as_deref())
            .bind(&item.created_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_memory_item(&self, id: &str) -> Result<Option<MemoryItem>> {
        sqlx::query("SELECT * FROM memory_item WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(|row| MemoryItem::from_row(&row).map_err(DbError::from))
            .transpose()
    }

    async fn search_memory_items(
        &self,
        project_id: &str,
        query: &str,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<(Vec<MemoryItem>, bool)> {
        let limit = limit.clamp(1, 500);
        let cursor = decode_memory_cursor(cursor)?;
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT memory_item.* FROM memory_item JOIN memory_item_fts ON memory_item_fts.rowid = memory_item.row_id WHERE memory_item.project_id = ? AND memory_item_fts MATCH ? AND (memory_item.created_at < ? OR (memory_item.created_at = ? AND memory_item.id < ?)) ORDER BY rank, memory_item.created_at DESC, memory_item.id DESC LIMIT ?",
            )
            .bind(project_id)
            .bind(query)
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.id)
            .bind(limit as i64 + 1)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT memory_item.* FROM memory_item JOIN memory_item_fts ON memory_item_fts.rowid = memory_item.row_id WHERE memory_item.project_id = ? AND memory_item_fts MATCH ? ORDER BY rank, memory_item.created_at DESC, memory_item.id DESC LIMIT ?",
            )
            .bind(project_id)
            .bind(query)
            .bind(limit as i64 + 1)
            .fetch_all(&self.pool)
            .await?
        };
        let mut items = map_memory_rows(rows)?;
        let has_more = items.len() > limit;
        if has_more {
            items.truncate(limit);
        }
        Ok((items, has_more))
    }

    async fn list_memory_items_by_source(
        &self,
        project_id: &str,
        source_type: &str,
        source_id: &str,
    ) -> Result<Vec<MemoryItem>> {
        let rows = sqlx::query("SELECT * FROM memory_item WHERE project_id = ? AND source_type = ? AND (task_id = ? OR execution_id = ? OR conversation_id = ?) ORDER BY created_at DESC, id DESC")
            .bind(project_id)
            .bind(source_type)
            .bind(source_id)
            .bind(source_id)
            .bind(source_id)
            .fetch_all(&self.pool)
            .await?;
        map_memory_rows(rows)
    }
}
