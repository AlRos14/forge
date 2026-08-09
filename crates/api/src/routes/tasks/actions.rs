use super::*;

pub async fn list_task_actions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::TaskActionsResponse>> {
    let available_actions = state.task_service.available_task_actions(id).await?;
    Ok(Json(api_types::TaskActionsResponse { available_actions }))
}

pub async fn start_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Start, body).await
}

pub async fn pause_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Pause, body).await
}

pub async fn resume_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Resume, body).await
}

pub async fn submit_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Submit, body).await
}

pub async fn request_changes_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::RequestChanges, body).await
}

pub async fn approve_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Approve, body).await
}

pub async fn cancel_task(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    execute(&state, id, TaskAction::Cancel, body).await
}

async fn execute(
    state: &AppState,
    id: String,
    action: TaskAction,
    body: Option<Json<Option<TaskActionRequest>>>,
) -> ApiResult<Json<TaskResponse>> {
    let request = body.and_then(|body| body.0).unwrap_or_default();
    let result = state
        .task_service
        .perform_task_action(id, action, request.reason, request.version)
        .await?;
    Ok(Json(task_response(&state.db, result.task).await?))
}
