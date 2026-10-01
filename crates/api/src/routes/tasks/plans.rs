use super::*;
use api_types::TaskPlanHistoryResponse;

pub async fn get_task_plan(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<TaskPlanHistoryResponse>> {
    require_task_visible(&state, &id, &user).await?;
    let artifacts = services::plan_artifact::list_plan_artifacts(&state.db, &id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(TaskPlanHistoryResponse { artifacts }))
}
