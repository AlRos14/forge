use api_types::{RequestReworkRequest, RequestReworkResponse, TaskReworkAutomationIntentsResponse};
use axum::{
    extract::{Path, State},
    Json,
};
use services::{CollaborationActorSource, RequestReworkInput, TaskReworkService};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, tasks::require_task_visible},
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
                correction_role_id: request.correction_role_id,
                correction_actor: request.correction_actor,
                review_title: request.review_title,
                review_scope: request.review_scope,
                review_actor: request.review_actor,
                idempotency_key: request.idempotency_key,
                expected_source_configuration_json: None,
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

pub async fn list_automatic_rework_intents(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskReworkAutomationIntentsResponse>> {
    require_task_visible(&state, &task_id, &user).await?;
    let service = TaskReworkService::new(
        std::sync::Arc::clone(&state.db),
        std::sync::Arc::clone(&state.event_bus),
    );
    Ok(Json(TaskReworkAutomationIntentsResponse {
        items: service
            .list_automatic_intents(&task_id)
            .await
            .map_err(ApiError::from)?,
    }))
}

pub async fn list_review_lines(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<Vec<api_types::ReviewLineResponse>>> {
    require_task_visible(&state, &task_id, &user).await?;
    let lines = services::task_rework::list_review_lines(&state.db, &task_id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(lines))
}
