use api_types::{StateKind, WorkflowDefinition};
use db::{ExecutionRepo, ExecutionStatus, PageRequest, ResumePolicy, SortBy, SortOrder};
use serde_json::Value;

use crate::{Result, ServiceError};

pub(super) fn is_io_or_workspace_error(error: &ServiceError) -> bool {
    match error {
        ServiceError::InvalidOperation { message } => {
            message.contains("io error:") || message.contains("No such file or directory")
        }
        _ => false,
    }
}

pub(super) fn has_blocking_annotation(task: &db::Task) -> bool {
    let Some(raw_annotation) = task.error_annotation.as_deref() else {
        return false;
    };
    let Ok(annotation) = serde_json::from_str::<Value>(raw_annotation) else {
        return false;
    };
    let Some(kind) = annotation.get("type").and_then(Value::as_str) else {
        return false;
    };
    matches!(
        kind,
        "manual_stop"
            | "workspace_error"
            | "agent_timeout"
            | "recovery_required"
            | "workspace_reset_required"
            | "max_turns_exceeded"
            | "before_work_hook_failed"
            | "before_work_hook_timeout"
    )
}

pub(super) fn role_assignment_unassigned(assignment: Option<&db::TaskRoleAssignment>) -> bool {
    !assignment.is_some_and(|assignment| {
        assignment.assignee_type.is_some() && assignment.assignee_id.is_some()
    })
}

pub(super) fn execution_guard_roles(role: &str) -> Vec<&str> {
    let mut roles = vec![role];
    if role == crate::workflow::default_roles::CODER {
        roles.push("executor");
    }
    roles
}

pub(super) async fn reviewer_dispatch_ready(
    _db: &db::SqliteDb,
    _task_id: &str,
    _state_config: &Value,
) -> Result<bool> {
    // Deterministic checks are independent ValidationRuns. Reviewer dispatch
    // does not read the retired Review.step_results projection.
    Ok(true)
}

pub(super) async fn latest_stopped_execution_blocks_dispatch(
    db: &db::SqliteDb,
    task_id: &str,
    role_name: &str,
) -> Result<bool> {
    let page = ExecutionRepo::list_by_task_and_role(
        db,
        task_id,
        role_name,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;

    let Some(execution) = page
        .items
        .into_iter()
        .find(|execution| execution.work_unit_id.is_none())
    else {
        return Ok(false);
    };
    if execution.status == ExecutionStatus::Running {
        return Ok(false);
    }

    Ok(matches!(
        execution.resume_policy,
        None | Some(ResumePolicy::Manual)
    ))
}

pub(super) async fn has_running_execution_for_roles(
    db: &db::SqliteDb,
    task_id: &str,
    roles: &[&str],
) -> Result<bool> {
    let page = ExecutionRepo::list_by_task(
        db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(page.items.iter().any(|execution| {
        execution.work_unit_id.is_none()
            && execution.status == ExecutionStatus::Running
            && roles.iter().any(|role| execution.role == *role)
    }))
}

pub(super) fn first_transition_to_kind<'a>(
    workflow: &'a WorkflowDefinition,
    from_state: &str,
    kinds: &[StateKind],
) -> Option<&'a api_types::StateDefinition> {
    workflow
        .outgoing_trigger_targets(from_state)
        .filter(|(trigger, _)| !trigger.system_only())
        .find_map(|(_, target)| {
            workflow
                .states
                .iter()
                .find(|state| state.name == target && kinds.contains(&state.kind))
        })
}

pub(super) fn merged_state_config(
    state: &api_types::StateDefinition,
    project: &db::Project,
    task_state_config_json: Option<&str>,
) -> Value {
    let mut merged = state.config.clone();
    if state.name == crate::workflow::default_states::REVIEW {
        merge_project_review_config(&mut merged, project);
    }

    let Some(task_state_config_json) = task_state_config_json else {
        return merged;
    };
    let Ok(Value::Object(task_config)) = serde_json::from_str::<Value>(task_state_config_json)
    else {
        return merged;
    };
    let Some(Value::Object(overrides)) = task_config.get(&state.name) else {
        return merged;
    };

    match &mut merged {
        Value::Object(defaults) => {
            for (key, value) in overrides {
                defaults.insert(key.clone(), value.clone());
            }
            merged
        }
        _ => Value::Object(overrides.clone()),
    }
}

fn merge_project_review_config(merged: &mut Value, project: &db::Project) {
    let Ok(settings) = serde_json::from_str::<Value>(&project.settings) else {
        return;
    };
    let Some(Value::Object(review_config)) = settings.get("default_review_config") else {
        return;
    };
    match merged {
        Value::Object(defaults) => {
            for (key, value) in review_config {
                defaults.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        _ => {
            *merged = Value::Object(review_config.clone());
        }
    }
}
