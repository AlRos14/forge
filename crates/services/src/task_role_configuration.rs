use std::collections::{HashMap, HashSet};

use api_types::{
    ProjectTaskRoleConfiguration, ReworkActorSelection, TaskRoleCapability, TaskRoleConfiguration,
    TaskRoleReworkConfiguration, TaskRoleReworkMode,
};
use serde_json::Value;

use crate::{Result, ServiceError};

pub fn parse_task_role_configuration(policy_json: &str) -> Result<TaskRoleConfiguration> {
    let document: Value = serde_json::from_str(policy_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid TaskRole configuration: {error}"))
    })?;
    let configuration = if document == Value::Object(Default::default()) {
        TaskRoleConfiguration::default()
    } else {
        serde_json::from_value(document).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid TaskRole configuration: {error}"))
        })?
    };
    validate_task_role_configuration(&configuration)?;
    Ok(configuration)
}

pub fn store_task_role_configuration(configuration: &TaskRoleConfiguration) -> Result<String> {
    validate_task_role_configuration(configuration)?;
    serde_json::to_string(configuration).map_err(|error| {
        ServiceError::invalid_operation(format!("TaskRole configuration cannot be stored: {error}"))
    })
}

pub fn project_configuration_to_task_role(
    configuration: &ProjectTaskRoleConfiguration,
    task_role_ids: &HashMap<String, String>,
) -> Result<TaskRoleConfiguration> {
    let rework = configuration
        .rework
        .as_ref()
        .map(|routing| {
            let allowed_destination_role_ids = routing
                .allowed_destination_roles
                .iter()
                .map(|role| {
                    task_role_ids.get(role).cloned().ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "Project rework destination role {role:?} was not copied into this Task"
                        ))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let next_review_role_id = routing
                .next_review_role
                .as_ref()
                .map(|role| {
                    task_role_ids.get(role).cloned().ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "Project next review role {role:?} was not copied into this Task"
                        ))
                    })
                })
                .transpose()?;
            Ok::<TaskRoleReworkConfiguration, ServiceError>(TaskRoleReworkConfiguration {
                mode: routing.mode,
                allowed_destination_role_ids,
                next_review_role_id,
                correction_actor: routing.correction_actor.clone(),
                review_actor: routing.review_actor.clone(),
            })
        })
        .transpose()?;
    let result = TaskRoleConfiguration {
        schema_version: configuration.schema_version,
        display_label: configuration.display_label.clone(),
        capabilities: configuration.capabilities.clone(),
        rework,
    };
    validate_task_role_configuration(&result)?;
    Ok(result)
}

pub fn project_configuration_to_json(
    configuration: &ProjectTaskRoleConfiguration,
) -> Result<String> {
    validate_project_task_role_configuration(configuration)?;
    serde_json::to_string(configuration).map_err(|error| {
        ServiceError::invalid_operation(format!(
            "Project TaskRole configuration cannot be stored: {error}"
        ))
    })
}

pub fn validate_task_role_configuration(configuration: &TaskRoleConfiguration) -> Result<()> {
    if configuration.schema_version != 1 {
        return Err(ServiceError::invalid_operation(format!(
            "unsupported TaskRole configuration schema version {}",
            configuration.schema_version
        )));
    }
    if configuration
        .display_label
        .as_ref()
        .is_some_and(|label| label.trim().is_empty() || label.len() > 120)
    {
        return Err(ServiceError::invalid_operation(
            "TaskRole display_label must be non-empty and at most 120 bytes",
        ));
    }
    let mut capabilities = HashSet::new();
    if configuration
        .capabilities
        .iter()
        .any(|capability| !capabilities.insert(*capability))
    {
        return Err(ServiceError::invalid_operation(
            "TaskRole capabilities cannot contain duplicates",
        ));
    }
    let can_request_rework = configuration
        .capabilities
        .contains(&TaskRoleCapability::RequestRework);
    if can_request_rework != configuration.rework.is_some() {
        return Err(ServiceError::invalid_operation(
            "request_rework requires exactly one persisted rework routing policy",
        ));
    }
    if let Some(rework) = configuration.rework.as_ref() {
        validate_rework_configuration(rework)?;
    }
    Ok(())
}

