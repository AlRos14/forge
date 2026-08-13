//! Forge service adapter for the host's scope-derived native tools.
//!
//! This adapter deliberately exposes only read projections and proposal
//! envelopes.  It never calls Task mutation/workflow methods directly; an
//! admitted `task.propose` remains an `AgentAction` until the existing
//! coordination/Task services perform their normal policy and execution
//! steps.

use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use db::{
    AgentAction, AgentActionListQuery, AgentActionRepo, AgentCommitmentListQuery,
    AgentCommitmentRepo, AgentInboxListQuery, AgentInboxRepo, MemoryScopeGrant, SqliteDb,
};
use forge_agent_host::{
    AgentHostError, CanonicalScope, CanonicalScopeType, ForgeToolProvider, WorkspaceAccess,
};
use serde_json::{json, Value};
use sqlx::Row;

use crate::{
    agent_chat_policy::guard_agent_chat_content,
    coordination_service::{AgentActionService, ProposeActionInput},
    memory::{MemoryAccessContext, MemoryService},
};

/// Forge-owned provider injected into native Agent Runtime compositions.
#[derive(Clone)]
pub struct CoordinationToolProvider {
    db: Arc<SqliteDb>,
    actions: AgentActionService,
    memory: MemoryService,
}

impl std::fmt::Debug for CoordinationToolProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CoordinationToolProvider")
            .finish_non_exhaustive()
    }
}

