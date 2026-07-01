use super::*;

pub async fn get_task_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let workspace = WorkspaceRepo::get_by_task_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace", id))?;
    Ok(Json(workspace_response(workspace)))
}

pub async fn reset_task_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WorkspaceResponse>> {
    let workspace = state.task_service.reset_task_workspace(&id).await?;
    Ok(Json(workspace_response(workspace)))
}

pub async fn get_task_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<DiffEnvelope>> {
    let diff = DiffService::new(std::sync::Arc::clone(&state.db))
        .task_diff(&id)
        .await
        .map_err(map_diff_error)?;
    Ok(Json(DiffEnvelope { data: diff }))
}