pub fn validate_project_task_role_configuration(
    configuration: &ProjectTaskRoleConfiguration,
) -> Result<()> {
    if configuration.coordinator_actor.is_some()
        && (!configuration
            .capabilities
            .contains(&TaskRoleCapability::Orchestration)
            || !matches!(
                configuration.coordinator_actor,
                Some(api_types::ActorRef::Agent(_))
            ))
    {
        return Err(ServiceError::invalid_operation(
            "A default coordinator must be an Agent in an orchestration-capable role",
        ));
    }
    if configuration.schema_version != 1 {
        return Err(ServiceError::invalid_operation(format!(
            "unsupported Project TaskRole configuration schema version {}",
            configuration.schema_version
        )));
    }
    if configuration
        .display_label
        .as_ref()
        .is_some_and(|label| label.trim().is_empty() || label.len() > 120)
    {
        return Err(ServiceError::invalid_operation(
            "Project TaskRole display_label must be non-empty and at most 120 bytes",
        ));
    }
    let mut capabilities = HashSet::new();
    if configuration
        .capabilities
        .iter()
        .any(|capability| !capabilities.insert(*capability))
    {
        return Err(ServiceError::invalid_operation(
            "Project TaskRole capabilities cannot contain duplicates",
        ));
    }
    if let Some(rework) = configuration.rework.as_ref() {
        if !configuration
            .capabilities
            .contains(&TaskRoleCapability::RequestRework)
        {
            return Err(ServiceError::invalid_operation(
                "Project rework routing requires request_rework capability",
            ));
        }
        if rework
            .allowed_destination_roles
            .iter()
            .any(|role| role.trim().is_empty())
            || rework
                .next_review_role
                .as_deref()
                .is_some_and(|role| role.trim().is_empty())
        {
            return Err(ServiceError::invalid_operation(
                "Project rework routing role names cannot be empty",
            ));
        }
        let mut destinations = HashSet::new();
        if rework
            .allowed_destination_roles
            .iter()
            .any(|role| !destinations.insert(role))
        {
            return Err(ServiceError::invalid_operation(
                "Project rework destination roles cannot contain duplicates",
            ));
        }
        if rework.mode == TaskRoleReworkMode::Automatic
            && (rework.allowed_destination_roles.is_empty()
                || rework.next_review_role.is_none()
                || rework.correction_actor.is_none()
                || rework.review_actor.is_none())
        {
            return Err(ServiceError::invalid_operation(
                "automatic Project rework requires destination, next review role, correction Actor, and review Actor",
            ));
        }
    } else if configuration
        .capabilities
        .contains(&TaskRoleCapability::RequestRework)
    {
        return Err(ServiceError::invalid_operation(
            "request_rework requires a persisted Project routing policy",
        ));
    }
    Ok(())
}

fn validate_rework_configuration(configuration: &TaskRoleReworkConfiguration) -> Result<()> {
    if configuration
        .allowed_destination_role_ids
        .iter()
        .any(|role_id| role_id.trim().is_empty())
        || configuration
            .next_review_role_id
            .as_deref()
            .is_some_and(|role_id| role_id.trim().is_empty())
    {
        return Err(ServiceError::invalid_operation(
            "rework TaskRole IDs cannot be empty",
        ));
    }
    let mut destinations = HashSet::new();
    if configuration
        .allowed_destination_role_ids
        .iter()
        .any(|role_id| !destinations.insert(role_id))
    {
        return Err(ServiceError::invalid_operation(
            "rework destination TaskRole IDs cannot contain duplicates",
        ));
    }
    if configuration.mode == TaskRoleReworkMode::Automatic
        && (configuration.allowed_destination_role_ids.is_empty()
            || configuration.next_review_role_id.is_none()
            || configuration.correction_actor.is_none()
            || configuration.review_actor.is_none())
    {
        return Err(ServiceError::invalid_operation(
            "automatic rework requires destination, next review role, correction Actor, and review Actor",
        ));
    }
    if configuration.review_actor.is_some()
        && matches!(
            configuration.correction_actor,
            Some(ReworkActorSelection::Explicit { ref actor_ref }) if matches!(actor_ref, api_types::ActorRef::Human(_)) && configuration.review_actor.as_ref() == Some(actor_ref)
        )
    {
        return Err(ServiceError::invalid_operation(
            "correction and review Actors must be different",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_types::{ProjectTaskRoleReworkConfiguration, TaskRoleReworkMode};

    #[test]
    fn configuration_is_separate_from_orchestrator_policy_and_round_trips() {
        let stored = store_task_role_configuration(&TaskRoleConfiguration {
            capabilities: vec![TaskRoleCapability::FormalReview],
            ..TaskRoleConfiguration::default()
        })
        .expect("configuration stores");
        assert_eq!(
            parse_task_role_configuration(&stored)
                .expect("configuration parses")
                .capabilities,
            vec![TaskRoleCapability::FormalReview]
        );
    }

    #[test]
    fn project_role_references_resolve_to_task_local_ids() {
        let mut ids = HashMap::new();
        ids.insert("implementer".to_owned(), "task-role-impl".to_owned());
        ids.insert("reviewer".to_owned(), "task-role-review".to_owned());
        let config = ProjectTaskRoleConfiguration {
            capabilities: vec![TaskRoleCapability::RequestRework],
            rework: Some(ProjectTaskRoleReworkConfiguration {
                mode: TaskRoleReworkMode::Manual,
                allowed_destination_roles: vec!["implementer".to_owned()],
                next_review_role: Some("reviewer".to_owned()),
                correction_actor: None,
                review_actor: None,
            }),
            ..ProjectTaskRoleConfiguration::default()
        };
        let result = project_configuration_to_task_role(&config, &ids).expect("role refs resolve");
        let rework = result.rework.expect("rework config");
        assert_eq!(rework.allowed_destination_role_ids, ["task-role-impl"]);
        assert_eq!(
            rework.next_review_role_id.as_deref(),
            Some("task-role-review")
        );
    }
}
