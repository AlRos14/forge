use api_types::{RequestReworkRequest, RequestReworkResponse};
use axum::{
    extract::{Path, State},
    Json,
};
use services::{CollaborationActorSource, RequestReworkInput, TaskReworkService};

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

pub async fn request_rework(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<RequestReworkRequest>,
) -> ApiResult<Json<RequestReworkResponse>> {
    crate::routes::tasks::require_task_mutable(&state, &task_id, &user).await?;
    let service = TaskReworkService::new(
        std::sync::Arc::clone(&state.db),
        std::sync::Arc::clone(&state.event_bus),
    );
    let result = service
        .request_rework(
            CollaborationActorSource::Human(user.user_id.clone()),
            RequestReworkInput {
                task_id,
                expected_task_version: request.expected_task_version,
                expected_lifecycle_version: request.expected_lifecycle_version,
                review_report_artifact_id: request.review_report_artifact_id,
                reviewed_work_unit_id: request.reviewed_work_unit_id,
                reviewed_work_unit_version: request.reviewed_work_unit_version,
                correction_title: request.correction_title,
                correction_scope: request.correction_scope,
                correction_role: request.correction_role,
                correction_actor: request.correction_actor,
                review_title: request.review_title,
                review_scope: request.review_scope,
                review_actor: request.review_actor,
                idempotency_key: request.idempotency_key,
            },
        )
        .await
        .map_err(ApiError::from)?;
    let correction =
        crate::routes::work_units::response_for(&state, &user, result.correction_work_unit).await?;
    let review =
        crate::routes::work_units::response_for(&state, &user, result.review_work_unit).await?;
    Ok(Json(RequestReworkResponse {
        correction_work_unit: correction,
        review_work_unit: review,
        review_subject: result
            .review_subject
            .as_ref()
            .map(services::task_rework::review_subject_response),
        rounds_remaining: result.rounds_remaining,
        replayed: result.replayed,
    }))
}
