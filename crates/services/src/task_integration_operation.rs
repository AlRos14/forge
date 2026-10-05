use db::{
    new_uuid_v4, now_rfc3339, CreateTaskIntegrationOperation, FinishTaskIntegrationOperation,
    SqliteDb, TaskIntegrationOperation, TaskIntegrationOperationKind, TaskIntegrationOperationRepo,
    TaskIntegrationOperationStatus,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{fs::File, fs::OpenOptions, path::PathBuf, sync::Arc};

use crate::ServiceError;

/// Owns the durable SQLite claim and the process-lifetime lock used to prove
/// that a persisted `running` row has no live owner before it is abandoned.
#[derive(Clone)]
pub(crate) struct TaskIntegrationOperationManager {
    db: Arc<SqliteDb>,
    workspace_root: PathBuf,
}

impl TaskIntegrationOperationManager {
    pub(crate) fn new(db: Arc<SqliteDb>, workspace_root: PathBuf) -> Self {
        Self { db, workspace_root }
    }

    pub(crate) async fn acquire(
        &self,
        task_id: &str,
        kind: TaskIntegrationOperationKind,
        owner_id: &str,
    ) -> crate::Result<TaskIntegrationOperationGuard> {
        self.acquire_with_gate(task_id, kind, owner_id, None).await
    }

    /// Hold the cross-process Task lock before a merge flow reads its source
    /// Workspace and validates the Gate candidate. If a durable running row
    /// remains after this lock is acquired, its former owner is gone; reconcile
    /// it and require the caller to evaluate Gate readiness again.
    pub(crate) async fn lock_for_gate_admission(&self, task_id: &str) -> crate::Result<File> {
        let file = match self.try_process_lock(task_id).await? {
            Ok(file) => file,
            Err(()) => return Err(busy(task_id)),
        };
        if let Some(operation) =
            TaskIntegrationOperationRepo::get_active_for_task(&*self.db, task_id).await?
        {
            let now = now_rfc3339();
            if let Some(abandoned) =
                TaskIntegrationOperationRepo::abandon_stale(&*self.db, task_id, &now).await?
            {
                tracing::info!(
                    task_id,
                    operation_id = %abandoned.id,
                    "reconciled stale Task integration operation before Gate admission"
                );
            }
            return Err(ServiceError::Conflict(format!(
                "previous Task integration operation {} was reconciled; re-evaluate merge readiness",
                operation.id
            )));
        }
        Ok(file)
    }

    /// PR polling and publication recovery use the same per-Task process lock
    /// as Git integration, but do not abandon the durable remote admission.
    pub(crate) async fn try_pr_recovery_lock(
        &self,
        task_id: &str,
        merge_operation_id: &str,
        publish_operation_id: &str,
    ) -> crate::Result<Option<File>> {
        let file = match self.try_process_lock(task_id).await? {
            Ok(file) => file,
            Err(()) => return Ok(None),
        };
        let Some(merge) =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, merge_operation_id).await?
        else {
            return Ok(None);
        };
        let Some(publish) =
            TaskIntegrationOperationRepo::get_by_id(&*self.db, publish_operation_id).await?
        else {
            return Ok(None);
        };
        if merge.task_id != task_id
            || merge.kind != TaskIntegrationOperationKind::TaskMerge
            || !merge.remote_waiting
            || publish.task_id != task_id
            || publish.kind != TaskIntegrationOperationKind::PublishPr
            || publish.parent_operation_id.as_deref() != Some(merge_operation_id)
            || publish.gate_evaluation_id != merge.gate_evaluation_id
        {
            return Ok(None);
        }
        Ok(Some(file))
    }

    pub(crate) async fn acquire_kind_after_gate_with_lock(
        &self,
        task_id: &str,
        kind: TaskIntegrationOperationKind,
        owner_id: &str,
        gate_evaluation_id: &str,
        file: File,
    ) -> crate::Result<TaskIntegrationOperationGuard> {
        let input = CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            kind,
            owner_id: owner_id.to_owned(),
            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
            remote_waiting: false,
            parent_operation_id: None,
            created_at: now_rfc3339(),
        };
        let operation = match TaskIntegrationOperationRepo::begin(&*self.db, input.clone()).await {
            Ok(operation) => operation,
            Err(db::DbError::TaskIntegrationOperationBusy) => {
                TaskIntegrationOperationRepo::recover_stale_and_begin(
                    &*self.db,
                    input,
                    &now_rfc3339(),
                )
                .await?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(TaskIntegrationOperationGuard {
            db: Arc::clone(&self.db),
            operation,
            _file: Some(file),
        })
    }

    pub(crate) async fn admit_pull_request_publication_with_lock(
        &self,
        task_id: &str,
        owner_id: &str,
        gate_evaluation_id: &str,
        provider_type: &str,
        source_branch: &str,
        target_branch: &str,
        file: File,
    ) -> crate::Result<(TaskIntegrationOperationGuard, TaskIntegrationOperationGuard)> {
        let now = now_rfc3339();
        let merge_id = new_uuid_v4();
        let publish_id = new_uuid_v4();
        let merge_input = CreateTaskIntegrationOperation {
            id: merge_id.clone(),
            task_id: task_id.to_owned(),
            kind: TaskIntegrationOperationKind::TaskMerge,
            owner_id: owner_id.to_owned(),
            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
            remote_waiting: true,
            parent_operation_id: None,
            created_at: now.clone(),
        };
        let publish_input = CreateTaskIntegrationOperation {
            id: publish_id.clone(),
            task_id: task_id.to_owned(),
            kind: TaskIntegrationOperationKind::PublishPr,
            owner_id: owner_id.to_owned(),
            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
            remote_waiting: false,
            parent_operation_id: Some(merge_id.clone()),
            created_at: now.clone(),
        };
        let metadata_input = db::CreatePrMetadata {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            provider_type: provider_type.to_owned(),
            provider_pr_id: None,
            pr_url: None,
            source_branch: source_branch.to_owned(),
            target_branch: target_branch.to_owned(),
            pr_state: "publishing".to_owned(),
            merge_status: "pending".to_owned(),
            task_merge_operation_id: merge_id,
            publish_operation_id: publish_id,
            last_synced_at: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let (merge, publish) = TaskIntegrationOperationRepo::begin_pull_request_publication(
            &*self.db,
            merge_input,
            publish_input,
            metadata_input,
        )
        .await?;
        Ok((
            TaskIntegrationOperationGuard {
                db: Arc::clone(&self.db),
                operation: merge,
                _file: Some(file),
            },
            TaskIntegrationOperationGuard {
                db: Arc::clone(&self.db),
                operation: publish,
                _file: None,
            },
        ))
    }

    async fn acquire_with_gate(
        &self,
        task_id: &str,
        kind: TaskIntegrationOperationKind,
        owner_id: &str,
        gate_evaluation_id: Option<&str>,
    ) -> crate::Result<TaskIntegrationOperationGuard> {
        let input = CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            kind,
            owner_id: owner_id.to_owned(),
            gate_evaluation_id: gate_evaluation_id.map(str::to_owned),
            remote_waiting: false,
            parent_operation_id: None,
            created_at: now_rfc3339(),
        };

        let file = match self.try_process_lock(task_id).await? {
            Ok(file) => file,
            Err(()) => return Err(busy(task_id)),
        };
        let operation = match TaskIntegrationOperationRepo::begin(&*self.db, input.clone()).await {
            Ok(operation) => operation,
            Err(db::DbError::TaskIntegrationOperationBusy) => {
                TaskIntegrationOperationRepo::recover_stale_and_begin(
                    &*self.db,
                    input,
                    &now_rfc3339(),
                )
                .await
                .map_err(|error| match error {
                    db::DbError::TaskIntegrationOperationBusy => busy(task_id),
                    error => error.into(),
                })?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(TaskIntegrationOperationGuard {
            db: Arc::clone(&self.db),
            operation,
            _file: Some(file),
        })
    }

    /// Reconcile a persisted running row for consumers that are blocked by
    /// durable state but do not start a replacement operation. The same
    /// canonical Task lock used by `acquire` is held through the durable
    /// transition, so a live owner is never abandoned.
    pub(crate) async fn reconcile_stale(&self, task_id: &str) -> crate::Result<()> {
        if TaskIntegrationOperationRepo::get_active_for_task(&*self.db, task_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        let Some(file) = self.try_reconciled_task_lock(task_id).await? else {
            return Err(busy(task_id));
        };
        drop(file);
        Ok(())
    }

    /// Return the existing Task operation lock, after reconciling any row whose
    /// owner no longer holds it. WorkUnit cleanup keeps this file lock for its
    /// Git work while the Workspace lifecycle remains its durable claim.
    pub(crate) async fn try_work_unit_cleanup_task_lock(
        &self,
        task_id: &str,
    ) -> crate::Result<Option<File>> {
        self.try_reconciled_task_lock(task_id).await
    }

    async fn try_reconciled_task_lock(&self, task_id: &str) -> crate::Result<Option<File>> {
        let file = match self.try_process_lock(task_id).await? {
            Ok(file) => file,
            Err(()) => return Ok(None),
        };
        let now = now_rfc3339();
        if let Some(operation) =
            TaskIntegrationOperationRepo::abandon_stale(&*self.db, task_id, &now).await?
        {
            tracing::info!(
                task_id,
                operation_id = %operation.id,
                "abandoned stale Task integration operation"
            );
        }
        Ok(Some(file))
    }

    async fn try_process_lock(&self, task_id: &str) -> crate::Result<Result<File, ()>> {
        self.try_file_lock(task_id.as_bytes()).await
    }

    async fn try_file_lock(&self, lock_key: &[u8]) -> crate::Result<Result<File, ()>> {
        let lock_root = self.lock_root().await?;
        tokio::fs::create_dir_all(&lock_root)
            .await
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not prepare Task integration operation lock: {error}"
                ))
            })?;
        let lock_key = hex::encode(Sha256::digest(lock_key));
        let lock_path = lock_root.join(format!("{lock_key}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not open Task integration operation lock: {error}"
                ))
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Ok(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(Err(())),
            Err(std::fs::TryLockError::Error(error)) => Err(ServiceError::invalid_operation(
                format!("could not acquire Task integration operation lock: {error}"),
            )),
        }
    }

    async fn lock_root(&self) -> crate::Result<PathBuf> {
        let databases = sqlx::query("PRAGMA database_list")
            .fetch_all(self.db.pool())
            .await?;
        let main_path = databases
            .iter()
            .find(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("main"))
            .and_then(|row| row.try_get::<String, _>("file").ok())
            .filter(|path| !path.is_empty());
        let lock_parent = if let Some(path) = main_path {
            let database_path = tokio::fs::canonicalize(path).await.map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not resolve Task integration database path: {error}"
                ))
            })?;
            database_path.parent().map(PathBuf::from).ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Task integration database path has no parent directory",
                )
            })?
        } else {
            self.workspace_root.clone()
        };
        Ok(lock_parent.join(".forge-task-integration-locks"))
    }
}

