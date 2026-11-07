use api_types::TaskRoleCapability;
use db::{
    ExecutionTaskRoleAuthorityRepo, SqliteDb, TaskRoleCapabilityConfigurationRepo, TaskRoleRepo,
};

use crate::{Result, ServiceError};

/// Read a capability admitted on this exact Execution. Current TaskRole edits
/// never grant new authority to an already admitted attempt.
pub async fn execution_has_capability(
    db: &SqliteDb,
    execution_id: &str,
    capability: TaskRoleCapability,
) -> Result<bool> {
    let Some(_snapshot) =
        ExecutionTaskRoleAuthorityRepo::get_by_execution_id(db, execution_id).await?
    else {
        return Ok(false);
    };
    let capabilities = execution_capabilities(db, execution_id).await?;
    Ok(capabilities.contains(&capability))
}

pub async fn execution_capabilities(
    db: &SqliteDb,
    execution_id: &str,
) -> Result<Vec<TaskRoleCapability>> {
    let Some(snapshot) =
        ExecutionTaskRoleAuthorityRepo::get_by_execution_id(db, execution_id).await?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str(&snapshot.capabilities_json).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "Execution TaskRole capability snapshot is invalid: {error}"
        ))
    })
}

pub async fn task_role_name_has_capability(
    db: &SqliteDb,
    task_id: &str,
    role_name: &str,
    capability: TaskRoleCapability,
) -> Result<bool> {
    let Some(role_name) = db::canonical_task_role_name(role_name) else {
        return Ok(false);
    };
    let Some(role) = TaskRoleRepo::get_by_task_and_role(db, task_id, &role_name).await? else {
        return Ok(false);
    };
    current_task_role_has_capability(db, &role.id, capability).await
}

/// Read current role configuration for a new administrative or mutating
/// action. This deliberately does not consult a historical Execution snapshot.
pub async fn current_task_role_has_capability(
    db: &SqliteDb,
    task_role_id: &str,
    capability: TaskRoleCapability,
) -> Result<bool> {
    let Some(configuration) = current_task_role_configuration(db, task_role_id).await? else {
        return Ok(false);
    };
    Ok(configuration.capabilities.contains(&capability))
}

pub async fn current_task_role_configuration(
    db: &SqliteDb,
    task_role_id: &str,
) -> Result<Option<api_types::TaskRoleConfiguration>> {
    let Some(stored) =
        TaskRoleCapabilityConfigurationRepo::get_by_task_role_id(db, task_role_id).await?
    else {
        return Ok(None);
    };
    let configuration: api_types::TaskRoleConfiguration =
        serde_json::from_str(&stored.configuration_json).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "TaskRole capability configuration is invalid: {error}"
            ))
        })?;
    crate::task_role_configuration::validate_task_role_configuration(&configuration)?;
    Ok(Some(configuration))
}
