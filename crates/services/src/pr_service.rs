use crate::credential_service::{CredentialService, Secret};
use crate::pr_external_action::PrExternalActionService;
use crate::pr_github_client::{DefaultPrProviderFactory, GitHubClient, PrProviderFactory};
use crate::pr_rework::PrReworkService;
use crate::{DomainEventService, Result, ServiceError};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use db::{
    now_rfc3339, CreateDomainEvent, DbError, GateEvaluationOutcome, GateRepo, PrMetadata,
    PrMetadataRepo, PrReworkRepo, PrReworkState, RemotePrAdmission, Repo, SqliteDb, Task,
    TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskLifecycleRepo,
    TaskLifecycleState, TaskMetadata, TaskRepo, TaskRepositoryMergeBatchRepo,
    TaskRepositoryMergeBatchRepoState, TransitionPrReworkSupersession,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
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

    pub(crate) fn observation(&self) -> &PrObservation {
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
    /// The currently `open` PR whose head is exactly `source_branch` against
    /// `target_branch`, without requiring a head SHA (same-PR re-admission,
    /// plan 11.5). Providers that cannot answer fail closed.
    async fn find_open_pr_by_head(
        &self,
        _repo_identity: &str,
        _source_branch: &str,
        _target_branch: &str,
    ) -> std::result::Result<Option<PrRecord>, PrProviderError> {
        Err(PrProviderError::Unavailable(
            "provider cannot find an open PR by head branch".to_owned(),
        ))
    }
}

pub struct PublishedPr {
    pub metadata: PrMetadata,
    pub pr_url: Option<String>,
}

/// Exact, durable identity used at every provider boundary. The admission ID
/// is the TaskMerge operation ID, but provider work also verifies the Task
/// and repository so a stale projection cannot redirect it to another PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrAdmissionKey {
    pub admission_id: String,
    pub task_id: String,
    pub repo_id: String,
}

/// Task-wide read projection that retains the repository identity of each PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRepositoryPrStatus {
    pub repo_id: String,
    pub admission_id: Option<String>,
    pub pr_metadata_id: String,
    pub provider_type: String,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub pr_state: String,
    pub merge_status: String,
    pub admission_status: String,
}

impl RemotePrAdmissionKey {
    pub(crate) fn from_admission(admission: &RemotePrAdmission) -> Result<Self> {
        let repo_id = admission
            .repo_id
            .as_deref()
            .filter(|repo_id| !repo_id.trim().is_empty())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "remote PR admission has no exact Task and repository identity",
                )
            })?;
        if admission.task_id.trim().is_empty()
            || admission.task_merge_operation_id.trim().is_empty()
        {
            return Err(ServiceError::invalid_operation(
                "remote PR admission has no exact Task and repository identity",
            ));
        }
        Ok(Self {
            admission_id: admission.task_merge_operation_id.clone(),
            task_id: admission.task_id.clone(),
            repo_id: repo_id.to_owned(),
        })
    }
}

#[derive(Clone)]
pub struct PrService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    credentials: Option<Arc<CredentialService>>,
    provider_factory: Arc<dyn PrProviderFactory>,
    #[cfg(test)]
    provider_override: Option<Arc<dyn PrProvider>>,
}

/// Task-wide feedback read projection. The underlying feedback repository is
/// keyed by PR admission, while the public API and MCP contracts are Task-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPrFeedbackItem {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub repo_id: String,
    pub provider_item_id: String,
    pub kind: String,
    pub author_login: String,
    pub author_association: String,
    pub review_state: Option<String>,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub commit_sha: Option<String>,
    pub body: String,
    pub body_truncated: bool,
    pub body_digest: String,
    pub provider_url: Option<String>,
    pub trust_class: String,
    pub head_state: String,
    pub disposition: Option<String>,
    pub dismiss_reason: Option<String>,
    pub scope_classification: Option<String>,
    pub human_decision_id: Option<String>,
    pub followup_task_id: Option<String>,
    pub ingested_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskPrFeedbackCursor {
    ingested_at: String,
    id: String,
}

