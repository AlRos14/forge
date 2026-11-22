use crate::gate_engine::GateEngine;
use crate::task_integration_operation::TaskIntegrationOperationManager;
use crate::{Result, ServiceError};
use db::{
    CreateDomainEvent, ExecutionRepo, GateEvaluation, GateEvaluationInput, GateEvaluationOutcome,
    GateRepo, PageRequest, PrMetadataRepo, PrProviderConfigRepo, RepoRepo, SortBy, SortOrder,
    SqliteDb, TaskIntegrationOperationRepo, TaskLifecycleRepo, TaskLifecycleState, TaskRepo,
    TaskRepositoryMergeBatchRepo, TaskRepositorySetRepo, WorkMode, WorkUnitRepo,
    WorkUnitRepositoryIntegrationRepo, WorkspaceRepo,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use tokio::process::Command;

pub struct MergeService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    workspace_root: PathBuf,
    workspace_exec_locks: Arc<crate::workspace_execution_lock::WorkspaceExecutionLockManager>,
    cleanup_scheduler: RwLock<Option<Arc<crate::WorkspaceCleanupScheduler>>>,
    credentials: RwLock<Option<Arc<crate::CredentialService>>>,
    integration_operations: TaskIntegrationOperationManager,
}

struct TaskMergeSource {
    workspace: db::Workspace,
    branch: String,
}

#[derive(Debug, Clone)]
struct RepositoryMergeCandidate {
    repo_id: String,
    position: i64,
    integration_id: String,
    source: TaskMergeSource,
    before_sha: String,
    after_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeCandidateSubject {
    workspace_id: String,
    commit_sha: String,
    snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Done {
        before_sha: String,
        after_sha: String,
        branch: String,
    },
    PullRequest {
        pr_url: Option<String>,
        branch: String,
        target_branch: String,
    },
    Conflict {
        details: String,
        conflict_paths: Vec<PathBuf>,
    },
    Dirty {
        files: Vec<String>,
    },
    TargetDirty {
        files: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeAfterGateResult {
    pub batch: db::TaskRepositoryMergeBatch,
    /// Preserves the legacy scalar outcome (including conflict details/files)
    /// for the primary repository. Repository rows remain the batch authority.
    pub primary_outcome: Option<MergeOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum MergeStrategy {
    #[default]
    Merge,
    Rebase,
}

impl MergeService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>, workspace_root: PathBuf) -> Self {
        let integration_operations =
            TaskIntegrationOperationManager::new(Arc::clone(&db), workspace_root.clone());
        Self {
            db,
            event_bus,
            workspace_root,
            workspace_exec_locks: Arc::new(
                crate::workspace_execution_lock::WorkspaceExecutionLockManager::default(),
            ),
            cleanup_scheduler: RwLock::new(None),
            credentials: RwLock::new(None),
            integration_operations,
        }
    }

    pub fn workspace_exec_locks(
        &self,
    ) -> Arc<crate::workspace_execution_lock::WorkspaceExecutionLockManager> {
        Arc::clone(&self.workspace_exec_locks)
    }

    /// Credential source used to open the frozen GitHub credential handle when
    /// publishing a PR. Without it PR publication fails closed.
    pub fn set_credential_service(&self, credentials: Arc<crate::CredentialService>) {
        match self.credentials.write() {
            Ok(mut configured) => *configured = Some(credentials),
            Err(error) => {
                tracing::warn!(%error, "merge credential service configuration lock poisoned")
            }
        }
    }

    pub fn set_cleanup_scheduler(&self, cleanup_scheduler: Arc<crate::WorkspaceCleanupScheduler>) {
        match self.cleanup_scheduler.write() {
            Ok(mut configured) => *configured = Some(cleanup_scheduler),
            Err(error) => {
                tracing::warn!(%error, "merge cleanup scheduler configuration lock poisoned")
            }
        }
    }

    /// Legacy entry point retained for pull-request publication. Direct local
    /// merges and PR publication require an exact GateEvaluation reference.
    pub async fn merge(&self, task_id: impl Into<String>) -> Result<MergeOutcome> {
        let task_id = task_id.into();
        if TaskIntegrationOperationRepo::get_active_for_task(&*self.db, &task_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "Task integration operation already active".to_owned(),
            ));
        }
        Err(ServiceError::invalid_operation(
            "Task merge requires an exact satisfied merge-readiness GateEvaluation",
        ))
    }

