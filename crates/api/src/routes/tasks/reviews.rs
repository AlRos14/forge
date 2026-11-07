use super::*;

pub async fn trigger_review(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
    Json(request): Json<api_types::StartReviewExecutionRequest>,
) -> ApiResult<Json<api_types::ReviewExecutionResponse>> {
    let _task = super::require_task_mutable(&state, &task_id, &user).await?;
    let execution = state
        .task_service
        .start_human_review_execution_for_role(
            &task_id,
            &user.user_id,
            &request.task_role_id,
            request.workspace_id.as_deref(),
        )
        .await?;
    Ok(Json(api_types::ReviewExecutionResponse {
        execution: execution_response(execution),
        report: None,
    }))
}

/// Compatibility URL; the response is a projection of exact reviewer
/// Executions and their ReviewReport outputs, never legacy Review rows.
pub async fn list_reviews(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Vec<api_types::ReviewExecutionResponse>>> {
    let _task = super::require_task_visible(&state, &task_id, &user).await?;
    let page = ExecutionRepo::list_by_task(
        &*state.db,
        &task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut reviews = Vec::new();
    for execution in page.items {
        if !services::role_capabilities::execution_has_capability(
            &state.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?
        {
            continue;
        }
        let report = db::CollaborationRepo::get_execution_artifact_output(
            &*state.db,
            &execution.id,
            db::ArtifactKind::ReviewReport,
        )
        .await?
        .map(|artifact| crate::routes::collaboration::artifact_response(artifact, true))
        .transpose()?;
        reviews.push(api_types::ReviewExecutionResponse {
            execution: execution_response(execution),
            report,
        });
    }
    Ok(Json(reviews))
}
