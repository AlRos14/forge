//! Validation check catalog, check requests and baseline comparisons
//! (plan coordinator-verification-and-pr-loop, phase 1).
//!
//! The catalog is written only by a Project owner/admin. Check requests and
//! comparisons are produced by the runner and read-only here: no REST route
//! enqueues a check or classifies a comparison.

use std::sync::Arc;

use api_types::{
    CreateValidationCheckRequest, RetireValidationCheckRequest, ValidationCheckDefinitionResponse,
    ValidationCheckListResponse, ValidationCheckRequestListResponse,
    ValidationCheckRequestResponse, ValidationCheckRequestState, ValidationCheckState,
    ValidationComparisonListResponse, ValidationComparisonResponse,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{
    PageRequest, SortBy, SortOrder, ValidationCatalogRepo, ValidationCheckRequestRepo,
    ValidationComparisonRepo,
};
use serde::Deserialize;
use services::{PublishCheckInput, ValidationCatalogService};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, projects, tasks},
    state::AppState,
};

const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub repo_id: Option<String>,
    pub state: Option<ValidationCheckState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CheckRequestListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub state: Option<ValidationCheckRequestState>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComparisonListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

pub(crate) fn page_request(cursor: Option<String>, limit: Option<i64>) -> PageRequest {
    PageRequest {
        cursor,
        limit: limit.unwrap_or(20).clamp(1, 100),
        include_total: false,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    }
}

pub(crate) fn validate_idempotency_key(key: &str) -> ApiResult<()> {
    if key.trim().is_empty() || key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(ApiError::bad_request(
            "idempotency_key must contain between 1 and 128 bytes",
        ));
    }
    Ok(())
}

pub(crate) fn api_run_origin(origin: db::ValidationRunOrigin) -> api_types::ValidationRunOrigin {
    match origin {
        db::ValidationRunOrigin::LegacyCommand => api_types::ValidationRunOrigin::LegacyCommand,
        db::ValidationRunOrigin::CoordinatorRequest => {
            api_types::ValidationRunOrigin::CoordinatorRequest
        }
        db::ValidationRunOrigin::AfterIntegration => {
            api_types::ValidationRunOrigin::AfterIntegration
        }
        db::ValidationRunOrigin::Baseline => api_types::ValidationRunOrigin::Baseline,
    }
}

fn api_check_state(state: db::ValidationCheckState) -> ValidationCheckState {
    match state {
        db::ValidationCheckState::Active => ValidationCheckState::Active,
        db::ValidationCheckState::Retired => ValidationCheckState::Retired,
    }
}

fn db_check_state(state: ValidationCheckState) -> db::ValidationCheckState {
    match state {
        ValidationCheckState::Active => db::ValidationCheckState::Active,
        ValidationCheckState::Retired => db::ValidationCheckState::Retired,
    }
}

fn db_request_state(state: ValidationCheckRequestState) -> db::ValidationCheckRequestState {
    match state {
        ValidationCheckRequestState::Queued => db::ValidationCheckRequestState::Queued,
        ValidationCheckRequestState::Running => db::ValidationCheckRequestState::Running,
        ValidationCheckRequestState::Completed => db::ValidationCheckRequestState::Completed,
        ValidationCheckRequestState::Cancelled => db::ValidationCheckRequestState::Cancelled,
        ValidationCheckRequestState::FailedToStart => {
            db::ValidationCheckRequestState::FailedToStart
        }
    }
}

fn api_request_state(state: db::ValidationCheckRequestState) -> ValidationCheckRequestState {
    match state {
        db::ValidationCheckRequestState::Queued => ValidationCheckRequestState::Queued,
        db::ValidationCheckRequestState::Running => ValidationCheckRequestState::Running,
        db::ValidationCheckRequestState::Completed => ValidationCheckRequestState::Completed,
        db::ValidationCheckRequestState::Cancelled => ValidationCheckRequestState::Cancelled,
        db::ValidationCheckRequestState::FailedToStart => {
            ValidationCheckRequestState::FailedToStart
        }
    }
}