    pub async fn merge_after_gate(
        &self,
        task_id: &str,
        gate_evaluation_id: &str,
    ) -> Result<MergeAfterGateResult> {
        let _ = self.event_bus.receiver_count();
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::NotFound {
                entity: "task",
                id: task_id.to_owned(),
            })?;
        if task.parent_task_id.is_some() {
            return Err(ServiceError::invalid_operation(
                "subtasks do not merge; only root tasks merge to the default branch",
            ));
        }
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, gate_evaluation_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("GateEvaluation", gate_evaluation_id.to_owned())
            })?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        if evaluation.task_id != task_id
            || evaluation.outcome != GateEvaluationOutcome::Satisfied
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
        {
            return Err(ServiceError::invalid_operation(
                "merge admission requires the current satisfied merge-readiness GateEvaluation",
            ));
        }
        let revision = TaskRepositorySetRepo::get_current(&*self.db, task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation("Task has no current repository-set revision")
            })?;
        let multi_repository = members_len(&revision) > 1;
        match lifecycle.state {
            TaskLifecycleState::ReadyToMerge
                if lifecycle.reason_ref.as_deref() == Some(gate_evaluation_id) => {}
            TaskLifecycleState::Merging => {}
            TaskLifecycleState::Active if multi_repository => {}
            TaskLifecycleState::ReadyToMerge => {
                return Err(ServiceError::Conflict(
                    "Task is not ready to merge from the supplied GateEvaluation".to_owned(),
                ));
            }
            _ => {
                return Err(ServiceError::invalid_operation(
                    "single-repository merge requires ready_to_merge; multi-repository merge prepares its exact batch from active",
                ));
            }
        }
        let _local_task_lock = self
            .workspace_exec_locks
            .acquire(&format!("task-integration:{task_id}"))
            .await;
        if work_unit_integration_is_running(&self.db, task_id).await? {
            return Err(ServiceError::invalid_operation(
                "Task integration is currently incorporating a WorkUnit result",
            ));
        }
        let candidates = self
            .repository_merge_candidates(task_id, &evaluation, &revision)
            .await?;
        if candidates.is_empty() {
            return Err(ServiceError::invalid_operation(
                "Task has no changed writable repositories eligible for merge",
            ));
        }
        self.ensure_current_gate_evaluation(task_id, &evaluation, &revision, &candidates)
            .await?;
        let digest = repository_set_digest(&revision)?;
        if lifecycle.state == TaskLifecycleState::Merging {
            let active_batch_id = lifecycle.reason_ref.as_deref().unwrap_or_default();
            let active_batch =
                TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, active_batch_id).await?;
            if active_batch.as_ref().is_none_or(|batch| {
                batch.task_id != task_id
                    || batch.gate_evaluation_id != gate_evaluation_id
                    || batch.repository_set_revision != revision.revision
                    || batch.repository_set_digest.as_deref() != Some(digest.as_str())
            }) {
                return Err(ServiceError::Conflict(
                    "Task is already merging from a different repository batch".to_owned(),
                ));
            }
        }
        let mut batch = match find_or_create_merge_batch(
            &self.db,
            task_id,
            gate_evaluation_id,
            &revision,
            &digest,
            &candidates,
        )
        .await?
        {
            Some(batch) => batch,
            None => {
                return Err(ServiceError::Conflict(
                    "Task already has an active repository merge batch from another GateEvaluation"
                        .to_owned(),
                ));
            }
        };
        emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id).await?;
        if batch.state == db::TaskRepositoryMergeBatchState::Completed {
            return Ok(MergeAfterGateResult {
                batch,
                primary_outcome: None,
            });
        }
        if lifecycle.state == TaskLifecycleState::Merging
            && lifecycle.reason_ref.as_deref() != Some(batch.id.as_str())
        {
            return Err(ServiceError::Conflict(
                "Task is already merging from a different repository batch".to_owned(),
            ));
        }
        if lifecycle.state == TaskLifecycleState::ReadyToMerge {
            self.ensure_ready_lifecycle_cause(task_id, gate_evaluation_id)
                .await?;
        }
        let mut candidates_by_repo = candidates
            .into_iter()
            .map(|candidate| (candidate.repo_id.clone(), candidate))
            .collect::<BTreeMap<_, _>>();
        let primary_repo_id = revision
            .repositories
            .iter()
            .find(|member| member.role == db::TaskRepositoryRole::Primary)
            .map(|member| member.repo_id.as_str());
        let mut primary_outcome = None;
        let mut rows =
            TaskRepositoryMergeBatchRepo::list_repositories(&*self.db, &batch.id).await?;
        rows.sort_by_key(|row| row.position);
        for row in rows {
            let Some(candidate) = candidates_by_repo.remove(&row.repo_id) else {
                return Err(ServiceError::Conflict(format!(
                    "repository merge batch {} contains a repository outside the exact Gate candidate set",
                    batch.id
                )));
            };
            if !matches!(row.state, db::TaskRepositoryMergeBatchRepoState::Pending) {
                continue;
            }
            batch = TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, &batch.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("repository merge batch", batch.id.clone())
                })?;
            let repo = RepoRepo::get_by_id(&*self.db, &row.repo_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("repo", row.repo_id.clone()))?;
            if repo.work_mode == WorkMode::PullRequest {
                if batch.state == db::TaskRepositoryMergeBatchState::Integrating {
                    batch = TaskRepositoryMergeBatchRepo::transition_batch(
                        &*self.db,
                        &batch.id,
                        batch.version,
                        db::TaskRepositoryMergeBatchState::Publishing,
                        &db::now_rfc3339(),
                    )
                    .await?;
                    emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id)
                        .await?;
                }
                self.publish_repository_pr(&task, &repo, gate_evaluation_id, &batch, &candidate)
                    .await?;
            } else {
                let outcome = self
                    .merge_repository_direct(&task, &repo, gate_evaluation_id, &batch, &candidate)
                    .await?;
                if primary_repo_id == Some(repo.id.as_str()) {
                    primary_outcome = Some(outcome.clone());
                }
                match outcome {
                    MergeOutcome::Done { .. } => {}
                    MergeOutcome::Conflict { .. }
                    | MergeOutcome::Dirty { .. }
                    | MergeOutcome::TargetDirty { .. } => break,
                    MergeOutcome::PullRequest { .. } => {
                        return Err(ServiceError::invalid_operation(
                            "direct merge returned a pull-request result",
                        ));
                    }
                }
            }
        }
        batch = refresh_merge_batch_state(&self.db, &batch).await?;
        emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id).await?;
        Ok(MergeAfterGateResult {
            batch,
            primary_outcome,
        })
    }

    async fn repository_merge_candidates(
        &self,
        task_id: &str,
        evaluation: &GateEvaluation,
        revision: &db::TaskRepositorySetRevision,
    ) -> Result<Vec<RepositoryMergeCandidate>> {
        let inputs = GateRepo::list_gate_evaluation_inputs(&*self.db, &evaluation.id).await?;
        let mut parent_integrations = Vec::new();
        for input in inputs
            .iter()
            .filter(|input| input.input_kind == "work_unit_integration")
        {
            let parent = WorkUnitRepo::get_integration_by_id(&*self.db, &input.input_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("WorkUnitIntegration", input.input_id.clone())
                })?;
            if parent.task_id != task_id
                || parent.version != input.input_version
                || parent.outcome != db::WorkUnitIntegrationOutcome::Success
            {
                return Err(ServiceError::Conflict(
                    "merge Gate WorkUnitIntegration input is no longer exact".to_owned(),
                ));
            }
            parent_integrations.push(parent);
        }
        if parent_integrations.is_empty() {
            return Err(ServiceError::invalid_operation(
                "Task merge requires an exact successful WorkUnit integration in Gate inputs",
            ));
        }

        let mut members = revision
            .repositories
            .iter()
            .filter(|member| {
                matches!(
                    member.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
            })
            .collect::<Vec<_>>();
        members.sort_by_key(|member| {
            (
                if member.role == db::TaskRepositoryRole::Primary {
                    0
                } else {
                    1
                },
                member.position,
            )
        });
        if members.is_empty()
            || members
                .iter()
                .filter(|member| member.role == db::TaskRepositoryRole::Primary)
                .count()
                != 1
            || task_id != evaluation.task_id
        {
            return Err(ServiceError::invalid_operation(
                "Task repository revision has no exact primary writable repository",
            ));
        }
        let mut candidates = Vec::new();
        for member in members {
            let Some(workspace) =
                WorkspaceRepo::get_integration_by_task_repo(&*self.db, task_id, &member.repo_id)
                    .await?
            else {
                continue;
            };
            if workspace.status != db::WorkspaceStatus::Ready {
                return Err(ServiceError::Conflict(format!(
                    "repository {} integration Workspace is not ready",
                    member.repo_id
                )));
            }
            let current_sha = git::get_current_sha(Path::new(&workspace.worktree_path)).await?;
            let mut exact = Vec::new();
            for parent in &parent_integrations {
                let Some(integration) = WorkUnitRepositoryIntegrationRepo::get_by_parent_and_repo(
                    &*self.db,
                    &parent.id,
                    &member.repo_id,
                )
                .await?
                else {
                    continue;
                };
                if integration.task_id != task_id
                    || integration.work_unit_id != parent.work_unit_id
                    || integration.execution_id != parent.execution_id
                    || integration.repository_set_revision != revision.revision
                    || integration.repo_id != member.repo_id
                    || integration.target_workspace_id != workspace.id
                    || integration.outcome != db::WorkUnitRepositoryIntegrationOutcome::Success
                    || integration.target_after_sha.as_deref() != Some(current_sha.as_str())
                {
                    continue;
                }
                exact.push(integration);
            }
            if exact.len() > 1 {
                return Err(ServiceError::Conflict(format!(
                    "GateEvaluation pins more than one final integration for repository {}",
                    member.repo_id
                )));
            }
            let Some(integration) = exact.pop() else {
                continue;
            };
            let before_sha = workspace
                .before_sha
                .clone()
                .filter(|sha| !sha.trim().is_empty())
                .or_else(|| {
                    (members_len(revision) == 1).then(|| integration.target_before_sha.clone())
                })
                .filter(|sha| !sha.trim().is_empty())
                .ok_or_else(|| {
                    ServiceError::Conflict(format!(
                        "repository {} integration Workspace has no pinned base SHA",
                        member.repo_id
                    ))
                })?;
            if !git_tree_differs(
                Path::new(&workspace.worktree_path),
                &before_sha,
                &current_sha,
            )
            .await?
            {
                continue;
            }
            candidates.push(RepositoryMergeCandidate {
                repo_id: member.repo_id.clone(),
                position: member.position,
                integration_id: integration.id,
                source: TaskMergeSource {
                    branch: workspace.branch.clone(),
                    workspace,
                },
                before_sha,
                after_sha: current_sha,
            });
        }
        if members_len(revision) == 1 {
            if let Some(candidate) = candidates.first() {
                let exact_sha = self
                    .validate_gate_candidate(task_id, evaluation, &candidate.source)
                    .await?;
                if exact_sha != candidate.after_sha {
                    return Err(ServiceError::Conflict(
                        "single-repository merge candidate differs from the exact Gate input"
                            .to_owned(),
                    ));
                }
            }
        }
        Ok(candidates)
    }

    async fn publish_repository_pr(
        &self,
        task: &db::Task,
        repo: &db::Repo,
        gate_evaluation_id: &str,
        batch: &db::TaskRepositoryMergeBatch,
        candidate: &RepositoryMergeCandidate,
    ) -> Result<()> {
        crate::pr_service::validate_credential_free_remote_identity(&repo.remote_url)?;
        let source_path = Path::new(&candidate.source.workspace.worktree_path);
        if !git::is_worktree_clean(source_path).await? {
            return Err(ServiceError::Conflict(format!(
                "repository {} integration Workspace is dirty",
                repo.id
            )));
        }
        if git::get_current_sha(source_path).await? != candidate.after_sha {
            return Err(ServiceError::Conflict(format!(
                "repository {} integration Workspace moved after Gate validation",
                repo.id
            )));
        }
        let target = target_branch(&task.merge_config, &repo.default_branch)?;
        if db::PrReworkRepo::get_open_supersession_by_task(&*self.db, &task.id)
            .await?
            .is_none()
        {
            push_branch(source_path, &candidate.source.branch).await?;
        }
        if git::get_current_sha(source_path).await? != candidate.after_sha {
            return Err(ServiceError::Conflict(format!(
                "repository {} source commit changed before PR admission",
                repo.id
            )));
        }
        let provider_config = PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::PrProviderMissing {
                repo_id: repo.id.clone(),
            })?;
        let lock = self
            .integration_operations
            .lock_for_gate_admission(&task.id)
            .await?;
        let (merge, publication) = self
            .integration_operations
            .admit_pull_request_publication_with_lock(
                &task.id,
                &repo.id,
                &batch.id,
                &db::new_uuid_v4(),
                gate_evaluation_id,
                &provider_config,
                &repo.remote_url,
                &candidate.source.branch,
                &target,
                &candidate.after_sha,
                lock,
            )
            .await?;
        self.publish_domain_event_by_dedupe(&format!("task-merge-admission:{}", merge.id()))
            .await;
        let metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &task.id, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("PR metadata", merge.id().to_owned()))?;
        let merge_id = merge.id().to_owned();
        let batch = TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, &batch.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repository merge batch", batch.id.clone()))?;
        TaskRepositoryMergeBatchRepo::transition_repo(
            &*self.db,
            &batch.id,
            &repo.id,
            batch.version,
            db::TaskRepositoryMergeBatchRepoState::Publishing,
            Some(&candidate.integration_id),
            Some(&merge_id),
            Some(&metadata.id),
            Some(&candidate.before_sha),
            Some(&candidate.after_sha),
        )
        .await?;
        emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id).await?;
        let mut pr_service =
            crate::pr_service::PrService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        if let Some(credentials) = self
            .credentials
            .read()
            .ok()
            .and_then(|configured| configured.clone())
        {
            pr_service = pr_service.with_credentials(credentials);
        }
        let result = pr_service
            .publish_pr(task, repo, &candidate.source.branch, &target)
            .await;
        drop(publication);
        drop(merge);
        if result.is_ok() {
            let fresh_batch = TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, &batch.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("repository merge batch", batch.id.clone())
                })?;
            let fresh_metadata = PrMetadataRepo::get_by_task_repo(&*self.db, &task.id, &repo.id)
                .await?
                .ok_or_else(|| ServiceError::not_found("PR metadata", metadata.id.clone()))?;
            let row_state = match fresh_metadata.admission_status.as_str() {
                "open" => db::TaskRepositoryMergeBatchRepoState::Open,
                "merged" => db::TaskRepositoryMergeBatchRepoState::Merged,
                "closed" => db::TaskRepositoryMergeBatchRepoState::Closed,
                "failed" => db::TaskRepositoryMergeBatchRepoState::Failed,
                _ => db::TaskRepositoryMergeBatchRepoState::Publishing,
            };
            TaskRepositoryMergeBatchRepo::transition_repo(
                &*self.db,
                &fresh_batch.id,
                &repo.id,
                fresh_batch.version,
                row_state,
                Some(&candidate.integration_id),
                Some(&merge_id),
                Some(&metadata.id),
                Some(&candidate.before_sha),
                Some(&candidate.after_sha),
            )
            .await?;
            emit_repository_merge_batch_events(&self.db, &self.event_bus, &fresh_batch.id).await?;
        } else {
            // An unknown or unavailable provider result remains attached to this
            // admission for reconciliation; never create a replacement PR here.
            tracing::warn!(task_id = %task.id, repo_id = %repo.id, "repository PR admission remains in reconciliation");
        }
        Ok(())
    }

    async fn merge_repository_direct(
        &self,
        task: &db::Task,
        repo: &db::Repo,
        gate_evaluation_id: &str,
        batch: &db::TaskRepositoryMergeBatch,
        candidate: &RepositoryMergeCandidate,
    ) -> Result<MergeOutcome> {
        let _workspace_guard = self
            .workspace_exec_locks
            .acquire(&candidate.source.workspace.id)
            .await;
        let lock = self
            .integration_operations
            .lock_for_gate_admission(&task.id)
            .await?;
        let operation = self
            .integration_operations
            .acquire_kind_after_gate_with_lock(
                &task.id,
                &repo.id,
                &batch.id,
                db::TaskIntegrationOperationKind::TaskMerge,
                &db::new_uuid_v4(),
                gate_evaluation_id,
                lock,
            )
            .await?;
        let fresh_batch = TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, &batch.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repository merge batch", batch.id.clone()))?;
        TaskRepositoryMergeBatchRepo::transition_repo(
            &*self.db,
            &batch.id,
            &repo.id,
            fresh_batch.version,
            db::TaskRepositoryMergeBatchRepoState::Integrating,
            Some(&candidate.integration_id),
            Some(operation.id()),
            None,
            Some(&candidate.before_sha),
            Some(&candidate.after_sha),
        )
        .await?;
        emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id).await?;
        self.publish_domain_event_by_dedupe(&format!("task-merge-admission:{}", operation.id()))
            .await;
        let result = async {
            let source_path = Path::new(&candidate.source.workspace.worktree_path);
            if !git::is_worktree_clean(source_path).await?
                || git::get_current_sha(source_path).await? != candidate.after_sha
            {
                return Ok(MergeOutcome::Dirty {
                    files: git::status_porcelain(source_path).await?,
                });
            }
            let target_branch = target_branch(&task.merge_config, &repo.default_branch)?;
            let repo_source = self.resolve_repo_source(repo).await?;
            let repo_path = Path::new(&repo_source);
            if !git::is_worktree_clean(repo_path).await? {
                return Ok(MergeOutcome::TargetDirty {
                    files: git::status_porcelain(repo_path).await?,
                });
            }
            let before_sha = git::get_current_sha(repo_path).await?;
            git::checkout_branch(repo_path, &target_branch).await?;
            match git::merge_branch_into(repo_path, &candidate.source.branch).await {
                Ok(()) => Ok(MergeOutcome::Done {
                    before_sha,
                    after_sha: git::get_current_sha(repo_path).await?,
                    branch: target_branch,
                }),
                Err(git::GitError::MergeConflict { stderr, .. }) => {
                    let conflict_paths = read_conflict_paths(repo_path).await;
                    if let Err(error) = git::abort_merge(repo_path).await {
                        tracing::warn!(repo_id = %repo.id, %error, "failed to abort repository merge");
                    }
                    Ok(MergeOutcome::Conflict {
                        details: stderr,
                        conflict_paths,
                    })
                }
                Err(error) => Err(error.into()),
            }
        }
        .await;
        let status = merge_operation_status(&result);
        let operation_id = operation.id().to_owned();
        operation.finish(status).await?;
        self.publish_domain_event_by_dedupe(&format!("task-merge-terminal:{operation_id}"))
            .await;
        let final_row_state = match &result {
            Ok(MergeOutcome::Done { .. }) => db::TaskRepositoryMergeBatchRepoState::Integrated,
            Ok(MergeOutcome::Conflict { .. }) => db::TaskRepositoryMergeBatchRepoState::Conflict,
            Ok(MergeOutcome::Dirty { .. } | MergeOutcome::TargetDirty { .. }) | Err(_) => {
                db::TaskRepositoryMergeBatchRepoState::Failed
            }
            Ok(MergeOutcome::PullRequest { .. }) => {
                return Err(ServiceError::invalid_operation(
                    "direct merge returned a pull-request result",
                ));
            }
        };
        {
            let latest = TaskRepositoryMergeBatchRepo::get_by_id(&*self.db, &batch.id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("repository merge batch", batch.id.clone())
                })?;
            TaskRepositoryMergeBatchRepo::transition_repo(
                &*self.db,
                &batch.id,
                &repo.id,
                latest.version,
                final_row_state,
                Some(&candidate.integration_id),
                Some(operation.id()),
                None,
                Some(&candidate.before_sha),
                Some(&candidate.after_sha),
            )
            .await?;
            emit_repository_merge_batch_events(&self.db, &self.event_bus, &batch.id).await?;
        }
        result
    }

    async fn publish_domain_event_by_dedupe(&self, dedupe_key: &str) {
        let service =
            crate::DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        if let Err(error) = service.publish_by_dedupe(dedupe_key).await {
            tracing::warn!(dedupe_key, %error, "failed to hint committed Task merge lifecycle event");
        }
    }

    pub async fn publish_pr(&self, task_id: impl Into<String>) -> Result<MergeOutcome> {
        let task_id = task_id.into();
        if TaskIntegrationOperationRepo::get_active_for_task(&*self.db, &task_id)
            .await?
            .is_some()
        {
            return Err(ServiceError::Conflict(
                "Task integration operation already active".to_owned(),
            ));
        }
        Err(ServiceError::invalid_operation(
            "PR publication requires an exact satisfied merge-readiness GateEvaluation",
        ))
    }

    async fn validate_gate_candidate(
        &self,
        task_id: &str,
        evaluation: &GateEvaluation,
        source: &TaskMergeSource,
    ) -> Result<String> {
        if evaluation.task_id != task_id || evaluation.outcome != GateEvaluationOutcome::Satisfied {
            return Err(ServiceError::Conflict(
                "merge candidate requires the exact satisfied GateEvaluation".to_owned(),
            ));
        }
        let inputs = GateRepo::list_gate_evaluation_inputs(&*self.db, &evaluation.id).await?;
        let work_units = WorkUnitRepo::list_by_task(&*self.db, task_id).await?;
        if work_units.is_empty() {
            return Err(ServiceError::invalid_operation(
                "Task merge requires an exact WorkUnit integration; a Task cannot select a source Execution by recency",
            ));
        }
        let current_sha = git::get_current_sha(Path::new(&source.workspace.worktree_path)).await?;
        let mut integrations = Vec::new();
        for input in inputs
            .iter()
            .filter(|input| input.input_kind == "work_unit_integration")
        {
            let integration = WorkUnitRepo::get_integration_by_id(&*self.db, &input.input_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("WorkUnitIntegration", input.input_id.clone())
                })?;
            let subject = merge_candidate_subject(input)?;
            if integration.task_id != task_id
                || integration.version != input.input_version
                || integration.outcome != db::WorkUnitIntegrationOutcome::Success
                || integration.target_workspace_id != subject.workspace_id
                || integration.target_after_sha.as_deref() != Some(subject.commit_sha.as_str())
            {
                return Err(ServiceError::Conflict(
                    "merge Gate WorkUnitIntegration input is no longer the exact successful integration"
                        .to_owned(),
                ));
            }
            integrations.push(subject);
        }
        let matching_candidates: Vec<_> = integrations
            .iter()
            .filter(|candidate| {
                candidate.workspace_id == source.workspace.id && candidate.commit_sha == current_sha
            })
            .collect();
        if matching_candidates.len() != 1 {
            return Err(ServiceError::Conflict(
                "merge GateEvaluation must pin one exact successful WorkUnitIntegration for the current integration Workspace commit"
                    .to_owned(),
            ));
        }
        let candidate = matching_candidates[0];
        let mut verifier_subject: Option<MergeCandidateSubject> = None;
        for input in inputs.iter().filter(|input| {
            matches!(
                input.input_kind.as_str(),
                "review_report" | "validation_run"
            )
        }) {
            let subject = merge_candidate_subject(input)?;
            if subject.workspace_id != candidate.workspace_id
                || subject.commit_sha != candidate.commit_sha
            {
                return Err(ServiceError::Conflict(
                    "merge Gate ReviewReport or ValidationRun does not cover the exact integrated WorkUnit commit"
                        .to_owned(),
                ));
            }
            if verifier_subject
                .as_ref()
                .is_some_and(|existing| existing != &subject)
            {
                return Err(ServiceError::Conflict(
                    "merge Gate ReviewReport and ValidationRun refer to different snapshots"
                        .to_owned(),
                ));
            }
            verifier_subject = Some(subject);
        }

        let executions = ExecutionRepo::list_by_task(
            &*self.db,
            task_id,
            PageRequest {
                cursor: None,
                limit: 500,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        if executions
            .items
            .iter()
            .any(|execution| execution.status == db::ExecutionStatus::Running)
        {
            return Err(ServiceError::Conflict(
                "Task has a running Execution and cannot enter merge admission".to_owned(),
            ));
        }
        Ok(candidate.commit_sha.clone())
    }

    async fn ensure_current_gate_evaluation(
        &self,
        task_id: &str,
        evaluation: &GateEvaluation,
        repository_set: &db::TaskRepositorySetRevision,
        candidates: &[RepositoryMergeCandidate],
    ) -> Result<()> {
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        if gate.task_id != task_id
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
        {
            return Err(ServiceError::Conflict(
                "merge-readiness policy changed after the supplied GateEvaluation".to_owned(),
            ));
        }
        let policy_revision =
            GateRepo::get_gate_policy_revision(&*self.db, &gate.id, evaluation.policy_revision)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found(
                        "gate policy revision",
                        format!("{}:{}", gate.id, evaluation.policy_revision),
                    )
                })?;
        let writable_repository_count = repository_set
            .repositories
            .iter()
            .filter(|member| {
                matches!(
                    member.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
            })
            .count();
        if writable_repository_count > 1 && policy_revision.schema_version != 3 {
            return Err(ServiceError::invalid_operation(
                "multi-repository merge readiness requires Gate policy v3",
            ));
        }
        let engine = GateEngine::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let current = if policy_revision.schema_version == 3 {
            let subjects = candidates
                .iter()
                .map(
                    |candidate| crate::gate_engine::RepositoryValidationSubject {
                        repo_id: candidate.repo_id.clone(),
                        workspace_id: candidate.source.workspace.id.clone(),
                        commit_sha: candidate.after_sha.clone(),
                    },
                )
                .collect::<Vec<_>>();
            engine
                .evaluate_required_checks_by_repo(task_id, &subjects)
                .await?
        } else {
            engine
                .evaluate_revision(gate, evaluation.policy_revision)
                .await?
                .evaluation
        };
        if current.id != evaluation.id || current.outcome != GateEvaluationOutcome::Satisfied {
            return Err(ServiceError::Conflict(
                "supplied GateEvaluation is stale; use the current satisfied evaluation".to_owned(),
            ));
        }
        Ok(())
    }

    async fn ensure_ready_lifecycle_cause(
        &self,
        task_id: &str,
        gate_evaluation_id: &str,
    ) -> Result<()> {
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        if lifecycle.state != TaskLifecycleState::ReadyToMerge
            || lifecycle.reason_ref.as_deref() != Some(gate_evaluation_id)
        {
            return Err(ServiceError::Conflict(
                "Task is not ready to merge from the supplied GateEvaluation".to_owned(),
            ));
        }
        Ok(())
    }

    async fn resolve_repo_source(&self, repo: &db::Repo) -> Result<String> {
        if let Some(local_path) = repo
            .local_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if Path::new(local_path).exists() {
                return Ok(local_path.to_owned());
            }
        }
        let managed = self.managed_repo_path(&repo.id);
        if managed.exists() {
            return Ok(managed.to_string_lossy().into_owned());
        }
        ensure_managed_clone(&repo.remote_url, &managed).await
    }

    fn managed_repo_path(&self, repo_id: &str) -> PathBuf {
        self.workspace_root.join(".repos").join(repo_id)
    }
}

