//! Read projections and human decisions for the phase-2 PR feedback loop.
//! Task bodies are available only after the normal Task visibility check;
//! public events never include provider-authored bodies.

use std::sync::Arc;

use api_types::{
    DecidePrExternalActionRequest, ExternalFeedbackItemListResponse, ExternalFeedbackItemResponse,
    PrExternalActionListResponse, PrExternalActionResponse, PrFeedbackAuthorization,
    PrFeedbackPolicy, PrFeedbackPolicyResponse, PrFeedbackSummaryMode, PrReworkListResponse,
    PrReworkSupersessionResponse, UpdatePrFeedbackPolicyRequest,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{PrExternalActionRepo, PrExternalActionState, PrProviderConfigRepo, RepoRepo};
use serde::{Deserialize, Serialize};
use services::{
    pr_external_action::PrExternalActionService, pr_provider_settings::PrProviderSettingsService,
    pr_service::PrService, ServiceError,
};
use sqlx::{sqlite::SqliteRow, Row};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{
        auth::AuthenticatedUser, projects, tasks, validation_checks::validate_idempotency_key,
    },
    state::AppState,
};

#[derive(Debug, Clone, Deserialize)]
pub struct PrListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PrActionListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrKeysetCursor {
    created_at: String,
    id: String,
}

fn decode_keyset_cursor(cursor: Option<String>) -> ApiResult<Option<PrKeysetCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let bytes = hex::decode(cursor).map_err(|_| {
        ApiError::bad_request_with_code("invalid_cursor", "invalid pagination cursor")
    })?;
    let value: PrKeysetCursor = serde_json::from_slice(&bytes).map_err(|_| {
        ApiError::bad_request_with_code("invalid_cursor", "invalid pagination cursor")
    })?;
    if value.created_at.trim().is_empty() || value.id.trim().is_empty() {
        return Err(ApiError::bad_request_with_code(
            "invalid_cursor",
            "invalid pagination cursor",
        ));
    }
    Ok(Some(value))
}

fn encode_keyset_cursor(created_at: String, id: String) -> ApiResult<String> {
    let bytes = serde_json::to_vec(&PrKeysetCursor { created_at, id })
        .map_err(|_| ApiError::internal("could not encode pagination cursor"))?;
    Ok(hex::encode(bytes))
}

fn page_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(20).clamp(1, 100)
}

fn action_state(value: Option<&str>) -> ApiResult<Option<PrExternalActionState>> {
    match value {
        None => Ok(None),
        Some("proposed") => Ok(Some(PrExternalActionState::Proposed)),
        Some("approved") => Ok(Some(PrExternalActionState::Approved)),
        Some("running") => Ok(Some(PrExternalActionState::Running)),
        Some("succeeded") => Ok(Some(PrExternalActionState::Succeeded)),
        Some("failed") => Ok(Some(PrExternalActionState::Failed)),
        Some("outcome_unknown") => Ok(Some(PrExternalActionState::OutcomeUnknown)),
        Some("reconciliation_required") => Ok(Some(PrExternalActionState::ReconciliationRequired)),
        Some("rejected") => Ok(Some(PrExternalActionState::Rejected)),
        Some("abandoned") => Ok(Some(PrExternalActionState::Abandoned)),
        Some(_) => Err(ApiError::bad_request_with_code(
            "pr_action.state_invalid",
            "unsupported PR external action state",
        )),
    }
}

fn feedback_item_response(
    item: services::pr_service::TaskPrFeedbackItem,
) -> ExternalFeedbackItemResponse {
    ExternalFeedbackItemResponse {
        id: item.id,
        task_id: item.task_id,
        admission_id: item.admission_id,
        provider_item_id: item.provider_item_id,
        kind: item.kind,
        author_login: item.author_login,
        author_association: item.author_association,
        review_state: item.review_state,
        path: item.path,
        line: item.line,
        commit_sha: item.commit_sha,
        body: item.body,
        body_truncated: item.body_truncated,
        body_digest: item.body_digest,
        provider_url: item.provider_url,
        trust_class: item.trust_class,
        head_state: item.head_state,
        disposition: item.disposition,
        dismiss_reason: item.dismiss_reason,
        scope_classification: item.scope_classification,
        human_decision_id: item.human_decision_id,
        followup_task_id: item.followup_task_id,
        ingested_at: item.ingested_at,
    }
}

