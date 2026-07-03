use std::sync::Arc;

use db::{DaemonRepo, SqliteDb};

use crate::daemon_transport::providers::{ExecutionProvider, FilesystemProvider};
use crate::daemon_transport::{
    DaemonConnectionRegistry, EmbeddedFilesystemProvider, RemoteExecutionProvider,
    RemoteFilesystemProvider,
};
use crate::ServiceError;

pub async fn select_filesystem_provider(
    daemon_id: &str,
    db: &SqliteDb,
    registry: &DaemonConnectionRegistry,
) -> Result<Arc<dyn FilesystemProvider>, ServiceError> {
    match resolve_provider_target(daemon_id, db, registry).await? {
        ProviderTarget::Remote => Ok(Arc::new(RemoteFilesystemProvider::new(
            Arc::new(registry.clone()),
            daemon_id.to_owned(),
        ))),
        ProviderTarget::Embedded => Ok(Arc::new(EmbeddedFilesystemProvider::new())),
    }
}

pub async fn select_execution_provider(
    daemon_id: Option<&str>,
    db: &SqliteDb,
    registry: &DaemonConnectionRegistry,
) -> Result<Arc<dyn ExecutionProvider>, ServiceError> {
    let Some(daemon_id) = daemon_id else {
        return registry.embedded_execution_provider();
    };
    match resolve_provider_target(daemon_id, db, registry).await? {
        ProviderTarget::Remote => Ok(Arc::new(RemoteExecutionProvider::new(
            Arc::new(registry.clone()),
            daemon_id.to_owned(),
        ))),
        ProviderTarget::Embedded => registry.embedded_execution_provider(),
    }
}

enum ProviderTarget {
    Remote,
    Embedded,
}

async fn resolve_provider_target(
    daemon_id: &str,
    db: &SqliteDb,
    registry: &DaemonConnectionRegistry,
) -> Result<ProviderTarget, ServiceError> {
    let daemon =
        DaemonRepo::get_by_id(db, daemon_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound {
                entity: "daemon",
                id: daemon_id.to_owned(),
            })?;

    // A live command socket always uses remote transport. Without one, only the
    // daemon row for this server's embedded machine id may execute in-process.
    if registry.is_connected(daemon_id) {
        Ok(ProviderTarget::Remote)
    } else if crate::embedded_daemon::is_embedded_daemon_machine(&daemon.machine_id) {
        Ok(ProviderTarget::Embedded)
    } else {
        Err(ServiceError::DaemonUnavailable {
            daemon_id: daemon_id.to_owned(),
        })
    }
}
