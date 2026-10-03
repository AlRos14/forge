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

    pub(crate) async fn acquire_after_gate(
        &self,
        task_id: &str,
        owner_id: &str,
        gate_evaluation_id: &str,
    ) -> crate::Result<TaskIntegrationOperationGuard> {
        self.acquire_kind_after_gate(
            task_id,
            TaskIntegrationOperationKind::TaskMerge,
            owner_id,
            gate_evaluation_id,
        )
        .await
    }

    pub(crate) async fn acquire_kind_after_gate(
        &self,
        task_id: &str,
        kind: TaskIntegrationOperationKind,
        owner_id: &str,
        gate_evaluation_id: &str,
    ) -> crate::Result<TaskIntegrationOperationGuard> {
        self.acquire_with_gate(task_id, kind, owner_id, Some(gate_evaluation_id))
            .await
    }

    /// Finish an operation when an external provider has durably confirmed
    /// that the exact admitted merge completed. If a prior process crashed
    /// after admission, resume that same operation under the Task lock.
    pub(crate) async fn record_provider_confirmed_merge(
        &self,
        task_id: &str,
        owner_id: &str,
        gate_evaluation_id: &str,
    ) -> crate::Result<TaskIntegrationOperation> {
        let file = match self.try_process_lock(task_id).await? {
            Ok(file) => file,
            Err(()) => return Err(busy(task_id)),
        };
        let operation =
            match TaskIntegrationOperationRepo::get_active_for_task(&*self.db, task_id).await? {
                Some(operation)
                    if operation.kind == TaskIntegrationOperationKind::TaskMerge
                        && operation.gate_evaluation_id.as_deref() == Some(gate_evaluation_id) =>
                {
                    operation
                }
                Some(_) => return Err(busy(task_id)),
                None => {
                    TaskIntegrationOperationRepo::begin(
                        &*self.db,
                        CreateTaskIntegrationOperation {
                            id: new_uuid_v4(),
                            task_id: task_id.to_owned(),
                            kind: TaskIntegrationOperationKind::TaskMerge,
                            owner_id: owner_id.to_owned(),
                            gate_evaluation_id: Some(gate_evaluation_id.to_owned()),
                            created_at: now_rfc3339(),
                        },
                    )
                    .await?
                }
            };
        TaskIntegrationOperationGuard {
            db: Arc::clone(&self.db),
            operation,
            _file: Some(file),
        }
        .finish(TaskIntegrationOperationStatus::Succeeded)
        .await
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
            created_at: now_rfc3339(),
        };

        match TaskIntegrationOperationRepo::begin(&*self.db, input.clone()).await {
            Ok(operation) => match self.try_process_lock(task_id).await {
                Ok(Ok(file)) => {
                    let active = match TaskIntegrationOperationRepo::get_active_for_task(
                        &*self.db, task_id,
                    )
                    .await
                    {
                        Ok(active) => active,
                        Err(error) => {
                            drop(file);
                            let original_error: ServiceError = error.into();
                            if let Err(abandon_error) = self.abandon_unowned(&operation).await {
                                return Err(ServiceError::invalid_operation(format!(
                                    "{original_error}; could not abandon the operation claim created by this process: {abandon_error}"
                                )));
                            }
                            return Err(original_error);
                        }
                    };
                    if active.as_ref().map(|active| active.id.as_str())
                        != Some(operation.id.as_str())
                    {
                        drop(file);
                        return Err(busy(task_id));
                    }
                    Ok(TaskIntegrationOperationGuard {
                        db: Arc::clone(&self.db),
                        operation,
                        _file: Some(file),
                    })
                }
                Ok(Err(())) => {
                    self.abandon_unowned(&operation).await?;
                    Err(busy(task_id))
                }
                Err(lock_error) => {
                    if let Err(abandon_error) = self.abandon_unowned(&operation).await {
                        return Err(ServiceError::invalid_operation(format!(
                            "{lock_error}; could not abandon the operation claim created by this process: {abandon_error}"
                        )));
                    }
                    Err(lock_error)
                }
            },
            Err(db::DbError::TaskIntegrationOperationBusy) => {
                let file = match self.try_process_lock(task_id).await? {
                    Ok(file) => file,
                    Err(()) => return Err(busy(task_id)),
                };
                let now = now_rfc3339();
                let operation =
                    TaskIntegrationOperationRepo::recover_stale_and_begin(&*self.db, input, &now)
                        .await
                        .map_err(|error| match error {
                            db::DbError::TaskIntegrationOperationBusy => busy(task_id),
                            error => error.into(),
                        })?;
                Ok(TaskIntegrationOperationGuard {
                    db: Arc::clone(&self.db),
                    operation,
                    _file: Some(file),
                })
            }
            Err(error) => Err(error.into()),
        }
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

    async fn abandon_unowned(&self, operation: &TaskIntegrationOperation) -> crate::Result<()> {
        let now = now_rfc3339();
        match TaskIntegrationOperationRepo::finish(
            &*self.db,
            FinishTaskIntegrationOperation {
                id: operation.id.clone(),
                expected_version: operation.version,
                status: TaskIntegrationOperationStatus::Abandoned,
                updated_at: now.clone(),
                finished_at: now,
            },
        )
        .await
        {
            Ok(_) | Err(db::DbError::VersionConflict) => Ok(()),
            Err(error) => Err(error.into()),
        }
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
        let now = now_rfc3339();
        let result = TaskIntegrationOperationRepo::finish(
            &*self.db,
            FinishTaskIntegrationOperation {
                id: self.operation.id.clone(),
                expected_version: self.operation.version,
                status,
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
        create_sqlite_pool, run_migrations, CreateProject, CreateRepo, CreateTask,
        CreateTerminalSession, CreateWorkspace, ProjectRepo, RepoRepo,
        TaskIntegrationOperationKind, TaskRepo, TerminalSessionRepo, UserRepo, WorkMode,
        WorkspaceRepo, WorkspaceStatus,
    };
    use std::path::Path;
    use tempfile::TempDir;

    struct Fixture {
        db: Arc<SqliteDb>,
        task_id: String,
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
                repo_id,
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
                kind: TaskIntegrationOperationKind::TaskMerge,
                owner_id: "simulated-operation-owner".to_owned(),
                gate_evaluation_id: None,
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("durable running operation")
    }

    #[tokio::test]
    async fn acquire_abandons_its_claim_when_process_lock_setup_fails() {
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
        assert_eq!(statuses, vec!["abandoned"]);
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
            TaskIntegrationOperationKind::TaskMerge,
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
                TaskIntegrationOperationKind::TaskMerge,
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
}
