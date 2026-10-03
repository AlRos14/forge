use crate::{
    task_integration_operation::TaskIntegrationOperationManager,
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    DomainEventService, Result, ServiceError,
};
use async_trait::async_trait;
use db::{
    new_uuid_v4, now_rfc3339, CreateDomainEvent, CreatePrMetadata, GateEvaluationOutcome, GateRepo,
    PrMetadata, PrMetadataRepo, PrProviderConfig, PrProviderConfigRepo, Repo, RepoRepo, SqliteDb,
    Task, TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskLifecycleRepo,
    TaskLifecycleState, TaskMetadata, TaskRepo, UpdatePrMetadata,
};
use events::EventBus;
use serde_json::json;
use sqlx::Row;
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCreateRequest {
    pub repo_remote_url: String,
    pub source_branch: String,
    pub target_branch: String,
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrUpdateRequest {
    pub provider_pr_id: String,
    pub source_branch: String,
    pub target_branch: String,
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRecord {
    pub provider_pr_id: String,
    pub pr_url: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemotePrStatus {
    Open,
    Merged,
    Closed,
}

#[async_trait]
pub trait PrProvider: Send + Sync {
    async fn create_pr(&self, request: PrCreateRequest) -> Result<PrRecord>;
    async fn update_pr(&self, request: PrUpdateRequest) -> Result<PrRecord>;
    async fn get_pr_status(&self, metadata: &PrMetadata) -> Result<RemotePrStatus>;
    async fn close_pr(&self, metadata: &PrMetadata) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct GitHubPrProvider {
    config: PrProviderConfig,
    token: String,
}

impl GitHubPrProvider {
    pub fn new(config: PrProviderConfig, token: String) -> Self {
        Self { config, token }
    }
}

#[async_trait]
impl PrProvider for GitHubPrProvider {
    async fn create_pr(&self, request: PrCreateRequest) -> Result<PrRecord> {
        tracing::info!(
            repo = %request.repo_remote_url,
            source_branch = %request.source_branch,
            target_branch = %request.target_branch,
            provider = %self.config.provider_type,
            "placeholder GitHub PR create"
        );
        let _ = self.token.len();
        Ok(PrRecord {
            provider_pr_id: format!("placeholder-{}", request.source_branch),
            pr_url: Some(format!(
                "{}/pull/{}",
                self.config
                    .base_url
                    .as_deref()
                    .unwrap_or("https://github.com/forge-placeholder"),
                request.source_branch
            )),
            state: "open".to_owned(),
        })
    }

    async fn update_pr(&self, request: PrUpdateRequest) -> Result<PrRecord> {
        tracing::info!(
            provider_pr_id = %request.provider_pr_id,
            source_branch = %request.source_branch,
            target_branch = %request.target_branch,
            provider = %self.config.provider_type,
            "placeholder GitHub PR update"
        );
        let _ = self.token.len();
        Ok(PrRecord {
            provider_pr_id: request.provider_pr_id,
            pr_url: Some(format!(
                "{}/pull/{}",
                self.config
                    .base_url
                    .as_deref()
                    .unwrap_or("https://github.com/forge-placeholder"),
                request.source_branch
            )),
            state: "open".to_owned(),
        })
    }

    async fn get_pr_status(&self, metadata: &PrMetadata) -> Result<RemotePrStatus> {
        tracing::info!(
            task_id = %metadata.task_id,
            provider_pr_id = ?metadata.provider_pr_id,
            provider = %self.config.provider_type,
            "placeholder GitHub PR status read"
        );
        let _ = self.token.len();
        Ok(match metadata.pr_state.as_str() {
            "merged" => RemotePrStatus::Merged,
            "closed" => RemotePrStatus::Closed,
            _ => RemotePrStatus::Open,
        })
    }

    async fn close_pr(&self, metadata: &PrMetadata) -> Result<()> {
        tracing::info!(
            task_id = %metadata.task_id,
            provider_pr_id = ?metadata.provider_pr_id,
            provider = %self.config.provider_type,
            "placeholder GitHub PR close"
        );
        let _ = self.token.len();
        Ok(())
    }
}

pub struct PublishedPr {
    pub metadata: PrMetadata,
    pub pr_url: Option<String>,
}

#[derive(Clone)]
pub struct PrService {
    db: Arc<SqliteDb>,
}

impl PrService {
    pub fn new(db: Arc<SqliteDb>) -> Self {
        Self { db }
    }

    pub(crate) async fn publish_pr(
        &self,
        task: &Task,
        repo: &Repo,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<PublishedPr> {
        let operation = TaskIntegrationOperationRepo::get_active_for_task(&*self.db, &task.id)
            .await?
            .filter(|operation| {
                operation.kind == TaskIntegrationOperationKind::PublishPr
                    && operation.gate_evaluation_id.is_some()
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "PR publication requires an active exact Gate-authorized operation",
                )
            })?;
        let evaluation_id = operation
            .gate_evaluation_id
            .as_deref()
            .expect("checked above");
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, evaluation_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("GateEvaluation", evaluation_id.to_owned()))?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if evaluation.task_id != task.id
            || evaluation.outcome != GateEvaluationOutcome::Satisfied
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task.id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
            || lifecycle.state != TaskLifecycleState::ReadyToMerge
        {
            return Err(ServiceError::invalid_operation(
                "PR publication GateEvaluation is stale, unsatisfied, or out of scope",
            ));
        }
        let config = PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::PrProviderMissing {
                repo_id: repo.id.clone(),
            })?;
        let provider = self.provider_for(&config)?;
        let existing = PrMetadataRepo::get_by_task_id(&*self.db, &task.id).await?;
        let body = Some(format!("Forge task: {}", task.id));
        let record = if let Some(metadata) = existing.as_ref() {
            if let Some(provider_pr_id) = metadata.provider_pr_id.clone() {
                provider
                    .update_pr(PrUpdateRequest {
                        provider_pr_id,
                        source_branch: source_branch.to_owned(),
                        target_branch: target_branch.to_owned(),
                        title: task.title.clone(),
                        body: body.clone(),
                    })
                    .await?
            } else {
                provider
                    .create_pr(PrCreateRequest {
                        repo_remote_url: repo.remote_url.clone(),
                        source_branch: source_branch.to_owned(),
                        target_branch: target_branch.to_owned(),
                        title: task.title.clone(),
                        body: body.clone(),
                    })
                    .await?
            }
        } else {
            provider
                .create_pr(PrCreateRequest {
                    repo_remote_url: repo.remote_url.clone(),
                    source_branch: source_branch.to_owned(),
                    target_branch: target_branch.to_owned(),
                    title: task.title.clone(),
                    body: body.clone(),
                })
                .await?
        };

        let now = now_rfc3339();
        let metadata = if let Some(existing) = existing {
            PrMetadataRepo::update(
                &*self.db,
                UpdatePrMetadata {
                    id: existing.id,
                    provider_type: Some(config.provider_type.clone()),
                    provider_pr_id: Some(Some(record.provider_pr_id)),
                    pr_url: Some(record.pr_url.clone()),
                    source_branch: Some(source_branch.to_owned()),
                    target_branch: Some(target_branch.to_owned()),
                    pr_state: Some(record.state),
                    merge_status: Some("pending".to_owned()),
                    last_synced_at: Some(Some(now.clone())),
                    updated_at: now.clone(),
                },
            )
            .await?
        } else {
            PrMetadataRepo::create(
                &*self.db,
                CreatePrMetadata {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    provider_type: config.provider_type.clone(),
                    provider_pr_id: Some(record.provider_pr_id),
                    pr_url: record.pr_url.clone(),
                    source_branch: source_branch.to_owned(),
                    target_branch: target_branch.to_owned(),
                    pr_state: record.state,
                    merge_status: "pending".to_owned(),
                    last_synced_at: Some(now.clone()),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await?
        };
        set_task_awaiting_human(&self.db, task, true).await?;
        Ok(PublishedPr {
            pr_url: metadata.pr_url.clone(),
            metadata,
        })
    }

    fn provider_for(&self, config: &PrProviderConfig) -> Result<Box<dyn PrProvider>> {
        let token =
            resolve_token_secret(config)?.ok_or_else(|| ServiceError::PrProviderTokenMissing {
                repo_id: config.repo_id.clone(),
            })?;
        match config.provider_type.as_str() {
            "github" => Ok(Box::new(GitHubPrProvider::new(config.clone(), token))),
            provider_type => Err(ServiceError::invalid_operation(format!(
                "unsupported PR provider type: {provider_type}"
            ))),
        }
    }
}

pub struct PrReconciler {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    interval: Duration,
}

impl PrReconciler {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, interval: Option<Duration>) -> Self {
        Self {
            db,
            event_bus,
            interval: interval.unwrap_or(Duration::from_secs(60)),
        }
    }

    pub fn run(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.interval);
            loop {
                ticker.tick().await;
                if let Err(error) = self.reconcile_once().await {
                    tracing::warn!(%error, "PR reconciliation pass failed");
                }
            }
        })
    }

    pub async fn reconcile_once(&self) -> Result<()> {
        for metadata in pending_pr_metadata(&self.db).await? {
            if let Err(error) = self.reconcile_metadata(metadata).await {
                tracing::warn!(%error, "PR metadata reconciliation failed");
            }
        }
        Ok(())
    }

    async fn reconcile_metadata(&self, metadata: PrMetadata) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, &metadata.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", metadata.task_id.clone()))?;
        let repo_id = task
            .repo_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        let config = PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::PrProviderMissing {
                repo_id: repo.id.clone(),
            })?;
        let provider = PrService::new(Arc::clone(&self.db)).provider_for(&config)?;
        let now = now_rfc3339();
        match provider.get_pr_status(&metadata).await? {
            RemotePrStatus::Open => {
                PrMetadataRepo::update(
                    &*self.db,
                    UpdatePrMetadata {
                        id: metadata.id,
                        provider_type: None,
                        provider_pr_id: None,
                        pr_url: None,
                        source_branch: None,
                        target_branch: None,
                        pr_state: Some("open".to_owned()),
                        merge_status: Some("pending".to_owned()),
                        last_synced_at: Some(Some(now)),
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
            }
            RemotePrStatus::Merged => {
                let status_event = self
                    .record_pr_status_event(&metadata, &task, "merged")
                    .await?;
                let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
                match lifecycle.state {
                    TaskLifecycleState::Done => {}
                    TaskLifecycleState::ReadyToMerge | TaskLifecycleState::Merging => {
                        let gate_evaluation_id =
                            lifecycle.reason_ref.as_deref().ok_or_else(|| {
                                ServiceError::invalid_operation(
                                    "PR merge readiness has no exact GateEvaluation reference",
                                )
                            })?;
                        let manager = TaskIntegrationOperationManager::new(
                            Arc::clone(&self.db),
                            std::env::temp_dir().join("forge-pr-reconciler"),
                        );
                        let operation = manager
                            .record_provider_confirmed_merge(
                                &task.id,
                                &status_event.id,
                                gate_evaluation_id,
                            )
                            .await?;
                        let events = DomainEventService::new(
                            Arc::clone(&self.db),
                            Arc::clone(&self.event_bus),
                        );
                        let _ = events
                            .publish_by_dedupe(&format!("task-merge-admission:{}", operation.id))
                            .await?;
                        let _ = events
                            .publish_by_dedupe(&format!("task-merge-terminal:{}", operation.id))
                            .await?;
                    }
                    _ => {
                        return Err(ServiceError::invalid_operation(
                            "provider confirmed a merged PR without an admitted merge-readiness Gate",
                        ));
                    }
                }
                PrMetadataRepo::update(
                    &*self.db,
                    UpdatePrMetadata {
                        id: metadata.id,
                        provider_type: None,
                        provider_pr_id: None,
                        pr_url: None,
                        source_branch: None,
                        target_branch: None,
                        pr_state: Some("merged".to_owned()),
                        merge_status: Some("merged".to_owned()),
                        last_synced_at: Some(Some(now.clone())),
                        updated_at: now.clone(),
                    },
                )
                .await?;
                if let Some(updated) = TaskRepo::get_by_id(&*self.db, &task.id, false).await? {
                    set_task_awaiting_human(&self.db, &updated, false).await?;
                }
            }
            RemotePrStatus::Closed => {
                let status_event = self
                    .record_pr_status_event(&metadata, &task, "closed")
                    .await?;
                let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
                if lifecycle.state != TaskLifecycleState::Blocked {
                    let expected_task_version =
                        serde_json::from_str::<serde_json::Value>(&status_event.payload_json)
                            .ok()
                            .and_then(|payload| {
                                payload.get("task_version").and_then(|v| v.as_i64())
                            })
                            .ok_or_else(|| {
                                ServiceError::invalid_operation(
                                    "PR status fact has no original Task version",
                                )
                            })?;
                    TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                        .transition(TransitionLifecycleInput {
                            task_id: task.id.clone(),
                            expected_task_version,
                            to_state: TaskLifecycleState::Blocked,
                            cause: LifecycleCause::DomainEvent(status_event.id.clone()),
                            reason_kind: Some("pr_closed_without_merge".to_owned()),
                            reason_ref: Some(metadata.id.clone()),
                            idempotency_key: format!("pr-status:{}:closed", metadata.id),
                        })
                        .await?;
                }
                let blocked = json!({
                    "kind": api_types::FailureKind::PrClosedWithoutMerge,
                    "reason": "pull request was closed without merge",
                    "provider_pr_id": metadata.provider_pr_id,
                    "pr_url": metadata.pr_url,
                    "recovery_actions": ["return_to_implementation", "retry_pr_publication", "cancel_task"],
                    "blocked_at": now,
                });
                let current = TaskRepo::get_by_id(&*self.db, &task.id, false)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
                let updated = TaskRepo::update_status(
                    &*self.db,
                    db::UpdateTaskStatus {
                        id: current.id.clone(),
                        expected_version: current.version,
                        status: "blocked".to_owned(),
                        assignee_id: Some(None),
                        error_annotation: None,
                        blocked_json: Some(Some(blocked.to_string())),
                        failed_json: Some(None),
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                self.publish_task_status_event(&updated).await;
                PrMetadataRepo::update(
                    &*self.db,
                    UpdatePrMetadata {
                        id: metadata.id,
                        provider_type: None,
                        provider_pr_id: None,
                        pr_url: None,
                        source_branch: None,
                        target_branch: None,
                        pr_state: Some("closed".to_owned()),
                        merge_status: Some("closed_without_merge".to_owned()),
                        last_synced_at: Some(Some(now.clone())),
                        updated_at: now,
                    },
                )
                .await?;
                set_task_awaiting_human(
                    &self.db,
                    &TaskRepo::get_by_id(&*self.db, &task.id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?,
                    false,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn publish_task_status_event(&self, task: &Task) {
        let service = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let dedupe_key = format!("task-status-update:{}:{}", task.id, task.version);
        if let Err(error) = service.publish_by_dedupe(&dedupe_key).await {
            tracing::warn!(task_id = %task.id, %error, "failed to mirror PR task status domain event");
        }
    }

    async fn record_pr_status_event(
        &self,
        metadata: &PrMetadata,
        task: &Task,
        status: &str,
    ) -> Result<db::DomainEvent> {
        let service = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let dedupe_key = format!("pr-status:{}:{status}", metadata.id);
        if let Some(existing) = service.get_by_dedupe(&dedupe_key).await? {
            return Ok(existing);
        }
        service
            .append(CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "pr.status_changed".to_owned(),
                entity_type: "pr_metadata".to_owned(),
                entity_id: metadata.id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task.id.clone(),
                correlation_id: metadata.id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(dedupe_key),
                payload_json: json!({
                    "task_id": task.id,
                    "pr_metadata_id": metadata.id,
                    "provider_pr_id": metadata.provider_pr_id,
                    "status": status,
                    "task_version": task.version,
                })
                .to_string(),
                created_at: now_rfc3339(),
            })
            .await
    }
}

async fn pending_pr_metadata(db: &SqliteDb) -> Result<Vec<PrMetadata>> {
    let rows = sqlx::query("SELECT task_id FROM pr_metadata WHERE merge_status = 'pending'")
        .fetch_all(db.pool())
        .await?;
    let mut metadata = Vec::with_capacity(rows.len());
    for row in rows {
        let task_id: String = row.try_get("task_id")?;
        if let Some(item) = PrMetadataRepo::get_by_task_id(db, &task_id).await? {
            metadata.push(item);
        }
    }
    Ok(metadata)
}

fn resolve_token_secret(config: &PrProviderConfig) -> Result<Option<String>> {
    let Some(secret_ref) = config
        .token_secret_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(std::env::var(secret_ref).ok())
}

async fn set_task_awaiting_human(db: &SqliteDb, task: &Task, awaiting_human: bool) -> Result<()> {
    let mut metadata = TaskMetadata::parse(task.metadata_json.as_deref()).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid task metadata: {error}"))
    })?;
    metadata
        .extra
        .insert("awaiting_human".to_owned(), json!(awaiting_human));
    if awaiting_human {
        metadata.extra.insert(
            "awaiting_human_reason".to_owned(),
            json!("pull_request_merge"),
        );
    } else {
        metadata.extra.remove("awaiting_human_reason");
    }
    TaskRepo::set_metadata_json(db, &task.id, metadata.to_json(), &now_rfc3339()).await?;
    Ok(())
}
