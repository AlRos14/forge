use crate::{DomainEventService, Result, ServiceError};
use async_trait::async_trait;
use db::{
    new_uuid_v4, now_rfc3339, CreateDomainEvent, CreatePrMetadata, GateEvaluationOutcome, GateRepo,
    PrMetadata, PrMetadataRepo, PrProviderConfig, PrProviderConfigRepo, Repo, RepoRepo, SqliteDb,
    Task, TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskLifecycleRepo,
    TaskMetadata, TaskRepo, UpdatePrMetadata,
};
use events::EventBus;
use serde_json::json;
use sqlx::Row;
use std::{path::PathBuf, sync::Arc, time::Duration};

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
    /// Find a PR by its exact repository/head/base tuple. Providers use this
    /// during recovery when Forge persisted publication intent before the
    /// external create call but crashed before storing the provider ID.
    async fn find_pr(&self, request: &PrCreateRequest) -> Result<Option<PrRecord>>;
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
    async fn find_pr(&self, request: &PrCreateRequest) -> Result<Option<PrRecord>> {
        tracing::info!(
            repo = %request.repo_remote_url,
            source_branch = %request.source_branch,
            target_branch = %request.target_branch,
            provider = %self.config.provider_type,
            "placeholder GitHub PR lookup"
        );
        let _ = self.token.len();
        Ok(None)
    }

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
    event_bus: Arc<EventBus>,
}

