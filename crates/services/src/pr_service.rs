use crate::{DomainEventService, Result, ServiceError};
use async_trait::async_trait;
use db::{
    now_rfc3339, GateEvaluationOutcome, GateRepo, PrMetadata, PrMetadataRepo, RemotePrAdmission,
    Repo, SqliteDb, Task, TaskIntegrationOperationKind, TaskIntegrationOperationRepo,
    TaskLifecycleRepo, TaskLifecycleState, TaskMetadata, TaskRepo,
};
use events::EventBus;
use serde_json::json;
use sqlx::Row;
use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrCreateRequest {
    pub repo_remote_url: String,
    pub source_branch: String,
    pub target_branch: String,
    pub source_sha: String,
    /// The provider must use this key to make repeated create calls for one
    /// durable admission converge on one PR.
    pub idempotency_key: String,
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrObservation {
    pub provider_event_id: String,
    pub remote_repo_identity: String,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: String,
    pub merged_commit_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemotePrStatus {
    Open(PrObservation),
    Merged(PrObservation),
    Closed(PrObservation),
}

impl RemotePrStatus {
    fn state(&self) -> &'static str {
        match self {
            Self::Open(_) => "open",
            Self::Merged(_) => "merged",
            Self::Closed(_) => "closed",
        }
    }

    fn observation(&self) -> &PrObservation {
        match self {
            Self::Open(observation) | Self::Merged(observation) | Self::Closed(observation) => {
                observation
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRecord {
    pub provider_pr_id: String,
    pub pr_url: Option<String>,
    pub status: RemotePrStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrProviderError {
    /// The provider explicitly rejected creation and guarantees that it did
    /// not create a PR.
    DefinitiveRejection(String),
    /// The request may have taken effect, but its response did not arrive.
    OutcomeUnknown(String),
    /// The frozen provider identity cannot currently be used.
    Unavailable(String),
}

impl fmt::Display for PrProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefinitiveRejection(reason) => {
                write!(f, "definitive provider rejection: {reason}")
            }
            Self::OutcomeUnknown(reason) => write!(f, "provider outcome unknown: {reason}"),
            Self::Unavailable(reason) => write!(f, "provider unavailable: {reason}"),
        }
    }
}

#[async_trait]
pub trait PrProvider: Send + Sync {
    /// Find by the frozen repository, source branch, target branch, and SHA.
    /// Recovery calls this before repeating create with the same idempotency
    /// key.
    async fn find_pr(
        &self,
        request: &PrCreateRequest,
    ) -> std::result::Result<Option<PrRecord>, PrProviderError>;
    async fn create_pr(
        &self,
        request: PrCreateRequest,
    ) -> std::result::Result<PrRecord, PrProviderError>;
    async fn get_pr_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
    ) -> std::result::Result<RemotePrStatus, PrProviderError>;
}

#[derive(Debug, Clone)]
pub struct GitHubPrProvider {
    provider_type: String,
    base_url: Option<String>,
    token: String,
}

impl GitHubPrProvider {
    fn new(provider_type: String, base_url: Option<String>, token: String) -> Self {
        Self {
            provider_type,
            base_url,
            token,
        }
    }
}

#[async_trait]
impl PrProvider for GitHubPrProvider {
    async fn find_pr(
        &self,
        request: &PrCreateRequest,
    ) -> std::result::Result<Option<PrRecord>, PrProviderError> {
        tracing::info!(
            repo = %request.repo_remote_url,
            source_branch = %request.source_branch,
            target_branch = %request.target_branch,
            source_sha = %request.source_sha,
            provider = %self.provider_type,
            "placeholder GitHub PR lookup"
        );
        let _ = self.token.len();
        Ok(None)
    }

    async fn create_pr(
        &self,
        request: PrCreateRequest,
    ) -> std::result::Result<PrRecord, PrProviderError> {
        tracing::info!(
            repo = %request.repo_remote_url,
            source_branch = %request.source_branch,
            target_branch = %request.target_branch,
            source_sha = %request.source_sha,
            provider = %self.provider_type,
            "placeholder GitHub PR create"
        );
        let _ = self.token.len();
        Ok(PrRecord {
            provider_pr_id: format!("placeholder-{}", request.idempotency_key),
            pr_url: Some(format!(
                "{}/pull/{}",
                self.base_url
                    .as_deref()
                    .unwrap_or("https://github.com/forge-placeholder"),
                request.idempotency_key
            )),
            status: RemotePrStatus::Open(PrObservation {
                provider_event_id: format!("placeholder-create:{}", request.idempotency_key),
                remote_repo_identity: request.repo_remote_url,
                source_branch: request.source_branch,
                target_branch: request.target_branch,
                head_sha: request.source_sha,
                merged_commit_sha: None,
            }),
        })
    }

    async fn get_pr_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
    ) -> std::result::Result<RemotePrStatus, PrProviderError> {
        tracing::info!(
            task_id = %metadata.task_id,
            provider_pr_id = ?metadata.provider_pr_id,
            provider = %self.provider_type,
            "placeholder GitHub PR status read"
        );
        let _ = self.token.len();
        let observation = PrObservation {
            provider_event_id: format!(
                "placeholder-status:{}:{}",
                metadata.provider_pr_id.as_deref().unwrap_or("unknown"),
                metadata.pr_state
            ),
            remote_repo_identity: admission.remote_repo_identity.clone(),
            source_branch: admission.source_branch.clone(),
            target_branch: admission.target_branch.clone(),
            head_sha: admission.admitted_source_sha.clone(),
            merged_commit_sha: (metadata.pr_state == "merged")
                .then(|| format!("placeholder-merge:{}", admission.admitted_source_sha)),
        };
        Ok(match metadata.pr_state.as_str() {
            "merged" => RemotePrStatus::Merged(observation),
            "closed" => RemotePrStatus::Closed(observation),
            _ => RemotePrStatus::Open(observation),
        })
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
    #[cfg(test)]
    provider_override: Option<Arc<dyn PrProvider>>,
}

impl PrService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            db,
            event_bus,
            #[cfg(test)]
            provider_override: None,
        }
    }

    pub(crate) async fn publish_pr(
        &self,
        task: &Task,
        _repo: &Repo,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<PublishedPr> {
        let publication = TaskIntegrationOperationRepo::get_active_for_task(&*self.db, &task.id)
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
        let merge_id = publication.parent_operation_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("PR publication has no TaskMerge admission")
        })?;
        let admission = TaskIntegrationOperationRepo::get_remote_pr_admission(&*self.db, merge_id)
            .await?
            .ok_or_else(|| ServiceError::invalid_operation("remote PR admission is missing"))?;
        let metadata = PrMetadataRepo::get_by_task_id(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", task.id.clone()))?;
        self.validate_admission(task, &admission, &metadata, &publication.id)
            .await?;
        if admission.source_branch != source_branch || admission.target_branch != target_branch {
            return Err(ServiceError::invalid_operation(
                "PR publication arguments differ from the frozen remote admission",
            ));
        }

        let provider = match self.provider_for(&admission) {
            Ok(provider) => provider,
            Err(error) => {
                self.mark_reconciliation_required(&admission, Some(&metadata), &error.to_string())
                    .await?;
                return Err(ServiceError::conflict(
                    "remote PR provider is unavailable; the existing admission remains in reconciliation",
                ));
            }
        };
        let request = create_request(&admission, task);
        let (record, create_attempted) = find_or_create(provider.as_ref(), request).await;
        match record {
            Ok(record) => {
                self.apply_record(&admission, &metadata, record).await?;
                let metadata = PrMetadataRepo::get_by_task_id(&*self.db, &task.id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("PR metadata", task.id.clone()))?;
                let status = metadata.pr_state.as_str();
                if status == "open" || metadata.admission_status == "reconciliation_required" {
                    set_task_awaiting_human_best_effort(&self.db, task, true).await;
                } else if status == "merged" || status == "closed" {
                    set_task_awaiting_human_best_effort(&self.db, task, false).await;
                }
                Ok(PublishedPr {
                    pr_url: metadata.pr_url.clone(),
                    metadata,
                })
            }
            Err(PrProviderError::DefinitiveRejection(reason)) if create_attempted => {
                self.apply_publication_failure(&admission, &metadata, &reason)
                    .await?;
                Err(ServiceError::invalid_operation(format!(
                    "provider definitively rejected PR creation: {reason}"
                )))
            }
            Err(error) => {
                self.mark_reconciliation_required(&admission, Some(&metadata), &error.to_string())
                    .await?;
                set_task_awaiting_human_best_effort(&self.db, task, true).await;
                Err(ServiceError::conflict(
                    "remote PR outcome is not yet known; the same admission remains active for reconciliation",
                ))
            }
        }
    }

    async fn validate_admission(
        &self,
        task: &Task,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        publish_operation_id: &str,
    ) -> Result<()> {
        let merge =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, &admission.task_merge_operation_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "TaskMerge admission",
                        admission.task_merge_operation_id.clone(),
                    )
                })?;
        let publish =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, &admission.publish_operation_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "PublishPr operation",
                        admission.publish_operation_id.clone(),
                    )
                })?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if admission.task_id != task.id
            || metadata.task_id != task.id
            || metadata.id != admission.metadata_id
            || metadata.task_merge_operation_id.as_deref()
                != Some(admission.task_merge_operation_id.as_str())
            || metadata.publish_operation_id.as_deref()
                != Some(admission.publish_operation_id.as_str())
            || publish_operation_id != admission.publish_operation_id
            || merge.task_id != task.id
            || merge.kind != TaskIntegrationOperationKind::TaskMerge
            || !merge.remote_waiting
            || merge.status != db::TaskIntegrationOperationStatus::Running
            || publish.task_id != task.id
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.parent_operation_id.as_deref() != Some(merge.id.as_str())
            || publish.gate_evaluation_id != merge.gate_evaluation_id
            || lifecycle.state != TaskLifecycleState::Merging
            || lifecycle.reason_ref.as_deref() != Some(merge.id.as_str())
        {
            return Err(ServiceError::invalid_operation(
                "PR publication does not match its exact durable remote admission",
            ));
        }
        let evaluation_id = merge.gate_evaluation_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("PR TaskMerge has no frozen GateEvaluation")
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
                "remote PR admission has invalid exact Gate provenance",
            ));
        }
        Ok(())
    }

    async fn apply_record(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        record: PrRecord,
    ) -> Result<()> {
        let observation = record.status.observation();
        if observation.remote_repo_identity != admission.remote_repo_identity
            || observation.source_branch != admission.source_branch
            || observation.target_branch != admission.target_branch
        {
            self.mark_reconciliation_required(
                admission,
                Some(metadata),
                "provider PR identity differs from the frozen repository or branches",
            )
            .await?;
            return Err(ServiceError::conflict(
                "provider PR identity differs from the frozen admission; reconciliation remains required",
            ));
        }
        let input = db::RecordRemotePrOutcome {
            expected_task_id: admission.task_id.clone(),
            task_merge_operation_id: admission.task_merge_operation_id.clone(),
            publish_operation_id: admission.publish_operation_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: observation.remote_repo_identity.clone(),
            source_branch: observation.source_branch.clone(),
            target_branch: observation.target_branch.clone(),
            status: record.status.state().to_owned(),
            provider_event_id: Some(observation.provider_event_id.clone()),
            provider_pr_id: Some(record.provider_pr_id),
            pr_url: record.pr_url,
            observed_head_sha: Some(observation.head_sha.clone()),
            merged_commit_sha: observation.merged_commit_sha.clone(),
            reconciliation_reason: None,
            updated_at: now_rfc3339(),
        };
        self.persist_outcome(input).await
    }

    async fn apply_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        status: RemotePrStatus,
    ) -> Result<()> {
        let observation = status.observation();
        if observation.remote_repo_identity != admission.remote_repo_identity
            || observation.source_branch != admission.source_branch
            || observation.target_branch != admission.target_branch
        {
            self.mark_reconciliation_required(
                admission,
                Some(metadata),
                "provider status identity differs from the frozen repository or branches",
            )
            .await?;
            return Ok(());
        }
        self.persist_outcome(db::RecordRemotePrOutcome {
            expected_task_id: admission.task_id.clone(),
            task_merge_operation_id: admission.task_merge_operation_id.clone(),
            publish_operation_id: admission.publish_operation_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: observation.remote_repo_identity.clone(),
            source_branch: observation.source_branch.clone(),
            target_branch: observation.target_branch.clone(),
            status: status.state().to_owned(),
            provider_event_id: Some(observation.provider_event_id.clone()),
            provider_pr_id: metadata.provider_pr_id.clone(),
            pr_url: metadata.pr_url.clone(),
            observed_head_sha: Some(observation.head_sha.clone()),
            merged_commit_sha: observation.merged_commit_sha.clone(),
            reconciliation_reason: None,
            updated_at: now_rfc3339(),
        })
        .await
    }

    async fn apply_publication_failure(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        reason: &str,
    ) -> Result<()> {
        self.persist_outcome(db::RecordRemotePrOutcome {
            expected_task_id: admission.task_id.clone(),
            task_merge_operation_id: admission.task_merge_operation_id.clone(),
            publish_operation_id: admission.publish_operation_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: admission.remote_repo_identity.clone(),
            source_branch: admission.source_branch.clone(),
            target_branch: admission.target_branch.clone(),
            status: "publication_failed".to_owned(),
            provider_event_id: None,
            provider_pr_id: metadata.provider_pr_id.clone(),
            pr_url: metadata.pr_url.clone(),
            observed_head_sha: None,
            merged_commit_sha: None,
            reconciliation_reason: Some(reason.to_owned()),
            updated_at: now_rfc3339(),
        })
        .await
    }

    async fn mark_reconciliation_required(
        &self,
        admission: &RemotePrAdmission,
        metadata: Option<&PrMetadata>,
        reason: &str,
    ) -> Result<()> {
        self.persist_outcome(db::RecordRemotePrOutcome {
            expected_task_id: admission.task_id.clone(),
            task_merge_operation_id: admission.task_merge_operation_id.clone(),
            publish_operation_id: admission.publish_operation_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: admission.remote_repo_identity.clone(),
            source_branch: admission.source_branch.clone(),
            target_branch: admission.target_branch.clone(),
            status: "reconciliation_required".to_owned(),
            provider_event_id: None,
            provider_pr_id: metadata.and_then(|metadata| metadata.provider_pr_id.clone()),
            pr_url: metadata.and_then(|metadata| metadata.pr_url.clone()),
            observed_head_sha: None,
            merged_commit_sha: None,
            reconciliation_reason: Some(reason.to_owned()),
            updated_at: now_rfc3339(),
        })
        .await
    }

    async fn persist_outcome(&self, input: db::RecordRemotePrOutcome) -> Result<()> {
        let provider_event_key =
            input
                .provider_event_id
                .as_deref()
                .unwrap_or(if input.status == "publication_failed" {
                    "definitive-publication-rejection"
                } else {
                    ""
                });
        let dedupe_key = (!provider_event_key.is_empty()).then(|| {
            format!(
                "remote-pr-result:{}:{provider_event_key}",
                input.task_merge_operation_id
            )
        });
        let terminal = matches!(
            input.status.as_str(),
            "merged" | "closed" | "publication_failed"
        );
        let event =
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*self.db, input).await?;
        if let Some(event) = event.as_ref() {
            let event_service =
                DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
            if let Some(dedupe_key) = dedupe_key.as_deref() {
                if let Err(error) = event_service.publish_by_dedupe(dedupe_key).await {
                    tracing::warn!(%error, "could not publish durable remote PR event hint");
                }
            }
            if terminal {
                for key in [
                    format!("task-merge-admission:{}", event.correlation_id),
                    format!("task-merge-terminal:{}", event.correlation_id),
                ] {
                    if let Err(error) = event_service.publish_by_dedupe(&key).await {
                        tracing::warn!(%error, "could not publish TaskMerge event hint");
                    }
                }
            }
        }
        Ok(())
    }

    fn provider_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        #[cfg(test)]
        if let Some(provider) = &self.provider_override {
            return Ok(Arc::clone(provider));
        }
        provider_for_admission(admission)
    }
}

