use super::*;

pub async fn get_task_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<WorkspaceResponse>> {
    require_task_visible(&state, &id, &user).await?;
    let workspace = WorkspaceRepo::get_integration_by_task_id(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("workspace", id))?;
    Ok(Json(workspace_response(workspace)))
}

pub async fn get_task_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<DiffEnvelope>> {
    require_task_visible(&state, &id, &user).await?;
    let diff = DiffService::new(std::sync::Arc::clone(&state.db))
        .task_diff(&id)
        .await
        .map_err(map_diff_error)?;
    Ok(Json(DiffEnvelope { data: diff }))
}
