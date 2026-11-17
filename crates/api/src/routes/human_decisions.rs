//! Structured human decisions raised by the coordinator (plan
//! coordinator-verification-and-pr-loop, phase 1).
//!
//! Answering is a human-only REST operation: the answering user is always the
//! authenticated user, authority follows the chosen option's `authority_effect`
//! (enforced by `HumanDecisionService`), and no MCP tool answers a decision.

use std::sync::Arc;

use api_types::{
    AnswerHumanDecisionRequest, AnswerHumanDecisionResponse, AuthorityEffect,
    HumanDecisionEvidenceLink, HumanDecisionListResponse, HumanDecisionOptionResponse,
    HumanDecisionResponse, HumanDecisionResumeResponse, HumanDecisionResumeStatus,
    HumanDecisionSourceKind, HumanDecisionState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use services::human_decision_service::{AnswerDecisionInput, HumanDecisionService, ResumeOutcome};

use crate::{
    errors::{ApiError, ApiResult},
    routes::{
        auth::AuthenticatedUser,
        tasks,
        validation_checks::{page_request, validate_idempotency_key},
    },
    state::AppState,
};

#[derive(Debug, Clone, Deserialize)]
pub struct HumanDecisionListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub state: Option<HumanDecisionState>,
}

fn service(state: &AppState) -> HumanDecisionService {
    HumanDecisionService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
}

fn db_state(state: HumanDecisionState) -> db::HumanDecisionState {
    match state {
        HumanDecisionState::Open => db::HumanDecisionState::Open,
        HumanDecisionState::Answered => db::HumanDecisionState::Answered,
        HumanDecisionState::Superseded => db::HumanDecisionState::Superseded,
        HumanDecisionState::Cancelled => db::HumanDecisionState::Cancelled,
    }
}

fn api_state(state: db::HumanDecisionState) -> HumanDecisionState {
    match state {
        db::HumanDecisionState::Open => HumanDecisionState::Open,
        db::HumanDecisionState::Answered => HumanDecisionState::Answered,
        db::HumanDecisionState::Superseded => HumanDecisionState::Superseded,
        db::HumanDecisionState::Cancelled => HumanDecisionState::Cancelled,
    }
}

fn api_effect(effect: db::AuthorityEffect) -> AuthorityEffect {
    match effect {
        db::AuthorityEffect::None => AuthorityEffect::None,
        db::AuthorityEffect::Scope => AuthorityEffect::Scope,
        db::AuthorityEffect::Authority => AuthorityEffect::Authority,
    }
}

fn api_source_kind(kind: db::HumanDecisionSourceKind) -> HumanDecisionSourceKind {
    match kind {
        db::HumanDecisionSourceKind::CoordinationRequest => {
            HumanDecisionSourceKind::CoordinationRequest
        }
        db::HumanDecisionSourceKind::QualityRequest => HumanDecisionSourceKind::QualityRequest,
        db::HumanDecisionSourceKind::GateEvaluation => HumanDecisionSourceKind::GateEvaluation,
        db::HumanDecisionSourceKind::ValidationRun => HumanDecisionSourceKind::ValidationRun,
        db::HumanDecisionSourceKind::ExternalFeedback => HumanDecisionSourceKind::ExternalFeedback,
        db::HumanDecisionSourceKind::CoordinatorReplan => {
            HumanDecisionSourceKind::CoordinatorReplan
        }
    }
}

pub(crate) fn decision_response(
    decision: db::HumanDecisionRequest,
) -> ApiResult<HumanDecisionResponse> {
    let options: Vec<HumanDecisionOptionResponse> = serde_json::from_str(&decision.options_json)
        .map_err(|_| ApiError::internal("Stored human decision options are invalid JSON"))?;
    let evidence_links: Vec<HumanDecisionEvidenceLink> =
        serde_json::from_str(&decision.evidence_links_json)
            .map_err(|_| ApiError::internal("Stored human decision evidence is invalid JSON"))?;
    // Only the exact paused request id is public; the binding's bounded
    // subject descriptor stays internal.
    let resumes_request_id =
        serde_json::from_str::<serde_json::Value>(&decision.resume_binding_json)
            .ok()
            .and_then(|binding| {
                binding
                    .get("request_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
    Ok(HumanDecisionResponse {
        id: decision.id,
        task_id: decision.task_id,
        source_kind: api_source_kind(decision.source_kind),
        source_id: decision.source_id,
        raised_by_execution_id: decision.raised_by_execution_id,
        authorization_version: decision.authorization_version,
        question: decision.question,
        options,
        recommended_option_id: decision.recommended_option_id,
        recommendation_rationale: decision.recommendation_rationale,
        evidence_links,
        max_authority_effect: api_effect(decision.max_authority_effect),
        state: api_state(decision.state),
        answered_by_user_id: decision.answered_by_user_id,
        answered_option_id: decision.answered_option_id,
        answer_note: decision.answer_note,
        answered_at: decision.answered_at,
        resumes_request_id,
        resume_request_id: decision.resume_request_id,
        version: decision.version,
        created_at: decision.created_at,
        updated_at: decision.updated_at,
    })
}

pub async fn list_task_decisions(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<HumanDecisionListQuery>,
) -> ApiResult<Json<HumanDecisionListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = service(&state)
        .list_by_task(
            &task_id,
            query.state.map(db_state),
            &page_request(query.cursor, query.limit),
        )
        .await?;
    Ok(Json(HumanDecisionListResponse {
        items: page
            .items
            .into_iter()
            .map(decision_response)
            .collect::<ApiResult<Vec<_>>>()?,
        next_cursor: page.next_cursor,
    }))
}

pub async fn get_task_decision(
    State(state): State<AppState>,
    Path((task_id, decision_id)): Path<(String, String)>,
    user: AuthenticatedUser,
) -> ApiResult<Json<HumanDecisionResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let decision = service(&state)
        .get(&decision_id)
        .await?
        .filter(|decision| decision.task_id == task_id)
        .ok_or_else(|| ApiError::not_found("human decision", decision_id.clone()))?;
    Ok(Json(decision_response(decision)?))
}

pub async fn answer_task_decision(
    State(state): State<AppState>,
    Path((task_id, decision_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<AnswerHumanDecisionRequest>,
) -> ApiResult<Json<AnswerHumanDecisionResponse>> {
    // Visibility first (404 for hidden Tasks). Write/admin authority is
    // decided by the service from the chosen option's authority_effect.
    tasks::require_task_visible(&state, &task_id, &user).await?;
    validate_idempotency_key(&request.idempotency_key)?;
    let outcome = service(&state)
        .answer(AnswerDecisionInput {
            task_id,
            decision_id,
            expected_version: request.expected_version,
            option_id: request.option_id,
            user_id: user.user_id,
            note: request.note,
        })
        .await?;
    let resume = match outcome.resume {
        ResumeOutcome::Created(request) => HumanDecisionResumeResponse {
            status: HumanDecisionResumeStatus::Created,
            request_id: Some(request.id),
            code: None,
        },
        ResumeOutcome::AlreadyBound(request_id) => HumanDecisionResumeResponse {
            status: HumanDecisionResumeStatus::AlreadyBound,
            request_id: Some(request_id),
            code: None,
        },
        ResumeOutcome::Deferred { code, .. } => HumanDecisionResumeResponse {
            status: HumanDecisionResumeStatus::Deferred,
            request_id: None,
            code: Some(code.to_owned()),
        },
    };
    Ok(Json(AnswerHumanDecisionResponse {
        decision: decision_response(outcome.decision)?,
        resume,
    }))
}