fn action_response(row: SqliteRow) -> Result<PrExternalActionResponse, sqlx::Error> {
    Ok(PrExternalActionResponse {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        admission_id: row.try_get("admission_id")?,
        kind: row.try_get("kind")?,
        state: row.try_get("state")?,
        authorization_mode: row.try_get("authorization_mode")?,
        gate_evaluation_id: row.try_get("gate_evaluation_id")?,
        subject_commit_sha: row.try_get("subject_commit_sha")?,
        expected_remote_head_sha: row.try_get("expected_remote_head_sha")?,
        summary_artifact_id: row.try_get("summary_artifact_id")?,
        body_digest: row.try_get("body_digest")?,
        proposal_id: row.try_get("proposal_id")?,
        decision_id: row.try_get("decision_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        finished_at: row.try_get("finished_at")?,
    })
}

fn supersession_response(row: SqliteRow) -> Result<PrReworkSupersessionResponse, sqlx::Error> {
    Ok(PrReworkSupersessionResponse {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        task_merge_operation_id: row.try_get("task_merge_operation_id")?,
        publish_operation_id: row.try_get("publish_operation_id")?,
        metadata_id: row.try_get("metadata_id")?,
        request_id: row.try_get("request_id")?,
        triage_set_digest: row.try_get("triage_set_digest")?,
        provider_pr_id: row.try_get("provider_pr_id")?,
        superseded_head_sha: row.try_get("superseded_head_sha")?,
        state: row.try_get("state")?,
        readmission_merge_operation_id: row.try_get("readmission_merge_operation_id")?,
        convergence_decision_id: row.try_get("convergence_decision_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub async fn list_task_feedback(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PrListQuery>,
) -> ApiResult<Json<ExternalFeedbackItemListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = PrService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .list_task_external_feedback(&task_id, query.cursor, query.limit.unwrap_or(20))
        .await?;
    Ok(Json(ExternalFeedbackItemListResponse {
        items: page.items.into_iter().map(feedback_item_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

pub async fn list_task_rework(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PrListQuery>,
) -> ApiResult<Json<PrReworkListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let cursor = decode_keyset_cursor(query.cursor)?;
    let limit = page_limit(query.limit);
    let mut sql = "SELECT * FROM pr_rework_supersession WHERE task_id = ?".to_owned();
    if cursor.is_some() {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    let mut request = sqlx::query(&sql).bind(&task_id);
    if let Some(cursor) = &cursor {
        request = request
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.id);
    }
    let mut rows = request.bind(limit + 1).fetch_all(state.db.pool()).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        let last = rows
            .last()
            .ok_or_else(|| ApiError::internal("PR rework page cursor has no final row"))?;
        Some(encode_keyset_cursor(
            last.try_get("created_at")?,
            last.try_get("id")?,
        )?)
    } else {
        None
    };
    Ok(Json(PrReworkListResponse {
        items: rows
            .into_iter()
            .map(supersession_response)
            .collect::<Result<Vec<_>, _>>()?,
        next_cursor,
    }))
}

pub async fn list_task_actions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PrActionListQuery>,
) -> ApiResult<Json<PrExternalActionListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let state_filter = action_state(query.state.as_deref())?.map(|state| state.to_string());
    let cursor = decode_keyset_cursor(query.cursor)?;
    let limit = page_limit(query.limit);
    let mut sql = "SELECT * FROM pr_external_action WHERE task_id = ?".to_owned();
    if state_filter.is_some() {
        sql.push_str(" AND state = ?");
    }
    if cursor.is_some() {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    let mut request = sqlx::query(&sql).bind(&task_id);
    if let Some(state_filter) = &state_filter {
        request = request.bind(state_filter);
    }
    if let Some(cursor) = &cursor {
        request = request
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.id);
    }
    let mut rows = request.bind(limit + 1).fetch_all(state.db.pool()).await?;
    let has_more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_cursor = if has_more {
        let last = rows
            .last()
            .ok_or_else(|| ApiError::internal("PR action page cursor has no final row"))?;
        Some(encode_keyset_cursor(
            last.try_get("created_at")?,
            last.try_get("id")?,
        )?)
    } else {
        None
    };
    Ok(Json(PrExternalActionListResponse {
        items: rows
            .into_iter()
            .map(action_response)
            .collect::<Result<Vec<_>, _>>()?,
        next_cursor,
    }))
}

pub async fn approve_task_action(
    State(state): State<AppState>,
    Path((task_id, action_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<DecidePrExternalActionRequest>,
) -> ApiResult<Json<PrExternalActionResponse>> {
    decide_task_action(&state, task_id, action_id, user, request, true).await
}

pub async fn reject_task_action(
    State(state): State<AppState>,
    Path((task_id, action_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<DecidePrExternalActionRequest>,
) -> ApiResult<Json<PrExternalActionResponse>> {
    decide_task_action(&state, task_id, action_id, user, request, false).await
}

async fn decide_task_action(
    state: &AppState,
    task_id: String,
    action_id: String,
    user: AuthenticatedUser,
    request: DecidePrExternalActionRequest,
    approve: bool,
) -> ApiResult<Json<PrExternalActionResponse>> {
    tasks::require_task_visible(state, &task_id, &user).await?;
    validate_idempotency_key(&request.idempotency_key)?;
    let action = PrExternalActionRepo::get_pr_external_action(&*state.db, &action_id)
        .await?
        .filter(|action| action.task_id == task_id)
        .ok_or_else(|| ApiError::not_found("pr external action", action_id.clone()))?;
    if action.version != request.expected_version {
        return Err(ApiError::conflict_with_code(
            "pr_action.version_conflict",
            "PR external action version changed before the decision",
        ));
    }
    let pr = PrService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus));
    let service =
        PrExternalActionService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus), pr);
    let decided = if approve {
        service
            .approve(
                &action_id,
                &user.user_id,
                request.expected_version,
                request.note,
            )
            .await
    } else {
        service
            .reject(
                &action_id,
                &user.user_id,
                request.expected_version,
                request.note,
            )
            .await
    }
    .map_err(map_action_error)?;
    Ok(Json(action_response(decided)))
}

pub async fn get_repo_feedback_policy(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<PrFeedbackPolicyResponse>> {
    require_repo_admin(&state, &repo_id, &user).await?;
    let config = PrProviderConfigRepo::get_by_repo_id(&*state.db, &repo_id)
        .await?
        .ok_or_else(|| ApiError::not_found("pr_provider_config", repo_id.clone()))?;
    let policy = db::PrFeedbackPolicy::from_json(&config.feedback_policy_json)
        .map_err(|_| ApiError::internal("stored PR feedback policy is invalid"))?;
    Ok(Json(PrFeedbackPolicyResponse {
        policy: api_policy(policy),
        updated_at: config.updated_at,
    }))
}

pub async fn set_repo_feedback_policy(
    State(state): State<AppState>,
    Path(repo_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<UpdatePrFeedbackPolicyRequest>,
) -> ApiResult<Json<PrFeedbackPolicyResponse>> {
    require_repo_admin(&state, &repo_id, &user).await?;
    let policy = db_policy(request.policy);
    let updated = PrProviderSettingsService::new(
        Arc::clone(&state.db),
        Arc::clone(&state.credential_service),
    )
    .set_feedback_policy(
        &user.user_id,
        &repo_id,
        request.expected_updated_at.as_deref(),
        policy,
    )
    .await
    .map_err(map_policy_error)?;
    let policy = db::PrFeedbackPolicy::from_json(&updated.feedback_policy_json)
        .map_err(|_| ApiError::internal("stored PR feedback policy is invalid"))?;
    Ok(Json(PrFeedbackPolicyResponse {
        policy: api_policy(policy),
        updated_at: updated.updated_at,
    }))
}

async fn require_repo_admin(
    state: &AppState,
    repo_id: &str,
    user: &AuthenticatedUser,
) -> ApiResult<db::Repo> {
    let repo = RepoRepo::get_by_id(&*state.db, repo_id)
        .await?
        .ok_or_else(|| ApiError::not_found("repo", repo_id.to_owned()))?;
    projects::require_project_admin(state, &repo.project_id, &user.user_id).await?;
    Ok(repo)
}

fn db_policy(policy: PrFeedbackPolicy) -> db::PrFeedbackPolicy {
    db::PrFeedbackPolicy {
        schema_version: policy.schema_version,
        ingest: policy.ingest,
        trusted_author_associations: policy.trusted_author_associations,
        trusted_logins: policy.trusted_logins,
        max_items_per_poll: policy.max_items_per_poll,
        push_corrections: match policy.push_corrections {
            PrFeedbackAuthorization::HumanApproval => db::PrFeedbackAuthorization::HumanApproval,
            PrFeedbackAuthorization::Automatic => db::PrFeedbackAuthorization::Automatic,
        },
        post_summary: match policy.post_summary {
            PrFeedbackSummaryMode::HumanApproval => db::PrFeedbackSummaryMode::HumanApproval,
            PrFeedbackSummaryMode::Automatic => db::PrFeedbackSummaryMode::Automatic,
            PrFeedbackSummaryMode::Off => db::PrFeedbackSummaryMode::Off,
        },
    }
}

fn api_policy(policy: db::PrFeedbackPolicy) -> PrFeedbackPolicy {
    PrFeedbackPolicy {
        schema_version: policy.schema_version,
        ingest: policy.ingest,
        trusted_author_associations: policy.trusted_author_associations,
        trusted_logins: policy.trusted_logins,
        max_items_per_poll: policy.max_items_per_poll,
        push_corrections: match policy.push_corrections {
            db::PrFeedbackAuthorization::HumanApproval => PrFeedbackAuthorization::HumanApproval,
            db::PrFeedbackAuthorization::Automatic => PrFeedbackAuthorization::Automatic,
        },
        post_summary: match policy.post_summary {
            db::PrFeedbackSummaryMode::HumanApproval => PrFeedbackSummaryMode::HumanApproval,
            db::PrFeedbackSummaryMode::Automatic => PrFeedbackSummaryMode::Automatic,
            db::PrFeedbackSummaryMode::Off => PrFeedbackSummaryMode::Off,
        },
    }
}

fn map_policy_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::AuthorizationDenied { message } => {
            ApiError::forbidden_with_code("pr_feedback.policy_forbidden", message)
        }
        ServiceError::InvalidOperation { message } => {
            ApiError::bad_request_with_code("pr_feedback.policy_invalid", message)
        }
        ServiceError::Db(db::DbError::VersionConflict) => ApiError::conflict_with_code(
            "pr_feedback.policy_version_conflict",
            "PR feedback policy changed before the update committed",
        ),
        other => other.into(),
    }
}

fn map_action_error(error: ServiceError) -> ApiError {
    match error {
        ServiceError::AuthorizationDenied { message } => {
            ApiError::forbidden_with_code("pr_action.not_authorized", message)
        }
        ServiceError::Db(db::DbError::VersionConflict) => ApiError::conflict_with_code(
            "pr_action.version_conflict",
            "PR external action changed before the decision committed",
        ),
        ServiceError::Conflict(message) => {
            ApiError::conflict_with_code("pr_action.decision_conflict", message)
        }
        other => other.into(),
    }
}