fn members_len(revision: &db::TaskRepositorySetRevision) -> usize {
    revision
        .repositories
        .iter()
        .filter(|member| {
            matches!(
                member.role,
                db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
            )
        })
        .count()
}

pub(crate) async fn emit_repository_merge_batch_events(
    db: &Arc<SqliteDb>,
    event_bus: &Arc<EventBus>,
    batch_id: &str,
) -> Result<()> {
    let Some(batch) = TaskRepositoryMergeBatchRepo::get_by_id(&**db, batch_id).await? else {
        return Err(ServiceError::not_found(
            "repository merge batch",
            batch_id.to_owned(),
        ));
    };
    let rows = TaskRepositoryMergeBatchRepo::list_repositories(&**db, batch_id).await?;
    let events = crate::DomainEventService::new(Arc::clone(db), Arc::clone(event_bus));
    for row in rows {
        let dedupe_key = format!(
            "task-repository-merge-batch:{}:{}:{}",
            batch.id, row.repo_id, batch.version
        );
        events
            .append(CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "task.repository_merge_batch_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: batch.task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: batch.task_id.clone(),
                correlation_id: batch.id.clone(),
                causation_id: row.task_merge_operation_id.clone(),
                causation_depth: 1,
                dedupe_key: Some(dedupe_key),
                payload_json: serde_json::json!({
                    "task_id": &batch.task_id,
                    "repo_id": &row.repo_id,
                    "repository_revision": batch.repository_set_revision,
                    "repository_merge_batch_id": &batch.id,
                    "integration_id": row.work_unit_repository_integration_id.as_deref(),
                    "admission_id": row.task_merge_operation_id.as_deref(),
                    "state": row.state.to_string(),
                    "batch_state": batch.state.to_string(),
                    "version": batch.version,
                })
                .to_string(),
                created_at: db::now_rfc3339(),
            })
            .await?;
    }
    Ok(())
}