impl CoordinationToolProvider {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self {
            actions: AgentActionService::new(Arc::clone(&db)),
            memory: MemoryService::new(Arc::clone(&db)),
            db,
        }
    }

    async fn summary(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
    ) -> Result<Value, AgentHostError> {
        let (query, bind_id) = match scope.scope_type {
            CanonicalScopeType::Account => (
                "SELECT id, name, status, paused, visibility FROM agent_identity WHERE id = ?",
                actor_identity_id,
            ),
            CanonicalScopeType::Project => (
                "SELECT id, name FROM project WHERE id = ?",
                scope.scope_id.as_str(),
            ),
            CanonicalScopeType::AgentChat => (
                "SELECT id, kind, status, kind AS scope_type, id AS scope_id FROM agent_chat WHERE id = ?",
                scope.scope_id.as_str(),
            ),
            CanonicalScopeType::Task => (
                "SELECT id, project_id, title, status, priority FROM task WHERE id = ?",
                scope.scope_id.as_str(),
            ),
        };
        let row = sqlx::query(query)
            .bind(bind_id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?
            .ok_or_else(|| {
                AgentHostError::Authority("current Forge scope is unavailable".into())
            })?;
        let mut result = serde_json::Map::new();
        for column in [
            "id",
            "name",
            "title",
            "status",
            "paused",
            "visibility",
            "scope_type",
            "scope_id",
            "project_id",
            "priority",
        ] {
            if let Ok(value) = row.try_get::<String, _>(column) {
                result.insert(column.to_owned(), Value::String(value));
            } else if let Ok(value) = row.try_get::<i64, _>(column) {
                result.insert(column.to_owned(), Value::Number(value.into()));
            }
        }
        result.insert(
            "canonical_scope".to_owned(),
            json!({
                "type": scope_type_name(scope.scope_type),
                "id": scope.scope_id,
                "workspace_access": workspace_access_name(scope.workspace_access),
            }),
        );
        Ok(Value::Object(result))
    }

    async fn memory_read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        arguments: Value,
        decision_only: bool,
    ) -> Result<Value, AgentHostError> {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .take(512)
            .collect::<String>();
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 20) as u32;
        let visibility = match scope.scope_type {
            CanonicalScopeType::Account => vec!["account".to_owned(), "private".to_owned()],
            CanonicalScopeType::Project => vec!["project".to_owned(), "private".to_owned()],
            CanonicalScopeType::AgentChat => vec![
                "chat".to_owned(),
                "project".to_owned(),
                "private".to_owned(),
            ],
            CanonicalScopeType::Task => vec![
                "task".to_owned(),
                "project".to_owned(),
                "private".to_owned(),
            ],
        };
        // Agent Chat history is owned by the chat. The chat repository
        // performs the binding check before this provider is composed.
        let access = MemoryAccessContext {
            identity_id: Some(actor_identity_id.to_owned()),
            grants: vec![MemoryScopeGrant {
                scope_type: scope_type_name(scope.scope_type).to_owned(),
                scope_id: scope.scope_id.clone(),
                visibility,
                identity_id: Some(actor_identity_id.to_owned()),
            }],
        };
        let (items, has_more, cursor) = self
            .memory
            .search_scoped(
                &access,
                query,
                Some(2),
                if decision_only {
                    limit.saturating_mul(5).min(100)
                } else {
                    limit
                },
                None,
            )
            .await
            .map_err(service_error)?;
        let items = items
            .into_iter()
            .filter(|item| !decision_only || item.kind == db::MemoryKind::Decision)
            .take(limit as usize)
            .map(|item| {
                json!({
                    "id": item.id.to_string(),
                    "kind": item.kind.to_string(),
                    "title": item.title,
                    "summary": item.summary,
                    "source_type": item.source_type.to_string(),
                    "created_at": item.created_at,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"items": items, "has_more": has_more, "next_cursor": cursor}))
    }

    async fn scoped_rows(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .clamp(1, 50) as i64;
        match operation {
            "work.read" => self.read_work(scope, limit).await,
            "events.read" => self.read_events(scope, limit).await,
            "inbox.read" => self.read_inbox(actor_identity_id, scope, limit).await,
            "commitments.read" => self.read_commitments(actor_identity_id, scope, limit).await,
            "delivery.read" => self.read_delivery(actor_identity_id, scope, limit).await,
            _ => Err(AgentHostError::Unsupported(
                "Forge scoped read operation is not implemented".to_owned(),
            )),
        }
    }

    /// Resolve the account represented by a Main Chat without trusting the
    /// opaque chat id or any account id supplied in model arguments.  Main
    /// projections are intentionally account-owned and never fan out into
    /// Project Chat history or private Project memory.
    async fn main_account_id(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
    ) -> Result<String, AgentHostError> {
        let owner_id = sqlx::query_scalar::<_, Option<String>>(
            "SELECT owner_id FROM agent_identity WHERE id = ?",
        )
        .bind(actor_identity_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?
        .flatten()
        .ok_or_else(|| AgentHostError::Authority("Main Agent account is unavailable".to_owned()))?;
        let account_id = match scope.scope_type {
            CanonicalScopeType::Account => scope.scope_id.clone(),
            CanonicalScopeType::AgentChat => {
                let row =
                    sqlx::query("SELECT kind, account_id FROM agent_chat WHERE id = ? LIMIT 1")
                        .bind(&scope.scope_id)
                        .fetch_optional(self.db.pool())
                        .await
                        .map_err(|_| AgentHostError::ProtectedPersistence)?
                        .ok_or_else(|| {
                            AgentHostError::Authority("Main Agent Chat is unavailable".to_owned())
                        })?;
                let kind: String = row
                    .try_get("kind")
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                if kind != "account_main" {
                    return Err(AgentHostError::Authority(
                        "global Main Agent operations are unavailable in Project Chat".to_owned(),
                    ));
                }
                row.try_get::<Option<String>, _>("account_id")
                    .map_err(|_| AgentHostError::ProtectedPersistence)?
                    .ok_or_else(|| {
                        AgentHostError::Authority("Main Agent account is unavailable".to_owned())
                    })?
            }
            _ => {
                return Err(AgentHostError::Authority(
                    "global Main Agent operation is unavailable in this scope".to_owned(),
                ));
            }
        };
        if owner_id != account_id {
            return Err(AgentHostError::Authority(
                "actor identity does not own the Main Agent scope".to_owned(),
            ));
        }
        Ok(account_id)
    }

    async fn discovery_read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        let account_id = self.main_account_id(actor_identity_id, scope).await?;
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 20) as i64;
        let rows = sqlx::query(
            "SELECT id, maturity, lifecycle, project_id, handoff_id, version,
                    created_at, updated_at
             FROM product_genesis_session
             WHERE account_id = ?
             ORDER BY updated_at DESC, id DESC LIMIT ?",
        )
        .bind(account_id)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        Ok(json!({
            "items": rows.into_iter().map(|row| json!({
                "id": row.try_get::<String, _>("id").unwrap_or_default(),
                "maturity": row.try_get::<String, _>("maturity").unwrap_or_default(),
                "lifecycle": row.try_get::<String, _>("lifecycle").unwrap_or_default(),
                "project_id": row.try_get::<Option<String>, _>("project_id").ok().flatten(),
                "handoff_id": row.try_get::<Option<String>, _>("handoff_id").ok().flatten(),
                "version": row.try_get::<i64, _>("version").unwrap_or_default(),
                "created_at": row.try_get::<String, _>("created_at").unwrap_or_default(),
                "updated_at": row.try_get::<String, _>("updated_at").unwrap_or_default(),
            })).collect::<Vec<_>>()
        }))
    }

    async fn portfolio_read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        let account_id = self.main_account_id(actor_identity_id, scope).await?;
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .clamp(1, 20) as i64;
        let rows = sqlx::query(
            "SELECT id, name, paused_at, created_at, updated_at
             FROM project WHERE owner_id = ? ORDER BY updated_at DESC, id DESC LIMIT ?",
        )
        .bind(account_id)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        Ok(json!({
            "items": rows.into_iter().map(|row| json!({
                "id": row.try_get::<String, _>("id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "paused": row.try_get::<Option<String>, _>("paused_at").ok().flatten().is_some(),
                "created_at": row.try_get::<String, _>("created_at").unwrap_or_default(),
                "updated_at": row.try_get::<String, _>("updated_at").unwrap_or_default(),
            })).collect::<Vec<_>>()
        }))
    }

    async fn project_summary_read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        let account_id = self.main_account_id(actor_identity_id, scope).await?;
        let project_id = arguments
            .get("project_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AgentHostError::Authority("project_id is required".to_owned()))?;
        let row = sqlx::query(
            "SELECT p.id, p.name, p.paused_at, p.created_at, p.updated_at,
                    COUNT(t.id) AS task_count
             FROM project AS p
             LEFT JOIN task AS t ON t.project_id = p.id AND t.deleted_at IS NULL
             WHERE p.id = ? AND p.owner_id = ?
             GROUP BY p.id",
        )
        .bind(project_id)
        .bind(account_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?
        .ok_or_else(|| AgentHostError::Authority("Project summary is unavailable".to_owned()))?;
        Ok(json!({
            "id": row.try_get::<String, _>("id").unwrap_or_default(),
            "name": row.try_get::<String, _>("name").unwrap_or_default(),
            "paused": row.try_get::<Option<String>, _>("paused_at").ok().flatten().is_some(),
            "task_count": row.try_get::<i64, _>("task_count").unwrap_or_default(),
            "created_at": row.try_get::<String, _>("created_at").unwrap_or_default(),
            "updated_at": row.try_get::<String, _>("updated_at").unwrap_or_default(),
        }))
    }

    async fn read_work(&self, scope: &CanonicalScope, limit: i64) -> Result<Value, AgentHostError> {
        let rows = match scope.scope_type {
            CanonicalScopeType::Project => sqlx::query(
                "SELECT id, title, status, priority, assignee_type, assignee_id
                     FROM task WHERE project_id = ? AND deleted_at IS NULL
                     ORDER BY updated_at DESC, id DESC LIMIT ?",
            )
            .bind(&scope.scope_id)
            .bind(limit)
            .fetch_all(self.db.pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
            CanonicalScopeType::Task => sqlx::query(
                "SELECT id, title, status, priority, assignee_type, assignee_id
                     FROM task WHERE id = ? AND deleted_at IS NULL LIMIT 1",
            )
            .bind(&scope.scope_id)
            .fetch_all(self.db.pool())
            .await
            .map_err(|_| AgentHostError::ProtectedPersistence)?,
            _ => {
                return Err(AgentHostError::Authority(
                    "work is not available in this canonical scope".to_owned(),
                ));
            }
        };
        let items = rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.try_get::<String, _>("id").unwrap_or_default(),
                    "title": row.try_get::<String, _>("title").unwrap_or_default(),
                    "status": row.try_get::<String, _>("status").unwrap_or_default(),
                    "priority": row.try_get::<i64, _>("priority").unwrap_or_default(),
                    "assignee_type": row.try_get::<Option<String>, _>("assignee_type").ok().flatten(),
                    "assignee_id": row.try_get::<Option<String>, _>("assignee_id").ok().flatten(),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"items": items}))
    }

    async fn read_events(
        &self,
        scope: &CanonicalScope,
        limit: i64,
    ) -> Result<Value, AgentHostError> {
        let rows = sqlx::query(
            "SELECT sequence, id, event_type, entity_type, entity_id, actor_type,
                    correlation_id, causation_id, causation_depth, created_at
             FROM domain_event
             WHERE scope_type = ? AND scope_id = ?
             ORDER BY sequence DESC LIMIT ?",
        )
        .bind(scope_type_name(scope.scope_type))
        .bind(&scope.scope_id)
        .bind(limit)
        .fetch_all(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let items = rows
            .into_iter()
            .map(|row| {
                json!({
                    "sequence": row.try_get::<i64, _>("sequence").unwrap_or_default(),
                    "id": row.try_get::<String, _>("id").unwrap_or_default(),
                    "event_type": row.try_get::<String, _>("event_type").unwrap_or_default(),
                    "entity_type": row.try_get::<String, _>("entity_type").unwrap_or_default(),
                    "entity_id": row.try_get::<String, _>("entity_id").unwrap_or_default(),
                    "actor_type": row.try_get::<String, _>("actor_type").unwrap_or_default(),
                    "correlation_id": row.try_get::<String, _>("correlation_id").unwrap_or_default(),
                    "causation_id": row.try_get::<Option<String>, _>("causation_id").ok().flatten(),
                    "causation_depth": row.try_get::<i64, _>("causation_depth").unwrap_or_default(),
                    "created_at": row.try_get::<String, _>("created_at").unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"items": items}))
    }

    async fn read_inbox(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        limit: i64,
    ) -> Result<Value, AgentHostError> {
        let items = AgentInboxRepo::list_inbox_items(
            &*self.db,
            AgentInboxListQuery {
                recipient_identity_id: actor_identity_id.to_owned(),
                status: None,
                scope_type: Some(scope_type_name(scope.scope_type).to_owned()),
                scope_id: Some(scope.scope_id.clone()),
                limit,
            },
        )
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        Ok(json!({
            "items": items.into_iter().map(|item| json!({
                "id": item.id,
                "kind": item.kind.to_string(),
                "status": item.status.to_string(),
                "title": truncate(&item.title, 256),
                "source_type": item.source_type,
                "source_id": item.source_id,
                "correlation_id": item.correlation_id,
                "version": item.version,
                "created_at": item.created_at,
            })).collect::<Vec<_>>()
        }))
    }

    async fn read_commitments(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        limit: i64,
    ) -> Result<Value, AgentHostError> {
        let items = AgentCommitmentRepo::list_commitments(
            &*self.db,
            AgentCommitmentListQuery {
                owner_identity_id: Some(actor_identity_id.to_owned()),
                scope_type: Some(scope_type_name(scope.scope_type).to_owned()),
                scope_id: Some(scope.scope_id.clone()),
                status: None,
                limit,
            },
        )
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        Ok(json!({
            "items": items.into_iter().map(|item| json!({
                "id": item.id,
                "title": truncate(&item.title, 256),
                "status": item.status.to_string(),
                "due_at": item.due_at,
                "originating_task_id": item.originating_task_id,
                "evidence_required": item.evidence_required,
                "blocked_reason": item.blocked_reason.map(|reason| truncate(&reason, 256)),
                "version": item.version,
            })).collect::<Vec<_>>()
        }))
    }

    async fn read_delivery(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        limit: i64,
    ) -> Result<Value, AgentHostError> {
        let inbox = AgentInboxRepo::list_inbox_items(
            &*self.db,
            AgentInboxListQuery {
                recipient_identity_id: actor_identity_id.to_owned(),
                status: None,
                scope_type: Some(scope_type_name(scope.scope_type).to_owned()),
                scope_id: Some(scope.scope_id.clone()),
                limit,
            },
        )
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        let actions = AgentActionRepo::list_actions(
            &*self.db,
            AgentActionListQuery {
                actor_identity_id: Some(actor_identity_id.to_owned()),
                scope_type: Some(scope_type_name(scope.scope_type).to_owned()),
                scope_id: Some(scope.scope_id.clone()),
                status: None,
                limit,
            },
        )
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?;
        Ok(json!({
            "inbox": inbox.into_iter().filter(|item| matches!(&item.kind, db::AgentInboxKind::TaskOutcome | db::AgentInboxKind::ActionResult)).map(|item| json!({
                "id": item.id,
                "kind": item.kind.to_string(),
                "status": item.status.to_string(),
                "title": truncate(&item.title, 256),
                "source_id": item.source_id,
                "created_at": item.created_at,
            })).collect::<Vec<_>>(),
            "actions": actions.into_iter().map(|action| json!({
                "id": action.id,
                "operation": action.operation,
                "status": action.status.to_string(),
                "policy_result": action.policy_result.to_string(),
                "target_type": action.target_type,
                "target_id": action.target_id,
                "version": action.version,
                "created_at": action.created_at,
            })).collect::<Vec<_>>(),
        }))
    }

    async fn propose(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        let payload = arguments
            .get("payload")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                AgentHostError::Authority("proposal payload must be an object".into())
            })?;
        validate_proposal_payload(operation, &payload)?;
        let project_chat_target =
            if operation == "task.propose" && scope.scope_type == CanonicalScopeType::AgentChat {
                Some(
                    self.project_chat_task_target(actor_identity_id, scope)
                        .await?,
                )
            } else {
                None
            };
        let (requested_permission, target_type, target_id) = match operation {
            "web.search" => {
                let _ = self.main_account_id(actor_identity_id, scope).await?;
                (
                    "propose_discovery",
                    Some("account".to_owned()),
                    Some(self.main_account_id(actor_identity_id, scope).await?),
                )
            }
            "project.lifecycle" => {
                let account_id = self.main_account_id(actor_identity_id, scope).await?;
                if let Some(project_id) = payload.get("project_id").and_then(Value::as_str) {
                    let owned = sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM project WHERE id = ? AND owner_id = ?",
                    )
                    .bind(project_id)
                    .bind(&account_id)
                    .fetch_one(self.db.pool())
                    .await
                    .map_err(|_| AgentHostError::ProtectedPersistence)?;
                    if owned == 0 {
                        return Err(AgentHostError::Authority(
                            "Project lifecycle target is unavailable".to_owned(),
                        ));
                    }
                }
                (
                    "propose_project",
                    Some("account".to_owned()),
                    Some(account_id),
                )
            }
            "handoff.publish" => {
                let account_id = self.main_account_id(actor_identity_id, scope).await?;
                let project_id = payload
                    .get("target_project_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        AgentHostError::Authority("target_project_id is required".to_owned())
                    })?;
                let target_exists = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM project AS p
                     JOIN agent_chat AS chat ON chat.project_id = p.id AND chat.kind = 'project'
                     WHERE p.id = ? AND p.owner_id = ?",
                )
                .bind(project_id)
                .bind(&account_id)
                .fetch_one(self.db.pool())
                .await
                .map_err(|_| AgentHostError::ProtectedPersistence)?;
                if target_exists == 0 {
                    return Err(AgentHostError::Authority(
                        "handoff target is unavailable".to_owned(),
                    ));
                }
                (
                    "propose_handoff",
                    Some("project".to_owned()),
                    Some(project_id.to_owned()),
                )
            }
            "message.propose" | "message.send" => (
                "propose_message",
                Some(scope_type_name(scope.scope_type).to_owned()),
                Some(scope.scope_id.clone()),
            ),
            "task.propose" if scope.scope_type == CanonicalScopeType::Project => (
                "propose_task",
                Some("project".to_owned()),
                Some(scope.scope_id.clone()),
            ),
            "task.propose" if project_chat_target.is_some() => (
                "propose_task",
                Some("project".to_owned()),
                project_chat_target,
            ),
            "review.propose" | "review.request"
                if matches!(
                    scope.scope_type,
                    CanonicalScopeType::Project
                        | CanonicalScopeType::AgentChat
                        | CanonicalScopeType::Task
                ) && (scope.scope_type != CanonicalScopeType::Task
                    || scope.workspace_access == WorkspaceAccess::TaskRead) =>
            {
                (
                    "propose_review",
                    Some(scope_type_name(scope.scope_type).to_owned()),
                    Some(scope.scope_id.clone()),
                )
            }
            "commitment.propose" | "commitment.update" => (
                "propose_commitment",
                Some(scope_type_name(scope.scope_type).to_owned()),
                Some(scope.scope_id.clone()),
            ),
            "memory.publish" | "memory.supersede" => (
                "propose_memory",
                Some(scope_type_name(scope.scope_type).to_owned()),
                Some(scope.scope_id.clone()),
            ),
            "decision.request" if scope.scope_type == CanonicalScopeType::Project => (
                "propose_decision",
                Some("project".to_owned()),
                Some(scope.scope_id.clone()),
            ),
            "session.action"
                if matches!(
                    scope.scope_type,
                    CanonicalScopeType::Account
                        | CanonicalScopeType::Project
                        | CanonicalScopeType::AgentChat
                ) =>
            {
                (
                    "propose_session",
                    Some("scope".to_owned()),
                    Some(scope.scope_id.clone()),
                )
            }
            _ => {
                return Err(AgentHostError::Authority(
                    "proposal operation is not admitted for this scope".into(),
                ));
            }
        };
        let dedupe_key = required_argument(&arguments, "dedupe_key")?;
        let correlation_id = required_argument(&arguments, "correlation_id")?;
        let action = self
            .actions
            .propose(ProposeActionInput {
                id: None,
                actor_identity_id: actor_identity_id.to_owned(),
                scope_type: scope_type_name(scope.scope_type).to_owned(),
                scope_id: scope.scope_id.clone(),
                operation: operation.to_owned(),
                payload_json: payload.to_string(),
                dedupe_key,
                correlation_id,
                causation_id: arguments
                    .get("causation_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                causation_depth: arguments
                    .get("causation_depth")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                requested_permission: requested_permission.to_owned(),
                policy_reason: None,
                target_type,
                target_id,
            })
            .await
            .map_err(service_error)?;
        Ok(action_value(&action))
    }

    async fn project_chat_task_target(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
    ) -> Result<String, AgentHostError> {
        let row = sqlx::query(
            "SELECT chat.kind, chat.project_id, binding.permission_ceiling_json
             FROM agent_chat AS chat
             LEFT JOIN project_agent_binding AS binding
               ON binding.project_id = chat.project_id
              AND binding.identity_id = ?
              AND binding.state = 'active'
             WHERE chat.id = ?
             LIMIT 1",
        )
        .bind(actor_identity_id)
        .bind(&scope.scope_id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|_| AgentHostError::ProtectedPersistence)?
        .ok_or_else(|| AgentHostError::Authority("Agent Chat scope is unavailable".to_owned()))?;
        let kind = row
            .try_get::<String, _>("kind")
            .map_err(|_| AgentHostError::ProtectedPersistence)?;
        if kind != "project" {
            return Err(AgentHostError::Authority(
                "Main Agent Chat cannot manage Tasks".to_owned(),
            ));
        }
        let project_id = row
            .try_get::<Option<String>, _>("project_id")
            .map_err(|_| AgentHostError::ProtectedPersistence)?
            .ok_or_else(|| {
                AgentHostError::Authority("Project Agent Chat has no owning Project".to_owned())
            })?;
        let ceiling = row
            .try_get::<Option<String>, _>("permission_ceiling_json")
            .map_err(|_| AgentHostError::ProtectedPersistence)?
            .ok_or_else(|| {
                AgentHostError::Authority(
                    "Project Agent Chat binding does not admit Task management".to_owned(),
                )
            })?;
        if !permission_set(&ceiling).contains("propose_task") {
            return Err(AgentHostError::Authority(
                "Project Agent Chat binding does not admit Task management".to_owned(),
            ));
        }
        Ok(project_id)
    }
}

