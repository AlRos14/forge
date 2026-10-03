use super::*;

pub async fn trigger_review(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
    Json(request): Json<api_types::StartReviewExecutionRequest>,
) -> ApiResult<Json<api_types::ReviewExecutionResponse>> {
    let _task = super::require_task_visible(&state, &task_id, &user).await?;
    let workspace_id = match request.workspace_id {
        Some(workspace_id) => Some(workspace_id),
        None => WorkspaceRepo::get_by_task_id(&*state.db, &task_id)
            .await?
            .map(|workspace| workspace.id),
    };
    let execution = state
        .task_service
        .start_human_review_execution(&task_id, &user.user_id, workspace_id.as_deref())
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
    for execution in page.items.into_iter().filter(|execution| {
        execution.role == services::workflow::default_roles::REVIEWER
            && execution.purpose == Some(db::ExecutionPurpose::Review)
    }) {
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

pub async fn approve_review(
    State(_state): State<AppState>,
    Path(_task_id): Path<String>,
) -> ApiResult<Json<api_types::ReviewDecisionResponse>> {
    Err(ApiError::invalid_operation_conflict(
        "Task-level Review approval is retired; submit a ReviewReport for an exact Human Review Execution",
    ))
}

pub async fn reject_review(
    State(_state): State<AppState>,
    Path(_task_id): Path<String>,
    Json(_request): Json<RejectReviewRequest>,
) -> ApiResult<Json<api_types::ReviewDecisionResponse>> {
    Err(ApiError::invalid_operation_conflict(
        "Task-level Review rejection is retired; submit a ReviewReport for an exact Human Review Execution",
    ))
}
