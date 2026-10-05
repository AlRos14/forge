use crate::gate_engine::GateEngine;
use crate::task_integration_operation::TaskIntegrationOperationManager;
use crate::{Result, ServiceError};
use db::{
    now_rfc3339, Execution, ExecutionRepo, GateEvaluation, GateEvaluationInput,
    GateEvaluationOutcome, GateRepo, PageRequest, PrProviderConfigRepo, RepoRepo, SortBy,
    SortOrder, SqliteDb, TaskIntegrationOperationRepo, TaskLifecycleRepo, TaskLifecycleState,
    TaskRepo, WorkMode, WorkUnitRepo, WorkspaceRepo,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
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
    integration_operations: TaskIntegrationOperationManager,
}

struct TaskMergeSource {
    workspace: db::Workspace,
    execution: Option<Execution>,
    branch: String,
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
            integration_operations,
        }
    }

    pub fn workspace_exec_locks(
        &self,
    ) -> Arc<crate::workspace_execution_lock::WorkspaceExecutionLockManager> {
        Arc::clone(&self.workspace_exec_locks)
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
        task_id: impl Into<String>,
        gate_evaluation_id: &str,
    ) -> Result<MergeOutcome> {
        let _ = self.event_bus.receiver_count();
        let task_id = task_id.into();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::NotFound {
                entity: "task",
                id: task_id.clone(),
            })?;
        if task.parent_task_id.is_some() {
            return Err(ServiceError::invalid_operation(
                "subtasks do not merge; only root tasks merge to the default branch",
            ));
        }
        let repo_id = task
            .repo_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound {
                entity: "repo",
                id: repo_id.to_owned(),
            })?;
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, gate_evaluation_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("GateEvaluation", gate_evaluation_id.to_owned())
            })?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.clone()))?;
        if evaluation.task_id != task_id
            || evaluation.outcome != GateEvaluationOutcome::Satisfied
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
            || lifecycle.state != TaskLifecycleState::ReadyToMerge
        {
            return Err(ServiceError::invalid_operation(
                "merge admission requires the current satisfied merge-readiness GateEvaluation",
            ));
        }
        if repo.work_mode == WorkMode::PullRequest {
            return self
                .publish_pr_after_gate(&task_id, gate_evaluation_id)
                .await;
        }
        let _local_task_lock = self
            .workspace_exec_locks
            .acquire(&format!("task-integration:{task_id}"))
            .await;
        let task_operation_file = self
            .integration_operations
            .lock_for_gate_admission(&task_id)
            .await?;
        let source = task_merge_source(&self.db, &task_id).await?;
        let _workspace_guard = self
            .workspace_exec_locks
            .acquire(&source.workspace.id)
            .await;
        self.ensure_current_gate_evaluation(&task_id, &evaluation)
            .await?;
        self.validate_gate_candidate(&task_id, &evaluation, &source)
            .await?;
        self.ensure_ready_lifecycle_cause(&task_id, gate_evaluation_id)
            .await?;
        let operation = self
            .integration_operations
            .acquire_kind_after_gate_with_lock(
                &task_id,
                db::TaskIntegrationOperationKind::TaskMerge,
                &db::new_uuid_v4(),
                gate_evaluation_id,
                task_operation_file,
            )
            .await?;
        self.publish_domain_event_by_dedupe(&format!("task-merge-admission:{}", operation.id()))
            .await;
        let result = async {
            let workspace = &source.workspace;
            if work_unit_integration_is_running(&self.db, &task_id).await? {
                return Err(ServiceError::invalid_operation(
                    "Task integration is currently incorporating a WorkUnit result",
                ));
            }
            let target_branch = target_branch(&task.merge_config, &repo.default_branch)?;
            let repo_source = self.resolve_repo_source(&repo).await?;
            let repo_path = Path::new(&repo_source);
            let worktree_path = Path::new(&workspace.worktree_path);

            if !git::is_worktree_clean(worktree_path).await? {
                return Ok(MergeOutcome::Dirty {
                    files: git::status_porcelain(worktree_path).await?,
                });
            }
            if !git::is_worktree_clean(repo_path).await? {
                return Ok(MergeOutcome::TargetDirty {
                    files: git::status_porcelain(repo_path).await?,
                });
            }

            let before_sha = git::get_current_sha(repo_path).await?;
            let worktree_sha = git::get_current_sha(worktree_path).await?;
            if let Some(execution) = source.execution.as_ref() {
                ExecutionRepo::update(
                    &*self.db,
                    db::UpdateExecution {
                        id: execution.id.clone(),
                        status: None,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        agent_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: None,
                        logs_path: None,
                        before_sha: Some(Some(worktree_sha)),
                        after_sha: None,
                        error: None,
                        executor_config_snapshot_json: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
            }

            git::checkout_branch(repo_path, &target_branch).await?;
            match git::merge_branch_into(repo_path, &source.branch).await {
                Ok(()) => {
                    let after_sha = git::get_current_sha(repo_path).await?;
                    if let Some(execution) = source.execution.as_ref() {
                        ExecutionRepo::update(
                            &*self.db,
                            db::UpdateExecution {
                                id: execution.id.clone(),
                                status: None,
                                stop_reason: None,
                                stopped_by: None,
                                resume_policy: None,
                                stopped_at: None,
                                agent_session_id: None,
                                agent_message_id: None,
                                last_activity_at: None,
                                summary: None,
                                logs_path: None,
                                before_sha: None,
                                after_sha: Some(Some(after_sha.clone())),
                                error: None,
                                executor_config_snapshot_json: None,
                                updated_at: now_rfc3339(),
                            },
                        )
                        .await?;
                    }
                    Ok(MergeOutcome::Done {
                        before_sha,
                        after_sha,
                        branch: target_branch,
                    })
                }
                Err(git::GitError::MergeConflict { stderr, .. }) => {
                    let conflict_paths = read_conflict_paths(repo_path).await;
                    if let Err(error) = git::abort_merge(repo_path).await {
                        tracing::warn!(%task_id, %error, "failed to abort merge");
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
        let finish_result = operation.finish(status).await;
        if let Ok(operation) = finish_result.as_ref() {
            self.publish_domain_event_by_dedupe(&format!("task-merge-terminal:{}", operation.id))
                .await;
        }
        drop(_workspace_guard);
        drop(_local_task_lock);
        if matches!(&result, Ok(MergeOutcome::Done { .. })) && finish_result.is_ok() {
            let cleanup_scheduler = self
                .cleanup_scheduler
                .read()
                .ok()
                .and_then(|configured| configured.clone());
            if let Some(cleanup_scheduler) = cleanup_scheduler {
                match WorkspaceRepo::get_by_task_id(&*self.db, &task_id).await {
                    Ok(Some(workspace)) => {
                        if let Err(error) = cleanup_scheduler.cleanup_now(&workspace.id).await {
                            tracing::warn!(task_id = %task_id, workspace_id = %workspace.id, %error, "post-merge workspace cleanup failed");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(task_id = %task_id, %error, "post-merge workspace lookup failed");
                    }
                }
            }
        }
        match (result, finish_result) {
            (Ok(outcome), Ok(_)) => Ok(outcome),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
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

    async fn publish_pr_after_gate(
        &self,
        task_id: &str,
        gate_evaluation_id: &str,
    ) -> Result<MergeOutcome> {
        let task_id = task_id.to_owned();
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        if task.parent_task_id.is_some() {
            return Err(ServiceError::invalid_operation(
                "subtasks do not publish pull requests; only root tasks publish",
            ));
        }
        let repo_id = task
            .repo_id
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("task has no associated repo"))?;
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        if repo.work_mode != WorkMode::PullRequest {
            return Err(ServiceError::invalid_operation(
                "publish_pr requires pull_request work mode",
            ));
        }
        let _local_task_lock = self
            .workspace_exec_locks
            .acquire(&format!("task-integration:{task_id}"))
            .await;
        let task_operation_file = self
            .integration_operations
            .lock_for_gate_admission(&task_id)
            .await?;
        let evaluation = GateRepo::get_gate_evaluation(&*self.db, gate_evaluation_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("GateEvaluation", gate_evaluation_id.to_owned())
            })?;
        let gate = GateRepo::get_gate(&*self.db, &evaluation.gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("Gate", evaluation.gate_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.clone()))?;
        if evaluation.task_id != task_id
            || evaluation.outcome != GateEvaluationOutcome::Satisfied
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != db::GateScopeKind::Task
            || gate.scope_id != task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
            || lifecycle.state != TaskLifecycleState::ReadyToMerge
        {
            return Err(ServiceError::invalid_operation(
                "PR publication requires the current satisfied merge-readiness GateEvaluation",
            ));
        }
        let source = task_merge_source(&self.db, &task_id).await?;
        let _workspace_guard = self
            .workspace_exec_locks
            .acquire(&source.workspace.id)
            .await;
        self.ensure_current_gate_evaluation(&task_id, &evaluation)
            .await?;
        self.validate_gate_candidate(&task_id, &evaluation, &source)
            .await?;
        self.ensure_ready_lifecycle_cause(&task_id, gate_evaluation_id)
            .await?;
        if work_unit_integration_is_running(&self.db, &task_id).await? {
            return Err(ServiceError::invalid_operation(
                "Task integration is currently incorporating a WorkUnit result",
            ));
        }
        let target_branch = target_branch(&task.merge_config, &repo.default_branch)?;
        let source_branch = source.branch.clone();
        let worktree_path = Path::new(&source.workspace.worktree_path);
        if !git::is_worktree_clean(worktree_path).await? {
            return Ok(MergeOutcome::Dirty {
                files: git::status_porcelain(worktree_path).await?,
            });
        }
        // Push the pinned source before persisting the PR admission. A crash
        // after admission therefore always leaves a provider-ready branch and
        // an atomic PublishPr plus metadata intent for startup recovery.
        push_branch(worktree_path, &source_branch).await?;
        self.ensure_current_gate_evaluation(&task_id, &evaluation)
            .await?;
        let source_sha = self
            .validate_gate_candidate(&task_id, &evaluation, &source)
            .await?;
        self.ensure_ready_lifecycle_cause(&task_id, gate_evaluation_id)
            .await?;
        let provider_config = PrProviderConfigRepo::get_by_repo_id(&*self.db, &repo.id)
            .await?
            .ok_or_else(|| ServiceError::PrProviderMissing {
                repo_id: repo.id.clone(),
            })?;
        let (merge_admission, publication) = self
            .integration_operations
            .admit_pull_request_publication_with_lock(
                &task_id,
                &db::new_uuid_v4(),
                gate_evaluation_id,
                &provider_config,
                &repo.remote_url,
                &source_branch,
                &target_branch,
                &source_sha,
                task_operation_file,
            )
            .await?;
        self.publish_domain_event_by_dedupe(&format!(
            "task-merge-admission:{}",
            merge_admission.id()
        ))
        .await;
        let result = async {
            let pr_service = crate::pr_service::PrService::new(
                Arc::clone(&self.db),
                Arc::clone(&self.event_bus),
            );
            let published = pr_service
                .publish_pr(&task, &repo, &source_branch, &target_branch)
                .await?;

            Ok(MergeOutcome::PullRequest {
                pr_url: published.pr_url,
                branch: source_branch,
                target_branch,
            })
        }
        .await;
        // The provider result writer owns the single transaction that may
        // finish PublishPr, TaskMerge, metadata, and lifecycle. Dropping these
        // guards releases local locks only; an unknown outcome remains running.
        drop(publication);
        drop(merge_admission);
        result
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
        let candidate = if work_units.is_empty() {
            let mut candidate: Option<MergeCandidateSubject> = None;
            for input in inputs.iter().filter(|input| {
                matches!(
                    input.input_kind.as_str(),
                    "review_report" | "validation_run"
                )
            }) {
                let subject = merge_candidate_subject(input)?;
                if candidate
                    .as_ref()
                    .is_some_and(|current| current != &subject)
                {
                    return Err(ServiceError::Conflict(
                        "merge Gate inputs refer to different workspace commits or snapshots"
                            .to_owned(),
                    ));
                }
                candidate = Some(subject);
            }
            candidate.ok_or_else(|| {
                ServiceError::Conflict(
                    "merge-readiness evaluation has no exact ReviewReport or ValidationRun candidate"
                        .to_owned(),
                )
            })?
        } else {
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
                integrations.push((integration.created_at, integration.id, subject));
            }
            integrations.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
            let (_, _, candidate) = integrations.pop().ok_or_else(|| {
                ServiceError::Conflict(
                    "Task with WorkUnits requires an exact successful WorkUnitIntegration input before merge"
                        .to_owned(),
                )
            })?;
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
            candidate
        };

        if source.workspace.id != candidate.workspace_id {
            return Err(ServiceError::Conflict(
                "merge source Workspace differs from the exact GateEvaluation candidate".to_owned(),
            ));
        }
        if let Some(execution) = source.execution.as_ref() {
            if execution.status != db::ExecutionStatus::Completed
                || execution.workspace_id.as_deref() != Some(candidate.workspace_id.as_str())
                || execution
                    .after_sha
                    .as_deref()
                    .is_some_and(|sha| sha != candidate.commit_sha)
            {
                return Err(ServiceError::Conflict(
                    "merge source Execution does not match the exact GateEvaluation candidate"
                        .to_owned(),
                ));
            }
        } else if work_units.is_empty() {
            return Err(ServiceError::Conflict(
                "merge candidate has no exact completed source Execution".to_owned(),
            ));
        }
        let current_sha = git::get_current_sha(Path::new(&source.workspace.worktree_path)).await?;
        if current_sha != candidate.commit_sha {
            return Err(ServiceError::Conflict(
                "merge source commit is stale relative to the exact GateEvaluation candidate"
                    .to_owned(),
            ));
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
        Ok(candidate.commit_sha)
    }

    async fn ensure_current_gate_evaluation(
        &self,
        task_id: &str,
        evaluation: &GateEvaluation,
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
        let current = GateEngine::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .evaluate_revision(gate, evaluation.policy_revision)
            .await?;
        if current.evaluation.id != evaluation.id
            || current.evaluation.outcome != GateEvaluationOutcome::Satisfied
        {
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
        return Err(ServiceError::invalid_operation(format!(
            "failed to clone repo from {remote_url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
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
            "failed to push branch {branch}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
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

async fn latest_executor_execution(db: &SqliteDb, task_id: &str) -> Result<Execution> {
    let page = ExecutionRepo::list_by_task(
        db,
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
    page.items
        .into_iter()
        .find(|execution| {
            execution.work_unit_id.is_none()
                && execution.status == db::ExecutionStatus::Completed
                && matches!(
                    execution.role.as_str(),
                    "executor" | "coder" | "worker" | "implementer"
                )
        })
        .ok_or_else(|| ServiceError::InvalidOperation {
            message: format!("task {task_id} has no executor execution"),
        })
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

async fn task_merge_source(db: &SqliteDb, task_id: &str) -> Result<TaskMergeSource> {
    if !WorkUnitRepo::list_by_task(db, task_id).await?.is_empty() {
        let workspace = WorkspaceRepo::get_by_task_id(db, task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation("Task with WorkUnits has no integration workspace")
            })?;
        return Ok(TaskMergeSource {
            branch: workspace.branch.clone(),
            workspace,
            execution: None,
        });
    }

    let execution = latest_executor_execution(db, task_id).await?;
    let workspace_id = execution.workspace_id.as_deref().ok_or_else(|| {
        ServiceError::invalid_operation("executor execution missing workspace_id")
    })?;
    let workspace = WorkspaceRepo::get_by_id(db, workspace_id)
        .await?
        .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
    Ok(TaskMergeSource {
        branch: workspace::task_branch_name(task_id),
        workspace,
        execution: Some(execution),
    })
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
            error,
            ServiceError::InvalidOperation { message }
                if message.contains("exact satisfied merge-readiness GateEvaluation")
        ));
    }
}
