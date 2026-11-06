use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};
use api_types::{ReviewExecutionResponse, SubmitReviewReportResponse, SubmitWorkUnitReviewRequest};
use axum::{
    extract::{Path, State},
    Json,
};

pub async fn submit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<SubmitWorkUnitReviewRequest>,
) -> ApiResult<Json<SubmitReviewReportResponse>> {
    let (execution, report) = state
        .work_unit_service
        .submit_human_review(
            &state.task_service,
            &id,
            &user.user_id,
            request.expected_version,
            request.report,
        )
        .await
        .map_err(ApiError::from)?;
    let report = crate::routes::collaboration::artifact_response(report, true)?;
    Ok(Json(SubmitReviewReportResponse {
        review_execution: ReviewExecutionResponse {
            execution: crate::routes::execution_response(execution),
            report: Some(report.clone()),
        },
        report,
    }))
}