fn api_run_status(status: db::ValidationRunStatus) -> api_types::ValidationRunStatus {
    match status {
        db::ValidationRunStatus::Running => api_types::ValidationRunStatus::Running,
        db::ValidationRunStatus::Passed => api_types::ValidationRunStatus::Passed,
        db::ValidationRunStatus::Failed => api_types::ValidationRunStatus::Failed,
        db::ValidationRunStatus::Error => api_types::ValidationRunStatus::Error,
        db::ValidationRunStatus::Cancelled => api_types::ValidationRunStatus::Cancelled,
        db::ValidationRunStatus::Stale => api_types::ValidationRunStatus::Stale,
    }
}

pub(crate) fn definition_response(
    definition: db::ValidationCheckDefinition,
) -> ValidationCheckDefinitionResponse {
    ValidationCheckDefinitionResponse {
        id: definition.id,
        project_id: definition.project_id,
        repo_id: definition.repo_id,
        check_key: definition.check_key,
        version: definition.version,
        command: definition.command,
        timeout_seconds: definition.timeout_seconds,
        env_policy: definition.env_policy,
        definition_digest: definition.definition_digest,
        state: api_check_state(definition.state),
        created_by: definition.created_by,
        created_at: definition.created_at,
        retired_at: definition.retired_at,
    }
}

pub(crate) fn check_request_response(
    request: db::ValidationCheckRequest,
) -> ValidationCheckRequestResponse {
    ValidationCheckRequestResponse {
        id: request.id,
        task_id: request.task_id,
        authorization_version: request.authorization_version,
        check_definition_id: request.check_definition_id,
        definition_digest: request.definition_digest,
        trigger_kind: match request.trigger_kind {
            db::ValidationCheckRequestTrigger::CoordinatorRequest => {
                api_types::ValidationCheckRequestTrigger::CoordinatorRequest
            }
            db::ValidationCheckRequestTrigger::AfterIntegration => {
                api_types::ValidationCheckRequestTrigger::AfterIntegration
            }
        },
        subject_kind: match request.subject_kind {
            db::ValidationCheckSubjectKind::WorkUnitResult => {
                api_types::ValidationCheckSubjectKind::WorkUnitResult
            }
            db::ValidationCheckSubjectKind::TaskIntegration => {
                api_types::ValidationCheckSubjectKind::TaskIntegration
            }
        },
        subject_id: request.subject_id,
        subject_version: request.subject_version,
        workspace_id: request.workspace_id,
        commit_sha: request.commit_sha,
        workspace_snapshot_digest: request.workspace_snapshot_digest,
        baseline: request.baseline,
        baseline_commit_sha: request.baseline_commit_sha,
        requested_by_request_id: request.requested_by_request_id,
        requested_by_execution_id: request.requested_by_execution_id,
        reason: request.reason,
        state: api_request_state(request.state),
        candidate_run_id: request.candidate_run_id,
        failure_code: request.failure_code,
        version: request.version,
        created_at: request.created_at,
        updated_at: request.updated_at,
        completed_at: request.completed_at,
    }
}

pub(crate) fn comparison_response(
    comparison: db::ValidationComparison,
) -> ApiResult<ValidationComparisonResponse> {
    let baseline_provenance = serde_json::from_str(&comparison.baseline_provenance_json)
        .map_err(|_| ApiError::internal("Stored comparison provenance is invalid JSON"))?;
    Ok(ValidationComparisonResponse {
        id: comparison.id,
        task_id: comparison.task_id,
        check_request_id: comparison.check_request_id,
        candidate_run_id: comparison.candidate_run_id,
        candidate_status: api_run_status(comparison.candidate_status),
        candidate_output_digest: comparison.candidate_output_digest,
        baseline_commit_sha: comparison.baseline_commit_sha,
        baseline_snapshot_digest: comparison.baseline_snapshot_digest,
        baseline_provenance,
        baseline_status: api_run_status(comparison.baseline_status),
        baseline_exit_code: comparison.baseline_exit_code,
        baseline_output_digest: comparison.baseline_output_digest,
        classification: match comparison.classification {
            db::ValidationComparisonClass::Regression => {
                api_types::ValidationComparisonClass::Regression
            }
            db::ValidationComparisonClass::PreExistingFailure => {
                api_types::ValidationComparisonClass::PreExistingFailure
            }
            db::ValidationComparisonClass::Fixed => api_types::ValidationComparisonClass::Fixed,
            db::ValidationComparisonClass::BothPass => {
                api_types::ValidationComparisonClass::BothPass
            }
            db::ValidationComparisonClass::Inconclusive => {
                api_types::ValidationComparisonClass::Inconclusive
            }
        },
        same_output_digest: comparison.same_output_digest,
        created_at: comparison.created_at,
    })
}