pub struct PrReconciler {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    integration_operations: crate::task_integration_operation::TaskIntegrationOperationManager,
    interval: Duration,
    #[cfg(test)]
    provider_override: Option<Arc<dyn PrProvider>>,
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
            #[cfg(test)]
            provider_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_for_test(mut self, provider: Arc<dyn PrProvider>) -> Self {
        self.provider_override = Some(provider);
        self
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
        let pending = pending_remote_pr_admissions(&self.db).await?;
        for (metadata, admission) in pending {
            if let Err(error) = self.reconcile_metadata(metadata, admission).await {
                tracing::warn!(%error, "remote PR admission reconciliation failed");
            }
        }
        Ok(())
    }

    async fn reconcile_metadata(
        &self,
        _metadata: PrMetadata,
        admission: RemotePrAdmission,
    ) -> Result<()> {
        let Some(_recovery_lock) = self
            .integration_operations
            .try_pr_recovery_lock(
                &admission.task_id,
                &admission.task_merge_operation_id,
                &admission.publish_operation_id,
            )
            .await?
        else {
            return Ok(());
        };
        let metadata = PrMetadataRepo::get_by_task_id(&*self.db, &admission.task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", admission.task_id.clone()))?;
        let admission = TaskIntegrationOperationRepo::get_remote_pr_admission(
            &*self.db,
            &admission.task_merge_operation_id,
        )
        .await?
        .ok_or_else(|| {
            ServiceError::not_found(
                "remote PR admission",
                admission.task_merge_operation_id.clone(),
            )
        })?;
        if !matches!(
            admission.state.as_str(),
            "admitted" | "reconciliation_required" | "open"
        ) {
            return Ok(());
        }
        let task = TaskRepo::get_by_id(&*self.db, &admission.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", admission.task_id.clone()))?;
        let publish_operation =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, &admission.publish_operation_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "PublishPr operation",
                        admission.publish_operation_id.clone(),
                    )
                })?;
        PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .validate_admission(&task, &admission, &metadata, &publish_operation.id)
            .await?;

