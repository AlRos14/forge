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

pub async fn rebase_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::MergeOutcomeResponse>> {
    let outcome = state.merge_service.rebase(id).await?;
    Ok(Json(merge_outcome_response(outcome)))
}

pub async fn get_conflict_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::ConflictStateResponse>> {
    let cs = state.merge_service.conflict_state(id).await?;
    Ok(Json(api_types::ConflictStateResponse {
        operation: format!("{:?}", cs.operation).to_lowercase(),
        conflict_paths: cs.conflict_paths,
    }))
}

pub async fn abort_task_conflict(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    state.merge_service.abort_conflict(id).await?;
    Ok(StatusCode::NO_CONTENT)
}