async fn require_catalog_admin(
    state: &AppState,
    project_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<()> {
    let project = projects::require_project_visible(state, project_id, &user.user_id).await?;
    if !services::project_authority::user_can_administer_project(&state.db, &project, &user.user_id)
        .await?
    {
        return Err(ApiError::forbidden_with_code(
            "validation.catalog_forbidden",
            "only a Project owner or admin may change the validation catalog",
        ));
    }
    Ok(())
}

pub async fn list_project_checks(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CatalogListQuery>,
) -> ApiResult<Json<ValidationCheckListResponse>> {
    let page = ValidationCatalogService::new(Arc::clone(&state.db))
        .list(
            &project_id,
            &user.user_id,
            query.repo_id.as_deref(),
            query.state.map(db_check_state),
            &page_request(query.cursor, query.limit),
        )
        .await?;
    Ok(Json(ValidationCheckListResponse {
        items: page.items.into_iter().map(definition_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

pub async fn create_project_check(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<CreateValidationCheckRequest>,
) -> ApiResult<Json<ValidationCheckDefinitionResponse>> {
    require_catalog_admin(&state, &project_id, &user).await?;
    validate_idempotency_key(&request.idempotency_key)?;
    // A retried publish with identical content returns the active version
    // instead of minting another one.
    if let Some(active) = ValidationCatalogRepo::get_active_validation_check_definition(
        &*state.db,
        &project_id,
        request.repo_id.as_deref(),
        &request.check_key,
    )
    .await?
    {
        if active.command == request.command
            && active.timeout_seconds == request.timeout_seconds
            && active.env_policy == "captured_default"
        {
            return Ok(Json(definition_response(active)));
        }
    }
    let definition = ValidationCatalogService::new(Arc::clone(&state.db))
        .create_version(
            &project_id,
            &user.user_id,
            PublishCheckInput {
                repo_id: request.repo_id,
                check_key: request.check_key,
                command: request.command,
                timeout_seconds: request.timeout_seconds,
                env_policy: None,
            },
        )
        .await?;
    Ok(Json(definition_response(definition)))
}

pub async fn retire_check(
    State(state): State<AppState>,
    Path(definition_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<RetireValidationCheckRequest>,
) -> ApiResult<Json<ValidationCheckDefinitionResponse>> {
    let definition =
        ValidationCatalogRepo::get_validation_check_definition(&*state.db, &definition_id)
            .await?
            .ok_or_else(|| ApiError::not_found("validation check", definition_id.clone()))?;
    // `require_catalog_admin` answers 404 for hidden Projects before 403.
    require_catalog_admin(&state, &definition.project_id, &user).await?;
    if request.expected_state != ValidationCheckState::Active {
        return Err(ApiError::unprocessable(
            "validation.catalog_invalid_expected_state",
            "only an active validation check can be retired",
        ));
    }
    if definition.state != db::ValidationCheckState::Active {
        return Err(ApiError::conflict_with_code(
            "validation.catalog_state_conflict",
            "the validation check is already retired",
        ));
    }
    let retired = ValidationCatalogService::new(Arc::clone(&state.db))
        .retire(&definition.project_id, &user.user_id, &definition.id)
        .await?;
    Ok(Json(definition_response(retired)))
}

pub async fn list_task_check_requests(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<CheckRequestListQuery>,
) -> ApiResult<Json<ValidationCheckRequestListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = ValidationCheckRequestRepo::list_validation_check_requests_by_task(
        &*state.db,
        &task_id,
        query.state.map(db_request_state),
        &page_request(query.cursor, query.limit),
    )
    .await?;
    Ok(Json(ValidationCheckRequestListResponse {
        items: page.items.into_iter().map(check_request_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

pub async fn list_task_comparisons(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<ComparisonListQuery>,
) -> ApiResult<Json<ValidationComparisonListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = ValidationComparisonRepo::list_validation_comparisons_by_task(
        &*state.db,
        &task_id,
        &page_request(query.cursor, query.limit),
    )
    .await?;
    Ok(Json(ValidationComparisonListResponse {
        items: page
            .items
            .into_iter()
            .map(comparison_response)
            .collect::<ApiResult<Vec<_>>>()?,
        next_cursor: page.next_cursor,
    }))
}