fn repository_set_digest(revision: &db::TaskRepositorySetRevision) -> Result<String> {
    if let Some(digest) = revision.digest.as_deref() {
        if digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(digest.to_owned());
        }
        return Err(ServiceError::invalid_operation(
            "Task repository-set revision has an invalid digest",
        ));
    }
    let mut repositories = revision.repositories.iter().collect::<Vec<_>>();
    repositories.sort_by_key(|member| (member.position, member.repo_id.as_str()));
    let canonical = repositories
        .iter()
        .map(|member| {
            (
                member.repo_id.as_str(),
                member.role.to_string(),
                member.position,
            )
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "could not serialize Task repository-set revision: {error}"
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

async fn find_or_create_merge_batch(
    db: &SqliteDb,
    task_id: &str,
    gate_evaluation_id: &str,
    revision: &db::TaskRepositorySetRevision,
    digest: &str,
    candidates: &[RepositoryMergeCandidate],
) -> Result<Option<db::TaskRepositoryMergeBatch>> {
    if candidates.is_empty() {
        return Err(ServiceError::invalid_operation(
            "Task merge batches require at least one changed writable repository",
        ));
    }
    let batches = TaskRepositoryMergeBatchRepo::list_by_task(db, task_id).await?;
    if let Some(existing) = batches.iter().find(|batch| {
        batch.gate_evaluation_id == gate_evaluation_id
            && batch.repository_set_revision == revision.revision
    }) {
        if existing.repository_set_digest.as_deref() != Some(digest) {
            return Err(ServiceError::Conflict(
                "repository merge batch repository-set digest changed".to_owned(),
            ));
        }
        let mut expected = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.repo_id.as_str(),
                    candidate.integration_id.as_str(),
                    candidate.before_sha.as_str(),
                    candidate.after_sha.as_str(),
                )
            })
            .collect::<Vec<_>>();
        expected.sort();
        let mut actual = TaskRepositoryMergeBatchRepo::list_repositories(db, &existing.id)
            .await?
            .into_iter()
            .map(|row| {
                (
                    row.repo_id,
                    row.work_unit_repository_integration_id.unwrap_or_default(),
                    row.before_sha.unwrap_or_default(),
                    row.after_sha.unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        actual.sort();
        let actual = actual
            .iter()
            .map(|(repo_id, integration_id, before_sha, after_sha)| {
                (
                    repo_id.as_str(),
                    integration_id.as_str(),
                    before_sha.as_str(),
                    after_sha.as_str(),
                )
            })
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(ServiceError::Conflict(
                "repository merge batch does not match the exact Gate integration candidates"
                    .to_owned(),
            ));
        }
        return Ok(Some(existing.clone()));
    }
    if batches.iter().any(|batch| {
        matches!(
            batch.state,
            db::TaskRepositoryMergeBatchState::Integrating
                | db::TaskRepositoryMergeBatchState::Publishing
                | db::TaskRepositoryMergeBatchState::Open
                | db::TaskRepositoryMergeBatchState::Blocked
        )
    }) {
        return Ok(None);
    }

    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|candidate| (candidate.position, candidate.repo_id.as_str()));
    let batch_id = db::new_uuid_v4();
    let now = db::now_rfc3339();
    let state = db::TaskRepositoryMergeBatchState::Integrating;
    let batch = db::TaskRepositoryMergeBatch {
        id: batch_id.clone(),
        task_id: task_id.to_owned(),
        gate_evaluation_id: gate_evaluation_id.to_owned(),
        repository_set_revision: revision.revision,
        repository_set_digest: Some(digest.to_owned()),
        state,
        version: 1,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let repos = ordered
        .into_iter()
        .map(|candidate| db::TaskRepositoryMergeBatchRepoRow {
            batch_id: batch_id.clone(),
            task_id: task_id.to_owned(),
            repository_set_revision: revision.revision,
            repo_id: candidate.repo_id.clone(),
            position: candidate.position,
            state: db::TaskRepositoryMergeBatchRepoState::Pending,
            work_unit_repository_integration_id: Some(candidate.integration_id.clone()),
            task_merge_operation_id: None,
            pr_metadata_id: None,
            before_sha: Some(candidate.before_sha.clone()),
            after_sha: Some(candidate.after_sha.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        })
        .collect::<Vec<_>>();
    Ok(Some(
        TaskRepositoryMergeBatchRepo::create(db, &batch, &repos).await?,
    ))
}

pub(crate) async fn refresh_merge_batch_state(
    db: &SqliteDb,
    batch: &db::TaskRepositoryMergeBatch,
) -> Result<db::TaskRepositoryMergeBatch> {
    let batch = TaskRepositoryMergeBatchRepo::get_by_id(db, &batch.id)
        .await?
        .ok_or_else(|| ServiceError::not_found("repository merge batch", batch.id.clone()))?;
    let rows = TaskRepositoryMergeBatchRepo::list_repositories(db, &batch.id).await?;
    let state = if rows.is_empty()
        || rows.iter().all(|row| {
            matches!(
                row.state,
                db::TaskRepositoryMergeBatchRepoState::Integrated
                    | db::TaskRepositoryMergeBatchRepoState::Merged
            )
        }) {
        db::TaskRepositoryMergeBatchState::Completed
    } else if rows.iter().any(|row| {
        matches!(
            row.state,
            db::TaskRepositoryMergeBatchRepoState::Conflict
                | db::TaskRepositoryMergeBatchRepoState::Closed
        )
    }) {
        db::TaskRepositoryMergeBatchState::Blocked
    } else if rows
        .iter()
        .any(|row| row.state == db::TaskRepositoryMergeBatchRepoState::Failed)
    {
        db::TaskRepositoryMergeBatchState::Failed
    } else if rows.iter().all(|row| {
        matches!(
            row.state,
            db::TaskRepositoryMergeBatchRepoState::Integrated
                | db::TaskRepositoryMergeBatchRepoState::Open
                | db::TaskRepositoryMergeBatchRepoState::Merged
        )
    }) {
        db::TaskRepositoryMergeBatchState::Open
    } else if rows
        .iter()
        .any(|row| row.state == db::TaskRepositoryMergeBatchRepoState::Publishing)
    {
        db::TaskRepositoryMergeBatchState::Publishing
    } else {
        db::TaskRepositoryMergeBatchState::Integrating
    };
    if batch.state == state {
        return Ok(batch);
    }
    Ok(TaskRepositoryMergeBatchRepo::transition_batch(
        db,
        &batch.id,
        batch.version,
        state,
        &db::now_rfc3339(),
    )
    .await?)
}

async fn ensure_managed_clone(remote_url: &str, clone_path: &Path) -> Result<String> {
    if clone_path.exists() {
        return Ok(clone_path.to_string_lossy().into_owned());
    }
    if let Some(parent) = clone_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            ServiceError::invalid_operation(format!("failed to create repo cache: {error}"))
        })?;
    }
    let output = Command::new("git")
        .args(["clone", remote_url, &clone_path.to_string_lossy()])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to clone repo: {error}"))
        })?;
    if !output.status.success() {
        return Err(ServiceError::invalid_operation(
            "failed to clone the configured repository",
        ));
    }
    Ok(clone_path.to_string_lossy().into_owned())
}