        let provider = match self.provider_for(&admission) {
            Ok(provider) => provider,
            Err(error) => {
                self.mark_reconciliation_required(&admission, &metadata, &error.to_string())
                    .await?;
                return Ok(());
            }
        };
        let request = create_request(&admission, &task);
        if metadata.provider_pr_id.is_none() {
            let (record, create_attempted) = find_or_create(provider.as_ref(), request).await;
            match record {
                Ok(record) => {
                    self.apply_record(&admission, &metadata, record).await?;
                    if metadata.admission_status != "legacy_unadmitted" {
                        set_task_awaiting_human_best_effort(&self.db, &task, true).await;
                    }
                }
                Err(PrProviderError::DefinitiveRejection(reason)) if create_attempted => {
                    self.apply_publication_failure(&admission, &metadata, &reason)
                        .await?;
                }
                Err(error) => {
                    self.mark_reconciliation_required(&admission, &metadata, &error.to_string())
                        .await?;
                }
            }
            return Ok(());
        }

        match provider.get_pr_status(&admission, &metadata).await {
            Ok(status) => self.apply_status(&admission, &metadata, status).await?,
            Err(PrProviderError::DefinitiveRejection(reason)) => {
                // A rejected status read says nothing definitive about the PR's
                // external state, so it remains a reconciliation obligation.
                self.mark_reconciliation_required(
                    &admission,
                    &metadata,
                    &format!("provider status read unavailable: {reason}"),
                )
                .await?;
            }
            Err(error @ PrProviderError::OutcomeUnknown(_))
            | Err(error @ PrProviderError::Unavailable(_)) => {
                self.mark_reconciliation_required(&admission, &metadata, &error.to_string())
                    .await?;
            }
        }
        Ok(())
    }

    async fn apply_record(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        record: PrRecord,
    ) -> Result<()> {
        let service = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        service.apply_record(admission, metadata, record).await
    }

    async fn apply_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        status: RemotePrStatus,
    ) -> Result<()> {
        let service = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        service.apply_status(admission, metadata, status).await
    }

    async fn apply_publication_failure(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        reason: &str,
    ) -> Result<()> {
        let service = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        service
            .apply_publication_failure(admission, metadata, reason)
            .await
    }

    async fn mark_reconciliation_required(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        reason: &str,
    ) -> Result<()> {
        let service = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        service
            .mark_reconciliation_required(admission, Some(metadata), reason)
            .await
    }

    fn provider_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        #[cfg(test)]
        if let Some(provider) = &self.provider_override {
            return Ok(Arc::clone(provider));
        }
        provider_for_admission(admission)
    }
}

