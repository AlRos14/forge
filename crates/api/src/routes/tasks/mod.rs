use api_types::{
    AddDependencyRequest, AddRoleMembershipRequest, CreateTaskRequest, CreateTaskRoleRequest,
    DiffEnvelope, ReorderSubtasksRequest, RoleMembershipResponse, TaskDependency, TaskResponse,
    TaskRoleResponse, TasksResponse, UpdateRoleMembershipRequest, UpdateTaskRequest,
    UpdateTaskRoleRequest, WorkspaceResponse,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use db::{
    now_rfc3339, ExecutionRepo, PageRequest, ProjectRepo, SharedMediaRepo, SortBy, SortOrder,
    TaskBoardRepo, TaskDependencyRepo, TaskListQuery, TaskMediaRepo, TaskRepo, UpdateTask,
    WorkspaceRepo,
};
use serde_json::Value;
use services::{DiffService, ServiceError};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{
        auth::AuthenticatedUser, execution_response, parse_csv, task_page_request, task_response,
        task_response_light, workspace_response, ListParams,
    },
    state::AppState,
};

mod crud;
mod dependencies;
mod gates;
mod media;
mod reviews;
mod roles;
mod workspace;

pub use crud::{create_task, delete_task, get_task, list_tasks, reorder_subtasks, update_task};
pub use dependencies::{add_dependency, list_dependencies, list_dependents, remove_dependency};
pub use gates::{
    create_task_gate, evaluate_gate, get_gate, get_gate_evaluation, get_task_lifecycle,
    list_task_gates, list_task_lifecycle_transitions, merge_after_gate, revise_gate_policy,
    transition_task_lifecycle,
};
pub use reviews::{list_reviews, trigger_review};
pub use roles::{
    add_task_role_member, create_task_role_model, list_task_role_model, update_task_role_member,
    update_task_role_model,
};
pub use workspace::{get_task_diff, get_task_workspace, reset_task_workspace};

pub(super) async fn require_task_visible(
    state: &AppState,
    task_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<db::Task> {
    let task = TaskRepo::get_by_id(&*state.db, task_id, false)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;
    let project = ProjectRepo::get_by_id(&*state.db, &task.project_id)
        .await?
        .ok_or_else(|| ApiError::not_found("task", task_id.to_owned()))?;

    if project.owner_id.is_none() || project.owner_id.as_deref() == Some(user.user_id.as_str()) {
        return Ok(task);
    }

    let member = db::ProjectMemberRepo::get_member(&*state.db, &project.id, &user.user_id).await?;
    if member.is_none() {
        return Err(ApiError::not_found("task", task_id.to_owned()));
    }

    Ok(task)
}

fn map_diff_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::NotFound { entity, id } if entity == "workspace" => {
            ApiError::not_found_with_code("workspace.not_found", entity, id)
        }
        ServiceError::InvalidOperation { message } if message.contains("error state") => {
            ApiError::conflict_with_code("workspace.error_state", message)
        }
        other => ApiError::from(other),
    }
}