impl PrService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            db,
            event_bus,
            credentials: None,
            provider_factory: Arc::new(DefaultPrProviderFactory),
            #[cfg(test)]
            provider_override: None,
        }
    }

    /// List current PR metadata without collapsing a multi-repository Task to
    /// one row. Legacy rows without a repository cannot authorize provider
    /// work and are reported as invalid rather than assigned to a guessed repo.
    pub async fn list_task_prs(&self, task_id: &str) -> Result<Vec<TaskRepositoryPrStatus>> {
        let metadata = PrMetadataRepo::list_by_task(&*self.db, task_id).await?;
        let mut statuses = Vec::with_capacity(metadata.len());
        for item in metadata {
            let repo_id = item
                .repo_id
                .as_deref()
                .filter(|repo_id| !repo_id.trim().is_empty())
                .ok_or_else(|| {
                    ServiceError::invalid_operation("PR metadata has no exact repository identity")
                })?;
            statuses.push(TaskRepositoryPrStatus {
                repo_id: repo_id.to_owned(),
                admission_id: item.task_merge_operation_id,
                pr_metadata_id: item.id,
                provider_type: item.provider_type,
                provider_pr_id: item.provider_pr_id,
                pr_url: item.pr_url,
                pr_state: item.pr_state,
                merge_status: item.merge_status,
                admission_status: item.admission_status,
            });
        }
        Ok(statuses)
    }

    /// List all provider feedback for a Task across its PR admissions. This
    /// read accessor is needed because the durable feedback repository is
    /// intentionally scoped to one admission at a time.
    pub async fn list_task_external_feedback(
        &self,
        task_id: &str,
        cursor: Option<String>,
        requested_limit: i64,
    ) -> Result<db::Page<TaskPrFeedbackItem>> {
        let after = match cursor {
            Some(cursor) => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(cursor)
                    .map_err(|_| ServiceError::Db(DbError::InvalidCursor))?;
                let value: TaskPrFeedbackCursor = serde_json::from_slice(&bytes)
                    .map_err(|_| ServiceError::Db(DbError::InvalidCursor))?;
                if value.ingested_at.trim().is_empty() || value.id.trim().is_empty() {
                    return Err(ServiceError::Db(DbError::InvalidCursor));
                }
                Some(value)
            }
            None => None,
        };
        let limit = requested_limit.clamp(1, 100);
        let query = "SELECT i.id, i.task_id, i.admission_id,
                    COALESCE(admission.repo_id, merge_op.repo_id, task.repo_id) AS repo_id,
                    i.provider_item_id, i.kind,
                    i.author_login, i.author_association, i.review_state, i.path, i.line,
                    i.commit_sha, i.body, i.body_truncated, i.body_digest, i.provider_url,
                    i.trust_class, i.head_state, i.ingested_at,
                    d.disposition, d.dismiss_reason, d.scope_classification,
                    d.human_decision_id, d.followup_task_id
             FROM external_feedback_item i
             JOIN remote_pr_admission admission
               ON admission.task_merge_operation_id = i.admission_id
              AND admission.task_id = i.task_id
             JOIN task_integration_operation merge_op
               ON merge_op.id = admission.task_merge_operation_id
              AND merge_op.task_id = admission.task_id AND merge_op.kind = 'task_merge'
             JOIN task ON task.id = admission.task_id
             LEFT JOIN task_external_feedback_disposition d ON d.id = (
                 SELECT latest.id FROM task_external_feedback_disposition latest
                 WHERE latest.feedback_item_id = i.id
                 ORDER BY latest.created_at DESC, latest.id DESC LIMIT 1
             )
             WHERE i.task_id = ?
               AND (
                   (merge_op.repository_merge_batch_id IS NOT NULL
                    AND admission.repo_id = merge_op.repo_id)
                   OR (merge_op.repository_merge_batch_id IS NULL
                       AND (admission.repo_id IS NULL OR admission.repo_id = task.repo_id)
                       AND (merge_op.repo_id IS NULL OR merge_op.repo_id = task.repo_id)
                       AND task.repo_id IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM task_requires_repository_merge_batch required
                           WHERE required.task_id = task.id
                       )
                       AND EXISTS (
                           SELECT 1 FROM task_repository_set_revision revision
                           JOIN task_repository_set_revision_repo repository
                             ON repository.task_id = revision.task_id
                            AND repository.revision = revision.revision
                           WHERE revision.task_id = task.id AND revision.state = 'current'
                             AND repository.repo_id = task.repo_id
                             AND repository.role = 'primary'
                             AND (SELECT COUNT(*) FROM task_repository_set_revision_repo writable
                                  WHERE writable.task_id = revision.task_id
                                    AND writable.revision = revision.revision
                                    AND writable.role IN ('primary', 'affected')) = 1
                       ))
               )";
        let sql = if after.is_some() {
            format!(
                "{query} AND (i.ingested_at < ? OR (i.ingested_at = ? AND i.id < ?))
                 ORDER BY i.ingested_at DESC, i.id DESC LIMIT ?"
            )
        } else {
            format!("{query} ORDER BY i.ingested_at DESC, i.id DESC LIMIT ?")
        };
        let mut query = sqlx::query(&sql).bind(task_id);
        if let Some(after) = &after {
            query = query
                .bind(&after.ingested_at)
                .bind(&after.ingested_at)
                .bind(&after.id);
        }
        let rows = query
            .bind(limit + 1)
            .fetch_all(self.db.pool())
            .await
            .map_err(DbError::from)?;
        let has_more = rows.len() > limit as usize;
        let items = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| {
                Ok(TaskPrFeedbackItem {
                    id: row.try_get("id").map_err(DbError::from)?,
                    task_id: row.try_get("task_id").map_err(DbError::from)?,
                    admission_id: row.try_get("admission_id").map_err(DbError::from)?,
                    repo_id: row.try_get("repo_id").map_err(DbError::from)?,
                    provider_item_id: row.try_get("provider_item_id").map_err(DbError::from)?,
                    kind: row.try_get("kind").map_err(DbError::from)?,
                    author_login: row.try_get("author_login").map_err(DbError::from)?,
                    author_association: row.try_get("author_association").map_err(DbError::from)?,
                    review_state: row.try_get("review_state").map_err(DbError::from)?,
                    path: row.try_get("path").map_err(DbError::from)?,
                    line: row.try_get("line").map_err(DbError::from)?,
                    commit_sha: row.try_get("commit_sha").map_err(DbError::from)?,
                    body: row.try_get("body").map_err(DbError::from)?,
                    body_truncated: row
                        .try_get::<i64, _>("body_truncated")
                        .map_err(DbError::from)?
                        != 0,
                    body_digest: row.try_get("body_digest").map_err(DbError::from)?,
                    provider_url: row.try_get("provider_url").map_err(DbError::from)?,
                    trust_class: row.try_get("trust_class").map_err(DbError::from)?,
                    head_state: row.try_get("head_state").map_err(DbError::from)?,
                    disposition: row.try_get("disposition").map_err(DbError::from)?,
                    dismiss_reason: row.try_get("dismiss_reason").map_err(DbError::from)?,
                    scope_classification: row
                        .try_get("scope_classification")
                        .map_err(DbError::from)?,
                    human_decision_id: row.try_get("human_decision_id").map_err(DbError::from)?,
                    followup_task_id: row.try_get("followup_task_id").map_err(DbError::from)?,
                    ingested_at: row.try_get("ingested_at").map_err(DbError::from)?,
                })
            })
            .collect::<std::result::Result<Vec<_>, DbError>>()?;
        let next_cursor = if has_more {
            let last = items
                .last()
                .ok_or_else(|| ServiceError::Db(DbError::InvalidCursor))?;
            let cursor = TaskPrFeedbackCursor {
                ingested_at: last.ingested_at.clone(),
                id: last.id.clone(),
            };
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&cursor)
                        .map_err(|_| ServiceError::Db(DbError::InvalidCursor))?,
                ),
            )
        } else {
            None
        };
        Ok(db::Page {
            items,
            next_cursor,
            total_count: None,
        })
    }

    /// Credential source for the frozen admission's GitHub handle. Without
    /// it every provider resolution fails closed as unavailable.
    pub fn with_credentials(mut self, credentials: Arc<CredentialService>) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Replace the provider factory (tests point it at a local server).
    pub fn with_provider_factory(mut self, factory: Arc<dyn PrProviderFactory>) -> Self {
        self.provider_factory = factory;
        self
    }

    pub(crate) async fn publish_pr(
        &self,
        task: &Task,
        repo: &Repo,
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
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        if key.task_id != task.id || key.repo_id != repo.id {
            return Err(ServiceError::invalid_operation(
                "PR publication does not match the exact Task and repository admission",
            ));
        }
        if publication.repository_merge_batch_id.is_some() {
            // Multirepo admissions are scoped to an exact configured Repo.
            // Keep the historical batchless path tied to its frozen admission
            // identity, as it was in 0.9.1; the Repo argument is only used to
            // resolve the exact single primary repository in that path.
            validate_credential_free_remote_identity(&repo.remote_url)?;
            if admission.remote_repo_identity != repo.remote_url {
                return Err(ServiceError::invalid_operation(
                    "PR admission repository identity differs from the configured repository",
                ));
            }
        }
        let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
        self.validate_admission(task, &admission, &key, &metadata, &publication.id)
            .await?;
        if admission.source_branch != source_branch || admission.target_branch != target_branch {
            return Err(ServiceError::invalid_operation(
                "PR publication arguments differ from the frozen remote admission",
            ));
        }

        let provider = match self.provider_for(&admission).await {
            Ok(provider) => provider,
            Err(error) => {
                self.mark_reconciliation_required(&admission, Some(&metadata), &error.to_string())
                    .await?;
                return Err(ServiceError::conflict(
                    "remote PR provider is unavailable; the existing admission remains in reconciliation",
                ));
            }
        };
        if admission.readmits_supersession_id.is_some() {
            return self
                .publish_readmission(task, &admission, &metadata, provider.as_ref())
                .await;
        }
        let request = create_request(&admission, task);
        let (record, create_attempted) = find_or_create(provider.as_ref(), request).await;
        match record {
            Ok(record) => {
                self.apply_record(&admission, &metadata, record).await?;
                let metadata =
                    PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
                        .await?
                        .ok_or_else(|| {
                            ServiceError::not_found("PR metadata", key.admission_id.clone())
                        })?;
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

    /// External-action service sharing this service's credential source and
    /// provider factory.
    pub(crate) fn actions(&self) -> PrExternalActionService {
        PrExternalActionService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
            self.clone(),
        )
    }

    /// Raw GitHub client built from the admission's FROZEN credential.
    pub(crate) async fn client_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<GitHubClient, PrProviderError> {
        let secret =
            acquire_admission_secret(&self.db, self.credentials.as_deref(), admission).await?;
        self.provider_factory
            .build_client(
                admission.provider_type.as_str(),
                admission.provider_base_url.as_deref(),
                secret,
            )
            .await
    }

    /// The admission's frozen credential, opened in memory only.
    pub(crate) async fn secret_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<Secret, PrProviderError> {
        acquire_admission_secret(&self.db, self.credentials.as_deref(), admission).await
    }

    /// Re-admission of a paused PR (plan 11.5). The PR is reused, never
    /// created: the admission stays `admitted` while the exact `push_update`
    /// moves the PR head to the admitted SHA.
    async fn publish_readmission(
        &self,
        task: &Task,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        provider: &dyn PrProvider,
    ) -> Result<PublishedPr> {
        let supersession_id = admission
            .readmits_supersession_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("admission does not re-admit a paused PR")
            })?;
        let supersession = PrReworkRepo::get_supersession(&*self.db, supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.to_owned())
            })?;
        let readmitted_by_this = supersession.state == PrReworkState::Readmitted
            && supersession.readmission_merge_operation_id.as_deref()
                == Some(admission.task_merge_operation_id.as_str());
        if supersession.state != PrReworkState::Open && !readmitted_by_this {
            return self
                .readmission_blocked(
                    admission,
                    metadata,
                    "pr_rework.supersession_open: the paused PR no longer awaits this re-admission",
                )
                .await;
        }
        if !readmitted_by_this {
            match provider
                .find_open_pr_by_head(
                    &admission.remote_repo_identity,
                    &admission.source_branch,
                    &admission.target_branch,
                )
                .await
            {
                Ok(Some(record)) if record.provider_pr_id == supersession.provider_pr_id => {
                    let observation = record.status.observation();
                    if observation.remote_repo_identity != admission.remote_repo_identity
                        || observation.source_branch != admission.source_branch
                        || observation.target_branch != admission.target_branch
                    {
                        return self
                            .readmission_blocked(
                                admission,
                                metadata,
                                "provider PR identity differs from the frozen re-admission",
                            )
                            .await;
                    }
                    if !matches!(&record.status, RemotePrStatus::Open(_)) {
                        if let Err(error) =
                            PrReworkService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                                .observe_remote(&supersession.id, &record.status)
                                .await
                        {
                            tracing::warn!(%error, "could not record the remote PR change during re-admission");
                        }
                        return self
                            .readmission_blocked(
                                admission,
                                metadata,
                                "pr_rework.reuse_target_missing: the matching PR is not open",
                            )
                            .await;
                    }
                    let head = observation.head_sha.clone();
                    if head == supersession.superseded_head_sha {
                        if let Err(error) = self
                            .actions()
                            .ensure_push_update(&admission.task_merge_operation_id)
                            .await
                        {
                            // The executor tick recreates a missing action.
                            tracing::warn!(%error, "could not create the re-admission push_update yet");
                        }
                    } else if head == admission.admitted_source_sha {
                        self.finalize_readmission(admission).await?;
                    } else {
                        return self
                            .readmission_blocked(
                                admission,
                                metadata,
                                "pr_action.remote_head_changed: the open PR head is neither the paused head nor the admitted head",
                            )
                            .await;
                    }
                }
                Ok(Some(_)) | Ok(None) => {
                    if let Ok(status @ (RemotePrStatus::Closed(_) | RemotePrStatus::Merged(_))) =
                        provider.get_pr_status(admission, metadata).await
                    {
                        if let Err(error) =
                            PrReworkService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                                .observe_remote(&supersession.id, &status)
                                .await
                        {
                            tracing::warn!(%error, "could not record the remote PR change during re-admission");
                        }
                    }
                    return self
                        .readmission_blocked(
                            admission,
                            metadata,
                            "pr_rework.reuse_target_missing: the paused PR is no longer open",
                        )
                        .await;
                }
                Err(error) => {
                    return self
                        .readmission_blocked(admission, metadata, &error.to_string())
                        .await;
                }
            }
        }
        let key = RemotePrAdmissionKey::from_admission(admission)?;
        let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
        set_task_awaiting_human_best_effort(&self.db, task, true).await;
        Ok(PublishedPr {
            pr_url: metadata.pr_url.clone(),
            metadata,
        })
    }

    async fn readmission_blocked(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        reason: &str,
    ) -> Result<PublishedPr> {
        self.mark_reconciliation_required(admission, Some(metadata), reason)
            .await?;
        Err(ServiceError::conflict(
            "the paused PR cannot be re-admitted; the admission remains in reconciliation",
        ))
    }

    /// Complete a re-admission whose PR head already is the admitted SHA:
    /// the admission becomes `open` and the pause `readmitted`. Idempotent;
    /// every step is retried by the executor tick.
    pub(crate) async fn finalize_readmission(&self, admission: &RemotePrAdmission) -> Result<()> {
        let supersession_id = admission
            .readmits_supersession_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("admission does not re-admit a paused PR")
            })?;
        let supersession = PrReworkRepo::get_supersession(&*self.db, supersession_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("pr rework supersession", supersession_id.to_owned())
            })?;
        let fresh = TaskIntegrationOperationRepo::get_remote_pr_admission(
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
        let key = RemotePrAdmissionKey::from_admission(&fresh)?;
        if matches!(fresh.state.as_str(), "admitted" | "reconciliation_required") {
            let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
            self.persist_outcome(db::RecordRemotePrOutcome {
                expected_task_id: fresh.task_id.clone(),
                task_merge_operation_id: fresh.task_merge_operation_id.clone(),
                publish_operation_id: fresh.publish_operation_id.clone(),
                metadata_id: fresh.metadata_id.clone(),
                provider_config_id: fresh.provider_config_id.clone(),
                provider_config_digest: fresh.provider_config_digest.clone(),
                remote_repo_identity: fresh.remote_repo_identity.clone(),
                source_branch: fresh.source_branch.clone(),
                target_branch: fresh.target_branch.clone(),
                status: "open".to_owned(),
                provider_event_id: Some(format!(
                    "github-pr:{}:open:{}",
                    supersession.provider_pr_id, fresh.admitted_source_sha
                )),
                provider_pr_id: Some(supersession.provider_pr_id.clone()),
                pr_url: metadata.pr_url.clone(),
                observed_head_sha: Some(fresh.admitted_source_sha.clone()),
                merged_commit_sha: None,
                reconciliation_reason: None,
                updated_at: now_rfc3339(),
            })
            .await?;
        }
        if supersession.state == PrReworkState::Open {
            PrReworkRepo::transition_supersession(
                &*self.db,
                TransitionPrReworkSupersession {
                    id: supersession.id.clone(),
                    expected_version: supersession.version,
                    to_state: PrReworkState::Readmitted,
                    readmission_merge_operation_id: Some(fresh.task_merge_operation_id.clone()),
                    convergence_decision_id: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn validate_admission(
        &self,
        task: &Task,
        admission: &RemotePrAdmission,
        key: &RemotePrAdmissionKey,
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
        let operation_repo_matches = |repo_id: Option<&str>| {
            if merge.repository_merge_batch_id.is_some() {
                repo_id == Some(key.repo_id.as_str())
            } else {
                // 0.9.1 batchless TaskMerge/PublishPr rows may have no repo_id.
                // Their exact repository is the current single primary repo;
                // any persisted repo_id must still agree with that identity.
                task.repo_id.as_deref() == Some(key.repo_id.as_str())
                    && repo_id.is_none_or(|repo_id| repo_id == key.repo_id.as_str())
            }
        };
        if admission.task_id != task.id
            || key.task_id != task.id
            || key.admission_id != admission.task_merge_operation_id
            || !operation_repo_matches(admission.repo_id.as_deref())
            || metadata.task_id != task.id
            || !operation_repo_matches(metadata.repo_id.as_deref())
            || metadata.id != admission.metadata_id
            || metadata.task_merge_operation_id.as_deref()
                != Some(admission.task_merge_operation_id.as_str())
            || metadata.publish_operation_id.as_deref()
                != Some(admission.publish_operation_id.as_str())
            || publish_operation_id != admission.publish_operation_id
            || merge.task_id != task.id
            || !operation_repo_matches(merge.repo_id.as_deref())
            || merge.kind != TaskIntegrationOperationKind::TaskMerge
            || !merge.remote_waiting
            || merge.status != db::TaskIntegrationOperationStatus::Running
            || publish.task_id != task.id
            || !operation_repo_matches(publish.repo_id.as_deref())
            || publish.repository_merge_batch_id != merge.repository_merge_batch_id
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.parent_operation_id.as_deref() != Some(merge.id.as_str())
            || publish.gate_evaluation_id != merge.gate_evaluation_id
            || lifecycle.state != TaskLifecycleState::Merging
            || lifecycle.reason_kind.as_deref()
                != Some(if merge.repository_merge_batch_id.is_some() {
                    "task_repository_merge_batch"
                } else {
                    "task_merge"
                })
            || lifecycle.reason_ref.as_deref()
                != Some(
                    merge
                        .repository_merge_batch_id
                        .as_deref()
                        .unwrap_or(merge.id.as_str()),
                )
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
        let expected_lifecycle_reason = merge
            .repository_merge_batch_id
            .as_deref()
            .unwrap_or(merge.id.as_str());
        if lifecycle.reason_ref.as_deref() != Some(expected_lifecycle_reason) {
            return Err(ServiceError::invalid_operation(
                "remote PR admission does not belong to the current TaskMerge lifecycle",
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

    pub(crate) async fn apply_publication_failure(
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

    pub(crate) async fn mark_reconciliation_required(
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
        let admission_id = input.task_merge_operation_id.clone();
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
        self.emit_repository_pr_status(&admission_id).await?;
        Ok(())
    }

    async fn emit_repository_pr_status(&self, admission_id: &str) -> Result<()> {
        let Some(admission) =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*self.db, admission_id).await?
        else {
            return Ok(());
        };
        let Some(repo_id) = admission.repo_id.as_deref() else {
            return Ok(());
        };
        let Some(operation) =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, admission_id).await?
        else {
            return Ok(());
        };
        let batch_id = operation.repository_merge_batch_id.as_deref();
        let metadata =
            PrMetadataRepo::get_by_task_repo(&*self.db, &admission.task_id, repo_id).await?;
        let (repository_revision, correlation_id, version) = if let Some(batch_id) = batch_id {
            let Some(mut batch) =
                TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, batch_id).await?
            else {
                return Ok(());
            };
            let rows = TaskRepositoryMergeBatchRepo::list_repositories(&*self.db, batch_id).await?;
            if let Some(row) = rows.iter().find(|row| row.repo_id == repo_id) {
                if admission.rework_outcome.is_none() {
                    let state = match admission.state.as_str() {
                        "open" => TaskRepositoryMergeBatchRepoState::Open,
                        "merged" => TaskRepositoryMergeBatchRepoState::Merged,
                        "closed" => TaskRepositoryMergeBatchRepoState::Closed,
                        "failed" | "head_mismatch" | "publication_failed" => {
                            TaskRepositoryMergeBatchRepoState::Failed
                        }
                        _ => TaskRepositoryMergeBatchRepoState::Publishing,
                    };
                    let current_state = if row.state == TaskRepositoryMergeBatchRepoState::Open
                        && state == TaskRepositoryMergeBatchRepoState::Publishing
                    {
                        row.state
                    } else {
                        state
                    };
                    TaskRepositoryMergeBatchRepo::transition_repo(
                        &*self.db,
                        batch_id,
                        repo_id,
                        batch.version,
                        current_state,
                        row.work_unit_repository_integration_id.as_deref(),
                        Some(admission_id),
                        Some(&admission.metadata_id),
                        row.before_sha.as_deref(),
                        row.after_sha
                            .as_deref()
                            .or(Some(admission.admitted_source_sha.as_str())),
                    )
                    .await?;
                    batch =
                        crate::merge_service::refresh_merge_batch_state(&self.db, &batch).await?;
                    crate::merge_service::emit_repository_merge_batch_events(
                        &self.db,
                        &self.event_bus,
                        batch_id,
                    )
                    .await?;
                }
            }
            (
                batch.repository_set_revision,
                batch.id.clone(),
                batch.version,
            )
        } else {
            let revision = db::TaskRepositorySetRepo::get_current(&*self.db, &admission.task_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("Task repository revision", admission.task_id.clone())
                })?;
            let writable_repositories = revision
                .repositories
                .iter()
                .filter(|repository| {
                    matches!(
                        repository.role,
                        db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                    )
                })
                .collect::<Vec<_>>();
            let exact_single_primary = writable_repositories.len() == 1
                && writable_repositories[0].repo_id.as_str() == repo_id
                && writable_repositories[0].role == db::TaskRepositoryRole::Primary;
            if !exact_single_primary {
                return Err(ServiceError::invalid_operation(
                    "batchless PR status requires the exact single primary repository",
                ));
            }
            (
                revision.revision,
                admission_id.to_owned(),
                operation.version,
            )
        };
        let state = admission.state.as_str();
        let dedupe_key = format!(
            "task-repository-pr-status:{}:{}:{}",
            admission_id, admission.updated_at, state
        );
        DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .append(CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "task.repository_pr_status_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: admission.task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: admission.task_id.clone(),
                correlation_id,
                causation_id: Some(admission_id.to_owned()),
                causation_depth: 1,
                dedupe_key: Some(dedupe_key),
                payload_json: json!({
                    "task_id": admission.task_id,
                    "repo_id": repo_id,
                    "repository_revision": repository_revision,
                    "repository_merge_batch_id": batch_id,
                    "admission_id": admission_id,
                    "pr_metadata_id": admission.metadata_id,
                    "state": state,
                    "admission_status": metadata.as_ref().map(|metadata| metadata.admission_status.as_str()),
                    "pr_state": metadata.as_ref().map(|metadata| metadata.pr_state.as_str()),
                    "version": version,
                })
                .to_string(),
                created_at: admission.updated_at,
            })
            .await?;
        Ok(())
    }

    async fn provider_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        #[cfg(test)]
        if let Some(provider) = &self.provider_override {
            return Ok(Arc::clone(provider));
        }
        provider_for_admission(
            &self.db,
            self.credentials.as_deref(),
            self.provider_factory.as_ref(),
            admission,
        )
        .await
    }
}

pub struct PrReconciler {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    integration_operations: crate::task_integration_operation::TaskIntegrationOperationManager,
    interval: Duration,
    credentials: Option<Arc<CredentialService>>,
    provider_factory: Arc<dyn PrProviderFactory>,
    feedback: crate::pr_feedback::PrFeedbackIngestor,
    watcher: crate::pr_rework_watcher::PrReworkWatcher,
    #[cfg(test)]
    provider_override: Option<Arc<dyn PrProvider>>,
}

impl PrReconciler {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, interval: Option<Duration>) -> Self {
        let lock_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let feedback = crate::pr_feedback::PrFeedbackIngestor::new(Arc::clone(&db));
        let watcher =
            crate::pr_rework_watcher::PrReworkWatcher::new(Arc::clone(&db), Arc::clone(&event_bus));
        Self {
            integration_operations:
                crate::task_integration_operation::TaskIntegrationOperationManager::new(
                    Arc::clone(&db),
                    lock_root,
                ),
            db,
            event_bus,
            interval: interval.unwrap_or(Duration::from_secs(60)),
            credentials: None,
            provider_factory: Arc::new(DefaultPrProviderFactory),
            feedback,
            watcher,
            #[cfg(test)]
            provider_override: None,
        }
    }

    pub fn with_credentials(mut self, credentials: Arc<CredentialService>) -> Self {
        self.feedback = self.feedback.with_credentials(Arc::clone(&credentials));
        self.watcher = self
            .watcher
            .clone()
            .with_credentials(Arc::clone(&credentials));
        self.credentials = Some(credentials);
        self
    }

    pub fn with_provider_factory(mut self, factory: Arc<dyn PrProviderFactory>) -> Self {
        self.feedback = self.feedback.with_provider_factory(Arc::clone(&factory));
        self.watcher = self
            .watcher
            .clone()
            .with_provider_factory(Arc::clone(&factory));
        self.provider_factory = factory;
        self
    }

    fn service(&self) -> PrService {
        let mut service = PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .with_provider_factory(Arc::clone(&self.provider_factory));
        if let Some(credentials) = &self.credentials {
            service = service.with_credentials(Arc::clone(credentials));
        }
        service
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
        // External feedback ingestion shares the tick but never blocks or
        // fails reconciliation.
        if let Err(error) = self.feedback.ingest_once().await {
            tracing::warn!(%error, "PR feedback ingestion pass failed");
        }
        // Read-only convergence of paused same-PR reworks (plan 11.6) and the
        // authorized external actions (re-admission push, summary comment).
        if let Err(error) = self.watcher.watch_once().await {
            tracing::warn!(%error, "PR rework watcher pass failed");
        }
        if let Err(error) = self.service().actions().run_once().await {
            tracing::warn!(%error, "PR external action pass failed");
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
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &key.task_id, &key.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
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
        // A re-admission awaiting its exact `push_update` belongs to the
        // external-action executor until its exact supersession is readmitted
        // (plan 11.5). This includes `reconciliation_required`: polling here
        // could overwrite an unresolved push with the old or newly moved PR
        // head before the executor has recorded its action outcome.
        let readmission_pending =
            if let Some(supersession_id) = admission.readmits_supersession_id.as_deref() {
                match PrReworkRepo::get_supersession(&*self.db, supersession_id).await? {
                    Some(supersession) => {
                        supersession.state != PrReworkState::Readmitted
                            || supersession.readmission_merge_operation_id.as_deref()
                                != Some(admission.task_merge_operation_id.as_str())
                    }
                    None => true,
                }
            } else {
                false
            };
        if admission.rework_outcome.is_some()
            || readmission_pending
            || !matches!(
                admission.state.as_str(),
                "admitted" | "reconciliation_required" | "open"
            )
        {
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
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        if metadata.repo_id.as_deref() != Some(key.repo_id.as_str())
            || metadata.task_id != key.task_id
            || metadata.id != admission.metadata_id
        {
            return Err(ServiceError::invalid_operation(
                "PR reconciliation did not resolve the exact Task, repository and metadata admission",
            ));
        }
        self.service()
            .validate_admission(&task, &admission, &key, &metadata, &publish_operation.id)
            .await?;

        let provider = match self.provider_for(&admission).await {
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
        let service = self.service();
        service.apply_record(admission, metadata, record).await
    }

    async fn apply_status(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        status: RemotePrStatus,
    ) -> Result<()> {
        let service = self.service();
        service.apply_status(admission, metadata, status).await
    }

    async fn apply_publication_failure(
        &self,
        admission: &RemotePrAdmission,
        metadata: &PrMetadata,
        reason: &str,
    ) -> Result<()> {
        let service = self.service();
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
        let service = self.service();
        service
            .mark_reconciliation_required(admission, Some(metadata), reason)
            .await
    }

    async fn provider_for(
        &self,
        admission: &RemotePrAdmission,
    ) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
        #[cfg(test)]
        if let Some(provider) = &self.provider_override {
            return Ok(Arc::clone(provider));
        }
        provider_for_admission(
            &self.db,
            self.credentials.as_deref(),
            self.provider_factory.as_ref(),
            admission,
        )
        .await
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

/// Open the frozen credential of an admission and verify that its owner still
/// holds owner/admin authority over the Project. The secret exists only in
/// memory for the caller.
pub(crate) async fn acquire_admission_secret(
    db: &SqliteDb,
    credentials: Option<&CredentialService>,
    admission: &RemotePrAdmission,
) -> std::result::Result<crate::credential_service::Secret, PrProviderError> {
    let unavailable = |reason: &str| PrProviderError::Unavailable(reason.to_owned());
    let key = RemotePrAdmissionKey::from_admission(admission)
        .map_err(|_| unavailable("remote PR admission has no exact repository identity"))?;
    let (Some(handle_id), Some(owner_id)) = (
        admission
            .credential_handle_id
            .as_deref()
            .filter(|value| !value.is_empty()),
        admission
            .credential_owner_user_id
            .as_deref()
            .filter(|value| !value.is_empty()),
    ) else {
        return Err(unavailable("pr_provider.credential_not_configured"));
    };
    let credentials =
        credentials.ok_or_else(|| unavailable("pr_provider.credential_not_configured"))?;
    let secret = credentials
        .acquire_scm_secret(owner_id, handle_id)
        .await
        .map_err(|_| unavailable("frozen provider credential is unavailable"))?;
    let owner_authorized = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(
             SELECT 1 FROM remote_pr_admission admission
             JOIN task ON task.id = admission.task_id
             JOIN task_integration_operation merge_op
               ON merge_op.id = admission.task_merge_operation_id
              AND merge_op.task_id = admission.task_id
             JOIN pr_metadata metadata
               ON metadata.id = admission.metadata_id
              AND metadata.task_id = admission.task_id
             JOIN project_member member ON member.project_id = task.project_id
             WHERE admission.task_merge_operation_id = ?
               AND admission.task_id = ?
               AND COALESCE(admission.repo_id, merge_op.repo_id, task.repo_id) = ?
               AND COALESCE(metadata.repo_id, admission.repo_id, merge_op.repo_id, task.repo_id) = ?
               AND (
                   (merge_op.repository_merge_batch_id IS NOT NULL
                    AND admission.repo_id = merge_op.repo_id
                    AND metadata.repo_id = admission.repo_id)
                   OR (merge_op.repository_merge_batch_id IS NULL
                       AND (admission.repo_id IS NULL OR admission.repo_id = task.repo_id)
                       AND (merge_op.repo_id IS NULL OR merge_op.repo_id = task.repo_id)
                       AND task.repo_id = ?
                       AND NOT EXISTS (
                           SELECT 1 FROM task_requires_repository_merge_batch required
                           WHERE required.task_id = task.id
                       )
                       AND EXISTS (
                           SELECT 1 FROM task_repository_set_revision revision
                           JOIN task_repository_set_revision_repo repository
                             ON repository.task_id = revision.task_id
                            AND repository.revision = revision.revision
                           WHERE revision.task_id = task.id AND revision.state = 'current'
                             AND repository.repo_id = task.repo_id AND repository.role = 'primary'
                             AND (SELECT COUNT(*) FROM task_repository_set_revision_repo writable
                                  WHERE writable.task_id = revision.task_id
                                    AND writable.revision = revision.revision
                                    AND writable.role IN ('primary', 'affected')) = 1
                       ))
               )
               AND member.user_id = ?
               AND member.role IN ('owner', 'admin'))",
    )
    .bind(&key.admission_id)
    .bind(&key.task_id)
    .bind(&key.repo_id)
    .bind(&key.repo_id)
    .bind(&key.repo_id)
    .bind(owner_id)
    .fetch_one(db.pool())
    .await
    .map_err(|_| unavailable("credential owner authority could not be verified"))?;
    if owner_authorized == 0 {
        return Err(unavailable("credential.owner_not_authorized"));
    }
    Ok(secret)
}

pub(crate) fn validate_credential_free_remote_identity(identity: &str) -> Result<()> {
    if identity.trim().is_empty() {
        return Err(ServiceError::invalid_operation(
            "PR repository identity is empty",
        ));
    }
    if identity.contains('?') || identity.contains('#') {
        return Err(ServiceError::invalid_operation(
            "PR repository identity must not contain query parameters or fragments",
        ));
    }
    let parsed = match url::Url::parse(identity) {
        Ok(parsed) => parsed,
        Err(_) if identity.contains("://") => {
            return Err(ServiceError::invalid_operation(
                "PR repository identity is malformed",
            ));
        }
        Err(_) => {
            if identity
                .split_once('@')
                .is_some_and(|(username, _)| username != "git")
            {
                return Err(ServiceError::invalid_operation(
                    "PR repository identity must not contain credentials",
                ));
            }
            return Ok(());
        }
    };
    let ssh_identity = matches!(parsed.scheme(), "ssh" | "git+ssh");
    if (!parsed.username().is_empty() && (!ssh_identity || parsed.username() != "git"))
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(ServiceError::invalid_operation(
            "PR repository identity must not contain credentials, query parameters, or fragments",
        ));
    }
    Ok(())
}

pub(crate) async fn provider_for_admission(
    db: &SqliteDb,
    credentials: Option<&CredentialService>,
    factory: &dyn PrProviderFactory,
    admission: &RemotePrAdmission,
) -> std::result::Result<Arc<dyn PrProvider>, PrProviderError> {
    RemotePrAdmissionKey::from_admission(admission).map_err(|_| {
        PrProviderError::Unavailable(
            "remote PR admission has no exact repository identity".to_owned(),
        )
    })?;
    let secret = acquire_admission_secret(db, credentials, admission).await?;
    factory
        .build(
            admission.provider_type.as_str(),
            admission.provider_base_url.as_deref(),
            secret,
        )
        .await
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
         JOIN task task ON task.id = admission.task_id
         WHERE admission.state IN ('admitted', 'reconciliation_required', 'open')
           AND admission.rework_outcome IS NULL
           AND merge_op.kind = 'task_merge' AND merge_op.status = 'running'
           AND merge_op.remote_waiting = 1
           AND publish_op.kind = 'publish_pr'
           AND publish_op.status IN ('running', 'succeeded')
           AND (
               (merge_op.repository_merge_batch_id IS NOT NULL
                AND admission.repo_id = merge_op.repo_id)
               OR (merge_op.repository_merge_batch_id IS NULL
                   AND (admission.repo_id IS NULL OR admission.repo_id = task.repo_id)
                   AND (merge_op.repo_id IS NULL OR merge_op.repo_id = task.repo_id)
                   AND task.repo_id IS NOT NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM task_requires_repository_merge_batch required
                       WHERE required.task_id = task.id
                   )
                   AND EXISTS (
                       SELECT 1 FROM task_repository_set_revision revision
                       JOIN task_repository_set_revision_repo repository
                         ON repository.task_id = revision.task_id
                        AND repository.revision = revision.revision
                       WHERE revision.task_id = task.id AND revision.state = 'current'
                         AND repository.repo_id = task.repo_id AND repository.role = 'primary'
                         AND (SELECT COUNT(*) FROM task_repository_set_revision_repo writable
                              WHERE writable.task_id = revision.task_id
                                AND writable.revision = revision.revision
                                AND writable.role IN ('primary', 'affected')) = 1
                   ))
           )",
    )
    .fetch_all(db.pool())
    .await?;
    let mut pending = Vec::with_capacity(rows.len());
    for row in rows {
        let merge_id: String = row.try_get("task_merge_operation_id")?;
        let admission = TaskIntegrationOperationRepo::get_remote_pr_admission(db, &merge_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("remote PR admission", merge_id.clone()))?;
        let key = RemotePrAdmissionKey::from_admission(&admission)?;
        let metadata = PrMetadataRepo::get_by_task_repo(db, &key.task_id, &key.repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", key.admission_id.clone()))?;
        pending.push((metadata, admission));
    }
    Ok(pending)
}

async fn set_task_awaiting_human_best_effort(db: &SqliteDb, task: &Task, awaiting_human: bool) {
    let awaiting_human = if awaiting_human {
        true
    } else {
        match PrMetadataRepo::list_by_task(db, &task.id).await {
            Ok(metadata) => metadata.iter().any(|item| {
                matches!(
                    item.admission_status.as_str(),
                    "admitted" | "reconciliation_required" | "open"
                )
            }),
            Err(error) => {
                tracing::warn!(task_id = %task.id, %error, "could not verify remaining Task PR admissions");
                return;
            }
        }
    };
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

#[cfg(test)]
mod multirepo_identity_tests {
    use super::*;

    #[test]
    fn multirepo_pr_remote_identity_rejects_embedded_credentials() {
        assert!(
            validate_credential_free_remote_identity("https://github.com/acme/widgets.git").is_ok()
        );
        assert!(
            validate_credential_free_remote_identity("git@github.com:acme/widgets.git").is_ok()
        );
        assert!(
            validate_credential_free_remote_identity("ssh://git@github.com/acme/widgets.git")
                .is_ok()
        );
        for identity in [
            "https://token@github.com/acme/widgets.git",
            "https://git:token@github.com/acme/widgets.git",
            "ssh://token@github.com/acme/widgets.git",
            "token@github.com:acme/widgets.git",
            "https://github.com/acme/widgets.git?access_token=secret",
            "https://github.com/acme/widgets.git#token",
        ] {
            assert!(
                validate_credential_free_remote_identity(identity).is_err(),
                "credential-bearing repository identity must be rejected"
            );
        }
    }
}