pub(crate) struct TaskIntegrationOperationGuard {
    db: Arc<SqliteDb>,
    operation: TaskIntegrationOperation,
    _file: Option<File>,
}

impl TaskIntegrationOperationGuard {
    pub(crate) fn id(&self) -> &str {
        &self.operation.id
    }

    pub(crate) async fn finish(
        self,
        status: TaskIntegrationOperationStatus,
    ) -> crate::Result<TaskIntegrationOperation> {
        self.finish_with_result_event(status, None).await
    }

    pub(crate) async fn finish_with_result_event(
        self,
        status: TaskIntegrationOperationStatus,
        result_event_id: Option<String>,
    ) -> crate::Result<TaskIntegrationOperation> {
        let now = now_rfc3339();
        let result = TaskIntegrationOperationRepo::finish(
            &*self.db,
            FinishTaskIntegrationOperation {
                id: self.operation.id.clone(),
                expected_version: self.operation.version,
                status,
                result_event_id,
                updated_at: now.clone(),
                finished_at: now,
            },
        )
        .await;
        drop(self);
        result.map_err(Into::into)
    }
}

fn busy(task_id: &str) -> ServiceError {
    ServiceError::conflict(format!(
        "Task {task_id} already has an exclusive integration workspace operation"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, run_migrations, CreatePrProviderConfig, CreateProject, CreateRepo,
        CreateTask, CreateTerminalSession, CreateWorkspace, PrMetadataRepo, PrProviderConfigRepo,
        ProjectRepo, RepoRepo, TaskIntegrationOperationKind, TaskRepo, TerminalSessionRepo,
        UserRepo, WorkMode, WorkspaceRepo, WorkspaceStatus,
    };
    use events::EventBus;
    use std::path::Path;
    use tempfile::TempDir;

    struct Fixture {
        db: Arc<SqliteDb>,
        task_id: String,
        repo_id: String,
        user_id: String,
        workspace_id: String,
    }

    async fn fixture(database_url: &str, root: &Path) -> Fixture {
        let pool = create_sqlite_pool(database_url).await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        seed_fixture(Arc::new(SqliteDb::new(pool)), root).await
    }

    async fn seed_fixture(db: Arc<SqliteDb>, root: &Path) -> Fixture {
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let repo_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let user_id = new_uuid_v4();
        let workspace_id = new_uuid_v4();
        UserRepo::create_user(
            &*db,
            &db::User {
                id: user_id.clone(),
                email: format!("{}@example.invalid", user_id),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("user");
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "Task integration operation recovery".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(user_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project");
        RepoRepo::create(
            &*db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "repo".to_owned(),
                remote_url: "https://example.invalid/repo.git".to_owned(),
                local_path: None,
                work_mode: WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("repository");
        TaskRepo::create(
            &*db,
            CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: Some(repo_id.clone()),
                parent_task_id: None,
                subtask_order: None,
                assignee_type: None,
                assignee_id: None,
                title: "Operation recovery".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task");
        WorkspaceRepo::create(
            &*db,
            CreateWorkspace {
                id: workspace_id.clone(),
                task_id: task_id.clone(),
                repo_id: repo_id.clone(),
                worktree_path: root.join("task-worktree").to_string_lossy().into_owned(),
                branch: format!("task/{}", &task_id[..8]),
                status: WorkspaceStatus::Ready,
                before_sha: Some("base-sha".to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("integration workspace");
        Fixture {
            db,
            task_id,
            repo_id,
            user_id,
            workspace_id,
        }
    }

    async fn begin_operation(db: &SqliteDb, task_id: &str) -> db::TaskIntegrationOperation {
        TaskIntegrationOperationRepo::begin(
            db,
            db::CreateTaskIntegrationOperation {
                id: new_uuid_v4(),
                task_id: task_id.to_owned(),
                kind: TaskIntegrationOperationKind::WorkUnitIntegration,
                owner_id: "simulated-operation-owner".to_owned(),
                gate_evaluation_id: None,
                remote_waiting: false,
                parent_operation_id: None,
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("durable running operation")
    }

    struct TestTokenEnv(String);

    impl TestTokenEnv {
        fn set() -> Self {
            let key = format!("FORGE_TEST_PR_TOKEN_{}", new_uuid_v4().replace('-', "_"));
            std::env::set_var(&key, "test-token");
            Self(key)
        }
    }

    impl Drop for TestTokenEnv {
        fn drop(&mut self) {
            std::env::remove_var(&self.0);
        }
    }

    async fn prepare_pr_admission(
        fixture: &Fixture,
        root: &Path,
        event_bus: Arc<EventBus>,
    ) -> (
        TaskIntegrationOperationManager,
        TaskIntegrationOperationGuard,
        TaskIntegrationOperationGuard,
        db::GateEvaluation,
        String,
        crate::gate_engine::GatePolicyDocument,
        TestTokenEnv,
    ) {
        RepoRepo::update(
            &*fixture.db,
            db::UpdateRepo {
                id: fixture.repo_id.clone(),
                name: None,
                local_path: None,
                remote_url: None,
                work_mode: Some(WorkMode::PullRequest),
                default_branch: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("repository enters pull-request mode");
        let token_env = TestTokenEnv::set();
        PrProviderConfigRepo::create(
            &*fixture.db,
            CreatePrProviderConfig {
                id: new_uuid_v4(),
                repo_id: fixture.repo_id.clone(),
                provider_type: "github".to_owned(),
                base_url: Some("https://github.example.invalid".to_owned()),
                polling_interval_seconds: 1,
                token_secret_ref: Some(token_env.0.clone()),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("PR provider config");

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: fixture.task_id.clone(),
            expected_task_version: task.version,
            to_state: db::TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::System(api_types::SystemComponent::Test),
            reason_kind: Some("test_work_started".to_owned()),
            reason_ref: Some("test".to_owned()),
            idempotency_key: format!("test-active:{}", fixture.task_id),
        })
        .await
        .expect("Task enters active work");
        let collaboration = crate::collaboration_service::CollaborationService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        );
        let proposal = collaboration
            .create_proposal(
                crate::collaboration_service::CollaborationActorSource::Human(
                    fixture.user_id.clone(),
                ),
                crate::collaboration_service::CreateProposalInput {
                    task_id: fixture.task_id.clone(),
                    target: db::ProposalTarget {
                        kind: db::ProposalTargetKind::Task,
                        id: fixture.task_id.clone(),
                    },
                    action: "merge".to_owned(),
                    reason: "Approve the exact readiness decision for this test".to_owned(),
                    target_version: None,
                    target_digest: None,
                    required_policy_ref: None,
                    required_policy_version: None,
                    required_policy_digest: None,
                    supersedes_proposal_id: None,
                    artifact_ids: Vec::new(),
                },
            )
            .await
            .expect("merge Proposal");
        let decision = collaboration
            .record_decision(
                crate::collaboration_service::CreateDecisionInput {
                    task_id: fixture.task_id.clone(),
                    proposal_id: proposal.id.clone(),
                    proposal_version: proposal.content_version,
                    outcome: db::DecisionOutcome::Approve,
                    rationale: "Approve this exact merge readiness test".to_owned(),
                    policy_ref: None,
                    policy_version: None,
                    policy_digest: None,
                },
                vec![
                    crate::collaboration_service::CollaborationActorSource::Human(
                        fixture.user_id.clone(),
                    ),
                ],
            )
            .await
            .expect("exact Human Decision");
        let policy = crate::gate_engine::GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: Vec::new(),
            decisions: vec![crate::gate_engine::DecisionRequirement {
                proposal_id: proposal.id,
                proposal_version: decision.proposal_version,
                decision_id: decision.id,
                outcome: db::DecisionOutcome::Approve,
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
                permitted_deciders: vec![db::ActorRef::Human(fixture.user_id.clone())],
            }],
            work_units: Vec::new(),
        };
        let engine =
            crate::gate_engine::GateEngine::new(Arc::clone(&fixture.db), Arc::clone(&event_bus));
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &fixture.task_id,
                "merge_readiness",
                db::GateScopeKind::Task,
                &fixture.task_id,
                policy.clone(),
            )
            .await
            .expect("exact merge-readiness Gate");
        let evaluation = engine
            .evaluate_active(&gate.id)
            .await
            .expect("Gate evaluation");
        assert_eq!(
            evaluation.evaluation.outcome,
            db::GateEvaluationOutcome::Satisfied
        );
        let evaluation_event = evaluation
            .event
            .as_ref()
            .expect("Gate evaluation has a durable event");
        engine
            .process_domain_event(evaluation_event)
            .await
            .expect("exact GateEvaluation event admits merge readiness");
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("merge-readiness lifecycle lookup")
            .expect("Task lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::ReadyToMerge);
        assert_eq!(
            lifecycle.reason_ref.as_deref(),
            Some(evaluation.evaluation.id.as_str())
        );
        let manager =
            TaskIntegrationOperationManager::new(Arc::clone(&fixture.db), root.to_path_buf());
        let lock = manager
            .lock_for_gate_admission(&fixture.task_id)
            .await
            .expect("per-Task lock before PR admission");
        let (merge, publish) = manager
            .admit_pull_request_publication_with_lock(
                &fixture.task_id,
                &new_uuid_v4(),
                &evaluation.evaluation.id,
                "github",
                &format!("task/{}", &fixture.task_id[..8]),
                "main",
                lock,
            )
            .await
            .expect("TaskMerge, PublishPr, and publication intent commit atomically");
        (
            manager,
            merge,
            publish,
            evaluation.evaluation,
            gate.id,
            policy,
            token_env,
        )
    }

    async fn create_test_pr_metadata(
        fixture: &Fixture,
        merge: &TaskIntegrationOperationGuard,
        publish: &TaskIntegrationOperationGuard,
        pr_state: &str,
    ) -> db::PrMetadata {
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("PR metadata reload")
            .expect("PR metadata exists from the atomic admission");
        PrMetadataRepo::update(
            &*fixture.db,
            db::UpdatePrMetadata {
                id: metadata.id,
                provider_type: None,
                provider_pr_id: Some(Some(format!("provider-{}", fixture.task_id))),
                pr_url: Some(Some(format!(
                    "https://github.example.invalid/pull/{}",
                    fixture.task_id
                ))),
                source_branch: None,
                target_branch: None,
                pr_state: Some(pr_state.to_owned()),
                merge_status: None,
                task_merge_operation_id: None,
                publish_operation_id: None,
                last_synced_at: Some(Some(now_rfc3339())),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("exact provider response is stored over durable publication intent");
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("PR metadata reload")
            .expect("PR metadata remains present");
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge.id())
        );
        assert_eq!(metadata.publish_operation_id.as_deref(), Some(publish.id()));
        metadata
    }

    #[tokio::test]
    async fn acquire_writes_no_claim_when_process_lock_setup_fails() {
        let temp = TempDir::new().expect("temporary directory");
        let db_fixture = fixture("sqlite::memory:", temp.path()).await;
        let lock_root_file = temp.path().join("not-a-directory");
        std::fs::write(&lock_root_file, "file blocks lock directory").expect("write file");
        let manager =
            TaskIntegrationOperationManager::new(Arc::clone(&db_fixture.db), lock_root_file);

        let error = match manager
            .acquire(
                &db_fixture.task_id,
                TaskIntegrationOperationKind::TaskMerge,
                "claim-that-cannot-own-a-lock",
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("filesystem lock setup should fail"),
        };
        assert!(error
            .to_string()
            .contains("could not prepare Task integration operation lock"));
        assert!(TaskIntegrationOperationRepo::get_active_for_task(
            &*db_fixture.db,
            &db_fixture.task_id,
        )
        .await
        .expect("active operation lookup")
        .is_none());
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM task_integration_operation WHERE task_id = ?")
                .bind(&db_fixture.task_id)
                .fetch_all(db_fixture.db.pool())
                .await
                .expect("operation history");
        assert!(statuses.is_empty(), "claim starts only after its OS lock");
    }

    #[tokio::test]
    async fn reconciliation_abandons_running_operation_without_process_lock_owner() {
        let temp = TempDir::new().expect("temporary directory");
        let db_fixture = fixture("sqlite::memory:", temp.path()).await;
        let active = begin_operation(&db_fixture.db, &db_fixture.task_id).await;
        let manager = TaskIntegrationOperationManager::new(
            Arc::clone(&db_fixture.db),
            temp.path().to_path_buf(),
        );

        manager
            .reconcile_stale(&db_fixture.task_id)
            .await
            .expect("free canonical process lock proves stale owner");
        let status: String =
            sqlx::query_scalar("SELECT status FROM task_integration_operation WHERE id = ?")
                .bind(&active.id)
                .fetch_one(db_fixture.db.pool())
                .await
                .expect("durable operation history");
        assert_eq!(status, "abandoned");
    }

    #[tokio::test]
    async fn reconciliation_keeps_operation_running_while_owner_lock_is_held() {
        let temp = TempDir::new().expect("temporary directory");
        let database_path = temp.path().join("live-owner.db");
        let database_url = format!("sqlite://{}", database_path.display());
        let db_fixture = fixture(&database_url, temp.path()).await;
        let competing_pool = create_sqlite_pool(&database_url)
            .await
            .expect("independent pool");
        let competing_db = Arc::new(SqliteDb::new(competing_pool));
        let owner = TaskIntegrationOperationManager::new(
            Arc::clone(&db_fixture.db),
            temp.path().to_path_buf(),
        )
        .acquire(
            &db_fixture.task_id,
            TaskIntegrationOperationKind::WorkUnitIntegration,
            "live-owner",
        )
        .await
        .expect("live owner holds the canonical process lock");
        let active =
            TaskIntegrationOperationRepo::get_active_for_task(&*db_fixture.db, &db_fixture.task_id)
                .await
                .expect("active operation lookup")
                .expect("operation remains running");
        let reconciler =
            TaskIntegrationOperationManager::new(competing_db, temp.path().to_path_buf());

        assert!(matches!(
            reconciler.reconcile_stale(&db_fixture.task_id).await,
            Err(ServiceError::Conflict(_))
        ));
        assert_eq!(
            TaskIntegrationOperationRepo::get_active_for_task(
                &*db_fixture.db,
                &db_fixture.task_id,
            )
            .await
            .expect("operation still active")
            .expect("live owner's row remains")
            .id,
            active.id
        );
        drop(owner);
    }

    #[tokio::test]
    async fn gate_admission_lock_blocks_cross_process_operation_before_candidate_read() {
        let temp = TempDir::new().expect("temporary directory");
        let database_path = temp.path().join("gate-admission-lock.db");
        let database_url = format!("sqlite://{}", database_path.display());
        let db_fixture = fixture(&database_url, temp.path()).await;
        let competing_pool = create_sqlite_pool(&database_url)
            .await
            .expect("independent database pool");
        let competing_db = Arc::new(SqliteDb::new(competing_pool));
        let merge_owner = TaskIntegrationOperationManager::new(
            Arc::clone(&db_fixture.db),
            temp.path().to_path_buf(),
        );
        let competing = TaskIntegrationOperationManager::new(
            Arc::clone(&competing_db),
            temp.path().to_path_buf(),
        );

        let held = merge_owner
            .lock_for_gate_admission(&db_fixture.task_id)
            .await
            .expect("merge validates its candidate while holding the process lock");
        assert!(matches!(
            competing
                .acquire(
                    &db_fixture.task_id,
                    TaskIntegrationOperationKind::WorkUnitIntegration,
                    "competing-integration",
                )
                .await,
            Err(ServiceError::Conflict(_))
        ));
        assert!(TaskIntegrationOperationRepo::get_active_for_task(
            &*db_fixture.db,
            &db_fixture.task_id,
        )
        .await
        .expect("no operation row was written before acquiring the lock")
        .is_none());

        drop(held);
        let integration = competing
            .acquire(
                &db_fixture.task_id,
                TaskIntegrationOperationKind::WorkUnitIntegration,
                "after-merge-candidate-read",
            )
            .await
            .expect("operation begins after candidate lock is released");
        integration
            .finish(TaskIntegrationOperationStatus::Failed)
            .await
            .expect("finish operation");
    }

    #[tokio::test]
    async fn stale_running_operation_no_longer_blocks_terminal_session_admission() {
        let temp = TempDir::new().expect("temporary directory");
        let db_fixture = fixture("sqlite::memory:", temp.path()).await;
        let active = begin_operation(&db_fixture.db, &db_fixture.task_id).await;
        let terminal = || CreateTerminalSession {
            id: new_uuid_v4(),
            task_id: db_fixture.task_id.clone(),
            workspace_id: db_fixture.workspace_id.clone(),
            daemon_id: None,
            created_by_user_id: db_fixture.user_id.clone(),
            rows: 24,
            cols: 80,
            created_at: now_rfc3339(),
        };

        assert!(
            TerminalSessionRepo::create_terminal_session(&*db_fixture.db, terminal())
                .await
                .is_err()
        );
        TaskIntegrationOperationManager::new(Arc::clone(&db_fixture.db), temp.path().to_path_buf())
            .reconcile_stale(&db_fixture.task_id)
            .await
            .expect("dead operation owner is abandoned");
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM task_integration_operation WHERE id = ?",
            )
            .bind(&active.id)
            .fetch_one(db_fixture.db.pool())
            .await
            .expect("operation status"),
            "abandoned"
        );
        TerminalSessionRepo::create_terminal_session(&*db_fixture.db, terminal())
            .await
            .expect("terminal admission proceeds after stale state is reconciled");
    }

    #[tokio::test]
    async fn reconciliation_racing_acquire_never_abandons_a_new_live_owner() {
        let temp = TempDir::new().expect("temporary directory");
        let database_path = temp.path().join("reconcile-race.db");
        let database_url = format!("sqlite://{}", database_path.display());
        let db_fixture = fixture(&database_url, temp.path()).await;
        let competing_pool = create_sqlite_pool(&database_url)
            .await
            .expect("independent pool");
        let competing_db = Arc::new(SqliteDb::new(competing_pool));
        let original = begin_operation(&db_fixture.db, &db_fixture.task_id).await;
        let reconciler = TaskIntegrationOperationManager::new(
            Arc::clone(&db_fixture.db),
            temp.path().to_path_buf(),
        );
        let contender =
            TaskIntegrationOperationManager::new(competing_db, temp.path().to_path_buf());

        let (reconcile_result, acquire_result) = tokio::join!(
            reconciler.reconcile_stale(&db_fixture.task_id),
            contender.acquire(
                &db_fixture.task_id,
                TaskIntegrationOperationKind::WorkUnitIntegration,
                "new-operation-owner",
            ),
        );
        assert!(
            reconcile_result.is_ok() || matches!(reconcile_result, Err(ServiceError::Conflict(_)))
        );
        let active =
            TaskIntegrationOperationRepo::get_active_for_task(&*db_fixture.db, &db_fixture.task_id)
                .await
                .expect("active operation lookup");
        let running_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_integration_operation
             WHERE task_id = ? AND status = 'running'",
        )
        .bind(&db_fixture.task_id)
        .fetch_one(db_fixture.db.pool())
        .await
        .expect("running operation count");
        assert!(running_count <= 1);
        let original_status: String =
            sqlx::query_scalar("SELECT status FROM task_integration_operation WHERE id = ?")
                .bind(&original.id)
                .fetch_one(db_fixture.db.pool())
                .await
                .expect("original operation status");
        assert_eq!(original_status, "abandoned");
        match acquire_result {
            Ok(owner) => {
                let active = active.expect("successful acquisition remains running");
                assert_ne!(active.id, original.id);
                drop(owner);
            }
            Err(_) => {
                if let Some(active) = active {
                    assert_ne!(active.id, original.id);
                }
            }
        }
    }

    #[tokio::test]
    async fn publication_intent_recovers_after_restart_before_provider_call() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-intent.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus)).await;
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let intent = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("publication intent lookup")
            .expect("intent committed with operation admission");
        assert_eq!(intent.merge_status, "pending");
        assert_eq!(intent.pr_state, "publishing");
        assert_eq!(intent.provider_pr_id, None);
        drop(publish);
        drop(merge);

        crate::pr_service::PrReconciler::new(Arc::clone(&fixture.db), Arc::clone(&event_bus), None)
            .reconcile_once()
            .await
            .expect("restart resumes the exact publication intent");

        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("reconciled PR metadata")
            .expect("PR metadata remains");
        assert_eq!(metadata.pr_state, "open");
        assert!(metadata.provider_pr_id.is_some());
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge_id.as_str())
        );
        assert_eq!(
            metadata.publish_operation_id.as_deref(),
            Some(publish_id.as_str())
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
                .await
                .expect("merge lookup")
                .expect("TaskMerge remains durable")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &publish_id)
                .await
                .expect("publish lookup")
                .expect("PublishPr remains durable")
                .status,
            TaskIntegrationOperationStatus::Succeeded
        );
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Merging);
        let admissions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_integration_operation
             WHERE task_id = ? AND kind = 'task_merge'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("admission count");
        assert_eq!(admissions, 1, "restart does not create another admission");
    }

    #[tokio::test]
    async fn remote_task_merge_cannot_finish_without_exact_provider_result() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}",
            temp.path().join("pr-terminal-guard.db").display()
        );
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, _publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), event_bus).await;
        let current = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, merge.id())
            .await
            .expect("TaskMerge lookup")
            .expect("durable PR admission exists");

        let result = TaskIntegrationOperationRepo::finish(
            &*fixture.db,
            FinishTaskIntegrationOperation {
                id: current.id.clone(),
                expected_version: current.version,
                status: TaskIntegrationOperationStatus::Succeeded,
                result_event_id: None,
                updated_at: now_rfc3339(),
                finished_at: now_rfc3339(),
            },
        )
        .await;
        assert!(
            result.is_err(),
            "a remote merge needs exact provider evidence"
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &current.id)
                .await
                .expect("TaskMerge lookup")
                .expect("admission remains")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        assert_eq!(
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("lifecycle lookup")
                .expect("lifecycle exists")
                .state,
            db::TaskLifecycleState::Merging,
            "rejected completion leaves the admitted Task in merging"
        );
    }

    #[tokio::test]
    async fn pull_request_admission_survives_open_restart_gate_change_and_merged_replay() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-open.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, evaluation, gate_id, policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus)).await;
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let metadata = create_test_pr_metadata(&fixture, &merge, &publish, "open").await;
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("publication completes without finishing remote TaskMerge");
        drop(merge);

        let recovery_manager = TaskIntegrationOperationManager::new(
            Arc::clone(&fixture.db),
            temp.path().to_path_buf(),
        );
        let recovered_lock = recovery_manager
            .try_pr_recovery_lock(&fixture.task_id, &merge_id, &publish_id)
            .await
            .expect("recovery lock lookup")
            .expect("restart recovers the exact still-open admission");
        drop(recovered_lock);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Merging);
        assert_eq!(lifecycle.reason_ref.as_deref(), Some(merge_id.as_str()));
        let task_merge_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_integration_operation
             WHERE task_id = ? AND kind = 'task_merge'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("TaskMerge history count");
        assert_eq!(task_merge_count, 1, "recovery reuses one admission");

        let reconciler = crate::pr_service::PrReconciler::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
            None,
        );
        reconciler
            .reconcile_once()
            .await
            .expect("provider Open is reconciled after restart");
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("open lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Merging);

        let engine =
            crate::gate_engine::GateEngine::new(Arc::clone(&fixture.db), Arc::clone(&event_bus));
        let mut changed_policy = policy;
        changed_policy.decisions[0].permitted_deciders = vec![db::ActorRef::Human(
            "policy-revision-after-admission".to_owned(),
        )];
        engine
            .revise_policy(&gate_id, Some(1), changed_policy)
            .await
            .expect("new Gate policy revision after PR admission");
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("post-policy-change lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Merging);
        assert_eq!(lifecycle.reason_ref.as_deref(), Some(merge_id.as_str()));

        PrMetadataRepo::update(
            &*fixture.db,
            db::UpdatePrMetadata {
                id: metadata.id.clone(),
                provider_type: None,
                provider_pr_id: None,
                pr_url: None,
                source_branch: None,
                target_branch: None,
                pr_state: Some("merged".to_owned()),
                merge_status: None,
                task_merge_operation_id: None,
                publish_operation_id: None,
                last_synced_at: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("provider reports Merged");
        reconciler
            .reconcile_once()
            .await
            .expect("exact admitted TaskMerge completes");
        let finished = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge persists");
        assert_eq!(finished.status, TaskIntegrationOperationStatus::Succeeded);
        assert_eq!(
            finished.gate_evaluation_id.as_deref(),
            Some(evaluation.id.as_str())
        );
        assert!(finished.result_event_id.is_some());
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("merged lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Done);

        let result_event_id = finished
            .result_event_id
            .clone()
            .expect("exact provider result");
        let replay = TaskIntegrationOperationRepo::finish(
            &*fixture.db,
            db::FinishTaskIntegrationOperation {
                id: merge_id.clone(),
                expected_version: finished.version,
                status: TaskIntegrationOperationStatus::Succeeded,
                result_event_id: Some(result_event_id),
                updated_at: now_rfc3339(),
                finished_at: now_rfc3339(),
            },
        )
        .await
        .expect("duplicate Merged callback replays same operation");
        assert_eq!(replay.id, merge_id);
        let done_transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_lifecycle_transition
             WHERE task_id = ? AND cause_kind = 'merge_operation'
               AND cause_ref = ? AND to_state = 'done'",
        )
        .bind(&fixture.task_id)
        .bind(&finished.id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("terminal lifecycle transition count");
        assert_eq!(done_transitions, 1, "replay cannot finish lifecycle twice");
    }

    #[tokio::test]
    async fn closed_pull_request_blocks_with_exact_provider_provenance_without_retry() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-closed.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus)).await;
        let merge_id = merge.id().to_owned();
        let metadata = create_test_pr_metadata(&fixture, &merge, &publish, "closed").await;
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("PR was published");
        drop(merge);

        crate::pr_service::PrReconciler::new(Arc::clone(&fixture.db), Arc::clone(&event_bus), None)
            .reconcile_once()
            .await
            .expect("provider Closed finishes exact admission");
        let terminal = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge exists");
        assert_eq!(terminal.status, TaskIntegrationOperationStatus::Failed);
        let result_event_id = terminal
            .result_event_id
            .as_deref()
            .expect("Closed callback provenance is durable");
        let result_event = db::DomainEventRepo::get_event(&*fixture.db, result_event_id)
            .await
            .expect("Closed callback event lookup")
            .expect("Closed callback event exists");
        assert_eq!(result_event.event_type, "pr.status_changed");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result_event.payload_json)
                .ok()
                .and_then(|payload| payload
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned))
                .as_deref(),
            Some("closed")
        );
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("closed lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("pr_closed_without_merge")
        );
        assert_eq!(
            lifecycle.reason_ref.as_deref(),
            Some(result_event.id.as_str())
        );
        let terminal_event = db::DomainEventRepo::get_event_by_dedupe(
            &*fixture.db,
            &format!("task-merge-terminal:{merge_id}"),
        )
        .await
        .expect("terminal event lookup")
        .expect("TaskMerge terminal event");
        let retry_service = crate::task_failure_retry::TaskFailureRetryService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        );
        assert_eq!(
            retry_service
                .process_domain_event(&terminal_event)
                .await
                .expect("closed PR is not a TaskMerge retry failure"),
            0
        );
        let _ = metadata;
    }

    #[tokio::test]
    async fn publication_failure_blocks_exact_merge_and_consumes_only_task_merge_retry() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-failed.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus)).await;
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let failure_service =
            crate::pr_service::PrService::new(Arc::clone(&fixture.db), Arc::clone(&event_bus));
        let failure_event = failure_service
            .record_publication_failure(&fixture.task_id, &merge_id, publish.id())
            .await
            .expect("exact provider failure is durable before operation completion");
        let replayed_failure = failure_service
            .record_publication_failure(&fixture.task_id, &merge_id, publish.id())
            .await
            .expect("publication failure replay");
        assert_eq!(failure_event.id, replayed_failure.id);
        drop(merge);
        drop(publish);
        crate::pr_service::PrReconciler::new(Arc::clone(&fixture.db), Arc::clone(&event_bus), None)
            .reconcile_once()
            .await
            .expect("restart closes the exact failed publication and TaskMerge");
        let blocked = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("failure lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(blocked.state, db::TaskLifecycleState::Blocked);
        assert_eq!(blocked.reason_kind.as_deref(), Some("task_merge_failed"));
        assert_eq!(blocked.reason_ref.as_deref(), Some(merge_id.as_str()));
        let finished_merge = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge exists");
        let finished_publish = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &publish_id)
            .await
            .expect("PublishPr lookup")
            .expect("PublishPr exists");
        assert_eq!(
            finished_merge.status,
            TaskIntegrationOperationStatus::Failed
        );
        assert_eq!(
            finished_merge.result_event_id.as_deref(),
            Some(failure_event.id.as_str())
        );
        assert_eq!(
            finished_publish.status,
            TaskIntegrationOperationStatus::Failed
        );
        assert_eq!(
            finished_publish.result_event_id.as_deref(),
            Some(failure_event.id.as_str())
        );
        let terminal_event = db::DomainEventRepo::get_event_by_dedupe(
            &*fixture.db,
            &format!("task-merge-terminal:{merge_id}"),
        )
        .await
        .expect("terminal event lookup")
        .expect("exact TaskMerge failure event");
        let retry_service = crate::task_failure_retry::TaskFailureRetryService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        );
        assert_eq!(
            retry_service
                .process_domain_event(&terminal_event)
                .await
                .expect("publication failure consumes TaskMerge budget"),
            1
        );
        assert_eq!(
            retry_service
                .process_domain_event(&terminal_event)
                .await
                .expect("terminal event replay is idempotent"),
            1
        );
        let attempt: i64 = sqlx::query_scalar(
            "SELECT attempt_number FROM task_failure_retry_receipt
             WHERE task_id = ? AND failure_kind = 'task_merge_failed'
               AND failure_ref = ?",
        )
        .bind(&fixture.task_id)
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("exact TaskMerge retry attempt");
        assert_eq!(attempt, 1);
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_failure_retry_receipt
             WHERE task_id = ? AND failure_kind = 'task_merge_failed'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("TaskMerge retry receipt count");
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn legacy_unadmitted_pull_request_cannot_be_rebound_under_a_new_gate() {
        let temp = TempDir::new().expect("temporary directory");
        let db_fixture = fixture("sqlite::memory:", temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&db_fixture, temp.path(), Arc::clone(&event_bus)).await;
        let metadata = create_test_pr_metadata(&db_fixture, &merge, &publish, "open").await;
        PrMetadataRepo::update(
            &*db_fixture.db,
            db::UpdatePrMetadata {
                id: metadata.id.clone(),
                provider_type: None,
                provider_pr_id: None,
                pr_url: None,
                source_branch: None,
                target_branch: None,
                pr_state: None,
                merge_status: Some("legacy_unadmitted".to_owned()),
                task_merge_operation_id: None,
                publish_operation_id: None,
                last_synced_at: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("legacy provider metadata state persists");

        let task = TaskRepo::get_by_id(&*db_fixture.db, &db_fixture.task_id, false)
            .await
            .expect("Task lookup")
            .expect("Task exists");
        let repo = RepoRepo::get_by_id(&*db_fixture.db, &db_fixture.repo_id)
            .await
            .expect("Repo lookup")
            .expect("Repo exists");
        let error = match crate::pr_service::PrService::new(
            Arc::clone(&db_fixture.db),
            Arc::clone(&event_bus),
        )
        .publish_pr(&task, &repo, "task/legacy", "main")
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("a pre-V109 PR must not be rebound to a later Gate"),
        };
        assert!(error
            .to_string()
            .contains("legacy PR has no durable TaskMerge admission"));

        let metadata = PrMetadataRepo::get_by_task_id(&*db_fixture.db, &db_fixture.task_id)
            .await
            .expect("PR metadata lookup")
            .expect("legacy PR metadata remains visible");
        assert_eq!(metadata.merge_status, "legacy_unadmitted");
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge.id())
        );
        assert_eq!(metadata.publish_operation_id.as_deref(), Some(publish.id()));
    }
}
