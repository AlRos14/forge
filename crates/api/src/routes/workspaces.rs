use api_types::DiffEnvelope;
use axum::{
    extract::{Path, State},
    Json,
};
use services::{DiffService, ServiceError};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, tasks::require_task_visible},
    state::AppState,
};
use db::WorkspaceRepo;

pub async fn get_workspace_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<DiffEnvelope>> {
    let workspace = WorkspaceRepo::get_by_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace", id.clone()))?;
    require_task_visible(&state, &workspace.task_id, &user).await?;
    let diff = DiffService::new(std::sync::Arc::clone(&state.db))
        .workspace_diff(&id)
        .await
        .map_err(map_diff_error)?;
    Ok(Json(DiffEnvelope { data: diff }))
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