async fn push_branch(worktree_path: &Path, branch: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to push branch: {error}"))
        })?;
    if !output.status.success() {
        return Err(ServiceError::invalid_operation(format!(
            "failed to push repository branch {branch}"
        )));
    }
    Ok(())
}

async fn read_conflict_paths(worktree_path: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=U"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await;

    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect(),
        Ok(output) => {
            tracing::warn!(
                worktree_path = %worktree_path.display(),
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "failed to read merge conflict paths"
            );
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(
                worktree_path = %worktree_path.display(),
                %error,
                "failed to run git diff for merge conflict paths"
            );
            Vec::new()
        }
    }
}

fn merge_candidate_subject(input: &GateEvaluationInput) -> Result<MergeCandidateSubject> {
    let subject: Value = serde_json::from_str(&input.subject_json).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "GateEvaluation input {} has invalid subject JSON: {error}",
            input.input_id
        ))
    })?;
    let (workspace_key, commit_key, snapshot_key) = match input.input_kind.as_str() {
        "review_report" => (
            "workspace_id",
            "head_commit_sha",
            Some("workspace_snapshot_digest"),
        ),
        "validation_run" => (
            "workspace_id",
            "commit_sha",
            Some("workspace_snapshot_digest"),
        ),
        "work_unit_integration" => ("workspace_id", "commit_sha", None),
        _ => {
            return Err(ServiceError::invalid_operation(
                "GateEvaluation input is not a merge candidate subject",
            ));
        }
    };
    let workspace_id = subject
        .get(workspace_key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ServiceError::invalid_operation(format!(
                "GateEvaluation input {} has no exact Workspace subject",
                input.input_id
            ))
        })?
        .to_owned();
    let commit_sha = subject
        .get(commit_key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ServiceError::invalid_operation(format!(
                "GateEvaluation input {} has no exact commit subject",
                input.input_id
            ))
        })?
        .to_owned();
    let snapshot_digest = snapshot_key
        .and_then(|key| subject.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    if snapshot_key.is_some() && snapshot_digest.is_none() {
        return Err(ServiceError::invalid_operation(format!(
            "GateEvaluation input {} has no exact Workspace snapshot subject",
            input.input_id
        )));
    }
    Ok(MergeCandidateSubject {
        workspace_id,
        commit_sha,
        snapshot_digest,
    })
}