fn create_request(admission: &RemotePrAdmission, task: &Task) -> PrCreateRequest {
    PrCreateRequest {
        repo_remote_url: admission.remote_repo_identity.clone(),
        source_branch: admission.source_branch.clone(),
        target_branch: admission.target_branch.clone(),
        source_sha: admission.admitted_source_sha.clone(),
        idempotency_key: admission.task_merge_operation_id.clone(),
        title: task.title.clone(),
        body: Some(format!("Forge task: {}", task.id)),
    }
}

async fn find_or_create(
    provider: &dyn PrProvider,
    request: PrCreateRequest,
) -> (std::result::Result<PrRecord, PrProviderError>, bool) {
    match provider.find_pr(&request).await {
        Ok(Some(record)) => (Ok(record), false),
        Ok(None) => (provider.create_pr(request).await, true),
        Err(error) => (Err(error), false),
    }
}

fn provider_for_admission(
    admission: &RemotePrAdmission,
) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
    let token_ref = admission
        .token_secret_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            PrProviderError::Unavailable("frozen token secret reference is missing".to_owned())
        })?;
    let token = std::env::var(token_ref).map_err(|_| {
        PrProviderError::Unavailable("frozen provider credential is unavailable".to_owned())
    })?;
    match admission.provider_type.as_str() {
        "github" => Ok(Arc::new(GitHubPrProvider::new(
            admission.provider_type.clone(),
            admission.provider_base_url.clone(),
            token,
        ))),
        provider_type => Err(PrProviderError::Unavailable(format!(
            "unsupported frozen provider type: {provider_type}"
        ))),
    }
}

