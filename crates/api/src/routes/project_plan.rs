use api_types::{
    ApplyProjectPlanRequest, AuthorizeProjectPlanBatchRequest, ProjectPlanBatchAction,
    ProjectPlanBatchResponse, ProjectPlanGraphResponse, ProjectPlanProposalResponse,
    ProposeProjectPlanRequest, UpdateProjectPlanBatchRequest,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{de::DeserializeOwned, Serialize};
use services::project_plan::ProjectPlanService;

use crate::{errors::ApiResult, routes::auth::AuthenticatedUser, state::AppState};

fn service(state: &AppState) -> ProjectPlanService {
    ProjectPlanService::new(state.db.clone(), state.event_bus.clone())
}

fn wire<T: DeserializeOwned>(value: impl Serialize) -> ApiResult<T> {
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

pub async fn graph(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<ProjectPlanGraphResponse>> {
    super::projects::require_project_visible(&state, &project_id, &user.user_id).await?;
    Ok(Json(wire(service(&state).graph(&project_id).await?)?))
}

pub async fn propose(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<ProposeProjectPlanRequest>,
) -> ApiResult<(StatusCode, Json<ProjectPlanProposalResponse>)> {
    super::projects::require_project_mutable(&state, &project_id, &user.user_id).await?;
    let document = serde_json::from_value(serde_json::to_value(body.document)?)?;
    let proposal = service(&state)
        .propose(
            &project_id,
            &user.user_id,
            body.expected_version,
            document,
            &body.idempotency_key,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(wire(proposal)?)))
}

pub async fn apply(
    State(state): State<AppState>,
    Path((project_id, proposal_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(body): Json<ApplyProjectPlanRequest>,
) -> ApiResult<Json<ProjectPlanGraphResponse>> {
    super::projects::require_project_mutable(&state, &project_id, &user.user_id).await?;
    Ok(Json(wire(
        service(&state)
            .apply(
                &project_id,
                &user.user_id,
                &proposal_id,
                body.expected_version,
                &body.idempotency_key,
            )
            .await?,
    )?))
}

pub async fn authorize_batch(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Json(body): Json<AuthorizeProjectPlanBatchRequest>,
) -> ApiResult<(StatusCode, Json<ProjectPlanBatchResponse>)> {
    super::projects::require_project_admin(&state, &project_id, &user.user_id).await?;
    let batch = service(&state)
        .authorize_batch(
            &project_id,
            &user.user_id,
            body.expected_version,
            body.task_ids,
            body.max_parallel_tasks,
            &body.idempotency_key,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(wire(batch)?)))
}

pub async fn update_batch(
    State(state): State<AppState>,
    Path((project_id, batch_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(body): Json<UpdateProjectPlanBatchRequest>,
) -> ApiResult<Json<ProjectPlanBatchResponse>> {
    super::projects::require_project_admin(&state, &project_id, &user.user_id).await?;
    let action = match body.action {
        ProjectPlanBatchAction::Pause => "pause",
        ProjectPlanBatchAction::Resume => "resume",
        ProjectPlanBatchAction::Revoke => "revoke",
    };
    Ok(Json(wire(
        service(&state)
            .update_batch(
                &project_id,
                &user.user_id,
                &batch_id,
                body.expected_version,
                action,
                &body.idempotency_key,
            )
            .await?,
    )?))
}