impl PrService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self { db, event_bus }
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
        let admission_id = operation.parent_operation_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("PR publication has no TaskMerge admission")
        })?;
        let admission = TaskIntegrationOperationRepo::get_by_id(&*self.db, admission_id)
            .await?
            .filter(|admission| {
                admission.task_id == task.id
                    && admission.kind == TaskIntegrationOperationKind::TaskMerge
                    && admission.remote_waiting
                    && admission.status == db::TaskIntegrationOperationStatus::Running
                    && admission.gate_evaluation_id == operation.gate_evaluation_id
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation("PR publication lost its exact TaskMerge admission")
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
            || lifecycle.state != db::TaskLifecycleState::Merging
            || lifecycle.reason_ref.as_deref() != Some(admission.id.as_str())
        {
            return Err(ServiceError::invalid_operation(
                "PR publication does not match its durable merge admission",
            ));
        }
        let config = PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::PrProviderMissing {
                repo_id: repo.id.clone(),
            })?;
        let provider = self.provider_for(&config)?;
        let existing = PrMetadataRepo::get_by_task_id(&*self.db, &task.id).await?;
        if existing
            .as_ref()
            .is_some_and(|metadata| metadata.provider_type != config.provider_type)
        {
            return Err(ServiceError::invalid_operation(
                "PR provider type changed after its durable publication intent",
            ));
        }
        let body = Some(format!("Forge task: {}", task.id));
        let now = now_rfc3339();
        let metadata = if let Some(existing) = existing {
            let is_same_publication = existing.task_merge_operation_id.as_deref()
                == Some(admission.id.as_str())
                && existing.publish_operation_id.as_deref() == Some(operation.id.as_str());
            if existing.merge_status == "legacy_unadmitted" {
                return Err(ServiceError::invalid_operation(
                    "legacy PR has no durable TaskMerge admission and cannot be rebound automatically",
                ));
            }
            if existing.merge_status == "pending" && !is_same_publication {
                return Err(ServiceError::invalid_operation(
                    "another admitted PR is still awaiting its provider outcome",
                ));
            }
            if is_same_publication {
                existing
            } else {
                PrMetadataRepo::update(
                    &*self.db,
                    UpdatePrMetadata {
                        id: existing.id.clone(),
                        provider_type: Some(config.provider_type.clone()),
                        provider_pr_id: Some(None),
                        pr_url: Some(None),
                        source_branch: Some(source_branch.to_owned()),
                        target_branch: Some(target_branch.to_owned()),
                        pr_state: Some("publishing".to_owned()),
                        merge_status: Some("pending".to_owned()),
                        task_merge_operation_id: Some(admission.id.clone()),
                        publish_operation_id: Some(operation.id.clone()),
                        last_synced_at: Some(None),
                        updated_at: now.clone(),
                    },
                )
                .await?
            }
        } else {
            PrMetadataRepo::create(
                &*self.db,
                CreatePrMetadata {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    provider_type: config.provider_type.clone(),
                    provider_pr_id: None,
                    pr_url: None,
                    source_branch: source_branch.to_owned(),
                    target_branch: target_branch.to_owned(),
                    pr_state: "publishing".to_owned(),
                    merge_status: "pending".to_owned(),
                    task_merge_operation_id: admission.id.clone(),
                    publish_operation_id: operation.id.clone(),
                    last_synced_at: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await?
        };
        let create_request = PrCreateRequest {
            repo_remote_url: repo.remote_url.clone(),
            source_branch: source_branch.to_owned(),
            target_branch: target_branch.to_owned(),
            title: task.title.clone(),
            body,
        };
        let record_result: Result<PrRecord> = async {
            if let Some(provider_pr_id) = metadata.provider_pr_id.clone() {
                provider
                    .update_pr(PrUpdateRequest {
                        provider_pr_id,
                        source_branch: source_branch.to_owned(),
                        target_branch: target_branch.to_owned(),
                        title: task.title.clone(),
                        body: create_request.body.clone(),
                    })
                    .await
            } else if let Some(record) = provider.find_pr(&create_request).await? {
                Ok(record)
            } else {
                provider.create_pr(create_request).await
            }
        }
        .await;
        let record = match record_result {
            Ok(record) => record,
            Err(error) => {
                self.record_publication_failure(&task.id, &admission.id, &operation.id)
                    .await?;
                return Err(error);
            }
        };
        let metadata = PrMetadataRepo::update(
            &*self.db,
            UpdatePrMetadata {
                id: metadata.id,
                provider_type: Some(config.provider_type.clone()),
                provider_pr_id: Some(Some(record.provider_pr_id)),
                pr_url: Some(record.pr_url.clone()),
                source_branch: None,
                target_branch: None,
                pr_state: Some(record.state),
                merge_status: Some("pending".to_owned()),
                task_merge_operation_id: None,
                publish_operation_id: None,
                last_synced_at: Some(Some(now_rfc3339())),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        if let Err(error) = set_task_awaiting_human(&self.db, task, true).await {
            tracing::warn!(task_id = %task.id, %error, "could not update legacy awaiting-human projection for PR");
        }
        Ok(PublishedPr {
            pr_url: metadata.pr_url.clone(),
            metadata,
        })
    }

    pub(crate) async fn record_publication_failure(
        &self,
        task_id: &str,
        merge_operation_id: &str,
        publish_operation_id: &str,
    ) -> Result<db::DomainEvent> {
        let metadata = PrMetadataRepo::get_by_task_id(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", task_id.to_owned()))?;
        if metadata.task_merge_operation_id.as_deref() != Some(merge_operation_id)
            || metadata.publish_operation_id.as_deref() != Some(publish_operation_id)
            || !matches!(
                metadata.merge_status.as_str(),
                "pending" | "publication_failed"
            )
        {
            return Err(ServiceError::invalid_operation(
                "publication failure does not match the exact active PR admission",
            ));
        }
        let dedupe_key = format!("pr-status:task-merge:{merge_operation_id}:publication_failed");
        let events = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let event = if let Some(existing) = events.get_by_dedupe(&dedupe_key).await? {
            existing
        } else {
            events
                .append(CreateDomainEvent {
                    id: new_uuid_v4(),
                    event_type: "pr.status_changed".to_owned(),
                    entity_type: "pr_metadata".to_owned(),
                    entity_id: metadata.id.clone(),
                    actor_type: "system".to_owned(),
                    actor_id: None,
                    scope_type: "task".to_owned(),
                    scope_id: task_id.to_owned(),
                    correlation_id: merge_operation_id.to_owned(),
                    causation_id: Some(publish_operation_id.to_owned()),
                    causation_depth: 1,
                    dedupe_key: Some(dedupe_key),
                    payload_json: json!({
                        "task_id": task_id,
                        "pr_metadata_id": metadata.id,
                        "provider_pr_id": metadata.provider_pr_id,
                        "status": "publication_failed",
                        "task_merge_operation_id": merge_operation_id,
                        "publish_operation_id": publish_operation_id,
                    })
                    .to_string(),
                    created_at: now_rfc3339(),
                })
                .await?
        };
        if metadata.merge_status != "publication_failed" {
            PrMetadataRepo::update(
                &*self.db,
                UpdatePrMetadata {
                    id: metadata.id,
                    provider_type: None,
                    provider_pr_id: None,
                    pr_url: None,
                    source_branch: None,
                    target_branch: None,
                    pr_state: Some("failed".to_owned()),
                    merge_status: Some("publication_failed".to_owned()),
                    task_merge_operation_id: None,
                    publish_operation_id: None,
                    last_synced_at: Some(Some(now_rfc3339())),
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
        }
        Ok(event)
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
    integration_operations: crate::task_integration_operation::TaskIntegrationOperationManager,
    interval: Duration,
}

impl PrReconciler {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, interval: Option<Duration>) -> Self {
        let lock_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            integration_operations:
                crate::task_integration_operation::TaskIntegrationOperationManager::new(
                    Arc::clone(&db),
                    lock_root,
                ),
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

    async fn reconcile_metadata(&self, mut metadata: PrMetadata) -> Result<()> {
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
        let merge_operation_id = metadata
            .task_merge_operation_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "PR has no durable TaskMerge admission; provider status cannot create one retroactively",
                )
            })?;
        let publish_operation_id = metadata.publish_operation_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("PR has no exact PublishPr operation provenance")
        })?;
        let merge_operation =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, merge_operation_id)
                .await?
                .filter(|operation| {
                    operation.task_id == task.id
                        && operation.kind == TaskIntegrationOperationKind::TaskMerge
                        && operation.remote_waiting
                })
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "PR TaskMerge admission is missing or out of scope",
                    )
                })?;
        let publish_operation =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, publish_operation_id)
                .await?
                .filter(|operation| {
                    operation.task_id == task.id
                        && operation.kind == TaskIntegrationOperationKind::PublishPr
                        && operation.parent_operation_id.as_deref()
                            == Some(merge_operation.id.as_str())
                        && operation.gate_evaluation_id == merge_operation.gate_evaluation_id
                })
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "PR PublishPr operation is missing or out of scope",
                    )
                })?;
        let evaluation_id = merge_operation
            .gate_evaluation_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("PR TaskMerge has no exact GateEvaluation")
            })?;
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, evaluation_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("GateEvaluation", evaluation_id.to_owned()))?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        if evaluation.task_id != task.id
            || evaluation.outcome != GateEvaluationOutcome::Satisfied
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task.id
        {
            return Err(ServiceError::invalid_operation(
                "PR TaskMerge admission has invalid frozen Gate provenance",
            ));
        }
        let Some(_recovery_lock) = self
            .integration_operations
            .try_pr_recovery_lock(&task.id, &merge_operation.id, &publish_operation.id)
            .await?
        else {
            return Ok(());
        };
        if metadata.merge_status == "publication_failed" {
            self.finish_publication_failure(&task.id, &merge_operation, &publish_operation)
                .await?;
            return Ok(());
        }
        let config = match PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id).await? {
            Some(config) if config.provider_type == metadata.provider_type => config,
            Some(_) | None => {
                self.finish_publication_failure(&task.id, &merge_operation, &publish_operation)
                    .await?;
                return Ok(());
            }
        };
        let provider = match PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .provider_for(&config)
        {
            Ok(provider) => provider,
            Err(_) => {
                self.finish_publication_failure(&task.id, &merge_operation, &publish_operation)
                    .await?;
                return Ok(());
            }
        };
        if metadata.provider_pr_id.is_none() {
            let request = PrCreateRequest {
                repo_remote_url: repo.remote_url.clone(),
                source_branch: metadata.source_branch.clone(),
                target_branch: metadata.target_branch.clone(),
                title: task.title.clone(),
                body: Some(format!("Forge task: {}", task.id)),
            };
            let record_result = async {
                if let Some(record) = provider.find_pr(&request).await? {
                    Ok(record)
                } else {
                    provider.create_pr(request).await
                }
            }
            .await;
            let record = match record_result {
                Ok(record) => record,
                Err(_) => {
                    self.finish_publication_failure(&task.id, &merge_operation, &publish_operation)
                        .await?;
                    return Ok(());
                }
            };
            metadata = PrMetadataRepo::update(
                &*self.db,
                UpdatePrMetadata {
                    id: metadata.id.clone(),
                    provider_type: None,
                    provider_pr_id: Some(Some(record.provider_pr_id)),
                    pr_url: Some(record.pr_url),
                    source_branch: None,
                    target_branch: None,
                    pr_state: Some(record.state),
                    merge_status: None,
                    task_merge_operation_id: None,
                    publish_operation_id: None,
                    last_synced_at: Some(Some(now_rfc3339())),
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
        }
        let now = now_rfc3339();
        match provider.get_pr_status(&metadata).await? {
            RemotePrStatus::Open => {
                let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
                if merge_operation.status != db::TaskIntegrationOperationStatus::Running
                    || lifecycle.state != db::TaskLifecycleState::Merging
                    || lifecycle.reason_ref.as_deref() != Some(merge_operation.id.as_str())
                {
                    return Err(ServiceError::invalid_operation(
                        "open PR does not match the current durable TaskMerge admission",
                    ));
                }
                self.finish_pr_publication(
                    &publish_operation,
                    db::TaskIntegrationOperationStatus::Succeeded,
                    None,
                )
                .await?;
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
                        task_merge_operation_id: None,
                        publish_operation_id: None,
                        last_synced_at: Some(Some(now)),
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                set_task_awaiting_human(&self.db, &task, true).await?;
            }
            RemotePrStatus::Merged => {
                let status_event = self
                    .record_pr_status_event(
                        &metadata,
                        &task,
                        "merged",
                        &merge_operation,
                        &publish_operation,
                    )
                    .await?;
                self.finish_pr_publication(
                    &publish_operation,
                    db::TaskIntegrationOperationStatus::Succeeded,
                    None,
                )
                .await?;
                self.finish_provider_merge(
                    &merge_operation,
                    db::TaskIntegrationOperationStatus::Succeeded,
                    &status_event,
                )
                .await?;
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
                        task_merge_operation_id: None,
                        publish_operation_id: None,
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
                    .record_pr_status_event(
                        &metadata,
                        &task,
                        "closed",
                        &merge_operation,
                        &publish_operation,
                    )
                    .await?;
                self.finish_pr_publication(
                    &publish_operation,
                    db::TaskIntegrationOperationStatus::Succeeded,
                    None,
                )
                .await?;
                self.finish_provider_merge(
                    &merge_operation,
                    db::TaskIntegrationOperationStatus::Failed,
                    &status_event,
                )
                .await?;
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
                        task_merge_operation_id: None,
                        publish_operation_id: None,
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

    async fn finish_pr_publication(
        &self,
        operation: &db::TaskIntegrationOperation,
        status: db::TaskIntegrationOperationStatus,
        result_event_id: Option<String>,
    ) -> Result<()> {
        if operation.status == status && operation.result_event_id == result_event_id {
            return Ok(());
        }
        if operation.status != db::TaskIntegrationOperationStatus::Running {
            return Err(ServiceError::invalid_operation(
                "PR publication already finished with a different outcome",
            ));
        }
        TaskIntegrationOperationRepo::finish(
            &*self.db,
            db::FinishTaskIntegrationOperation {
                id: operation.id.clone(),
                expected_version: operation.version,
                status,
                result_event_id,
                updated_at: now_rfc3339(),
                finished_at: now_rfc3339(),
            },
        )
        .await?;
        Ok(())
    }

    async fn finish_publication_failure(
        &self,
        task_id: &str,
        merge_operation: &db::TaskIntegrationOperation,
        publish_operation: &db::TaskIntegrationOperation,
    ) -> Result<()> {
        let event = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .record_publication_failure(task_id, &merge_operation.id, &publish_operation.id)
            .await?;
        self.finish_pr_publication(
            publish_operation,
            db::TaskIntegrationOperationStatus::Failed,
            Some(event.id),
        )
        .await?;
        let events = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        events
            .publish_by_dedupe(&format!("task-merge-admission:{}", merge_operation.id))
            .await?;
        events
            .publish_by_dedupe(&format!("task-merge-terminal:{}", merge_operation.id))
            .await?;
        Ok(())
    }

    async fn finish_provider_merge(
        &self,
        operation: &db::TaskIntegrationOperation,
        status: db::TaskIntegrationOperationStatus,
        result_event: &db::DomainEvent,
    ) -> Result<()> {
        TaskIntegrationOperationRepo::finish(
            &*self.db,
            db::FinishTaskIntegrationOperation {
                id: operation.id.clone(),
                expected_version: operation.version,
                status,
                result_event_id: Some(result_event.id.clone()),
                updated_at: now_rfc3339(),
                finished_at: now_rfc3339(),
            },
        )
        .await?;
        let events = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        events
            .publish_by_dedupe(&format!("task-merge-admission:{}", operation.id))
            .await?;
        events
            .publish_by_dedupe(&format!("task-merge-terminal:{}", operation.id))
            .await?;
        Ok(())
    }

    async fn record_pr_status_event(
        &self,
        metadata: &PrMetadata,
        task: &Task,
        status: &str,
        merge_operation: &db::TaskIntegrationOperation,
        publish_operation: &db::TaskIntegrationOperation,
    ) -> Result<db::DomainEvent> {
        let service = DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let dedupe_key = format!("pr-status:task-merge:{}:{status}", merge_operation.id);
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
                    "task_merge_operation_id": merge_operation.id,
                    "publish_operation_id": publish_operation.id,
                })
                .to_string(),
                created_at: now_rfc3339(),
            })
            .await
    }
}

async fn pending_pr_metadata(db: &SqliteDb) -> Result<Vec<PrMetadata>> {
    let rows = sqlx::query(
        "SELECT metadata.task_id FROM pr_metadata metadata
         JOIN task_integration_operation merge_op
           ON merge_op.id = metadata.task_merge_operation_id
         JOIN task_integration_operation publish_op
           ON publish_op.id = metadata.publish_operation_id
         WHERE metadata.merge_status IN ('pending', 'publication_failed')
           AND merge_op.status = 'running'
           AND publish_op.status IN ('running', 'succeeded')",
    )
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