#[async_trait]
impl ForgeToolProvider for CoordinationToolProvider {
    async fn read(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        match operation {
            "memory.read" => {
                self.memory_read(actor_identity_id, scope, arguments, false)
                    .await
            }
            "account.summary" | "project.summary" | "agent_chat.summary" | "task.summary" => {
                if operation == "project.summary"
                    && scope.scope_type == CanonicalScopeType::AgentChat
                {
                    self.project_summary_read(actor_identity_id, scope, arguments)
                        .await
                } else {
                    self.summary(actor_identity_id, scope).await
                }
            }
            "discovery.read" => {
                self.discovery_read(actor_identity_id, scope, arguments)
                    .await
            }
            "portfolio.read" => {
                self.portfolio_read(actor_identity_id, scope, arguments)
                    .await
            }
            "decisions.read" => {
                self.memory_read(actor_identity_id, scope, arguments, true)
                    .await
            }
            "work.read" | "events.read" | "inbox.read" | "commitments.read" | "delivery.read" => {
                self.scoped_rows(actor_identity_id, scope, operation, arguments)
                    .await
            }
            _ => Err(AgentHostError::Unsupported(
                "Forge read operation is not implemented".to_owned(),
            )),
        }
    }

    async fn propose(
        &self,
        actor_identity_id: &str,
        scope: &CanonicalScope,
        operation: &str,
        arguments: Value,
    ) -> Result<Value, AgentHostError> {
        self.propose(actor_identity_id, scope, operation, arguments)
            .await
    }
}