fn merge_operation_status(result: &Result<MergeOutcome>) -> db::TaskIntegrationOperationStatus {
    match result {
        Ok(MergeOutcome::Done { .. } | MergeOutcome::PullRequest { .. }) => {
            db::TaskIntegrationOperationStatus::Succeeded
        }
        Ok(MergeOutcome::Conflict { .. }) => db::TaskIntegrationOperationStatus::Conflict,
        Ok(MergeOutcome::Dirty { .. } | MergeOutcome::TargetDirty { .. }) | Err(_) => {
            db::TaskIntegrationOperationStatus::Failed
        }
    }
}

#[cfg(test)]
async fn task_merge_source(db: &SqliteDb, task_id: &str, repo_id: &str) -> Result<TaskMergeSource> {
    if WorkUnitRepo::list_by_task(db, task_id).await?.is_empty() {
        return Err(ServiceError::invalid_operation(
            "Task merge requires an exact WorkUnit integration; no WorkUnit source can be inferred from Task Executions",
        ));
    }

    let workspace = WorkspaceRepo::get_integration_by_task_repo(db, task_id, repo_id)
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(format!(
                "Task with WorkUnits has no integration workspace for repository {repo_id}"
            ))
        })?;
    Ok(TaskMergeSource {
        branch: workspace.branch.clone(),
        workspace,
    })
}

