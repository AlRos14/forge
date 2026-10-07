use std::sync::Arc;

use db::{DaemonRepo, Execution, SqliteDb};
use serde_json::Value;

use crate::daemon_transport::providers::{ExecutionProvider, FilesystemProvider};
use crate::daemon_transport::{
    DaemonConnectionRegistry, EmbeddedFilesystemProvider, RemoteExecutionProvider,
    RemoteFilesystemProvider,
};
use crate::ServiceError;

/// Read the daemon identity selected when an Execution was admitted. Existing
/// Executions never fall back to the Agent's mutable daemon binding because
/// that can route an attempt to another host or native account.
pub(crate) fn resolved_daemon_id_for_execution(
    execution: &Execution,
) -> Result<String, ServiceError> {
    let snapshot = execution
        .executor_config_snapshot_json
        .as_deref()
        .ok_or_else(|| {
            ServiceError::invalid_operation("execution has no frozen daemon routing snapshot")
        })?;
    let snapshot = serde_json::from_str::<Value>(snapshot).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "execution daemon routing snapshot is invalid: {error}"
        ))
    })?;
    snapshot
        .get("resolved_daemon_id")
        .and_then(Value::as_str)
        .filter(|daemon_id| !daemon_id.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "execution snapshot has no valid resolved_daemon_id; refusing to select a replacement host",
            )
        })
}

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
