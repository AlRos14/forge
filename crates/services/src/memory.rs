use std::{str::FromStr, sync::Arc};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use db::{
    now_rfc3339, CommentAuthorType, Execution, ExecutionStatus, MemoryConfidence, MemoryItem,
    MemoryKind, MemoryRepository, MemorySourceType, Review, ReviewStatus, SqliteDb, TaskComment,
    TransitionLog,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
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
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearchResult {
    pub id: Uuid,
    pub kind: MemoryKind,
    pub title: String,
    pub source_type: MemorySourceType,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub references: Option<MemoryReferences>,
    pub confidence: Option<MemoryConfidence>,
    pub created_at: Option<String>,
    pub creator: Option<MemoryCreator>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryItemInput {
    pub project_id: Uuid,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub conversation_id: Option<String>,
    pub source_type: MemorySourceType,
    pub source_ref: String,
    pub kind: MemoryKind,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub confidence: Option<MemoryConfidence>,
    pub quality_score: Option<i64>,
    pub creator: Option<MemoryCreator>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillSummary {
    pub items: Vec<BackfillTypeResult>,
    pub indexed: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillTypeResult {
    pub source_type: MemorySourceType,
    pub indexed: u64,
    pub skipped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryBackfillSource {
    pub project_id: String,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub conversation_id: Option<String>,
    pub source_type: MemorySourceType,
    pub source_ref: String,
    pub kind: MemoryKind,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub confidence: Option<MemoryConfidence>,
    pub creator: Option<MemoryCreator>,
}

#[async_trait]
pub trait MemoryBackfillRepository: MemoryRepository {
    async fn list_memory_backfill_sources(&self) -> Result<Vec<MemoryBackfillSource>>;

    async fn memory_source_exists(
        &self,
        project_id: &str,
        source_type: &MemorySourceType,
        source_ref: &str,
    ) -> Result<bool>;
}

#[derive(Clone)]
pub struct MemoryService<R = SqliteDb> {
    db: Arc<R>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryLayer {
    One,
    Two,
    Three,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryCursor {
    created_at: String,
    id: String,
}

impl<R> MemoryService<R>
where
    R: MemoryRepository + Send + Sync,
{
    pub fn new(db: Arc<R>) -> Self {
        Self { db }
    }

    pub async fn search(
        &self,
        project_id: Uuid,
        query: String,
        layer: Option<u8>,
        token_budget: Option<u32>,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<(Vec<MemorySearchResult>, bool, Option<String>)> {
        let layer = resolve_layer(layer, token_budget)?;
        let (items, has_more) = self
            .db
            .search_memory_items(&project_id.to_string(), &query, limit as usize, cursor)
            .await?;
        let next_cursor = if has_more {
            items.last().map(memory_cursor_for_item).transpose()?
        } else {
            None
        };
        let results = items
            .into_iter()
            .map(|item| shape_item(item, layer))
            .collect::<Result<Vec<_>>>()?;
        Ok((results, has_more, next_cursor))
    }

    pub async fn get(&self, id: Uuid, layer: Option<u8>) -> Result<MemorySearchResult> {
        let layer = resolve_layer(layer, None)?;
        let item = self
            .db
            .get_memory_item(&id.to_string())
            .await?
            .ok_or_else(|| ServiceError::not_found("memory_item", id.to_string()))?;
        shape_item(item, layer)
    }

    pub async fn record_from_source(&self, input: MemoryItemInput) -> Result<MemoryItem> {
        let item = MemoryItem {
            row_id: 0,
            id: Uuid::new_v4().to_string(),
            project_id: input.project_id.to_string(),
            task_id: input.task_id,
            execution_id: input.execution_id,
            conversation_id: input.conversation_id,
            source_type: input.source_type.to_string(),
            kind: input.kind.to_string(),
            title: input.title,
            summary: input.summary,
            body: input.body,
            metadata_json: source_metadata_json(&input.source_ref),
            confidence: input.confidence.map(|value| value.to_string()),
            quality_score: input.quality_score,
            created_by_type: input
                .creator
                .as_ref()
                .map(|creator| creator.creator_type.clone()),
            created_by_id: input.creator.and_then(|creator| creator.creator_id),
            created_at: now_rfc3339(),
        };
        self.db.insert_memory_item(&item).await?;
        Ok(item)
    }
}

impl<R> MemoryService<R>
where
    R: MemoryBackfillRepository + Send + Sync,
{
    pub async fn backfill_all(db: Arc<R>) -> Result<BackfillSummary> {
        Self::new(db).backfill_sources().await
    }

    pub async fn backfill_sources(&self) -> Result<BackfillSummary> {
        let mut results = backfill_results_by_type();
        for source in self.db.list_memory_backfill_sources().await? {
            let result = backfill_result_for_source(&mut results, &source.source_type)
                .expect("all source types are pre-seeded");
            if self
                .db
                .memory_source_exists(&source.project_id, &source.source_type, &source.source_ref)
                .await?
            {
                result.skipped += 1;
                continue;
            }

            let input = MemoryItemInput {
                project_id: parse_uuid(&source.project_id, "project_id")?,
                task_id: source.task_id,
                execution_id: source.execution_id,
                conversation_id: source.conversation_id,
                source_type: source.source_type.clone(),
                source_ref: source.source_ref,
                kind: source.kind,
                title: source.title,
                summary: source.summary,
                body: source.body,
                confidence: source.confidence,
                quality_score: None,
                creator: source.creator,
            };
            self.record_from_source(input).await?;
            result.indexed += 1;
        }
        Ok(backfill_summary_from_results(results))
    }

    pub(crate) async fn record_transition_if_failure(
        &self,
        project_id: &str,
        transition: &TransitionLog,
        hook_results_json: Option<&str>,
    ) -> Result<Option<Uuid>> {
        if !transition_has_failure_signal(
            &transition.from_state,
            &transition.to_state,
            &transition.trigger_reason,
            hook_results_json.or(transition.hook_results_json.as_deref()),
            transition.rejection,
        ) {
            return Ok(None);
        }
        if self
            .db
            .memory_source_exists(
                project_id,
                &MemorySourceType::Transition,
                transition.id.as_str(),
            )
            .await?
        {
            return Ok(None);
        }
        let body = transition_body(transition, hook_results_json);
        let item = self
            .record_from_source(MemoryItemInput {
                project_id: parse_uuid(project_id, "project_id")?,
                task_id: Some(transition.task_id.clone()),
                execution_id: None,
                conversation_id: None,
                source_type: MemorySourceType::Transition,
                source_ref: transition.id.clone(),
                kind: MemoryKind::Transition,
                title: format!(
                    "Transition {} -> {}",
                    transition.from_state, transition.to_state
                ),
                summary: snippet(&transition.trigger_reason).or_else(|| snippet(&body)),
                body,
                confidence: Some(MemoryConfidence::Confirmed),
                quality_score: None,
                creator: None,
            })
            .await?;
        parse_uuid(&item.id, "memory_item.id").map(Some)
    }
}

impl<R> MemoryService<R>
where
    R: MemoryRepository + Send + Sync,
{
    pub(crate) async fn record_execution_summary_if_present(
        &self,
        project_id: &str,
        execution: &Execution,
    ) -> Result<Option<Uuid>> {
        if !matches!(
            &execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed
        ) {
            return Ok(None);
        }
        let Some(body) = execution_summary_body(execution) else {
            return Ok(None);
        };
        let title = format!("{} execution {}", execution.status, execution.role);
        let item = self
            .record_from_source(MemoryItemInput {
                project_id: parse_uuid(project_id, "project_id")?,
                task_id: Some(execution.task_id.clone()),
                execution_id: Some(execution.id.clone()),
                conversation_id: None,
                source_type: MemorySourceType::Execution,
                source_ref: execution.id.clone(),
                kind: MemoryKind::ExecutionSummary,
                title,
                summary: snippet(&body),
                body,
                confidence: Some(if execution.status == ExecutionStatus::Completed {
                    MemoryConfidence::Confirmed
                } else {
                    MemoryConfidence::Partial
                }),
                quality_score: None,
                creator: execution.agent_id.clone().map(agent_creator),
            })
            .await?;
        parse_uuid(&item.id, "memory_item.id").map(Some)
    }

    pub(crate) async fn record_review_result_if_final(
        &self,
        project_id: &str,
        review: &Review,
    ) -> Result<Option<Uuid>> {
        if review.status == ReviewStatus::Running {
            return Ok(None);
        }
        let item = self
            .record_from_source(MemoryItemInput {
                project_id: parse_uuid(project_id, "project_id")?,
                task_id: Some(review.task_id.clone()),
                execution_id: Some(review.execution_id.clone()),
                conversation_id: None,
                source_type: MemorySourceType::Review,
                source_ref: review.id.clone(),
                kind: MemoryKind::ReviewResult,
                title: format!("Review {} attempt {}", review.status, review.attempt_number),
                summary: review_summary(review),
                body: review.step_results_json.clone(),
                confidence: Some(
                    if matches!(&review.status, ReviewStatus::Passed | ReviewStatus::Failed) {
                        MemoryConfidence::Confirmed
                    } else {
                        MemoryConfidence::Partial
                    },
                ),
                quality_score: None,
                creator: None,
            })
            .await?;
        parse_uuid(&item.id, "memory_item.id").map(Some)
    }

    pub(crate) async fn record_task_comment(
        &self,
        project_id: &str,
        comment: &TaskComment,
    ) -> Result<MemoryItem> {
        let creator = match &comment.author_type {
            CommentAuthorType::Agent => comment.author_id.clone().map(agent_creator),
            CommentAuthorType::User => comment.author_id.clone().map(user_creator),
            CommentAuthorType::System => Some(system_creator()),
        };
        self.record_from_source(MemoryItemInput {
            project_id: parse_uuid(project_id, "project_id")?,
            task_id: Some(comment.task_id.clone()),
            execution_id: None,
            conversation_id: None,
            source_type: MemorySourceType::Comment,
            source_ref: comment.id.clone(),
            kind: MemoryKind::Comment,
            title: format!("Comment by {}", comment.author_name),
            summary: snippet(&comment.content),
            body: comment.content.clone(),
            confidence: Some(MemoryConfidence::Confirmed),
            quality_score: None,
            creator,
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn record_conversation_message(
        &self,
        project_id: &str,
        conversation_agent_id: Option<&str>,
        message_id: &str,
        conversation_id: &str,
        role: &str,
        status: &str,
        content: &str,
        error: Option<&str>,
    ) -> Result<Option<Uuid>> {
        if role != "user" && role != "assistant" {
            return Ok(None);
        }
        if status == "streaming" {
            return Ok(None);
        }
        let body = conversation_message_body(role, status, content, error);
        let item = self
            .record_from_source(MemoryItemInput {
                project_id: parse_uuid(project_id, "project_id")?,
                task_id: None,
                execution_id: None,
                conversation_id: Some(conversation_id.to_owned()),
                source_type: MemorySourceType::Conversation,
                source_ref: message_id.to_owned(),
                kind: MemoryKind::ConversationMessage,
                title: format!("Conversation {role} message"),
                summary: snippet(&body),
                body,
                confidence: Some(match status {
                    "complete" => MemoryConfidence::Confirmed,
                    _ => MemoryConfidence::Partial,
                }),
                quality_score: None,
                creator: match role {
                    "assistant" => conversation_agent_id.map(str::to_owned).map(agent_creator),
                    "user" => Some(user_creator("conversation-user".to_owned())),
                    _ => None,
                },
            })
            .await?;
        parse_uuid(&item.id, "memory_item.id").map(Some)
    }
}

#[async_trait]
impl MemoryBackfillRepository for SqliteDb {
    async fn list_memory_backfill_sources(&self) -> Result<Vec<MemoryBackfillSource>> {
        let mut sources = Vec::new();
        sources.extend(list_execution_sources(self).await?);
        sources.extend(list_review_sources(self).await?);
        sources.extend(list_comment_sources(self).await?);
        sources.extend(list_transition_sources(self).await?);
        sources.extend(list_conversation_sources(self).await?);
        Ok(sources)
    }

    async fn memory_source_exists(
        &self,
        project_id: &str,
        source_type: &MemorySourceType,
        source_ref: &str,
    ) -> Result<bool> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM memory_item WHERE project_id = ? AND source_type = ? AND metadata_json = ?",
        )
        .bind(project_id)
        .bind(source_type.to_string())
        .bind(source_metadata_json(source_ref))
        .fetch_one(self.pool())
        .await?;
        Ok(count > 0)
    }
}

async fn list_execution_sources(db: &SqliteDb) -> Result<Vec<MemoryBackfillSource>> {
    let rows = sqlx::query(
        "SELECT t.project_id, e.id, e.task_id, e.agent_id, e.role, e.status, e.summary, e.error \
         FROM execution e JOIN task t ON t.id = e.task_id \
         WHERE e.status IN ('completed', 'failed') \
           AND (TRIM(COALESCE(e.summary, '')) <> '' OR TRIM(COALESCE(e.error, '')) <> '') \
         ORDER BY e.created_at ASC, e.id ASC",
    )
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            let status: String = row.try_get("status")?;
            let summary: Option<String> = row.try_get("summary")?;
            let error: Option<String> = row.try_get("error")?;
            let body = summary
                .clone()
                .or(error.clone())
                .unwrap_or_else(|| status.clone());
            Ok(MemoryBackfillSource {
                project_id: row.try_get("project_id")?,
                task_id: Some(row.try_get("task_id")?),
                execution_id: Some(row.try_get("id")?),
                conversation_id: None,
                source_type: MemorySourceType::Execution,
                source_ref: row.try_get("id")?,
                kind: MemoryKind::ExecutionSummary,
                title: format!("Execution {status}: {}", row.try_get::<String, _>("role")?),
                summary: snippet(&body),
                body,
                confidence: Some(if status == "completed" {
                    MemoryConfidence::Confirmed
                } else {
                    MemoryConfidence::Partial
                }),
                creator: row
                    .try_get::<Option<String>, _>("agent_id")?
                    .map(agent_creator),
            })
        })
        .collect()
}

async fn list_review_sources(db: &SqliteDb) -> Result<Vec<MemoryBackfillSource>> {
    let rows = sqlx::query(
        "SELECT t.project_id, r.id, r.task_id, r.execution_id, r.attempt_number, r.status, r.step_results_json \
         FROM review r JOIN task t ON t.id = r.task_id \
         WHERE r.status != 'running' \
         ORDER BY r.created_at ASC, r.id ASC",
    )
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            let status: String = row.try_get("status")?;
            let body: String = row.try_get("step_results_json")?;
            Ok(MemoryBackfillSource {
                project_id: row.try_get("project_id")?,
                task_id: Some(row.try_get("task_id")?),
                execution_id: Some(row.try_get("execution_id")?),
                conversation_id: None,
                source_type: MemorySourceType::Review,
                source_ref: row.try_get("id")?,
                kind: MemoryKind::ReviewResult,
                title: format!(
                    "Review {status} attempt {}",
                    row.try_get::<i64, _>("attempt_number")?
                ),
                summary: review_summary_from_json(&body, &status),
                body,
                confidence: Some(if status == "passed" || status == "failed" {
                    MemoryConfidence::Confirmed
                } else {
                    MemoryConfidence::Partial
                }),
                creator: None,
            })
        })
        .collect()
}

async fn list_comment_sources(db: &SqliteDb) -> Result<Vec<MemoryBackfillSource>> {
    let rows = sqlx::query(
        "SELECT t.project_id, c.id, c.task_id, c.author_type, c.author_id, c.author_name, c.content \
         FROM task_comment c JOIN task t ON t.id = c.task_id \
         ORDER BY c.created_at ASC, c.id ASC",
    )
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            let author_type: String = row.try_get("author_type")?;
            let author_id: Option<String> = row.try_get("author_id")?;
            let content: String = row.try_get("content")?;
            let author_name: String = row.try_get("author_name")?;
            Ok(MemoryBackfillSource {
                project_id: row.try_get("project_id")?,
                task_id: Some(row.try_get("task_id")?),
                execution_id: None,
                conversation_id: None,
                source_type: MemorySourceType::Comment,
                source_ref: row.try_get("id")?,
                kind: MemoryKind::Comment,
                title: format!("Comment by {author_name}"),
                summary: snippet(&content),
                body: content,
                confidence: Some(MemoryConfidence::Confirmed),
                creator: match author_type.as_str() {
                    "agent" => author_id.map(agent_creator),
                    "user" => author_id.map(user_creator),
                    "system" => Some(system_creator()),
                    _ => None,
                },
            })
        })
        .collect()
}

async fn list_transition_sources(db: &SqliteDb) -> Result<Vec<MemoryBackfillSource>> {
    let rows = sqlx::query(
        "SELECT t.project_id, tl.id, tl.task_id, tl.from_state, tl.to_state, tl.trigger_name, tl.triggered_by, \
                tl.trigger_reason, tl.hook_results_json, tl.rejection, tl.created_at \
         FROM transition_log tl JOIN task t ON t.id = tl.task_id \
         ORDER BY tl.created_at ASC, tl.id ASC",
    )
    .fetch_all(db.pool())
    .await?;
    let mut sources = Vec::new();
    for row in rows {
        let transition = TransitionLog {
            id: row.try_get("id")?,
            task_id: row.try_get("task_id")?,
            from_state: row.try_get("from_state")?,
            to_state: row.try_get("to_state")?,
            trigger_name: row.try_get("trigger_name")?,
            triggered_by: row.try_get("triggered_by")?,
            trigger_reason: row.try_get("trigger_reason")?,
            hook_results_json: row.try_get("hook_results_json")?,
            rejection: row.try_get::<i64, _>("rejection")? != 0,
            created_at: row.try_get("created_at")?,
        };
        if !transition_has_failure_signal(
            &transition.from_state,
            &transition.to_state,
            &transition.trigger_reason,
            transition.hook_results_json.as_deref(),
            transition.rejection,
        ) {
            continue;
        }
        let body = transition_body(&transition, None);
        sources.push(MemoryBackfillSource {
            project_id: row.try_get("project_id")?,
            task_id: Some(transition.task_id.clone()),
            execution_id: None,
            conversation_id: None,
            source_type: MemorySourceType::Transition,
            source_ref: transition.id.clone(),
            kind: MemoryKind::Transition,
            title: format!(
                "Transition {} -> {}",
                transition.from_state, transition.to_state
            ),
            summary: snippet(&transition.trigger_reason).or_else(|| snippet(&body)),
            body,
            confidence: Some(MemoryConfidence::Confirmed),
            creator: None,
        });
    }
    Ok(sources)
}

async fn list_conversation_sources(db: &SqliteDb) -> Result<Vec<MemoryBackfillSource>> {
    let rows = sqlx::query(
        "SELECT c.project_id, cm.id, cm.conversation_id, c.agent_id, cm.role, cm.content, cm.status, cm.error \
         FROM conversation_message cm JOIN conversation c ON c.id = cm.conversation_id \
         WHERE cm.role IN ('user', 'assistant') AND cm.status != 'streaming' \
         ORDER BY cm.created_at ASC, cm.id ASC",
    )
    .fetch_all(db.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            let role: String = row.try_get("role")?;
            let status: String = row.try_get("status")?;
            let content: String = row.try_get("content")?;
            let error: Option<String> = row.try_get("error")?;
            let body = conversation_message_body(&role, &status, &content, error.as_deref());
            Ok(MemoryBackfillSource {
                project_id: row.try_get("project_id")?,
                task_id: None,
                execution_id: None,
                conversation_id: Some(row.try_get("conversation_id")?),
                source_type: MemorySourceType::Conversation,
                source_ref: row.try_get("id")?,
                kind: MemoryKind::ConversationMessage,
                title: format!("Conversation {role} message"),
                summary: snippet(&body),
                body,
                confidence: Some(match status.as_str() {
                    "complete" => MemoryConfidence::Confirmed,
                    _ => MemoryConfidence::Partial,
                }),
                creator: match role.as_str() {
                    "assistant" => row
                        .try_get::<Option<String>, _>("agent_id")?
                        .map(agent_creator),
                    "user" => Some(user_creator("conversation-user".to_owned())),
                    _ => None,
                },
            })
        })
        .collect()
}

fn resolve_layer(layer: Option<u8>, token_budget: Option<u32>) -> Result<MemoryLayer> {
    match layer {
        Some(1) => Ok(MemoryLayer::One),
        Some(2) => Ok(MemoryLayer::Two),
        Some(3) => Ok(MemoryLayer::Three),
        Some(other) => Err(ServiceError::invalid_operation(format!(
            "invalid memory layer {other}; expected 1, 2, or 3"
        ))),
        None => Ok(match token_budget {
            Some(budget) if budget < 200 => MemoryLayer::One,
            Some(budget) if budget <= 1000 => MemoryLayer::Two,
            _ => MemoryLayer::Three,
        }),
    }
}

fn shape_item(item: MemoryItem, layer: MemoryLayer) -> Result<MemorySearchResult> {
    let id = parse_uuid(&item.id, "memory_item.id")?;
    let kind = MemoryKind::from_str(&item.kind).map_err(ServiceError::invalid_operation)?;
    let source_type =
        MemorySourceType::from_str(&item.source_type).map_err(ServiceError::invalid_operation)?;
    let references = MemoryReferences {
        source_ref: source_ref_from_metadata(&item.metadata_json)
            .unwrap_or_else(|| item.id.clone()),
        task_id: item.task_id.clone(),
        execution_id: item.execution_id.clone(),
        conversation_id: item.conversation_id.clone(),
    };
    let confidence = item
        .confidence
        .as_deref()
        .map(MemoryConfidence::from_str)
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

fn memory_cursor_for_item(item: &MemoryItem) -> Result<String> {
    let bytes = serde_json::to_vec(&MemoryCursor {
        created_at: item.created_at.clone(),
        id: item.id.clone(),
    })
    .map_err(|error| ServiceError::invalid_operation(format!("invalid memory cursor: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid {field} UUID '{value}': {error}"))
    })
}

fn source_metadata_json(source_ref: &str) -> String {
    json!({ "source_ref": source_ref }).to_string()
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

fn snippet(content: &str) -> Option<String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(240).collect())
}

fn execution_summary_body(execution: &Execution) -> Option<String> {
    execution
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            execution
                .error
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
}

fn review_summary(review: &Review) -> Option<String> {
    review_summary_from_json(&review.step_results_json, &review.status.to_string())
}

fn review_summary_from_json(step_results_json: &str, status: &str) -> Option<String> {
    let details = serde_json::from_str::<Value>(step_results_json).ok()?;
    if let Some(reason) = details
        .get("auditor")
        .and_then(|auditor| auditor.get("reason"))
        .and_then(Value::as_str)
    {
        return Some(reason.to_owned());
    }
    if let Some(verdict) = details
        .get("auditor")
        .and_then(|auditor| auditor.get("verdict"))
        .and_then(Value::as_str)
    {
        return Some(format!("review {status}: {verdict}"));
    }
    Some(format!("review {status}"))
}

fn transition_body(transition: &TransitionLog, hook_results_json: Option<&str>) -> String {
    json!({
        "from_state": &transition.from_state,
        "to_state": &transition.to_state,
        "trigger_name": &transition.trigger_name,
        "triggered_by": &transition.triggered_by,
        "trigger_reason": &transition.trigger_reason,
        "rejection": transition.rejection,
        "hook_results": hook_results_json.or(transition.hook_results_json.as_deref()),
    })
    .to_string()
}

fn transition_has_failure_signal(
    from_state: &str,
    to_state: &str,
    trigger_reason: &str,
    hook_results_json: Option<&str>,
    rejection: bool,
) -> bool {
    if rejection
        || state_name_is_failure(from_state)
        || state_name_is_failure(to_state)
        || text_has_failure_signal(trigger_reason)
    {
        return true;
    }
    let Some(hook_results_json) = hook_results_json else {
        return false;
    };
    serde_json::from_str::<Value>(hook_results_json)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .is_some_and(|entries| entries.iter().any(hook_result_is_failure))
}

fn hook_result_is_failure(entry: &Value) -> bool {
    entry
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|outcome| outcome == "failed")
        || entry
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| matches!(status, "failure" | "hook_error"))
        || entry
            .get("error")
            .and_then(Value::as_str)
            .is_some_and(text_has_failure_signal)
}