async fn git_tree_differs(worktree_path: &Path, before_sha: &str, after_sha: &str) -> Result<bool> {
    if !valid_repository_sha(before_sha) || !valid_repository_sha(after_sha) {
        return Err(ServiceError::invalid_operation(
            "repository merge candidate has an invalid commit SHA",
        ));
    }
    let output = Command::new("git")
        .args(["diff", "--quiet", before_sha, after_sha, "--"])
        .current_dir(worktree_path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to compare repository trees: {error}"))
        })?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(ServiceError::invalid_operation(format!(
            "failed to compare repository trees: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

fn valid_repository_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn work_unit_integration_is_running(db: &SqliteDb, task_id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM work_unit_integration
         WHERE task_id = ? AND outcome = 'running')",
    )
    .bind(task_id)
    .fetch_one(db.pool())
    .await?
        != 0)
}

fn target_branch(merge_config: &Option<String>, repo_default_branch: &str) -> Result<String> {
    if let Some(merge_config) = merge_config {
        let value: Value =
            serde_json::from_str(merge_config).map_err(|error| ServiceError::InvalidOperation {
                message: format!("invalid merge_config: {error}"),
            })?;
        if let Some(target_branch) = value
            .get("target_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|target_branch| !target_branch.is_empty())
        {
            return Ok(target_branch.to_owned());
        }
    }
    let repo_default_branch = repo_default_branch.trim();
    if repo_default_branch.is_empty() {
        Ok("main".to_owned())
    } else {
        Ok(repo_default_branch.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_merge_entry_requires_exact_gate_evaluation() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        db::run_migrations(&pool).await.expect("migrations run");
        let database = Arc::new(SqliteDb::new(pool));
        let service = MergeService::new(
            Arc::clone(&database),
            Arc::new(EventBus::new(4)),
            PathBuf::from("/tmp/forge-merge-test"),
        );

        let error = service
            .merge("task-without-gate")
            .await
            .expect_err("legacy merge cannot bypass Gate admission");
        assert!(matches!(
            &error,
            ServiceError::InvalidOperation { message }
                if message.contains("exact satisfied merge-readiness GateEvaluation")
        ));
    }

    #[tokio::test]
    async fn task_merge_never_selects_a_task_execution_by_recency() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        db::run_migrations(&pool).await.expect("migrations run");
        let database = SqliteDb::new(pool);

        let error = match task_merge_source(&database, "task-without-work-units", "repo").await {
            Err(error) => error,
            Ok(_) => panic!("Task without WorkUnits must not receive an inferred merge source"),
        };
        assert!(
            matches!(
                &error,
                ServiceError::InvalidOperation { message }
                    if message.contains("exact WorkUnit integration")
                        && message.contains("no WorkUnit source can be inferred")
            ),
            "unexpected error: {error:?}"
        );
    }

    #[tokio::test]
    async fn multirepo_pr_empty_candidates_do_not_create_batch() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        db::run_migrations(&pool).await.expect("migrations run");
        let database = SqliteDb::new(pool);
        let revision = db::TaskRepositorySetRevision {
            task_id: "task-with-no-changes".to_owned(),
            revision: 1,
            digest: Some("test-digest".to_owned()),
            state: db::TaskRepositorySetRevisionState::Current,
            created_at: db::now_rfc3339(),
            source_kind: "test".to_owned(),
            source_ref: None,
            decision_id: None,
            human_decision_request_id: None,
            repositories: Vec::new(),
        };

        let error = find_or_create_merge_batch(
            &database,
            &revision.task_id,
            "gate-evaluation",
            &revision,
            "test-digest",
            &[],
        )
        .await
        .expect_err("an empty candidate set must not produce a Completed batch");
        assert!(matches!(error, ServiceError::InvalidOperation { .. }));
        assert!(
            TaskRepositoryMergeBatchRepo::list_by_task(&database, &revision.task_id)
                .await
                .expect("batch lookup")
                .is_empty()
        );
    }
}
