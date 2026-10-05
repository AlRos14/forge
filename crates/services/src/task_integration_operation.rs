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
        provider_config: &db::PrProviderConfig,
        remote_repo_identity: &str,
        source_branch: &str,
        target_branch: &str,
        source_sha: &str,
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
            provider_type: provider_config.provider_type.clone(),
            provider_pr_id: None,
            pr_url: None,
            source_branch: source_branch.to_owned(),
            target_branch: target_branch.to_owned(),
            pr_state: "publishing".to_owned(),
            merge_status: "pending".to_owned(),
            task_merge_operation_id: merge_id.clone(),
            publish_operation_id: publish_id.clone(),
            last_synced_at: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let config_digest_payload = serde_json::json!({
            "id": provider_config.id,
            "repo_id": provider_config.repo_id,
            "provider_type": provider_config.provider_type,
            "base_url": provider_config.base_url,
            "polling_interval_seconds": provider_config.polling_interval_seconds,
            "token_secret_ref": provider_config.token_secret_ref,
            "updated_at": provider_config.updated_at,
        });
        let remote_admission = db::CreateRemotePrAdmission {
            task_merge_operation_id: merge_id,
            publish_operation_id: publish_id,
            metadata_id: metadata_input.id.clone(),
            task_id: task_id.to_owned(),
            provider_config_id: provider_config.id.clone(),
            provider_type: provider_config.provider_type.clone(),
            provider_config_revision: provider_config.updated_at.clone(),
            provider_config_digest: hex::encode(Sha256::digest(
                config_digest_payload.to_string().as_bytes(),
            )),
            provider_base_url: provider_config.base_url.clone(),
            token_secret_ref: provider_config.token_secret_ref.clone(),
            remote_repo_identity: remote_repo_identity.to_owned(),
            source_branch: source_branch.to_owned(),
            target_branch: target_branch.to_owned(),
            admitted_source_sha: source_sha.to_owned(),
            created_at: metadata_input.created_at.clone(),
            updated_at: metadata_input.updated_at.clone(),
        };
        let (merge, publish) = TaskIntegrationOperationRepo::begin_pull_request_publication(
            &*self.db,
            merge_input,
            publish_input,
            metadata_input,
            remote_admission,
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
        if self.operation.kind == TaskIntegrationOperationKind::TaskMerge
            && self.operation.remote_waiting
        {
            return Err(ServiceError::invalid_operation(
                "remote TaskMerge results are recorded only by the exact provider outcome primitive",
            ));
        }
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
    use std::{
        collections::VecDeque,
        fs,
        path::Path,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
    };
    use tempfile::TempDir;

    #[derive(Default)]
    struct ScriptedPrProvider {
        remote: Mutex<Option<crate::pr_service::PrRecord>>,
        status_script: Mutex<
            VecDeque<
                std::result::Result<
                    crate::pr_service::RemotePrStatus,
                    crate::pr_service::PrProviderError,
                >,
            >,
        >,
        unknown_create_once: AtomicBool,
        create_count: AtomicUsize,
        find_count: AtomicUsize,
        last_request: Mutex<Option<crate::pr_service::PrCreateRequest>>,
        last_admission: Mutex<Option<db::RemotePrAdmission>>,
    }

    impl ScriptedPrProvider {
        fn unknown_once() -> Self {
            Self {
                unknown_create_once: AtomicBool::new(true),
                ..Self::default()
            }
        }

        fn record(request: &crate::pr_service::PrCreateRequest) -> crate::pr_service::PrRecord {
            crate::pr_service::PrRecord {
                provider_pr_id: format!("provider-pr-{}", request.idempotency_key),
                pr_url: Some(format!("{}/pull/1", request.repo_remote_url)),
                status: crate::pr_service::RemotePrStatus::Open(crate::pr_service::PrObservation {
                    provider_event_id: format!("created:{}", request.idempotency_key),
                    remote_repo_identity: request.repo_remote_url.clone(),
                    source_branch: request.source_branch.clone(),
                    target_branch: request.target_branch.clone(),
                    head_sha: request.source_sha.clone(),
                    merged_commit_sha: None,
                }),
            }
        }

        fn push_status(
            &self,
            status: std::result::Result<
                crate::pr_service::RemotePrStatus,
                crate::pr_service::PrProviderError,
            >,
        ) {
            self.status_script
                .lock()
                .expect("provider status lock")
                .push_back(status);
        }
    }

    #[async_trait::async_trait]
    impl crate::pr_service::PrProvider for ScriptedPrProvider {
        async fn find_pr(
            &self,
            request: &crate::pr_service::PrCreateRequest,
        ) -> std::result::Result<
            Option<crate::pr_service::PrRecord>,
            crate::pr_service::PrProviderError,
        > {
            self.find_count.fetch_add(1, Ordering::SeqCst);
            *self.last_request.lock().expect("request lock") = Some(request.clone());
            Ok(self.remote.lock().expect("remote lock").clone())
        }

        async fn create_pr(
            &self,
            request: crate::pr_service::PrCreateRequest,
        ) -> std::result::Result<crate::pr_service::PrRecord, crate::pr_service::PrProviderError>
        {
            self.create_count.fetch_add(1, Ordering::SeqCst);
            let record = Self::record(&request);
            *self.remote.lock().expect("remote lock") = Some(record.clone());
            if self.unknown_create_once.swap(false, Ordering::SeqCst) {
                return Err(crate::pr_service::PrProviderError::OutcomeUnknown(
                    "simulated lost create response".to_owned(),
                ));
            }
            Ok(record)
        }

        async fn get_pr_status(
            &self,
            admission: &db::RemotePrAdmission,
            _metadata: &db::PrMetadata,
        ) -> std::result::Result<
            crate::pr_service::RemotePrStatus,
            crate::pr_service::PrProviderError,
        > {
            *self.last_admission.lock().expect("admission lock") = Some(admission.clone());
            self.status_script
                .lock()
                .expect("provider status lock")
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(crate::pr_service::RemotePrStatus::Open(
                        crate::pr_service::PrObservation {
                            provider_event_id: format!(
                                "open:{}",
                                admission.task_merge_operation_id
                            ),
                            remote_repo_identity: admission.remote_repo_identity.clone(),
                            source_branch: admission.source_branch.clone(),
                            target_branch: admission.target_branch.clone(),
                            head_sha: admission.admitted_source_sha.clone(),
                            merged_commit_sha: None,
                        },
                    ))
                })
        }
    }

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

    async fn fixture_with_legacy_pr(
        database_url: &str,
        root: &Path,
        pr_state: &str,
        merge_status: &str,
    ) -> (Fixture, String) {
        let migrations = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory")
            .join("db/migrations");
        let prefix = root.join("migrations-v108");
        fs::create_dir_all(&prefix).expect("V108 migration directory");
        for entry in fs::read_dir(&migrations).expect("migration directory reads") {
            let entry = entry.expect("migration entry reads");
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(version) = name
                .strip_prefix('V')
                .and_then(|name| name.split_once("__"))
                .and_then(|(version, _)| version.parse::<i64>().ok())
            else {
                continue;
            };
            if version <= 108 {
                fs::copy(&path, prefix.join(name)).expect("migration copies");
            }
        }
        let pool = create_sqlite_pool(database_url).await.expect("pool");
        db::run_migrations_from(&pool, &prefix)
            .await
            .expect("V108 baseline");
        let fixture = seed_fixture(Arc::new(SqliteDb::new(pool.clone())), root).await;
        let metadata_id = new_uuid_v4();
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO pr_metadata (
                id, task_id, provider_type, provider_pr_id, pr_url,
                source_branch, target_branch, pr_state, merge_status,
                last_synced_at, created_at, updated_at
             ) VALUES (?, ?, 'github', 'old-provider-pr',
                       'https://example.invalid/old-pr', 'task/old', 'main',
                       ?, ?, NULL, ?, ?)",
        )
        .bind(&metadata_id)
        .bind(&fixture.task_id)
        .bind(pr_state)
        .bind(merge_status)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("pre-V109 legacy PR metadata");
        run_migrations(&pool)
            .await
            .expect("upgrade V108 legacy database through current schema");
        (fixture, metadata_id)
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
    ) -> crate::Result<(
        TaskIntegrationOperationManager,
        TaskIntegrationOperationGuard,
        TaskIntegrationOperationGuard,
        db::GateEvaluation,
        String,
        crate::gate_engine::GatePolicyDocument,
        TestTokenEnv,
    )> {
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
        let provider_config = PrProviderConfigRepo::create(
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
                &provider_config,
                "https://example.invalid/repo.git",
                &format!("task/{}", &fixture.task_id[..8]),
                "main",
                "source-commit",
                lock,
            )
            .await?;
        Ok((
            manager,
            merge,
            publish,
            evaluation.evaluation,
            gate.id,
            policy,
            token_env,
        ))
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

    async fn set_test_provider_identity(
        fixture: &Fixture,
        merge: &TaskIntegrationOperationGuard,
        publish: &TaskIntegrationOperationGuard,
        provider_pr_id: &str,
        pr_url: &str,
        pr_state: &str,
    ) -> db::PrMetadata {
        let current = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("PR metadata lookup")
            .expect("PR metadata exists");
        PrMetadataRepo::update(
            &*fixture.db,
            db::UpdatePrMetadata {
                id: current.id,
                provider_type: None,
                provider_pr_id: Some(Some(provider_pr_id.to_owned())),
                pr_url: Some(Some(pr_url.to_owned())),
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
        .expect("provider identity is stored in current projection");
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("updated PR metadata lookup")
            .expect("PR metadata remains");
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge.id())
        );
        assert_eq!(metadata.publish_operation_id.as_deref(), Some(publish.id()));
        metadata
    }

    async fn satisfy_followup_gate(
        fixture: &Fixture,
        event_bus: Arc<EventBus>,
        gate_id: &str,
        source_sha: &str,
        policy: &crate::gate_engine::GatePolicyDocument,
    ) -> crate::Result<db::GateEvaluation> {
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
            .await?
            .ok_or(db::DbError::NotFound)?;
        crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: fixture.task_id.clone(),
            expected_task_version: task.version,
            to_state: db::TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::Actor(api_types::Actor::User {
                user_id: Some(fixture.user_id.clone()),
                source: api_types::UserActionSource::Api,
            }),
            reason_kind: Some("human_approved_pr_rework".to_owned()),
            reason_ref: Some(source_sha.to_owned()),
            idempotency_key: format!("human-pr-rework:{source_sha}"),
        })
        .await?;
        let engine =
            crate::gate_engine::GateEngine::new(Arc::clone(&fixture.db), Arc::clone(&event_bus));
        let previous_decision = policy
            .decisions
            .first()
            .cloned()
            .ok_or_else(|| ServiceError::invalid_operation("test Gate needs a decision"))?;
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
                    reason: format!("Approve the follow-up PR rework {source_sha}"),
                    target_version: None,
                    target_digest: None,
                    required_policy_ref: previous_decision.policy_ref.clone(),
                    required_policy_version: previous_decision.policy_version,
                    required_policy_digest: previous_decision.policy_digest.clone(),
                    supersedes_proposal_id: None,
                    artifact_ids: Vec::new(),
                },
            )
            .await?;
        let decision = collaboration
            .record_decision(
                crate::collaboration_service::CreateDecisionInput {
                    task_id: fixture.task_id.clone(),
                    proposal_id: proposal.id.clone(),
                    proposal_version: proposal.content_version,
                    outcome: db::DecisionOutcome::Approve,
                    rationale: format!("Approve follow-up PR rework {source_sha}"),
                    policy_ref: previous_decision.policy_ref,
                    policy_version: previous_decision.policy_version,
                    policy_digest: previous_decision.policy_digest,
                },
                vec![
                    crate::collaboration_service::CollaborationActorSource::Human(
                        fixture.user_id.clone(),
                    ),
                ],
            )
            .await?;
        let mut followup_policy = policy.clone();
        let requirement = followup_policy
            .decisions
            .first_mut()
            .ok_or_else(|| ServiceError::invalid_operation("test Gate needs a decision"))?;
        requirement.proposal_id = proposal.id;
        requirement.proposal_version = decision.proposal_version;
        requirement.decision_id = decision.id.clone();
        let gate = db::GateRepo::get_gate(&*fixture.db, gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("gate", gate_id.to_owned()))?;
        engine
            .revise_policy(gate_id, gate.active_policy_revision, followup_policy)
            .await?;
        let evaluation = engine.evaluate_active(gate_id).await?;
        if !evaluation
            .inputs
            .iter()
            .any(|input| input.input_kind == "decision" && input.input_id == decision.id)
        {
            return Err(ServiceError::invalid_operation(
                "follow-up GateEvaluation did not capture its new human Decision",
            ));
        }
        if let Some(event) = evaluation.event.as_ref() {
            engine.process_domain_event(event).await?;
        }
        if evaluation.evaluation.outcome != db::GateEvaluationOutcome::Satisfied {
            return Err(ServiceError::invalid_operation(
                "follow-up PR requires a satisfied exact GateEvaluation",
            ));
        }
        Ok(evaluation.evaluation)
    }

    async fn admit_followup_pr_from_gate(
        fixture: &Fixture,
        root: &Path,
        source_sha: &str,
        evaluation: &db::GateEvaluation,
    ) -> crate::Result<(
        TaskIntegrationOperationManager,
        TaskIntegrationOperationGuard,
        TaskIntegrationOperationGuard,
    )> {
        let provider_config = PrProviderConfigRepo::get_by_repo_id(&*fixture.db, &fixture.repo_id)
            .await?
            .ok_or(db::DbError::NotFound)?;
        let manager =
            TaskIntegrationOperationManager::new(Arc::clone(&fixture.db), root.to_path_buf());
        let lock = manager.lock_for_gate_admission(&fixture.task_id).await?;
        let (merge, publish) = manager
            .admit_pull_request_publication_with_lock(
                &fixture.task_id,
                &new_uuid_v4(),
                &evaluation.id,
                &provider_config,
                "https://example.invalid/repo.git",
                &format!("task/{}", &fixture.task_id[..8]),
                "main",
                source_sha,
                lock,
            )
            .await?;
        Ok((manager, merge, publish))
    }

    async fn record_test_remote_outcome(
        fixture: &Fixture,
        merge_id: &str,
        status: &str,
        provider_event_id: Option<&str>,
        provider_pr_id: Option<&str>,
        pr_url: Option<&str>,
        observed_head_sha: Option<&str>,
        merged_commit_sha: Option<&str>,
        reconciliation_reason: Option<&str>,
    ) -> crate::Result<db::DomainEvent> {
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, merge_id)
                .await?
                .ok_or(db::DbError::NotFound)?;
        TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            db::RecordRemotePrOutcome {
                expected_task_id: fixture.task_id.clone(),
                task_merge_operation_id: admission.task_merge_operation_id.clone(),
                publish_operation_id: admission.publish_operation_id.clone(),
                metadata_id: admission.metadata_id.clone(),
                provider_config_id: admission.provider_config_id.clone(),
                provider_config_digest: admission.provider_config_digest.clone(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                status: status.to_owned(),
                provider_event_id: provider_event_id.map(str::to_owned),
                provider_pr_id: provider_pr_id.map(str::to_owned),
                pr_url: pr_url.map(str::to_owned),
                observed_head_sha: observed_head_sha.map(str::to_owned),
                merged_commit_sha: merged_commit_sha.map(str::to_owned),
                reconciliation_reason: reconciliation_reason.map(str::to_owned),
                updated_at: now_rfc3339(),
            },
        )
        .await?
        .ok_or_else(|| ServiceError::not_found("remote PR result event", merge_id.to_owned()))
    }

    async fn fixture_at_v115(database_url: &str, root: &Path) -> Fixture {
        let prefix = root.join("migrations-v115");
        copy_migrations_up_to(115, &prefix);
        let pool = create_sqlite_pool(database_url).await.expect("pool");
        db::run_migrations_from(&pool, &prefix)
            .await
            .expect("V115 migration baseline");
        seed_fixture(Arc::new(SqliteDb::new(pool)), root).await
    }

    async fn fixture_at_v116(database_url: &str, root: &Path) -> Fixture {
        let prefix = root.join("migrations-v116");
        copy_migrations_up_to(116, &prefix);
        let pool = create_sqlite_pool(database_url).await.expect("pool");
        db::run_migrations_from(&pool, &prefix)
            .await
            .expect("V116 migration baseline");
        seed_fixture(Arc::new(SqliteDb::new(pool)), root).await
    }

    async fn assert_no_terminal_pr_merge_has_pending_metadata(fixture: &Fixture) {
        let inconsistent: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM task_integration_operation merge_op
             JOIN pr_metadata metadata
               ON metadata.task_merge_operation_id = merge_op.id
             WHERE merge_op.task_id = ? AND merge_op.kind = 'task_merge'
               AND merge_op.remote_waiting = 1 AND merge_op.status != 'running'
               AND metadata.merge_status = 'pending'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("terminal TaskMerge/PR metadata consistency query");
        assert_eq!(
            inconsistent, 0,
            "atomic remote results cannot commit terminal TaskMerge with pending PR metadata"
        );
    }

    struct V113WrongHeadRework {
        _temp: Option<TempDir>,
        fixture: Fixture,
        task_merge_id: String,
        provider_result_event_id: String,
        source_event_id: String,
        retry_receipt_id: String,
        retry_event_id: String,
        rework_transition_id: String,
        unrelated_decision_id: Option<String>,
        later_retry_receipt_id: Option<String>,
        later_retry_event_id: Option<String>,
    }

    #[derive(Default)]
    struct V113WrongHeadScenario {
        later_authority: bool,
        unrelated_human_decision_before_rework: bool,
        later_retry_without_transition: bool,
    }

    fn copy_migrations_up_to(max_version: i64, destination: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates directory")
            .join("db/migrations");
        fs::create_dir_all(destination).expect("migration directory");
        for entry in fs::read_dir(source).expect("migration directory reads") {
            let path = entry.expect("migration entry reads").path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(version) = name
                .strip_prefix('V')
                .and_then(|name| name.split_once("__"))
                .and_then(|(version, _)| version.parse::<i64>().ok())
            else {
                continue;
            };
            if version <= max_version {
                fs::copy(&path, destination.join(name)).expect("migration copies");
            }
        }
    }

    async fn migrate_v113_history_through(history: &V113WrongHeadRework, version: i64) {
        let prefix = history
            ._temp
            .as_ref()
            .expect("file-backed V113 database owner")
            .path()
            .join(format!("migrations-v{version}"));
        copy_migrations_up_to(version, &prefix);
        db::run_migrations_from(history.fixture.db.pool(), &prefix)
            .await
            .unwrap_or_else(|error| panic!("file-backed migration through V{version}: {error}"));
    }

    async fn v117_repair_count(fixture: &Fixture, task_merge_id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND scope_type = 'task' AND scope_id = ?
               AND json_extract(payload_json, '$.task_merge_operation_id') = ?
               AND json_extract(payload_json, '$.migration_version') = 'V117'",
        )
        .bind(&fixture.task_id)
        .bind(task_merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("V117 repair fact count")
    }

    async fn v117_repair_payload(fixture: &Fixture, task_merge_id: &str) -> serde_json::Value {
        let payload: String = sqlx::query_scalar(
            "SELECT payload_json FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND scope_type = 'task' AND scope_id = ?
               AND json_extract(payload_json, '$.task_merge_operation_id') = ?
               AND json_extract(payload_json, '$.migration_version') = 'V117'",
        )
        .bind(&fixture.task_id)
        .bind(task_merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("V117 repair payload");
        serde_json::from_str(&payload).expect("valid V117 repair payload")
    }

    async fn assert_old_wrong_retry_replay_stays_blocked(history: &V113WrongHeadRework) {
        let retry_event =
            db::DomainEventRepo::get_event(&*history.fixture.db, &history.retry_event_id)
                .await
                .expect("old retry event lookup")
                .expect("old retry event remains");
        assert!(
            !crate::task_failure_retry::TaskFailureRetryService::is_rework_request_event(
                &history.fixture.db,
                &retry_event,
            )
            .await
            .expect("repaired old rework is superseded")
        );
        let old_transition: (i64, String, String) = sqlx::query_as(
            "SELECT expected_task_version, reason_kind, reason_ref
             FROM task_lifecycle_transition WHERE id = ?",
        )
        .bind(&history.rework_transition_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("old transition replay identity");
        let replay = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&history.fixture.db),
            Arc::new(EventBus::new(8)),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: history.fixture.task_id.clone(),
            expected_task_version: old_transition.0,
            to_state: db::TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::DomainEvent(
                history.retry_event_id.clone(),
            ),
            reason_kind: Some(old_transition.1),
            reason_ref: Some(old_transition.2),
            idempotency_key: format!(
                "task-failure-lifecycle:{}:task_merge_failed:{}",
                history.fixture.task_id, history.task_merge_id
            ),
        })
        .await
        .expect("old retry replay resolves to the current lifecycle");
        assert_eq!(replay.lifecycle.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            replay.lifecycle.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );
    }

    async fn v113_wrong_head_rework(later_authority: bool, label: &str) -> V113WrongHeadRework {
        v113_wrong_head_rework_scenario(
            V113WrongHeadScenario {
                later_authority,
                ..V113WrongHeadScenario::default()
            },
            label,
        )
        .await
    }

    async fn v113_wrong_head_rework_scenario(
        scenario: V113WrongHeadScenario,
        label: &str,
    ) -> V113WrongHeadRework {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("v113.db").display());
        let v113_dir = temp.path().join("migrations-v113");
        copy_migrations_up_to(113, &v113_dir);
        let pool = create_sqlite_pool(&database_url)
            .await
            .expect("file-backed pool");
        db::run_migrations_from(&pool, &v113_dir)
            .await
            .expect("file-backed V113 baseline");
        let fixture = seed_fixture(Arc::new(SqliteDb::new(pool)), temp.path()).await;
        let mut history = build_v113_wrong_head_rework(fixture, temp.path(), scenario, label).await;
        history._temp = Some(temp);
        history
    }

    async fn build_v113_wrong_head_rework(
        fixture: Fixture,
        root: &Path,
        scenario: V113WrongHeadScenario,
        label: &str,
    ) -> V113WrongHeadRework {
        let event_bus = Arc::new(EventBus::new(64));
        let (_manager, merge, publish, _evaluation, _gate, policy, _token_env) =
            prepare_pr_admission(&fixture, root, Arc::clone(&event_bus))
                .await
                .expect("exact remote admission at V113");
        let task_merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let admission = sqlx::query(
            "SELECT metadata_id, provider_config_id, provider_type,
                    provider_config_revision, provider_config_digest,
                    remote_repo_identity, source_branch, target_branch,
                    admitted_source_sha
             FROM remote_pr_admission WHERE task_merge_operation_id = ?",
        )
        .bind(&task_merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("V113 frozen remote admission");
        let metadata_id: String = admission.try_get("metadata_id").expect("metadata id");
        let provider_config_id: String = admission
            .try_get("provider_config_id")
            .expect("provider config id");
        let provider_type: String = admission.try_get("provider_type").expect("provider type");
        let provider_config_revision: String = admission
            .try_get("provider_config_revision")
            .expect("provider config revision");
        let provider_config_digest: String = admission
            .try_get("provider_config_digest")
            .expect("provider config digest");
        let remote_repo_identity: String = admission
            .try_get("remote_repo_identity")
            .expect("remote repo identity");
        let source_branch: String = admission.try_get("source_branch").expect("source branch");
        let target_branch: String = admission.try_get("target_branch").expect("target branch");
        let admitted_source_sha: String = admission
            .try_get("admitted_source_sha")
            .expect("admitted source SHA");
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("publication completes before its provider callback");

        let provider_event_id = format!("old-merged-wrong-head-{label}");
        let provider_pr_id = format!("old-provider-pr-{label}");
        let pr_url = format!("https://example.invalid/{label}/pull/1");
        let observed_head_sha = format!("force-pushed-head-{label}");
        let merged_commit_sha = format!("wrong-head-merge-commit-{label}");
        let now = now_rfc3339();
        let provider_result_event_id = new_uuid_v4();
        db::DomainEventRepo::append_event(
            &*fixture.db,
            db::CreateDomainEvent {
                id: provider_result_event_id.clone(),
                event_type: "pr.status_changed".to_owned(),
                entity_type: "pr_metadata".to_owned(),
                entity_id: metadata_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: fixture.task_id.clone(),
                correlation_id: task_merge_id.clone(),
                causation_id: Some(publish_id.clone()),
                causation_depth: 1,
                dedupe_key: Some(format!(
                    "remote-pr-result:{task_merge_id}:{provider_event_id}"
                )),
                payload_json: serde_json::json!({
                    "task_id": fixture.task_id,
                    "pr_metadata_id": metadata_id,
                    "provider_pr_id": provider_pr_id,
                    "provider_type": provider_type,
                    "provider_config_id": provider_config_id,
                    "provider_config_revision": provider_config_revision,
                    "provider_config_digest": provider_config_digest,
                    "remote_repo_identity": remote_repo_identity,
                    "source_branch": source_branch,
                    "target_branch": target_branch,
                    "admitted_source_sha": admitted_source_sha,
                    "head_sha": observed_head_sha,
                    "merged_commit_sha": merged_commit_sha,
                    "provider_event_id": provider_event_id,
                    "pr_url": pr_url,
                    "reconciliation_reason": "provider merged a head other than the admitted source SHA",
                    "status": "head_mismatch",
                    "task_merge_operation_id": task_merge_id,
                    "publish_operation_id": publish_id,
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("old provider result event commits");
        sqlx::query(
            "UPDATE remote_pr_admission
             SET state = 'head_mismatch',
                 reconciliation_reason = 'provider merged a head other than the admitted source SHA',
                 provider_event_id = ?, observed_head_sha = ?, merged_commit_sha = ?,
                 result_event_id = ?, updated_at = ?
             WHERE task_merge_operation_id = ? AND state = 'admitted'",
        )
        .bind(&provider_event_id)
        .bind(&observed_head_sha)
        .bind(&merged_commit_sha)
        .bind(&provider_result_event_id)
        .bind(&now)
        .bind(&task_merge_id)
        .execute(fixture.db.pool())
        .await
        .expect("old code records head_mismatch after provider Merged");
        sqlx::query(
            "UPDATE pr_metadata
             SET provider_pr_id = ?, pr_url = ?, pr_state = 'head_mismatch',
                 merge_status = 'merged', admission_status = 'failed',
                 last_synced_at = ?, updated_at = ?
             WHERE id = ? AND task_merge_operation_id = ? AND publish_operation_id = ?",
        )
        .bind(&provider_pr_id)
        .bind(&pr_url)
        .bind(&now)
        .bind(&now)
        .bind(&metadata_id)
        .bind(&task_merge_id)
        .bind(&publish_id)
        .execute(fixture.db.pool())
        .await
        .expect("old PR projection records the provider observation");
        let merge = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &task_merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge exists");
        sqlx::query(
            "UPDATE task_integration_operation
             SET status = 'failed', result_event_id = ?, version = version + 1,
                 updated_at = ?, finished_at = ?
             WHERE id = ? AND status = 'running' AND version = ? AND remote_waiting = 1",
        )
        .bind(&provider_result_event_id)
        .bind(&now)
        .bind(&now)
        .bind(&task_merge_id)
        .bind(merge.version)
        .execute(fixture.db.pool())
        .await
        .expect("old code terminalizes the exact TaskMerge");

        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
            .await
            .expect("Task lookup")
            .expect("Task exists");
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        let terminal_key = format!("task-merge-terminal:{task_merge_id}");
        let terminal_event = db::CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.lifecycle_changed".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: fixture.task_id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: fixture.task_id.clone(),
            correlation_id: terminal_key.clone(),
            causation_id: Some(task_merge_id.clone()),
            causation_depth: 1,
            dedupe_key: Some(format!("task-lifecycle:{terminal_key}")),
            payload_json: serde_json::json!({
                "task_id": fixture.task_id,
                "from_state": lifecycle.state,
                "to_state": "blocked",
                "cause_kind": "merge_operation",
                "cause_ref": task_merge_id,
                "task_merge_status": "failed",
                "provider_status": "head_mismatch",
                "result_classification": null,
                "publish_operation_id": publish_id,
                "provider_result_event_id": provider_result_event_id,
            })
            .to_string(),
            created_at: now.clone(),
        };
        db::TaskLifecycleRepo::transition_task_lifecycle(
            &*fixture.db,
            db::TransitionTaskLifecycle {
                id: new_uuid_v4(),
                task_id: fixture.task_id.clone(),
                expected_task_version: task.version,
                expected_lifecycle_version: lifecycle.version,
                expected_state: lifecycle.state,
                to_state: db::TaskLifecycleState::Blocked,
                cause_kind: "merge_operation".to_owned(),
                cause_ref: Some(task_merge_id.clone()),
                gate_evaluation_id: None,
                reason_kind: Some("task_merge_failed".to_owned()),
                reason_ref: Some(task_merge_id.clone()),
                idempotency_key: terminal_key,
                updated_at: now.clone(),
                event: terminal_event,
            },
        )
        .await
        .expect("old terminal merge lifecycle transition");
        let source_event_id: String = sqlx::query_scalar(
            "SELECT domain_event_id FROM task_lifecycle_transition
             WHERE task_id = ? AND cause_kind = 'merge_operation' AND cause_ref = ?
               AND from_state = 'merging' AND to_state = 'blocked'",
        )
        .bind(&fixture.task_id)
        .bind(&task_merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("exact old terminal lifecycle event");

        let unrelated_decision_id = if scenario.unrelated_human_decision_before_rework {
            let prior_decision = policy
                .decisions
                .first()
                .expect("initial Gate decision exists");
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
                        action: "record_unrelated_task_note".to_owned(),
                        reason: "Record an unrelated human decision before retry consumption"
                            .to_owned(),
                        target_version: None,
                        target_digest: None,
                        required_policy_ref: prior_decision.policy_ref.clone(),
                        required_policy_version: prior_decision.policy_version,
                        required_policy_digest: prior_decision.policy_digest.clone(),
                        supersedes_proposal_id: None,
                        artifact_ids: Vec::new(),
                    },
                )
                .await
                .expect("unrelated human Proposal");
            Some(
                collaboration
                    .record_decision(
                        crate::collaboration_service::CreateDecisionInput {
                            task_id: fixture.task_id.clone(),
                            proposal_id: proposal.id.clone(),
                            proposal_version: proposal.content_version,
                            outcome: db::DecisionOutcome::Reject,
                            rationale:
                                "This unrelated task note does not accept the remote PR head"
                                    .to_owned(),
                            policy_ref: prior_decision.policy_ref.clone(),
                            policy_version: prior_decision.policy_version,
                            policy_digest: prior_decision.policy_digest.clone(),
                        },
                        vec![
                            crate::collaboration_service::CollaborationActorSource::Human(
                                fixture.user_id.clone(),
                            ),
                        ],
                    )
                    .await
                    .expect("unrelated human Decision before old rework")
                    .id,
            )
        } else {
            None
        };

        let policy_digest = hex::encode(Sha256::digest(
            b"forge.task_failure_retry:v3:review_request_changes=3;validation_failed=2;execution_failed=3;work_unit_integration_failed=1;task_merge_failed=1;scoped_retry_epochs=true",
        ));
        let retry_receipt_id = new_uuid_v4();
        let retry_event_id = new_uuid_v4();
        let retry_key = format!(
            "task-failure-retry:{}:task_merge_failed:{}",
            fixture.task_id, task_merge_id
        );
        let retry_event = db::CreateDomainEvent {
            id: retry_event_id.clone(),
            event_type: "task.rework_requested".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: fixture.task_id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: fixture.task_id.clone(),
            correlation_id: retry_key.clone(),
            causation_id: Some(source_event_id.clone()),
            causation_depth: 2,
            dedupe_key: Some(retry_key),
            payload_json: serde_json::json!({
                "task_id": fixture.task_id,
                "failure_kind": "task_merge_failed",
                "failure_ref": task_merge_id,
                "source_event_id": source_event_id,
                "attempt_number": 1,
                "retry_budget": 1,
                "retry_epoch": 0,
                "disposition": "rework",
                "policy_ref": "forge.task_failure_retry",
                "policy_version": 3,
                "policy_digest": policy_digest,
            })
            .to_string(),
            created_at: now.clone(),
        };
        db::DomainEventRepo::append_event(&*fixture.db, retry_event)
            .await
            .expect("old retry event commits");
        sqlx::query(
            "INSERT INTO task_failure_retry_receipt (
                id, task_id, failure_kind, failure_ref, source_event_id,
                attempt_number, retry_budget, disposition, policy_ref,
                policy_version, policy_digest, receipt_event_id, created_at,
                retry_epoch
             ) VALUES (?, ?, 'task_merge_failed', ?, ?, 1, 1, 'rework',
                       'forge.task_failure_retry', 3, ?, ?, ?, 0)",
        )
        .bind(&retry_receipt_id)
        .bind(&fixture.task_id)
        .bind(&task_merge_id)
        .bind(&source_event_id)
        .bind(&policy_digest)
        .bind(&retry_event_id)
        .bind(&now)
        .execute(fixture.db.pool())
        .await
        .expect("old retry receipt commits");
        let blocked_task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
            .await
            .expect("Task lookup after old terminal result")
            .expect("Task exists");
        let reworked = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: fixture.task_id.clone(),
            expected_task_version: blocked_task.version,
            to_state: db::TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::DomainEvent(retry_event_id.clone()),
            reason_kind: Some("merge_failure_rework".to_owned()),
            reason_ref: Some(task_merge_id.clone()),
            idempotency_key: format!(
                "task-failure-lifecycle:{}:task_merge_failed:{}",
                fixture.task_id, task_merge_id
            ),
        })
        .await
        .expect("old V113 consumer reopens the Task");
        let rework_transition_id = reworked
            .transition
            .as_ref()
            .expect("old retry transition is applied")
            .transition_id
            .clone();
        let (later_retry_receipt_id, later_retry_event_id) =
            if scenario.later_retry_without_transition {
                let execution_id = new_uuid_v4();
                let failed_at = now_rfc3339();
                let (_execution, failure_event) = db::ExecutionRepo::create_with_event(
                    &*fixture.db,
                    db::CreateExecution {
                        id: execution_id.clone(),
                        task_id: fixture.task_id.clone(),
                        agent_id: None,
                        actor_ref: None,
                        role: "implementer".to_owned(),
                        purpose: Some(db::ExecutionPurpose::Implement),
                        status: db::ExecutionStatus::Failed,
                        stop_reason: None,
                        stopped_by: None,
                        resume_policy: None,
                        stopped_at: None,
                        parent_execution_id: None,
                        agent_session_id: None,
                        harness_session_id: None,
                        agent_message_id: None,
                        last_activity_at: None,
                        summary: None,
                        logs_path: None,
                        before_sha: None,
                        after_sha: None,
                        error: Some("unrelated later execution failure".to_owned()),
                        executor_config_snapshot_json: None,
                        workspace_id: None,
                        created_at: failed_at.clone(),
                        updated_at: failed_at.clone(),
                    },
                    db::CreateDomainEvent {
                        id: new_uuid_v4(),
                        event_type: "execution.failed".to_owned(),
                        entity_type: "execution".to_owned(),
                        entity_id: execution_id.clone(),
                        actor_type: "system".to_owned(),
                        actor_id: None,
                        scope_type: "task".to_owned(),
                        scope_id: fixture.task_id.clone(),
                        correlation_id: format!("unrelated-execution-failure:{execution_id}"),
                        causation_id: None,
                        causation_depth: 0,
                        dedupe_key: Some(format!("unrelated-execution-failure:{execution_id}")),
                        payload_json: serde_json::json!({
                            "task_id": fixture.task_id,
                            "execution_id": execution_id,
                            "failure": "unrelated later failure",
                        })
                        .to_string(),
                        created_at: failed_at,
                    },
                )
                .await
                .expect("later unrelated failed Execution");
                let retry_service = crate::task_failure_retry::TaskFailureRetryService::new(
                    Arc::clone(&fixture.db),
                    Arc::clone(&event_bus),
                );
                assert_eq!(
                    retry_service
                        .process_domain_event(&failure_event)
                        .await
                        .expect("active Task records retry request without transition"),
                    1
                );
                let event = db::DomainEventRepo::get_event_by_dedupe(
                    &*fixture.db,
                    &format!(
                        "task-failure-retry:{}:execution_failed:{}",
                        fixture.task_id, execution_id
                    ),
                )
                .await
                .expect("later retry event lookup")
                .expect("later retry event remains durable");
                let receipt_id: String = sqlx::query_scalar(
                    "SELECT id FROM task_failure_retry_receipt WHERE receipt_event_id = ?",
                )
                .bind(&event.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("later retry receipt remains durable");
                let transition_count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM task_lifecycle_transition
                     WHERE task_id = ? AND cause_kind = 'domain_event' AND cause_ref = ?",
                )
                .bind(&fixture.task_id)
                .bind(&event.id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("later retry lifecycle transition count");
                assert_eq!(transition_count, 0);
                (Some(receipt_id), Some(event.id))
            } else {
                (None, None)
            };
        if scenario.later_authority {
            crate::task_lifecycle::TaskLifecycleService::new(
                Arc::clone(&fixture.db),
                Arc::clone(&event_bus),
            )
            .transition(crate::task_lifecycle::TransitionLifecycleInput {
                task_id: fixture.task_id.clone(),
                expected_task_version: reworked.task.version,
                to_state: db::TaskLifecycleState::Ready,
                cause: crate::task_lifecycle::LifecycleCause::Actor(api_types::Actor::user(
                    api_types::UserActionSource::Api,
                )),
                reason_kind: Some("later_human_lifecycle_action".to_owned()),
                reason_ref: Some(format!("human-followup-{label}")),
                idempotency_key: format!("later-human-transition-{label}"),
            })
            .await
            .expect("later legitimate Human lifecycle action");
        }

        V113WrongHeadRework {
            _temp: None,
            fixture,
            task_merge_id,
            provider_result_event_id,
            source_event_id,
            retry_receipt_id,
            retry_event_id,
            rework_transition_id,
            unrelated_decision_id,
            later_retry_receipt_id,
            later_retry_event_id,
        }
    }

    #[tokio::test]
    async fn v113_wrong_head_auto_rework_is_repaired_and_old_replay_stays_blocked() {
        let history = v113_wrong_head_rework(false, "repair").await;
        migrate_v113_history_through(&history, 116).await;

        let admission: (String, String, String, String, String, String, String) = sqlx::query_as(
            "SELECT state, provider_status, result_classification,
                        admitted_source_sha, observed_head_sha, merged_commit_sha,
                        result_event_id
                 FROM remote_pr_admission WHERE task_merge_operation_id = ?",
        )
        .bind(&history.task_merge_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("remote PR history after migration");
        assert_eq!(admission.0, "head_mismatch");
        assert_eq!(admission.1, "merged");
        assert_eq!(admission.2, "head_mismatch");
        assert_eq!(admission.3, "source-commit");
        assert_eq!(admission.4, "force-pushed-head-repair");
        assert_eq!(admission.5, "wrong-head-merge-commit-repair");
        assert_eq!(admission.6, history.provider_result_event_id);

        let historical_pr: (String, String, String, String, String, String) = sqlx::query_as(
            "SELECT provider_status, result_classification, observed_head_sha,
                    merged_commit_sha, pr_state, merge_status
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&history.task_merge_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("V116 preserves the old V113 Merged result snapshot");
        assert_eq!(historical_pr.0, "merged");
        assert_eq!(historical_pr.1, "head_mismatch");
        assert_eq!(historical_pr.2, "force-pushed-head-repair");
        assert_eq!(historical_pr.3, "wrong-head-merge-commit-repair");
        assert_eq!(historical_pr.4, "head_mismatch");
        assert_eq!(historical_pr.5, "merged");

        let preserved: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM task_failure_retry_receipt WHERE id = ?),
                 (SELECT COUNT(*) FROM domain_event WHERE id = ?),
                 (SELECT COUNT(*) FROM task_lifecycle_transition WHERE id = ?),
                 (SELECT COUNT(*) FROM task_lifecycle_transition
                  WHERE domain_event_id = ?)",
        )
        .bind(&history.retry_receipt_id)
        .bind(&history.retry_event_id)
        .bind(&history.rework_transition_id)
        .bind(&history.source_event_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("old receipt, events, and transitions remain queryable");
        assert_eq!(preserved, (1, 1, 1, 1));

        let repair: (String, String, String) = sqlx::query_as(
            "SELECT id, entity_id, payload_json FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND scope_type = 'task' AND scope_id = ?",
        )
        .bind(&history.fixture.task_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("durable repair event");
        assert_eq!(repair.1, history.fixture.task_id);
        let payload: serde_json::Value = serde_json::from_str(&repair.2).expect("repair payload");
        assert_eq!(payload["task_merge_operation_id"], history.task_merge_id);
        assert_eq!(
            payload["provider_result_event_id"],
            history.provider_result_event_id
        );
        assert_eq!(payload["wrong_retry_receipt_id"], history.retry_receipt_id);
        assert_eq!(payload["wrong_rework_event_id"], history.retry_event_id);
        assert_eq!(
            payload["prior_lifecycle_transition_id"],
            history.rework_transition_id
        );
        assert_eq!(payload["migration_version"], "V115");

        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("repaired lifecycle lookup")
        .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(
            lifecycle.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );

        let retry_event =
            db::DomainEventRepo::get_event(&*history.fixture.db, &history.retry_event_id)
                .await
                .expect("old retry event lookup")
                .expect("old retry event remains");
        assert!(
            !crate::task_failure_retry::TaskFailureRetryService::is_rework_request_event(
                &history.fixture.db,
                &retry_event,
            )
            .await
            .expect("superseded rework event check"),
            "the repair transition supersedes replay of the old retry receipt"
        );
        let old_transition: (i64, String, String) = sqlx::query_as(
            "SELECT expected_task_version, reason_kind, reason_ref
             FROM task_lifecycle_transition WHERE id = ?",
        )
        .bind(&history.rework_transition_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("old transition replay identity");
        let replay = crate::task_lifecycle::TaskLifecycleService::new(
            Arc::clone(&history.fixture.db),
            Arc::new(EventBus::new(8)),
        )
        .transition(crate::task_lifecycle::TransitionLifecycleInput {
            task_id: history.fixture.task_id.clone(),
            expected_task_version: old_transition.0,
            to_state: db::TaskLifecycleState::Active,
            cause: crate::task_lifecycle::LifecycleCause::DomainEvent(
                history.retry_event_id.clone(),
            ),
            reason_kind: Some(old_transition.1),
            reason_ref: Some(old_transition.2),
            idempotency_key: format!(
                "task-failure-lifecycle:{}:task_merge_failed:{}",
                history.fixture.task_id, history.task_merge_id
            ),
        })
        .await
        .expect("replay resolves the old transition receipt");
        assert_eq!(replay.lifecycle.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            replay.lifecycle.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );
        let repairs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired' AND scope_id = ?",
        )
        .bind(&history.fixture.task_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("repair is emitted once");
        assert_eq!(repairs, 1);
    }

    #[tokio::test]
    async fn v117_does_not_replace_a_later_lifecycle_authority() {
        let history = v113_wrong_head_rework(true, "later-authority").await;
        migrate_v113_history_through(&history, 117).await;

        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("lifecycle lookup")
        .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Ready);
        assert_eq!(
            lifecycle.reason_kind.as_deref(),
            Some("later_human_lifecycle_action")
        );
        assert_eq!(
            lifecycle.reason_ref.as_deref(),
            Some("human-followup-later-authority")
        );
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            0
        );
        let any_repair_facts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired' AND scope_id = ?",
        )
        .bind(&history.fixture.task_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("no repair event after newer lifecycle authority");
        assert_eq!(any_repair_facts, 0);
        let old_receipt: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_failure_retry_receipt WHERE id = ?")
                .bind(&history.retry_receipt_id)
                .fetch_one(history.fixture.db.pool())
                .await
                .expect("old retry receipt remains historical");
        assert_eq!(old_receipt, 1);
    }

    #[tokio::test]
    async fn v117_repairs_when_unrelated_human_decision_precedes_old_rework() {
        let history = v113_wrong_head_rework_scenario(
            V113WrongHeadScenario {
                unrelated_human_decision_before_rework: true,
                ..V113WrongHeadScenario::default()
            },
            "human-decision-before-rework",
        )
        .await;
        let decision_id = history
            .unrelated_decision_id
            .as_deref()
            .expect("unrelated Human Decision was recorded");
        let sequences: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT result.sequence, decision_event.sequence,
                    retry.sequence, transition_event.sequence
             FROM domain_event result
             JOIN domain_event decision_event
               ON decision_event.event_type = 'decision.recorded'
              AND decision_event.entity_id = ?
             JOIN domain_event retry ON retry.id = ?
             JOIN task_lifecycle_transition transition
               ON transition.id = ?
             JOIN domain_event transition_event
               ON transition_event.id = transition.domain_event_id
             WHERE result.id = ?",
        )
        .bind(decision_id)
        .bind(&history.retry_event_id)
        .bind(&history.rework_transition_id)
        .bind(&history.provider_result_event_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("provider result, unrelated Decision, retry, and transition order");
        assert!(sequences.0 < sequences.1);
        assert!(sequences.1 < sequences.2);
        assert!(sequences.2 < sequences.3);

        migrate_v113_history_through(&history, 116).await;
        let before_v117 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("V116 lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(before_v117.state, db::TaskLifecycleState::Active);
        assert_eq!(
            before_v117.reason_kind.as_deref(),
            Some("merge_failure_rework")
        );
        assert_eq!(
            before_v117.reason_ref.as_deref(),
            Some(history.task_merge_id.as_str())
        );
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            0
        );

        migrate_v113_history_through(&history, 117).await;
        let after_v117 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("V117 lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(after_v117.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            after_v117.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(
            after_v117.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );
        let repair = v117_repair_payload(&history.fixture, &history.task_merge_id).await;
        assert_eq!(repair["task_id"], history.fixture.task_id);
        assert_eq!(repair["task_merge_operation_id"], history.task_merge_id);
        assert_eq!(
            repair["provider_result_event_id"],
            history.provider_result_event_id
        );
        assert_eq!(repair["wrong_retry_receipt_id"], history.retry_receipt_id);
        assert_eq!(repair["wrong_rework_event_id"], history.retry_event_id);
        assert_eq!(
            repair["prior_lifecycle_transition_id"],
            history.rework_transition_id
        );
        assert_eq!(repair["migration_version"], "V117");
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            1
        );
        let retained: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM decision WHERE id = ? AND task_id = ?),
                 (SELECT COUNT(*) FROM decision_actor
                  WHERE decision_id = ? AND task_id = ? AND actor_kind = 'human'),
                 (SELECT COUNT(*) FROM domain_event
                  WHERE event_type = 'decision.recorded'
                    AND entity_type = 'decision' AND entity_id = ?
                    AND scope_type = 'task' AND scope_id = ?),
                 (SELECT COUNT(*) FROM task_failure_retry_receipt WHERE id = ?),
                 (SELECT COUNT(*) FROM domain_event WHERE id = ?),
                 (SELECT COUNT(*) FROM task_lifecycle_transition WHERE id = ?)",
        )
        .bind(decision_id)
        .bind(&history.fixture.task_id)
        .bind(decision_id)
        .bind(&history.fixture.task_id)
        .bind(decision_id)
        .bind(&history.fixture.task_id)
        .bind(&history.retry_receipt_id)
        .bind(&history.retry_event_id)
        .bind(&history.rework_transition_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("V117 preserves Decision and wrong-rework history");
        assert_eq!(retained, (1, 1, 1, 1, 1, 1));
        assert_old_wrong_retry_replay_stays_blocked(&history).await;
    }

    #[tokio::test]
    async fn v117_repairs_when_later_retry_event_did_not_change_lifecycle() {
        let history = v113_wrong_head_rework_scenario(
            V113WrongHeadScenario {
                later_retry_without_transition: true,
                ..V113WrongHeadScenario::default()
            },
            "later-retry-without-transition",
        )
        .await;
        let later_retry_id = history
            .later_retry_event_id
            .as_deref()
            .expect("later retry event exists");
        let later_receipt_id = history
            .later_retry_receipt_id
            .as_deref()
            .expect("later retry receipt exists");
        let event_order: (i64, i64) = sqlx::query_as(
            "SELECT wrong_transition_event.sequence, later_retry.sequence
             FROM task_lifecycle_transition wrong_transition
             JOIN domain_event wrong_transition_event
               ON wrong_transition_event.id = wrong_transition.domain_event_id
             JOIN domain_event later_retry ON later_retry.id = ?
             WHERE wrong_transition.id = ?",
        )
        .bind(later_retry_id)
        .bind(&history.rework_transition_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("later retry event follows the wrong rework transition");
        assert!(event_order.0 < event_order.1);
        let current = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("current lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(current.state, db::TaskLifecycleState::Active);
        assert_eq!(current.reason_kind.as_deref(), Some("merge_failure_rework"));
        assert_eq!(
            current.reason_ref.as_deref(),
            Some(history.task_merge_id.as_str())
        );

        migrate_v113_history_through(&history, 116).await;
        let before_v117 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("V116 lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(before_v117.state, db::TaskLifecycleState::Active);
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            0
        );

        migrate_v113_history_through(&history, 117).await;
        let after_v117 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("V117 lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(after_v117.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            after_v117.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(
            after_v117.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            1
        );
        let retained: (i64, i64, i64) = sqlx::query_as(
            "SELECT
                 (SELECT COUNT(*) FROM task_failure_retry_receipt WHERE id = ?),
                 (SELECT COUNT(*) FROM domain_event WHERE id = ?),
                 (SELECT COUNT(*) FROM task_lifecycle_transition
                  WHERE task_id = ? AND cause_kind = 'domain_event' AND cause_ref = ?)",
        )
        .bind(later_receipt_id)
        .bind(later_retry_id)
        .bind(&history.fixture.task_id)
        .bind(later_retry_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("later retry receipt and event remain without a transition");
        assert_eq!(retained, (1, 1, 0));
        let later_retry = db::DomainEventRepo::get_event(&*history.fixture.db, later_retry_id)
            .await
            .expect("later retry event lookup")
            .expect("later retry event remains");
        assert!(
            !crate::task_failure_retry::TaskFailureRetryService::is_rework_request_event(
                &history.fixture.db,
                &later_retry,
            )
            .await
            .expect("V117 transition supersedes the later retry without authority")
        );
        assert_old_wrong_retry_replay_stays_blocked(&history).await;
    }

    #[tokio::test]
    async fn v117_does_not_duplicate_a_task_repaired_by_v115() {
        let history = v113_wrong_head_rework(false, "already-repaired-v115").await;
        migrate_v113_history_through(&history, 115).await;
        let repaired_by_v115 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("V115 lifecycle lookup")
        .expect("Task lifecycle exists");
        assert_eq!(repaired_by_v115.state, db::TaskLifecycleState::Blocked);
        let v115_repairs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND scope_id = ? AND json_extract(payload_json, '$.migration_version') = 'V115'",
        )
        .bind(&history.fixture.task_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("existing V115 repair fact");
        assert_eq!(v115_repairs, 1);

        migrate_v113_history_through(&history, 117).await;
        let after_v117 = db::TaskLifecycleRepo::get_task_lifecycle(
            &*history.fixture.db,
            &history.fixture.task_id,
        )
        .await
        .expect("lifecycle after V117")
        .expect("Task lifecycle exists");
        assert_eq!(after_v117.state, db::TaskLifecycleState::Blocked);
        assert_eq!(after_v117.version, repaired_by_v115.version);
        assert_eq!(
            after_v117.reason_ref.as_deref(),
            Some(history.provider_result_event_id.as_str())
        );
        assert_eq!(
            v117_repair_count(&history.fixture, &history.task_merge_id).await,
            0
        );
        let correction_transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_lifecycle_transition
             WHERE task_id = ? AND reason_kind = 'remote_pr_head_mismatch'
               AND reason_ref = ?",
        )
        .bind(&history.fixture.task_id)
        .bind(&history.provider_result_event_id)
        .fetch_one(history.fixture.db.pool())
        .await
        .expect("single V115 correction transition");
        assert_eq!(correction_transitions, 1);
    }

    #[tokio::test]
    async fn v117_repairs_multiple_tasks_independently() {
        let first = v113_wrong_head_rework_scenario(
            V113WrongHeadScenario {
                unrelated_human_decision_before_rework: true,
                ..V113WrongHeadScenario::default()
            },
            "batch-human-decision",
        )
        .await;
        let temp_root = first
            ._temp
            .as_ref()
            .expect("shared file-backed database owner")
            .path();
        let second_root = temp_root.join("second-task-root");
        fs::create_dir_all(&second_root).expect("second Task fixture root");
        let second_fixture = seed_fixture(Arc::clone(&first.fixture.db), &second_root).await;
        let second = build_v113_wrong_head_rework(
            second_fixture,
            &second_root,
            V113WrongHeadScenario {
                later_retry_without_transition: true,
                ..V113WrongHeadScenario::default()
            },
            "batch-later-retry",
        )
        .await;
        assert_ne!(first.fixture.task_id, second.fixture.task_id);

        migrate_v113_history_through(&first, 116).await;
        for (history, reason) in [
            (&first, "merge_failure_rework"),
            (&second, "merge_failure_rework"),
        ] {
            let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(
                &*history.fixture.db,
                &history.fixture.task_id,
            )
            .await
            .expect("pre-V117 lifecycle lookup")
            .expect("Task lifecycle exists");
            assert_eq!(lifecycle.state, db::TaskLifecycleState::Active);
            assert_eq!(lifecycle.reason_kind.as_deref(), Some(reason));
            assert_eq!(
                v117_repair_count(&history.fixture, &history.task_merge_id).await,
                0
            );
        }

        migrate_v113_history_through(&first, 117).await;
        for history in [&first, &second] {
            let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(
                &*history.fixture.db,
                &history.fixture.task_id,
            )
            .await
            .expect("post-V117 lifecycle lookup")
            .expect("Task lifecycle exists");
            assert_eq!(lifecycle.state, db::TaskLifecycleState::Blocked);
            assert_eq!(
                lifecycle.reason_kind.as_deref(),
                Some("remote_pr_head_mismatch")
            );
            assert_eq!(
                lifecycle.reason_ref.as_deref(),
                Some(history.provider_result_event_id.as_str())
            );
            assert_eq!(
                v117_repair_count(&history.fixture, &history.task_merge_id).await,
                1
            );
            assert_old_wrong_retry_replay_stays_blocked(history).await;
        }
        let v117_repairs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND json_extract(payload_json, '$.migration_version') = 'V117'",
        )
        .fetch_one(first.fixture.db.pool())
        .await
        .expect("batch V117 repair fact count");
        assert_eq!(v117_repairs, 2);
        migrate_v113_history_through(&first, 117).await;
        let repeated_v117_repairs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.remote_pr_integrity_repaired'
               AND json_extract(payload_json, '$.migration_version') = 'V117'",
        )
        .fetch_one(first.fixture.db.pool())
        .await
        .expect("idempotent V117 migration marker");
        assert_eq!(repeated_v117_repairs, 2);
    }

    #[tokio::test]
    async fn v117_does_not_repair_a_modern_head_mismatch() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("modern-v116.db").display());
        let fixture = fixture_at_v116(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("modern V116 remote admission");
        let merge_id = merge.id().to_owned();
        set_test_provider_identity(
            &fixture,
            &merge,
            &publish,
            "modern-provider-pr-v117",
            "https://github.example.invalid/pull/117",
            "open",
        )
        .await;
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("modern PR publication succeeds");
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("modern admission lookup")
                .expect("modern admission exists");
        let result = record_test_remote_outcome(
            &fixture,
            &merge_id,
            "merged",
            Some("modern-provider-event-v117"),
            Some("modern-provider-pr-v117"),
            Some("https://github.example.invalid/pull/117"),
            Some("modern-observed-wrong-head"),
            Some("modern-provider-merged-commit"),
            None,
        )
        .await
        .expect("modern provider Merged mismatch result");
        drop(merge);
        let before = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("modern mismatch lifecycle lookup")
            .expect("Task lifecycle exists");
        assert_eq!(before.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            before.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(before.reason_ref.as_deref(), Some(result.id.as_str()));
        let admission_result: (String, String, String) = sqlx::query_as(
            "SELECT state, provider_status, result_classification
             FROM remote_pr_admission WHERE task_merge_operation_id = ?",
        )
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("modern mismatch admission facts");
        assert_eq!(admission_result.0, "head_mismatch");
        assert_eq!(admission_result.1, "merged");
        assert_eq!(admission_result.2, "head_mismatch");
        assert_eq!(admission.admitted_source_sha, "source-commit");

        let prefix = temp.path().join("migrations-v117");
        copy_migrations_up_to(117, &prefix);
        db::run_migrations_from(fixture.db.pool(), &prefix)
            .await
            .expect("file-backed modern V116 database upgrades through V117");
        let after = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle after V117")
            .expect("Task lifecycle exists");
        assert_eq!(after.state, db::TaskLifecycleState::Blocked);
        assert_eq!(after.version, before.version);
        assert_eq!(
            after.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(after.reason_ref.as_deref(), Some(result.id.as_str()));
        assert_eq!(v117_repair_count(&fixture, &merge_id).await, 0);
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
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
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
    async fn terminal_legacy_pr_is_archived_before_a_new_exact_admission() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}",
            temp.path().join("legacy-closed.db").display()
        );
        let (fixture, legacy_metadata_id) =
            fixture_with_legacy_pr(&database_url, temp.path(), "closed", "closed_without_merge")
                .await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
        let publish_id = publish.id().to_owned();

        let history: (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT legacy_metadata_id, provider_type, provider_pr_id, pr_url,
                        source_branch, target_branch, pr_state, merge_status,
                        task_merge_operation_id, publish_operation_id,
                        metadata_created_at || ':' || metadata_updated_at
                 FROM legacy_pr_history WHERE task_id = ?",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("exact legacy PR history is durable");
        assert_eq!(history.0, legacy_metadata_id);
        assert_eq!(history.1, "github");
        assert_eq!(history.2, "old-provider-pr");
        assert_eq!(history.3, "https://example.invalid/old-pr");
        assert_eq!(history.4, "task/old");
        assert_eq!(history.5, "main");
        assert_eq!(history.6, "closed");
        assert_eq!(history.7, "closed_without_merge");
        assert_eq!(history.8, None);
        assert_eq!(history.9, None);
        assert!(
            history.10.contains('T'),
            "legacy timestamps remain auditable"
        );

        let current = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("current PR projection lookup")
            .expect("new admission has current projection");
        assert_ne!(current.id, legacy_metadata_id);
        assert_eq!(current.provider_pr_id, None);
        assert_eq!(current.merge_status, "pending");
        assert_eq!(current.admission_status, "admitted");
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, merge.id())
                .await
                .expect("new remote admission lookup")
                .expect("new exact admission exists");
        assert_eq!(admission.metadata_id, current.id);
        assert_ne!(admission.metadata_id, legacy_metadata_id);
        assert_eq!(admission.task_merge_operation_id, merge.id());
        let legacy_bindings: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission WHERE metadata_id = ?")
                .bind(&legacy_metadata_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("legacy metadata has no modern admission");
        assert_eq!(legacy_bindings, 0);
        let current = create_test_pr_metadata(&fixture, &merge, &publish, "open").await;
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("new PR publication finishes");
        TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            db::RecordRemotePrOutcome {
                expected_task_id: fixture.task_id.clone(),
                task_merge_operation_id: merge.id().to_owned(),
                publish_operation_id: publish_id,
                metadata_id: admission.metadata_id.clone(),
                provider_config_id: admission.provider_config_id.clone(),
                provider_config_digest: admission.provider_config_digest.clone(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                status: "merged".to_owned(),
                provider_event_id: Some("legacy-followup-merged".to_owned()),
                provider_pr_id: current.provider_pr_id.clone(),
                pr_url: current.pr_url.clone(),
                observed_head_sha: Some(admission.admitted_source_sha.clone()),
                merged_commit_sha: Some("legacy-followup-merge-commit".to_owned()),
                reconciliation_reason: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("exact provider result completes the new modern admission");
        let project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id = ?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("Project identity for teardown");
        ProjectRepo::delete(&*fixture.db, &project_id)
            .await
            .expect("guarded teardown removes archived legacy history and current admission");
        let archived_after_teardown: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM legacy_pr_history WHERE task_id = ?")
                .bind(&fixture.task_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("history teardown query");
        assert_eq!(archived_after_teardown, 0);
    }

    #[tokio::test]
    async fn active_legacy_pr_stays_unadmitted_and_blocks_new_publication() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("legacy-open.db").display());
        let (fixture, legacy_metadata_id) =
            fixture_with_legacy_pr(&database_url, temp.path(), "open", "pending").await;
        let event_bus = Arc::new(EventBus::new(32));
        assert!(
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .is_err(),
            "an active legacy PR cannot be rebound under a new Gate"
        );
        let legacy: (String, String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT id, admission_status, task_merge_operation_id,
                    publish_operation_id
             FROM pr_metadata WHERE task_id = ?",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("active legacy PR remains current and visible");
        assert_eq!(legacy.0, legacy_metadata_id);
        assert_eq!(legacy.1, "legacy_unadmitted");
        assert_eq!(legacy.2, None);
        assert_eq!(legacy.3, None);
        let admissions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission WHERE task_id = ?")
                .bind(&fixture.task_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("no new admission for an ambiguous active legacy PR");
        assert_eq!(admissions, 0);
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
            prepare_pr_admission(&fixture, temp.path(), event_bus)
                .await
                .expect("PR publication admission");
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
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
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
        let result_event = db::DomainEventRepo::get_event(&*fixture.db, &result_event_id)
            .await
            .expect("exact merged result lookup")
            .expect("exact merged provider result exists");
        let result_payload: serde_json::Value =
            serde_json::from_str(&result_event.payload_json).expect("merged payload is valid");
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("remote admission lookup")
                .expect("remote admission remains immutable");
        let replay = TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            db::RecordRemotePrOutcome {
                expected_task_id: fixture.task_id.clone(),
                task_merge_operation_id: merge_id.clone(),
                publish_operation_id: publish_id.clone(),
                metadata_id: admission.metadata_id.clone(),
                provider_config_id: admission.provider_config_id.clone(),
                provider_config_digest: admission.provider_config_digest.clone(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                status: result_payload["status"].as_str().unwrap().to_owned(),
                provider_event_id: result_payload["provider_event_id"]
                    .as_str()
                    .map(str::to_owned),
                provider_pr_id: result_payload["provider_pr_id"].as_str().map(str::to_owned),
                pr_url: result_payload["pr_url"].as_str().map(str::to_owned),
                observed_head_sha: result_payload["head_sha"].as_str().map(str::to_owned),
                merged_commit_sha: result_payload["merged_commit_sha"]
                    .as_str()
                    .map(str::to_owned),
                reconciliation_reason: result_payload["reconciliation_reason"]
                    .as_str()
                    .map(str::to_owned),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("duplicate Merged callback replays through provider authority")
        .expect("duplicate callback returns same durable result event");
        assert_eq!(replay.id, result_event_id);
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
        assert_no_terminal_pr_merge_has_pending_metadata(&fixture).await;
    }

    #[tokio::test]
    async fn closed_pull_request_blocks_with_exact_provider_provenance_without_retry() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-closed.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
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
        assert_no_terminal_pr_merge_has_pending_metadata(&fixture).await;
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
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("remote admission lookup")
                .expect("remote admission exists");
        let outcome = db::RecordRemotePrOutcome {
            expected_task_id: fixture.task_id.clone(),
            task_merge_operation_id: merge_id.clone(),
            publish_operation_id: publish_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: admission.remote_repo_identity.clone(),
            source_branch: admission.source_branch.clone(),
            target_branch: admission.target_branch.clone(),
            status: "publication_failed".to_owned(),
            provider_event_id: None,
            provider_pr_id: None,
            pr_url: None,
            observed_head_sha: None,
            merged_commit_sha: None,
            reconciliation_reason: Some("provider definitively rejected creation".to_owned()),
            updated_at: now_rfc3339(),
        };
        let failure_event =
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, outcome.clone())
                .await
                .expect("exact provider failure atomically finishes the operations");
        let failure_event = failure_event.expect("durable failure event");
        let replayed_failure =
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, outcome)
                .await
                .expect("publication failure replay");
        assert_eq!(
            failure_event.id,
            replayed_failure.expect("replayed event").id
        );
        drop(merge);
        drop(publish);
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
        assert_no_terminal_pr_merge_has_pending_metadata(&fixture).await;
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
    async fn remote_pr_lost_response_and_provider_outage_recover_same_frozen_admission() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("pr-unknown.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("frozen admission lookup")
                .expect("frozen admission exists");
        drop(publish);
        drop(merge);

        // Current repo/provider configuration is mutable. Reconciliation must
        // continue addressing the exact identity recorded at admission.
        sqlx::query("UPDATE repo SET remote_url = 'https://drift.invalid/other.git' WHERE id = ?")
            .bind(&fixture.repo_id)
            .execute(fixture.db.pool())
            .await
            .expect("simulate repo remote drift");
        sqlx::query(
            "UPDATE pr_provider_config
             SET provider_type = 'gitlab', base_url = 'https://drift.invalid',
                 token_secret_ref = 'MISSING_ROTATED_SECRET'
             WHERE repo_id = ?",
        )
        .bind(&fixture.repo_id)
        .execute(fixture.db.pool())
        .await
        .expect("simulate provider configuration drift");

        let provider = Arc::new(ScriptedPrProvider::unknown_once());
        let reconciler = crate::pr_service::PrReconciler::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
            None,
        )
        .with_provider_for_test(provider.clone());
        reconciler
            .reconcile_once()
            .await
            .expect("lost create response remains a live reconciliation obligation");
        let after_lost_response =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission remains");
        assert_eq!(after_lost_response.state, "reconciliation_required");
        assert_eq!(
            after_lost_response.remote_repo_identity,
            admission.remote_repo_identity
        );
        assert_eq!(
            after_lost_response.provider_config_digest,
            admission.provider_config_digest
        );
        assert_eq!(provider.create_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
                .await
                .expect("TaskMerge lookup")
                .expect("TaskMerge exists")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &publish_id)
                .await
                .expect("PublishPr lookup")
                .expect("PublishPr exists")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata exists");
        assert_eq!(metadata.merge_status, "pending");
        assert_eq!(metadata.admission_status, "reconciliation_required");

        // The provider created the PR but lost its response. find_pr adopts it
        // under the same idempotency key, so no second create occurs.
        reconciler
            .reconcile_once()
            .await
            .expect("provider lookup adopts the already-created PR");
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata exists");
        assert_eq!(metadata.admission_status, "open");
        assert_eq!(metadata.pr_state, "open");
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge_id.as_str())
        );
        assert_eq!(
            metadata.publish_operation_id.as_deref(),
            Some(publish_id.as_str())
        );
        assert_eq!(provider.create_count.load(Ordering::SeqCst), 1);
        assert_eq!(provider.find_count.load(Ordering::SeqCst), 2);
        let frozen_request = provider
            .last_request
            .lock()
            .expect("request lock")
            .clone()
            .expect("provider lookup request");
        assert_eq!(
            frozen_request.repo_remote_url,
            admission.remote_repo_identity
        );
        assert_eq!(frozen_request.source_sha, admission.admitted_source_sha);
        assert_eq!(frozen_request.idempotency_key, merge_id);
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &publish_id)
                .await
                .expect("PublishPr lookup")
                .expect("PublishPr exists")
                .status,
            TaskIntegrationOperationStatus::Succeeded
        );

        provider.push_status(Err(crate::pr_service::PrProviderError::Unavailable(
            "temporary frozen provider endpoint outage".to_owned(),
        )));
        reconciler
            .reconcile_once()
            .await
            .expect("provider outage does not finish TaskMerge");
        let unavailable =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission remains");
        assert_eq!(unavailable.state, "reconciliation_required");
        assert_eq!(
            unavailable.remote_repo_identity,
            admission.remote_repo_identity
        );
        assert_eq!(unavailable.provider_config_id, admission.provider_config_id);
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
                .await
                .expect("TaskMerge lookup")
                .expect("TaskMerge exists")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        assert_eq!(
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("lifecycle lookup")
                .expect("lifecycle exists")
                .state,
            db::TaskLifecycleState::Merging
        );
        let retries_during_outage: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_failure_retry_receipt WHERE task_id = ?")
                .bind(&fixture.task_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("provider outage retry receipt count");
        assert_eq!(retries_during_outage, 0);

        provider.push_status(Ok(crate::pr_service::RemotePrStatus::Merged(
            crate::pr_service::PrObservation {
                provider_event_id: "provider-merge-result-1".to_owned(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                head_sha: admission.admitted_source_sha.clone(),
                merged_commit_sha: Some("provider-created-merge-commit".to_owned()),
            },
        )));
        reconciler
            .reconcile_once()
            .await
            .expect("provider availability and exact merge result finish the admission");
        let finished = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge exists");
        assert_eq!(finished.status, TaskIntegrationOperationStatus::Succeeded);
        let final_admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission remains historical");
        assert_eq!(final_admission.state, "merged");
        assert_eq!(
            final_admission.observed_head_sha.as_deref(),
            Some(admission.admitted_source_sha.as_str())
        );
        assert_eq!(
            final_admission.merged_commit_sha.as_deref(),
            Some("provider-created-merge-commit")
        );
        let final_metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata exists");
        assert_eq!(final_metadata.merge_status, "merged");
        assert_eq!(final_metadata.admission_status, "merged");
        assert_ne!(
            final_admission.merged_commit_sha.as_deref(),
            final_admission.observed_head_sha.as_deref()
        );
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Done);
        let replay = TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            db::RecordRemotePrOutcome {
                expected_task_id: fixture.task_id.clone(),
                task_merge_operation_id: merge_id.clone(),
                publish_operation_id: publish_id.clone(),
                metadata_id: final_admission.metadata_id.clone(),
                provider_config_id: final_admission.provider_config_id.clone(),
                provider_config_digest: final_admission.provider_config_digest.clone(),
                remote_repo_identity: final_admission.remote_repo_identity.clone(),
                source_branch: final_admission.source_branch.clone(),
                target_branch: final_admission.target_branch.clone(),
                status: "merged".to_owned(),
                provider_event_id: Some("provider-merge-result-1".to_owned()),
                provider_pr_id: final_metadata.provider_pr_id.clone(),
                pr_url: final_metadata.pr_url.clone(),
                observed_head_sha: Some(final_admission.admitted_source_sha.clone()),
                merged_commit_sha: Some("provider-created-merge-commit".to_owned()),
                reconciliation_reason: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("duplicate provider callback replays exact durable result")
        .expect("replayed provider event exists");
        assert_eq!(
            replay.id,
            finished
                .result_event_id
                .as_deref()
                .expect("TaskMerge result event")
        );
        let conflicting_same_event_id = db::RecordRemotePrOutcome {
            expected_task_id: fixture.task_id.clone(),
            task_merge_operation_id: merge_id.clone(),
            publish_operation_id: publish_id.clone(),
            metadata_id: final_admission.metadata_id.clone(),
            provider_config_id: final_admission.provider_config_id.clone(),
            provider_config_digest: final_admission.provider_config_digest.clone(),
            remote_repo_identity: final_admission.remote_repo_identity.clone(),
            source_branch: final_admission.source_branch.clone(),
            target_branch: final_admission.target_branch.clone(),
            status: "merged".to_owned(),
            provider_event_id: Some("provider-merge-result-1".to_owned()),
            provider_pr_id: final_metadata.provider_pr_id.clone(),
            pr_url: final_metadata.pr_url.clone(),
            observed_head_sha: Some(final_admission.admitted_source_sha.clone()),
            merged_commit_sha: Some("conflicting-merge-commit".to_owned()),
            reconciliation_reason: None,
            updated_at: now_rfc3339(),
        };
        assert!(
            TaskIntegrationOperationRepo::record_remote_pr_outcome(
                &*fixture.db,
                conflicting_same_event_id,
            )
            .await
            .is_err(),
            "provider event identity cannot replay a conflicting result payload"
        );
        let transitions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_lifecycle_transition
             WHERE task_id = ? AND cause_ref = ? AND to_state = 'done'",
        )
        .bind(&fixture.task_id)
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("exact terminal transition count");
        assert_eq!(transitions, 1);
    }

    #[tokio::test]
    async fn remote_pr_persistence_failure_replays_find_and_never_opens_a_second_admission() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}",
            temp.path().join("pr-persist-fail.db").display()
        );
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        drop(publish);
        drop(merge);
        let provider = Arc::new(ScriptedPrProvider::default());
        let reconciler = crate::pr_service::PrReconciler::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
            None,
        )
        .with_provider_for_test(provider.clone());
        sqlx::query(
            "CREATE TRIGGER test_fail_pr_result_persistence
             BEFORE UPDATE ON pr_metadata
             WHEN NEW.provider_pr_id IS NOT NULL
             BEGIN SELECT RAISE(ABORT, 'simulated local PR metadata write failure'); END",
        )
        .execute(fixture.db.pool())
        .await
        .expect("install deterministic persistence fault");
        reconciler
            .reconcile_once()
            .await
            .expect("reconciler retains the admission after local result write failure");
        let pending =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission remains");
        assert_eq!(pending.state, "admitted");
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata remains");
        assert_eq!(metadata.provider_pr_id, None);
        assert_eq!(metadata.merge_status, "pending");
        assert_eq!(provider.create_count.load(Ordering::SeqCst), 1);
        sqlx::query("DROP TRIGGER test_fail_pr_result_persistence")
            .execute(fixture.db.pool())
            .await
            .expect("remove persistence fault");

        reconciler
            .reconcile_once()
            .await
            .expect("restart finds and adopts the remote PR under the same admission");
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata remains");
        assert_eq!(metadata.pr_state, "open");
        assert_eq!(
            metadata.task_merge_operation_id.as_deref(),
            Some(merge_id.as_str())
        );
        assert_eq!(
            metadata.publish_operation_id.as_deref(),
            Some(publish_id.as_str())
        );
        assert_eq!(provider.create_count.load(Ordering::SeqCst), 1);
        assert_eq!(provider.find_count.load(Ordering::SeqCst), 2);
        let admissions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_integration_operation
             WHERE task_id = ? AND kind = 'task_merge'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("TaskMerge admission count");
        assert_eq!(admissions, 1);
    }

    #[tokio::test]
    async fn remote_merge_with_force_pushed_head_is_blocked_and_conflicting_result_rejected() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}",
            temp.path().join("pr-head-mismatch.db").display()
        );
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, _evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR publication admission");
        let merge_id = merge.id().to_owned();
        let publish_id = publish.id().to_owned();
        let admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission exists");
        let exact_scope = db::RecordRemotePrOutcome {
            expected_task_id: fixture.task_id.clone(),
            task_merge_operation_id: merge_id.clone(),
            publish_operation_id: publish_id.clone(),
            metadata_id: admission.metadata_id.clone(),
            provider_config_id: admission.provider_config_id.clone(),
            provider_config_digest: admission.provider_config_digest.clone(),
            remote_repo_identity: admission.remote_repo_identity.clone(),
            source_branch: admission.source_branch.clone(),
            target_branch: admission.target_branch.clone(),
            status: "reconciliation_required".to_owned(),
            provider_event_id: None,
            provider_pr_id: None,
            pr_url: None,
            observed_head_sha: None,
            merged_commit_sha: None,
            reconciliation_reason: Some("scope rejection probe".to_owned()),
            updated_at: now_rfc3339(),
        };
        let mut wrong_task = exact_scope.clone();
        wrong_task.expected_task_id = "another-task".to_owned();
        assert!(
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, wrong_task,)
                .await
                .is_err()
        );
        let mut wrong_pr = exact_scope.clone();
        wrong_pr.metadata_id = "another-pr".to_owned();
        assert!(
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, wrong_pr,)
                .await
                .is_err()
        );
        let mut wrong_operation = exact_scope.clone();
        wrong_operation.publish_operation_id = "another-publish-operation".to_owned();
        assert!(TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            wrong_operation,
        )
        .await
        .is_err());
        let mut wrong_repo = exact_scope;
        wrong_repo.remote_repo_identity = "https://drift.invalid/repo.git".to_owned();
        assert!(
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, wrong_repo,)
                .await
                .is_err()
        );

        let generic_remote_merge = || CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: fixture.task_id.clone(),
            kind: TaskIntegrationOperationKind::TaskMerge,
            owner_id: "generic-remote-merge-probe".to_owned(),
            gate_evaluation_id: merge.operation.gate_evaluation_id.clone(),
            remote_waiting: true,
            parent_operation_id: None,
            created_at: now_rfc3339(),
        };
        assert!(
            TaskIntegrationOperationRepo::begin(&*fixture.db, generic_remote_merge())
                .await
                .is_err(),
            "generic begin cannot create a remote TaskMerge"
        );
        assert!(
            TaskIntegrationOperationRepo::recover_stale_and_begin(
                &*fixture.db,
                generic_remote_merge(),
                &now_rfc3339(),
            )
            .await
            .is_err(),
            "generic stale recovery cannot create a remote TaskMerge"
        );
        let remote_operation = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("remote TaskMerge lookup")
            .expect("remote TaskMerge exists");
        for status in [
            TaskIntegrationOperationStatus::Succeeded,
            TaskIntegrationOperationStatus::Failed,
            TaskIntegrationOperationStatus::Conflict,
            TaskIntegrationOperationStatus::Abandoned,
        ] {
            assert!(
                TaskIntegrationOperationRepo::finish(
                    &*fixture.db,
                    db::FinishTaskIntegrationOperation {
                        id: merge_id.clone(),
                        expected_version: remote_operation.version,
                        status,
                        result_event_id: None,
                        updated_at: now_rfc3339(),
                        finished_at: now_rfc3339(),
                    },
                )
                .await
                .is_err(),
                "generic repository finish cannot terminalize remote TaskMerge as {status}"
            );
            let guard = TaskIntegrationOperationGuard {
                db: Arc::clone(&fixture.db),
                operation: remote_operation.clone(),
                _file: None,
            };
            assert!(
                guard.finish(status).await.is_err(),
                "generic service guard cannot terminalize remote TaskMerge as {status}"
            );
        }
        assert!(
            TaskIntegrationOperationRepo::abandon_stale(
                &*fixture.db,
                &fixture.task_id,
                &now_rfc3339(),
            )
            .await
            .expect("remote admission is excluded from local stale recovery")
            .is_none(),
            "remote_waiting TaskMerge stays owned by provider reconciliation"
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
                .await
                .expect("remote TaskMerge lookup")
                .expect("remote TaskMerge exists")
                .status,
            TaskIntegrationOperationStatus::Running
        );
        assert_eq!(
            TaskIntegrationOperationRepo::get_by_id(&*fixture.db, publish.id())
                .await
                .expect("PublishPr lookup after stale reconciliation")
                .expect("PublishPr remains under provider recovery")
                .status,
            TaskIntegrationOperationStatus::Running,
            "stale lock recovery cannot abandon a PublishPr child of remote TaskMerge"
        );

        let _metadata = create_test_pr_metadata(&fixture, &merge, &publish, "open").await;
        publish
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("publication succeeds");
        drop(merge);
        let provider = Arc::new(ScriptedPrProvider::default());
        provider.push_status(Ok(crate::pr_service::RemotePrStatus::Merged(
            crate::pr_service::PrObservation {
                provider_event_id: "provider-merged-wrong-head".to_owned(),
                remote_repo_identity: admission.remote_repo_identity.clone(),
                source_branch: admission.source_branch.clone(),
                target_branch: admission.target_branch.clone(),
                head_sha: "force-pushed-sha-B".to_owned(),
                merged_commit_sha: Some("wrong-head-merge-commit".to_owned()),
            },
        )));
        crate::pr_service::PrReconciler::new(Arc::clone(&fixture.db), Arc::clone(&event_bus), None)
            .with_provider_for_test(provider)
            .reconcile_once()
            .await
            .expect("mismatched provider head is a terminal fail-closed result");

        let finished = TaskIntegrationOperationRepo::get_by_id(&*fixture.db, &merge_id)
            .await
            .expect("TaskMerge lookup")
            .expect("TaskMerge exists");
        assert_eq!(finished.status, TaskIntegrationOperationStatus::Failed);
        let final_admission =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge_id)
                .await
                .expect("admission lookup")
                .expect("admission history remains");
        assert_eq!(final_admission.state, "head_mismatch");
        assert_eq!(final_admission.provider_status.as_deref(), Some("merged"));
        assert_eq!(
            final_admission.result_classification.as_deref(),
            Some("head_mismatch")
        );
        assert_eq!(
            final_admission.admitted_source_sha,
            admission.admitted_source_sha
        );
        assert_eq!(
            final_admission.observed_head_sha.as_deref(),
            Some("force-pushed-sha-B")
        );
        assert_eq!(
            final_admission.merged_commit_sha.as_deref(),
            Some("wrong-head-merge-commit")
        );
        let metadata = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("metadata lookup")
            .expect("metadata remains");
        assert_eq!(metadata.merge_status, "merged");
        assert_eq!(metadata.admission_status, "failed");
        assert_eq!(metadata.pr_state, "merged");
        let result_event_id = finished
            .result_event_id
            .as_deref()
            .expect("wrong-head provider result event is durable");
        let result_event = db::DomainEventRepo::get_event(&*fixture.db, result_event_id)
            .await
            .expect("remote result lookup")
            .expect("remote result exists");
        let result_payload: serde_json::Value =
            serde_json::from_str(&result_event.payload_json).expect("valid result payload");
        assert_eq!(result_payload["status"], "merged");
        assert_eq!(result_payload["result_classification"], "head_mismatch");
        assert_eq!(
            result_payload["admitted_source_sha"],
            admission.admitted_source_sha
        );
        assert_eq!(result_payload["head_sha"], "force-pushed-sha-B");
        assert_eq!(
            result_payload["merged_commit_sha"],
            "wrong-head-merge-commit"
        );
        let blocked = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(blocked.state, db::TaskLifecycleState::Blocked);
        assert_eq!(
            blocked.reason_kind.as_deref(),
            Some("remote_pr_head_mismatch")
        );
        assert_eq!(
            blocked.reason_ref.as_deref(),
            Some(result_event.id.as_str())
        );
        assert_no_terminal_pr_merge_has_pending_metadata(&fixture).await;

        let terminal_event = db::DomainEventRepo::get_event_by_dedupe(
            &*fixture.db,
            &format!("task-merge-terminal:{merge_id}"),
        )
        .await
        .expect("terminal lifecycle event lookup")
        .expect("terminal lifecycle event exists");
        let retry_service = crate::task_failure_retry::TaskFailureRetryService::new(
            Arc::clone(&fixture.db),
            Arc::clone(&event_bus),
        );
        assert_eq!(
            retry_service
                .process_domain_event(&terminal_event)
                .await
                .expect("integrity mismatch is not a retryable TaskMerge failure"),
            0
        );
        let retry_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_failure_retry_receipt
             WHERE task_id = ? AND failure_kind = 'task_merge_failed'
               AND failure_ref = ?",
        )
        .bind(&fixture.task_id)
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("head mismatch retry receipt count");
        assert_eq!(retry_receipts, 0);
        let rework_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.rework_requested' AND scope_type = 'task'
               AND scope_id = ? AND json_extract(payload_json, '$.failure_ref') = ?",
        )
        .bind(&fixture.task_id)
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("head mismatch auto-rework count");
        assert_eq!(rework_events, 0);

        let replay = TaskIntegrationOperationRepo::record_remote_pr_outcome(
            &*fixture.db,
            db::RecordRemotePrOutcome {
                expected_task_id: fixture.task_id.clone(),
                task_merge_operation_id: merge_id.clone(),
                publish_operation_id: publish_id.clone(),
                metadata_id: final_admission.metadata_id.clone(),
                provider_config_id: final_admission.provider_config_id.clone(),
                provider_config_digest: final_admission.provider_config_digest.clone(),
                remote_repo_identity: final_admission.remote_repo_identity.clone(),
                source_branch: final_admission.source_branch.clone(),
                target_branch: final_admission.target_branch.clone(),
                status: "merged".to_owned(),
                provider_event_id: Some("provider-merged-wrong-head".to_owned()),
                provider_pr_id: metadata.provider_pr_id.clone(),
                pr_url: metadata.pr_url.clone(),
                observed_head_sha: Some("force-pushed-sha-B".to_owned()),
                merged_commit_sha: Some("wrong-head-merge-commit".to_owned()),
                reconciliation_reason: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("exact duplicate mismatch callback replays")
        .expect("duplicate callback returns its original event");
        assert_eq!(replay.id, result_event.id);

        let conflicting = db::RecordRemotePrOutcome {
            expected_task_id: fixture.task_id.clone(),
            task_merge_operation_id: merge_id.clone(),
            publish_operation_id: publish_id,
            metadata_id: final_admission.metadata_id.clone(),
            provider_config_id: final_admission.provider_config_id.clone(),
            provider_config_digest: final_admission.provider_config_digest.clone(),
            remote_repo_identity: final_admission.remote_repo_identity.clone(),
            source_branch: final_admission.source_branch.clone(),
            target_branch: final_admission.target_branch.clone(),
            status: "closed".to_owned(),
            provider_event_id: Some("conflicting-closed-callback".to_owned()),
            provider_pr_id: metadata.provider_pr_id,
            pr_url: metadata.pr_url,
            observed_head_sha: Some("force-pushed-sha-B".to_owned()),
            merged_commit_sha: None,
            reconciliation_reason: None,
            updated_at: now_rfc3339(),
        };
        assert!(
            TaskIntegrationOperationRepo::record_remote_pr_outcome(&*fixture.db, conflicting,)
                .await
                .is_err()
        );
        let task_merge_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_lifecycle_transition
             WHERE task_id = ? AND cause_ref = ? AND to_state IN ('done', 'blocked')",
        )
        .bind(&fixture.task_id)
        .bind(&merge_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("terminal transition count");
        assert_eq!(
            task_merge_events, 1,
            "conflicting callback cannot retarget lifecycle"
        );
        assert_eq!(
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("lifecycle lookup after duplicate and conflict")
                .expect("Task lifecycle remains")
                .state,
            db::TaskLifecycleState::Blocked,
            "duplicate and conflicting callbacks leave the integrity block in place"
        );
        let project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id = ?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("Task project lookup");
        ProjectRepo::delete(&*fixture.db, &project_id)
            .await
            .expect("guarded Project teardown cascades remote admission history");
        let remaining_remote_admissions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission")
                .fetch_one(fixture.db.pool())
                .await
                .expect("remote admission teardown count");
        assert_eq!(remaining_remote_admissions, 0);
    }

    #[tokio::test]
    async fn terminal_modern_pr_history_survives_third_admission_and_old_callbacks() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("multi-pr.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(64));

        let (_manager1, merge1, publish1, _evaluation1, gate_id, policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR1 admission");
        let merge1_id = merge1.id().to_owned();
        let metadata1 = set_test_provider_identity(
            &fixture,
            &merge1,
            &publish1,
            "provider-pr-1",
            "https://github.example.invalid/pull/101",
            "open",
        )
        .await;
        publish1
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("PR1 publication succeeds");
        let admission1 =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge1_id)
                .await
                .expect("PR1 admission lookup")
                .expect("PR1 admission remains durable");
        let pr1_result = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "closed",
            Some("provider-event-pr1"),
            Some("provider-pr-1"),
            Some("https://github.example.invalid/pull/101"),
            Some(&admission1.admitted_source_sha),
            None,
            None,
        )
        .await
        .expect("PR1 Closed result commits");
        drop(merge1);

        // Fail after the old terminal PR has been snapshotted but before the
        // current projection can be rebound. SQLite must roll back both writes.
        let evaluation2 = satisfy_followup_gate(
            &fixture,
            Arc::clone(&event_bus),
            &gate_id,
            "source-sha-pr2",
            &policy,
        )
        .await
        .expect("human rework and exact Gate2");
        sqlx::query(
            "CREATE TRIGGER test_fail_pr_history_rebind
             BEFORE UPDATE OF task_merge_operation_id ON pr_metadata
             BEGIN SELECT RAISE(ABORT, 'simulated crash after PR snapshot'); END",
        )
        .execute(fixture.db.pool())
        .await
        .expect("fault injection trigger");
        assert!(
            admit_followup_pr_from_gate(&fixture, temp.path(), "source-sha-pr2", &evaluation2,)
                .await
                .is_err(),
            "failed rebind rolls back the snapshot and the new admission"
        );
        sqlx::query("DROP TRIGGER test_fail_pr_history_rebind")
            .execute(fixture.db.pool())
            .await
            .expect("remove fault injection trigger");
        let history_after_rollback: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("history after rolled-back admission");
        assert_eq!(history_after_rollback, 0);
        let old_projection = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("PR1 projection after rollback")
            .expect("current PR metadata remains");
        assert_eq!(old_projection.id, metadata1.id);
        assert_eq!(
            old_projection.task_merge_operation_id.as_deref(),
            Some(merge1_id.as_str())
        );
        let task_merge_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_integration_operation
             WHERE task_id = ? AND kind = 'task_merge'",
        )
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("TaskMerge count after rollback");
        assert_eq!(task_merge_count, 1);
        let lifecycle_after_rollback =
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("lifecycle after rolled-back admission")
                .expect("lifecycle remains");
        assert_eq!(
            lifecycle_after_rollback.state,
            db::TaskLifecycleState::ReadyToMerge
        );
        assert_eq!(
            lifecycle_after_rollback.reason_ref.as_deref(),
            Some(evaluation2.id.as_str())
        );

        let (_manager2, merge2, publish2) =
            admit_followup_pr_from_gate(&fixture, temp.path(), "source-sha-pr2", &evaluation2)
                .await
                .expect("PR2 admission after rolled-back attempt");
        let merge2_id = merge2.id().to_owned();
        let publish2_id = publish2.id().to_owned();
        let history1: (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT provider_pr_id, pr_url, admission_state, provider_status,
                    provider_event_id, result_event_id, pr_state, merge_status,
                    admitted_source_sha
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("immutable PR1 terminal snapshot");
        assert_eq!(history1.0, "provider-pr-1");
        assert_eq!(history1.1, "https://github.example.invalid/pull/101");
        assert_eq!(history1.2, "closed");
        assert_eq!(history1.3, "closed");
        assert_eq!(history1.4.as_deref(), Some("provider-event-pr1"));
        assert_eq!(history1.5.as_deref(), Some(pr1_result.id.as_str()));
        assert_eq!(history1.6, "closed");
        assert_eq!(history1.7, "closed_without_merge");
        assert_eq!(history1.8, admission1.admitted_source_sha);
        let admission1_after_pr2 =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge1_id)
                .await
                .expect("PR1 admission after PR2")
                .expect("PR1 admission remains");
        assert_eq!(admission1_after_pr2.metadata_id, metadata1.id);
        assert_eq!(
            admission1_after_pr2.result_event_id.as_deref(),
            Some(pr1_result.id.as_str())
        );
        let projection2 = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("PR2 current projection")
            .expect("current projection remains present");
        assert_eq!(
            projection2.id, metadata1.id,
            "the current row may be reused"
        );
        assert_eq!(
            projection2.task_merge_operation_id.as_deref(),
            Some(merge2_id.as_str())
        );
        assert_eq!(
            projection2.publish_operation_id.as_deref(),
            Some(publish2_id.as_str())
        );
        assert_eq!(projection2.provider_pr_id, None);
        assert_eq!(projection2.merge_status, "pending");
        let lifecycle2_before_pr1_callbacks =
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR2 lifecycle before PR1 callbacks")
                .expect("PR2 lifecycle is present");
        assert_eq!(
            lifecycle2_before_pr1_callbacks.state,
            db::TaskLifecycleState::Merging
        );

        let exact_pr1_replay = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "closed",
            Some("provider-event-pr1"),
            Some("provider-pr-1"),
            Some("https://github.example.invalid/pull/101"),
            Some(&admission1.admitted_source_sha),
            None,
            None,
        )
        .await
        .expect("exact PR1 callback replays after PR2");
        assert_eq!(exact_pr1_replay.id, pr1_result.id);
        let conflicting_pr1_replay = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "closed",
            Some("provider-event-pr1"),
            Some("provider-pr-1"),
            Some("https://github.example.invalid/pull/changed"),
            Some(&admission1.admitted_source_sha),
            None,
            None,
        )
        .await;
        assert!(conflicting_pr1_replay.is_err());
        assert!(
            record_test_remote_outcome(
                &fixture,
                &merge1_id,
                "closed",
                Some("provider-event-pr1-late"),
                Some("provider-pr-1"),
                Some("https://github.example.invalid/pull/101"),
                Some(&admission1.admitted_source_sha),
                None,
                None,
            )
            .await
            .is_err(),
            "a new terminal callback for PR1 is rejected"
        );
        let projection2_after_pr1_callbacks =
            PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR2 projection after PR1 callbacks")
                .expect("PR2 projection remains present");
        assert_eq!(
            projection2_after_pr1_callbacks
                .task_merge_operation_id
                .as_deref(),
            Some(merge2_id.as_str())
        );
        assert_eq!(
            projection2_after_pr1_callbacks
                .publish_operation_id
                .as_deref(),
            Some(publish2_id.as_str())
        );
        assert_eq!(projection2_after_pr1_callbacks.merge_status, "pending");
        let lifecycle2_after_pr1_callbacks =
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR2 lifecycle after PR1 callbacks")
                .expect("PR2 lifecycle remains present");
        assert_eq!(
            lifecycle2_after_pr1_callbacks.state,
            lifecycle2_before_pr1_callbacks.state
        );
        assert_eq!(
            lifecycle2_after_pr1_callbacks.version,
            lifecycle2_before_pr1_callbacks.version
        );

        let pr2_failure = record_test_remote_outcome(
            &fixture,
            &merge2_id,
            "publication_failed",
            None,
            None,
            None,
            None,
            None,
            Some("provider definitively rejected PR2 publication"),
        )
        .await
        .expect("PR2 publication failure commits");
        drop(merge2);
        drop(publish2);
        let evaluation3 = satisfy_followup_gate(
            &fixture,
            Arc::clone(&event_bus),
            &gate_id,
            "source-sha-pr3",
            &policy,
        )
        .await
        .expect("human rework and exact Gate3");
        let (_manager3, merge3, publish3) =
            admit_followup_pr_from_gate(&fixture, temp.path(), "source-sha-pr3", &evaluation3)
                .await
                .expect("third remote admission preserves earlier PRs");
        let merge3_id = merge3.id().to_owned();
        let publish3_id = publish3.id().to_owned();
        let history2: (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT provider_config_digest, admission_state, provider_status,
                    pr_state, provider_event_id, result_event_id
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge2_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("PR2 publication failure snapshot");
        assert_eq!(history2.0, admission1_after_pr2.provider_config_digest);
        assert_eq!(history2.1, "publication_failed");
        assert_eq!(history2.2, "publication_failed");
        assert_eq!(history2.3, "failed");
        assert_eq!(history2.4, None);
        assert_eq!(history2.5.as_deref(), Some(pr2_failure.id.as_str()));
        let history_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_history WHERE task_id = ?")
                .bind(&fixture.task_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("PR1 and PR2 history count");
        assert_eq!(history_count, 2);

        // Reuse PR1's provider event ID for a different admission to prove the
        // durable dedupe identity remains scoped to the exact TaskMerge.
        let admission3 =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge3_id)
                .await
                .expect("PR3 admission lookup")
                .expect("PR3 admission exists");
        let metadata3 = set_test_provider_identity(
            &fixture,
            &merge3,
            &publish3,
            "provider-pr-3",
            "https://github.example.invalid/pull/103",
            "open",
        )
        .await;
        publish3
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("PR3 publication succeeds");
        let pr3_result = record_test_remote_outcome(
            &fixture,
            &merge3_id,
            "closed",
            Some("provider-event-pr1"),
            Some("provider-pr-3"),
            Some("https://github.example.invalid/pull/103"),
            Some(&admission3.admitted_source_sha),
            None,
            None,
        )
        .await
        .expect("same provider event identity on PR3 remains scoped to PR3");
        assert_ne!(pr3_result.id, pr1_result.id);
        drop(merge3);

        let current_before_old_replays =
            PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR3 projection before old replays")
                .expect("PR3 current projection exists");
        assert_eq!(current_before_old_replays.id, metadata3.id);
        assert_eq!(
            current_before_old_replays
                .task_merge_operation_id
                .as_deref(),
            Some(merge3_id.as_str())
        );
        let lifecycle_before_old_replays =
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR3 lifecycle before old replays")
                .expect("lifecycle exists");
        assert_eq!(
            lifecycle_before_old_replays.state,
            db::TaskLifecycleState::Blocked
        );
        let replay_pr1_after_pr3 = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "closed",
            Some("provider-event-pr1"),
            Some("provider-pr-1"),
            Some("https://github.example.invalid/pull/101"),
            Some(&admission1.admitted_source_sha),
            None,
            None,
        )
        .await
        .expect("PR1 exact replay after PR3");
        let replay_pr2_after_pr3 = record_test_remote_outcome(
            &fixture,
            &merge2_id,
            "publication_failed",
            None,
            None,
            None,
            None,
            None,
            Some("provider definitively rejected PR2 publication"),
        )
        .await
        .expect("PR2 publication failure replay after PR3");
        assert_eq!(replay_pr1_after_pr3.id, pr1_result.id);
        assert_eq!(replay_pr2_after_pr3.id, pr2_failure.id);
        let current_after_old_replays =
            PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR3 projection after old replays")
                .expect("PR3 projection remains");
        assert_eq!(
            current_after_old_replays.task_merge_operation_id.as_deref(),
            Some(merge3_id.as_str())
        );
        assert_eq!(
            current_after_old_replays.publish_operation_id.as_deref(),
            Some(publish3_id.as_str())
        );
        assert_eq!(
            current_after_old_replays.provider_pr_id.as_deref(),
            Some("provider-pr-3")
        );
        assert_eq!(current_after_old_replays.pr_state, "closed");
        let lifecycle_after_old_replays =
            db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
                .await
                .expect("PR3 lifecycle after old replays")
                .expect("lifecycle remains");
        assert_eq!(
            lifecycle_after_old_replays.version,
            lifecycle_before_old_replays.version
        );
        assert_eq!(
            lifecycle_after_old_replays.state,
            lifecycle_before_old_replays.state
        );

        let duplicate_snapshot = sqlx::query(
            "INSERT INTO remote_pr_history (
                history_id, task_id, original_metadata_id, history_origin,
                task_merge_operation_id, publish_operation_id,
                provider_config_id, provider_type, provider_config_revision,
                provider_config_digest, provider_base_url, token_secret_ref,
                provider_pr_id, pr_url, remote_repo_identity, source_branch,
                target_branch, admitted_source_sha, admission_state, provider_status,
                result_classification, observed_head_sha, merged_commit_sha,
                pr_state, merge_status, admission_status, provider_event_id,
                result_event_id, admission_created_at, result_created_at, archived_at
             )
             SELECT ?, task_id, original_metadata_id, history_origin,
                    task_merge_operation_id, publish_operation_id,
                    provider_config_id, provider_type, provider_config_revision,
                    provider_config_digest, provider_base_url, token_secret_ref,
                    provider_pr_id, pr_url, remote_repo_identity, source_branch,
                    target_branch, admitted_source_sha, admission_state, provider_status,
                    result_classification, observed_head_sha, merged_commit_sha,
                    pr_state, merge_status, admission_status, provider_event_id,
                    result_event_id, admission_created_at, result_created_at, archived_at
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(new_uuid_v4())
        .bind(&merge1_id)
        .execute(fixture.db.pool())
        .await;
        assert!(
            duplicate_snapshot.is_err(),
            "one terminal TaskMerge has one snapshot"
        );
        assert!(
            sqlx::query("DELETE FROM remote_pr_history WHERE task_merge_operation_id = ?")
                .bind(&merge1_id)
                .execute(fixture.db.pool())
                .await
                .is_err(),
            "history deletion is rejected outside guarded Project teardown"
        );

        let project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id = ?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("Project lookup before teardown");
        ProjectRepo::delete(&*fixture.db, &project_id)
            .await
            .expect("guarded teardown removes the remote PR history");
        let remaining_history: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_history")
            .fetch_one(fixture.db.pool())
            .await
            .expect("history count after Project teardown");
        assert_eq!(remaining_history, 0);
    }

    #[tokio::test]
    async fn active_remote_pr_admission_blocks_a_second_admission() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("active-pr.db").display());
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager, merge, publish, evaluation, _gate_id, _policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), event_bus)
                .await
                .expect("active PR1 admission");
        let merge_id = merge.id().to_owned();
        drop(publish);
        drop(merge);

        let contender = TaskIntegrationOperationManager::new(
            Arc::clone(&fixture.db),
            temp.path().to_path_buf(),
        );
        assert!(
            matches!(
                contender.lock_for_gate_admission(&fixture.task_id).await,
                Err(ServiceError::Conflict(_))
            ),
            "running remote PublishPr prevents a second TaskMerge admission"
        );
        let admissions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission WHERE task_id = ?")
                .bind(&fixture.task_id)
                .fetch_one(fixture.db.pool())
                .await
                .expect("remote admission count");
        assert_eq!(admissions, 1);
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
            .await
            .expect("lifecycle lookup")
            .expect("lifecycle exists");
        assert_eq!(lifecycle.state, db::TaskLifecycleState::Merging);
        let current = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("current PR metadata lookup")
            .expect("PR metadata remains");
        assert_eq!(
            current.task_merge_operation_id.as_deref(),
            Some(merge_id.as_str())
        );
        assert_eq!(current.merge_status, "pending");
        assert_eq!(evaluation.outcome, db::GateEvaluationOutcome::Satisfied);
    }

    #[tokio::test]
    async fn v116_backfill_is_reused_idempotently_before_projection_rebind() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("v115-pr.db").display());
        let fixture = fixture_at_v115(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager1, merge1, publish1, _evaluation1, gate_id, policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR1 admission on V115");
        let merge1_id = merge1.id().to_owned();
        let metadata1 = set_test_provider_identity(
            &fixture,
            &merge1,
            &publish1,
            "provider-pr-v115",
            "https://github.example.invalid/pull/115",
            "open",
        )
        .await;
        publish1
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("PR1 publication succeeds on V115");
        let admission1 =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge1_id)
                .await
                .expect("V115 PR1 admission")
                .expect("admission exists");
        let result1 = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "closed",
            Some("provider-event-v115"),
            Some("provider-pr-v115"),
            Some("https://github.example.invalid/pull/115"),
            Some(&admission1.admitted_source_sha),
            None,
            None,
        )
        .await
        .expect("terminal PR1 result on V115");
        drop(merge1);

        db::run_migrations(fixture.db.pool())
            .await
            .expect("upgrade terminal V115 database through V116");
        let backfilled: (String, String, String, String) = sqlx::query_as(
            "SELECT history_origin, provider_pr_id, pr_url, result_event_id
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("V116 reconstructs the exact frozen PR1 result");
        assert_eq!(backfilled.0, "v116_backfill");
        assert_eq!(backfilled.1, "provider-pr-v115");
        assert_eq!(backfilled.2, "https://github.example.invalid/pull/115");
        assert_eq!(backfilled.3, result1.id);

        let evaluation2 = satisfy_followup_gate(
            &fixture,
            Arc::clone(&event_bus),
            &gate_id,
            "source-sha-v116-followup",
            &policy,
        )
        .await
        .expect("human rework and exact GateEvaluation");
        let (_manager2, merge2, _publish2) = admit_followup_pr_from_gate(
            &fixture,
            temp.path(),
            "source-sha-v116-followup",
            &evaluation2,
        )
        .await
        .expect("existing V116 snapshot is reused before projection rebind");
        assert_ne!(merge2.id(), merge1_id);
        let history_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("one backfilled snapshot after later admission");
        assert_eq!(history_count, 1);
        let still_backfilled: String = sqlx::query_scalar(
            "SELECT history_origin FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("snapshot origin is immutable");
        assert_eq!(still_backfilled, "v116_backfill");
        let projection2 = PrMetadataRepo::get_by_task_id(&*fixture.db, &fixture.task_id)
            .await
            .expect("rebound current projection")
            .expect("PR2 current projection exists");
        assert_eq!(projection2.id, metadata1.id);
        assert_eq!(
            projection2.task_merge_operation_id.as_deref(),
            Some(merge2.id())
        );
    }

    #[tokio::test]
    async fn v112_file_backed_schema_upgrades_sequentially_through_v117() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!("sqlite://{}", temp.path().join("v112.db").display());
        let v112_migrations = temp.path().join("migrations-v112");
        copy_migrations_up_to(112, &v112_migrations);
        let pool = create_sqlite_pool(&database_url).await.expect("V112 pool");
        db::run_migrations_from(&pool, &v112_migrations)
            .await
            .expect("file-backed V112 schema");
        let baseline: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
            .fetch_one(&pool)
            .await
            .expect("V112 migration marker");
        assert_eq!(baseline, 112);

        db::run_migrations(&pool)
            .await
            .expect("V112 database upgrades through all current migrations");
        let applied: Vec<i64> = sqlx::query_scalar(
            "SELECT version FROM _migration WHERE version BETWEEN 113 AND 117
             ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .expect("follow-up migration markers");
        assert_eq!(applied, vec![113, 114, 115, 116, 117]);
        let history_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'remote_pr_history'",
        )
        .fetch_one(&pool)
        .await
        .expect("modern PR history schema");
        assert_eq!(history_table, 1);
    }

    #[tokio::test]
    async fn head_mismatch_snapshot_preserves_provider_merged_and_integrity_classification() {
        let temp = TempDir::new().expect("temporary directory");
        let database_url = format!(
            "sqlite://{}",
            temp.path().join("mismatch-history.db").display()
        );
        let fixture = fixture(&database_url, temp.path()).await;
        let event_bus = Arc::new(EventBus::new(32));
        let (_manager1, merge1, publish1, _evaluation1, gate_id, policy, _token_env) =
            prepare_pr_admission(&fixture, temp.path(), Arc::clone(&event_bus))
                .await
                .expect("PR1 admission");
        let merge1_id = merge1.id().to_owned();
        set_test_provider_identity(
            &fixture,
            &merge1,
            &publish1,
            "provider-pr-mismatch",
            "https://github.example.invalid/pull/116",
            "open",
        )
        .await;
        publish1
            .finish(TaskIntegrationOperationStatus::Succeeded)
            .await
            .expect("PR1 publication succeeds");
        let admission1 =
            TaskIntegrationOperationRepo::get_remote_pr_admission(&*fixture.db, &merge1_id)
                .await
                .expect("PR1 admission")
                .expect("admission exists");
        let mismatch_result = record_test_remote_outcome(
            &fixture,
            &merge1_id,
            "merged",
            Some("provider-event-mismatch"),
            Some("provider-pr-mismatch"),
            Some("https://github.example.invalid/pull/116"),
            Some("observed-wrong-head"),
            Some("provider-merged-commit"),
            None,
        )
        .await
        .expect("provider Merged wrong-head result commits");
        drop(merge1);
        let evaluation2 = satisfy_followup_gate(
            &fixture,
            Arc::clone(&event_bus),
            &gate_id,
            "source-sha-after-mismatch",
            &policy,
        )
        .await
        .expect("human rework and exact GateEvaluation");
        let (_manager2, _merge2, _publish2) = admit_followup_pr_from_gate(
            &fixture,
            temp.path(),
            "source-sha-after-mismatch",
            &evaluation2,
        )
        .await
        .expect("new admission snapshots the terminal mismatch");
        let history: (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT admission_state, provider_status, result_classification,
                    observed_head_sha, merged_commit_sha, provider_event_id,
                    result_event_id, admitted_source_sha
             FROM remote_pr_history WHERE task_merge_operation_id = ?",
        )
        .bind(&merge1_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("head mismatch history snapshot");
        assert_eq!(history.0, "head_mismatch");
        assert_eq!(history.1, "merged");
        assert_eq!(history.2, "head_mismatch");
        assert_eq!(history.3.as_deref(), Some("observed-wrong-head"));
        assert_eq!(history.4.as_deref(), Some("provider-merged-commit"));
        assert_eq!(history.5.as_deref(), Some("provider-event-mismatch"));
        assert_eq!(history.6.as_deref(), Some(mismatch_result.id.as_str()));
        assert_eq!(history.7, admission1.admitted_source_sha);
    }
}