async fn pending_remote_pr_admissions(
    db: &SqliteDb,
) -> Result<Vec<(PrMetadata, RemotePrAdmission)>> {
    let rows = sqlx::query(
        "SELECT admission.task_merge_operation_id
         FROM remote_pr_admission admission
         JOIN task_integration_operation merge_op
           ON merge_op.id = admission.task_merge_operation_id
         JOIN task_integration_operation publish_op
           ON publish_op.id = admission.publish_operation_id
         WHERE admission.state IN ('admitted', 'reconciliation_required', 'open')
           AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
           AND merge_op.remote_waiting = 1
           AND publish_op.kind = 'publish_pr'
           AND publish_op.status IN ('running', 'succeeded')",
    )
    .fetch_all(db.pool())
    .await?;
    let mut pending = Vec::with_capacity(rows.len());
    for row in rows {
        let merge_id: String = row.try_get("task_merge_operation_id")?;
        let admission = TaskIntegrationOperationRepo::get_remote_pr_admission(db, &merge_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("remote PR admission", merge_id.clone()))?;
        let metadata = PrMetadataRepo::get_by_task_id(db, &admission.task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", admission.task_id.clone()))?;
        pending.push((metadata, admission));
    }
    Ok(pending)
}

async fn set_task_awaiting_human_best_effort(db: &SqliteDb, task: &Task, awaiting_human: bool) {
    if let Err(error) = set_task_awaiting_human(db, task, awaiting_human).await {
        tracing::warn!(task_id = %task.id, %error, "could not update legacy awaiting-human projection for PR");
    }
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