fn action_value(action: &AgentAction) -> Value {
    json!({
        "id": action.id,
        "operation": action.operation,
        "scope_type": action.scope_type,
        "scope_id": action.scope_id,
        "requested_permission": action.requested_permission,
        "policy_result": action.policy_result.to_string(),
        "status": action.status.to_string(),
        "target_type": action.target_type,
        "target_id": action.target_id,
        "version": action.version,
    })
}

fn required_argument(arguments: &Value, field: &str) -> Result<String, AgentHostError> {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| AgentHostError::Authority(format!("{field} is required")))
}

fn validate_proposal_payload(operation: &str, payload: &Value) -> Result<(), AgentHostError> {
    if serde_json::to_vec(payload)
        .map(|bytes| bytes.len() > 64 * 1024)
        .unwrap_or(true)
    {
        return Err(AgentHostError::Authority(
            "Forge proposal payload is too large".to_owned(),
        ));
    }
    if operation == "session.action" {
        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentHostError::Authority("session action is required".to_owned()))?;
        if !matches!(action, "cancel" | "steer") {
            return Err(AgentHostError::Authority(
                "only bounded cancel or steer session actions are admitted".to_owned(),
            ));
        }
        if action == "steer"
            && payload
                .get("content")
                .and_then(Value::as_str)
                .is_none_or(|content| content.chars().count() > 4096)
        {
            return Err(AgentHostError::Authority(
                "session steer content must be at most 4096 characters".to_owned(),
            ));
        }
    }
    match operation {
        "web.search" => {
            let query = payload
                .get("query")
                .and_then(Value::as_str)
                .filter(|query| !query.trim().is_empty())
                .ok_or_else(|| AgentHostError::Authority("search query is required".to_owned()))?;
            if query.chars().count() > 512 {
                return Err(AgentHostError::Authority(
                    "search query is too long".to_owned(),
                ));
            }
            if let Some(limit) = payload.get("limit").and_then(Value::as_u64) {
                if !(1..=10).contains(&limit) {
                    return Err(AgentHostError::Authority(
                        "search result limit must be between 1 and 10".to_owned(),
                    ));
                }
            }
        }
        "project.lifecycle" => {
            let action = payload
                .get("action")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AgentHostError::Authority("Project lifecycle action is required".to_owned())
                })?;
            if !matches!(
                action,
                "create" | "organize" | "pause" | "resume" | "archive"
            ) {
                return Err(AgentHostError::Authority(
                    "Project lifecycle action is not admitted".to_owned(),
                ));
            }
            if matches!(action, "organize" | "pause" | "resume" | "archive")
                && payload
                    .get("project_id")
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Err(AgentHostError::Authority(
                    "project_id is required for this lifecycle action".to_owned(),
                ));
            }
        }
        "handoff.publish" => {
            let target = payload
                .get("target_project_id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AgentHostError::Authority("target_project_id is required".to_owned())
                })?;
            if target.chars().count() > 200 {
                return Err(AgentHostError::Authority(
                    "handoff target is invalid".to_owned(),
                ));
            }
            let content = payload
                .get("content")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AgentHostError::Authority("handoff content is required".to_owned())
                })?;
            if content.chars().count() > 16_384 {
                return Err(AgentHostError::Authority(
                    "handoff content is too long".to_owned(),
                ));
            }
            if let Some(revisions) = payload.get("source_revisions") {
                if serde_json::to_vec(revisions)
                    .map(|bytes| bytes.len() > 16 * 1024)
                    .unwrap_or(true)
                {
                    return Err(AgentHostError::Authority(
                        "handoff source revisions are too large".to_owned(),
                    ));
                }
            }
        }
        _ => {}
    }
    if matches!(
        operation,
        "web.search" | "project.lifecycle" | "handoff.publish"
    ) {
        // The provider persists only guarded action envelopes.  This catches
        // credential-shaped model output before it reaches the action ledger,
        // while retaining the actual content in the protected runtime only.
        let serialized = serde_json::to_string(payload).map_err(|_| {
            AgentHostError::Authority("proposal payload is not serializable".to_owned())
        })?;
        guard_agent_chat_content(&serialized).map_err(|_| {
            AgentHostError::Authority("protected values cannot be proposed".to_owned())
        })?;
    }
    Ok(())
}

