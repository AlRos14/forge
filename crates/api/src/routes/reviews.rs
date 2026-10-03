use api_types::{ReviewExecutionResponse, SubmitReviewReportRequest, SubmitReviewReportResponse};
use axum::{
    extract::{Path, State},
    Json,
};
use db::{CollaborationRepo, ExecutionPurpose, ExecutionRepo, ValidationRunRepo};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{
        auth::AuthenticatedUser, collaboration::artifact_response, execution_response,
        tasks::require_task_visible,
    },
    state::AppState,
};

async fn exact_review_execution(state: &AppState, execution_id: &str) -> ApiResult<db::Execution> {
    let execution = ExecutionRepo::get_by_id(&*state.db, execution_id)
        .await?
        .ok_or_else(|| ApiError::not_found("review_execution", execution_id.to_owned()))?;
    if execution.role != services::workflow::default_roles::REVIEWER
        || execution.purpose != Some(ExecutionPurpose::Review)
    {
        return Err(ApiError::not_found(
            "review_execution",
            execution_id.to_owned(),
        ));
    }
    Ok(execution)
}

pub async fn get_review(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<ReviewExecutionResponse>> {
    let execution = exact_review_execution(&state, &id).await?;
    let _task = require_task_visible(&state, &execution.task_id, &user).await?;
    let report = CollaborationRepo::get_execution_artifact_output(
        &*state.db,
        &execution.id,
        db::ArtifactKind::ReviewReport,
    )
    .await?
    .map(|artifact| artifact_response(artifact, true))
    .transpose()?;
    Ok(Json(ReviewExecutionResponse {
        execution: execution_response(execution),
        report,
    }))
}

pub async fn submit_review_report(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(execution_id): Path<String>,
    Json(request): Json<SubmitReviewReportRequest>,
) -> ApiResult<Json<SubmitReviewReportResponse>> {
    let execution = exact_review_execution(&state, &execution_id).await?;
    let _task = require_task_visible(&state, &execution.task_id, &user).await?;
    let (execution, report) = state
        .task_service
        .submit_human_review_report(&execution_id, &user.user_id, request)
        .await?;
    let report = artifact_response(report, true)?;
    Ok(Json(SubmitReviewReportResponse {
        review_execution: ReviewExecutionResponse {
            execution: execution_response(execution),
            report: Some(report.clone()),
        },
        report,
    }))
}

pub async fn list_validation_runs(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Vec<api_types::ValidationRunResponse>>> {
    let _task = require_task_visible(&state, &task_id, &user).await?;
    let runs = ValidationRunRepo::list_validation_runs_by_task(&*state.db, &task_id).await?;
    let mut responses = Vec::with_capacity(runs.len());
    for run in runs {
        responses.push(validation_run_response(&state, run).await?);
    }
    Ok(Json(responses))
}

pub async fn get_validation_run(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::ValidationRunResponse>> {
    let run = ValidationRunRepo::get_validation_run(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("validation_run", id.clone()))?;
    let _task = require_task_visible(&state, &run.task_id, &user).await?;
    Ok(Json(validation_run_response(&state, run).await?))
}

pub async fn get_evidence(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(id): Path<String>,
) -> ApiResult<Json<api_types::EvidenceResponse>> {
    let evidence = ValidationRunRepo::get_evidence(&*state.db, &id)
        .await?
        .ok_or_else(|| ApiError::not_found("evidence", id.clone()))?;
    let _task = require_task_visible(&state, &evidence.task_id, &user).await?;
    let content = serde_json::from_str(&evidence.content_json)
        .map_err(|_| ApiError::internal("Stored Evidence content is invalid JSON"))?;
    Ok(Json(api_types::EvidenceResponse {
        id: evidence.id,
        task_id: evidence.task_id,
        kind: evidence.kind,
        content,
        digest: evidence.digest,
        producer_validation_run_id: evidence.producer_validation_run_id,
        evidence_key: evidence.evidence_key,
        created_at: evidence.created_at,
    }))
}

async fn validation_run_response(
    state: &AppState,
    run: db::ValidationRun,
) -> ApiResult<api_types::ValidationRunResponse> {
    let evidence = ValidationRunRepo::list_evidence_for_validation_run(&*state.db, &run.id).await?;
    let artifact =
        ValidationRunRepo::get_validation_run_artifact_output(&*state.db, &run.id).await?;
    let config_summary = serde_json::from_str(&run.config_summary_json).map_err(|_| {
        ApiError::internal("Stored ValidationRun configuration summary is invalid JSON")
    })?;
    Ok(api_types::ValidationRunResponse {
        id: run.id,
        task_id: run.task_id,
        work_unit_id: run.work_unit_id,
        caused_by_execution_id: run.caused_by_execution_id,
        check_identity: run.check_identity,
        command: run.command,
        config_summary,
        config_digest: run.config_digest,
        workspace_id: run.workspace_id,
        commit_sha: run.commit_sha,
        workspace_snapshot_digest: run.workspace_snapshot_digest,
        status: match run.status {
            db::ValidationRunStatus::Running => api_types::ValidationRunStatus::Running,
            db::ValidationRunStatus::Passed => api_types::ValidationRunStatus::Passed,
            db::ValidationRunStatus::Failed => api_types::ValidationRunStatus::Failed,
            db::ValidationRunStatus::Error => api_types::ValidationRunStatus::Error,
            db::ValidationRunStatus::Cancelled => api_types::ValidationRunStatus::Cancelled,
            db::ValidationRunStatus::Stale => api_types::ValidationRunStatus::Stale,
        },
        exit_code: run.exit_code,
        started_at: run.started_at,
        finished_at: run.finished_at,
        logs_ref: run.logs_ref,
        evidence_ids: evidence.into_iter().map(|item| item.id).collect(),
        validation_report_artifact_id: artifact.map(|artifact| artifact.id),
        created_at: run.created_at,
        updated_at: run.updated_at,
    })
}
