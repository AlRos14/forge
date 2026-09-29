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
        let input = CreateTaskIntegrationOperation {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            kind,
            owner_id: owner_id.to_owned(),
            created_at: now_rfc3339(),
        };

        match TaskIntegrationOperationRepo::begin(&*self.db, input.clone()).await {
            Ok(operation) => match self.try_process_lock(task_id).await? {
                Ok(file) => Ok(TaskIntegrationOperationGuard {
                    db: Arc::clone(&self.db),
                    operation,
                    _file: Some(file),
                }),
                Err(()) => {
                    self.abandon_unowned(&operation).await?;
                    Err(busy(task_id))
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

    async fn try_process_lock(&self, task_id: &str) -> crate::Result<Result<File, ()>> {
        let lock_root = self.lock_root().await?;
        tokio::fs::create_dir_all(&lock_root)
            .await
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "could not prepare Task integration operation lock: {error}"
                ))
            })?;
        let task_key = hex::encode(Sha256::digest(task_id.as_bytes()));
        let lock_path = lock_root.join(format!("{task_key}.lock"));
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
    pub(crate) async fn finish(self, status: TaskIntegrationOperationStatus) -> crate::Result<()> {
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
        result.map(|_| ()).map_err(Into::into)
    }
}

fn busy(task_id: &str) -> ServiceError {
    ServiceError::conflict(format!(
        "Task {task_id} already has an exclusive integration workspace operation"
    ))
}