fn service_error(error: crate::ServiceError) -> AgentHostError {
    match error {
        crate::ServiceError::NotFound { .. } | crate::ServiceError::Db(db::DbError::NotFound) => {
            AgentHostError::Authority("Forge scope resource is unavailable".to_owned())
        }
        crate::ServiceError::InvalidOperation { message } => AgentHostError::Authority(message),
        _ => AgentHostError::Runtime("Forge coordination operation failed".to_owned()),
    }
}

fn scope_type_name(scope_type: CanonicalScopeType) -> &'static str {
    match scope_type {
        CanonicalScopeType::Account => "account",
        CanonicalScopeType::Project => "project",
        CanonicalScopeType::AgentChat => "agent_chat",
        CanonicalScopeType::Task => "task",
    }
}

fn workspace_access_name(access: WorkspaceAccess) -> &'static str {
    match access {
        WorkspaceAccess::Deny => "deny",
        WorkspaceAccess::TaskRead => "task_read",
        WorkspaceAccess::TaskWrite => "task_write",
    }
}

fn permission_set(value: &str) -> BTreeSet<String> {
    let Ok(value) = serde_json::from_str::<Value>(value) else {
        return BTreeSet::new();
    };
    match value {
        Value::Array(values) => values
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        Value::Object(map) => map
            .get("permissions")
            .or_else(|| map.get("allowed"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
        _ => BTreeSet::new(),
    }
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_targets_are_derived_from_scope() {
        let scope = CanonicalScope {
            scope_type: CanonicalScopeType::Project,
            scope_id: "project-1".to_owned(),
            workspace_access: WorkspaceAccess::Deny,
        };
        let arguments = json!({
            "payload": {"title":"bounded"},
            "dedupe_key":"dedupe",
            "correlation_id":"corr",
        });
        assert_eq!(
            scope_type_name(scope.scope_type),
            "project",
            "the operation target is taken from the canonical scope"
        );
        assert_eq!(arguments["payload"]["title"], "bounded");
    }

    #[test]
    fn session_action_payload_is_bounded_and_allowlisted() {
        assert!(validate_proposal_payload("session.action", &json!({"action":"cancel"}),).is_ok());
        assert!(validate_proposal_payload(
            "session.action",
            &json!({"action":"steer","content":"continue"}),
        )
        .is_ok());
        assert!(
            validate_proposal_payload("session.action", &json!({"action":"execute"}),).is_err()
        );
        assert!(validate_proposal_payload("session.action", &json!({"action":"steer"}),).is_err());
    }
}