fn state_name_is_failure(state: &str) -> bool {
    let lower = state.to_ascii_lowercase();
    lower.contains("fail") || lower.contains("error") || lower.contains("blocked")
}

fn text_has_failure_signal(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("failed")
        || lower.contains("failure")
        || lower.contains("error")
        || lower.contains("hook_error")
}

fn conversation_message_body(
    role: &str,
    status: &str,
    content: &str,
    error: Option<&str>,
) -> String {
    let trimmed = content.trim();
    if !trimmed.is_empty() {
        return trimmed.to_owned();
    }
    error
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{role} message {status}"))
}

fn agent_creator(agent_id: String) -> MemoryCreator {
    MemoryCreator {
        creator_type: "agent".to_owned(),
        creator_id: Some(agent_id),
    }
}

fn user_creator(user_id: String) -> MemoryCreator {
    MemoryCreator {
        creator_type: "user".to_owned(),
        creator_id: Some(user_id),
    }
}

fn system_creator() -> MemoryCreator {
    MemoryCreator {
        creator_type: "system".to_owned(),
        creator_id: None,
    }
}

fn backfill_results_by_type() -> Vec<BackfillTypeResult> {
    [
        MemorySourceType::Execution,
        MemorySourceType::Review,
        MemorySourceType::Comment,
        MemorySourceType::Transition,
        MemorySourceType::Conversation,
    ]
    .into_iter()
    .map(|source_type| BackfillTypeResult {
        source_type,
        indexed: 0,
        skipped: 0,
    })
    .collect()
}

fn backfill_result_for_source<'a>(
    results: &'a mut [BackfillTypeResult],
    source_type: &MemorySourceType,
) -> Option<&'a mut BackfillTypeResult> {
    results
        .iter_mut()
        .find(|result| result.source_type == *source_type)
}

fn backfill_summary_from_results(items: Vec<BackfillTypeResult>) -> BackfillSummary {
    let indexed = items.iter().map(|item| item.indexed).sum();
    let skipped = items.iter().map(|item| item.skipped).sum();
    BackfillSummary {
        items,
        indexed,
        skipped,
    }
}
