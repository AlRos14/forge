//! Read projections for fallback receipts, bounded micro-fixes and coordinator replans.

use api_types::{
    CoordinatorReplanClassification, CoordinatorReplanListResponse, CoordinatorReplanResponse,
    CoordinatorReplanState, CostClass, MicroFixBoundsOutcome, MicroFixBoundsViolation,
    MicroFixReceiptListResponse, MicroFixReceiptResponse, MicroFixSourceKind, PhaseThreeActorKind,
    WorkUnitFallbackReceiptListResponse, WorkUnitFallbackReceiptResponse,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{
    MicroFixReceiptRepo, PageRequest, SortBy, SortOrder, TaskCoordinatorReplanRepo,
    WorkUnitFallbackReceiptRepo,
};
use serde::Deserialize;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, tasks},
    state::AppState,
};

#[derive(Debug, Clone, Deserialize)]
pub struct PhaseThreeListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

fn page_request(cursor: Option<String>, limit: Option<i64>) -> PageRequest {
    PageRequest {
        cursor,
        limit: limit.unwrap_or(20).clamp(1, 100),
        include_total: false,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(value: &str, field: &str) -> ApiResult<T> {
    serde_json::from_str(value)
        .map_err(|_| ApiError::internal(format!("Stored {field} is invalid JSON")))
}

fn actor_kind(kind: db::ActorKind) -> PhaseThreeActorKind {
    match kind {
        db::ActorKind::Human => PhaseThreeActorKind::Human,
        db::ActorKind::Agent => PhaseThreeActorKind::Agent,
    }
}

fn cost_class(value: db::CostClass) -> CostClass {
    match value {
        db::CostClass::Economy => CostClass::Economy,
        db::CostClass::Standard => CostClass::Standard,
        db::CostClass::Premium => CostClass::Premium,
    }
}

fn fallback_response(receipt: db::WorkUnitFallbackReceipt) -> WorkUnitFallbackReceiptResponse {
    WorkUnitFallbackReceiptResponse {
        id: receipt.id,
        task_id: receipt.task_id,
        work_unit_id: receipt.work_unit_id,
        work_unit_version_before: receipt.work_unit_version_before,
        work_unit_version_after: receipt.work_unit_version_after,
        origin_execution_id: receipt.origin_execution_id,
        failure_class: receipt.failure_class,
        route_attempts_digest: receipt.route_attempts_digest,
        task_role_id: receipt.task_role_id,
        from_actor_kind: actor_kind(receipt.from_actor_kind),
        from_actor_id: receipt.from_actor_id,
        to_actor_kind: actor_kind(receipt.to_actor_kind),
        to_actor_id: receipt.to_actor_id,
        chain_index: receipt.chain_index,
        cost_class: cost_class(receipt.cost_class),
        authorization_version: receipt.authorization_version,
        envelope_digest: receipt.envelope_digest,
        new_execution_id: receipt.new_execution_id,
        created_at: receipt.created_at,
    }
}

fn micro_fix_response(receipt: db::MicroFixReceipt) -> ApiResult<MicroFixReceiptResponse> {
    Ok(MicroFixReceiptResponse {
        id: receipt.id,
        task_id: receipt.task_id,
        request_id: receipt.request_id,
        source_kind: match receipt.source_kind {
            db::MicroFixSourceKind::ReviewFindings => MicroFixSourceKind::ReviewFindings,
            db::MicroFixSourceKind::ValidationFindings => MicroFixSourceKind::ValidationFindings,
            db::MicroFixSourceKind::ExternalFeedback => MicroFixSourceKind::ExternalFeedback,
        },
        finding_refs: parse_json(&receipt.finding_refs_json, "micro-fix finding references")?,
        declared_paths: parse_json(&receipt.declared_paths_json, "micro-fix declared paths")?,
        fix_work_unit_id: receipt.fix_work_unit_id,
        reviewed_commit_sha: receipt.reviewed_commit_sha,
        result_commit_sha: receipt.result_commit_sha,
        changed_files: receipt.changed_files,
        changed_lines: receipt.changed_lines,
        bounds_outcome: match receipt.bounds_outcome {
            db::MicroFixBoundsOutcome::Pending => MicroFixBoundsOutcome::Pending,
            db::MicroFixBoundsOutcome::WithinBounds => MicroFixBoundsOutcome::WithinBounds,
            db::MicroFixBoundsOutcome::BoundsExceeded => MicroFixBoundsOutcome::BoundsExceeded,
        },
        bounds_violation: receipt.bounds_violation.map(|violation| match violation {
            db::MicroFixBoundsViolation::PathsOutsideDeclared => {
                MicroFixBoundsViolation::PathsOutsideDeclared
            }
            db::MicroFixBoundsViolation::ForbiddenGlob => MicroFixBoundsViolation::ForbiddenGlob,
            db::MicroFixBoundsViolation::TooManyFiles => MicroFixBoundsViolation::TooManyFiles,
            db::MicroFixBoundsViolation::TooManyLines => MicroFixBoundsViolation::TooManyLines,
            db::MicroFixBoundsViolation::NewDependency => MicroFixBoundsViolation::NewDependency,
        }),
        reviewer_cost_class: receipt.reviewer_cost_class.map(cost_class),
        reviewer_actor_kind: receipt.reviewer_actor_kind.map(actor_kind),
        reviewer_actor_id: receipt.reviewer_actor_id,
        envelope_digest: receipt.envelope_digest,
        ordinal: receipt.ordinal,
        created_at: receipt.created_at,
        updated_at: receipt.updated_at,
    })
}

fn coordinator_replan_response(
    replan: db::TaskCoordinatorReplan,
) -> ApiResult<CoordinatorReplanResponse> {
    Ok(CoordinatorReplanResponse {
        id: replan.id,
        task_id: replan.task_id,
        ordinal: replan.ordinal,
        authorization_version: replan.authorization_version,
        source_request_id: replan.source_request_id,
        producer_execution_id: replan.producer_execution_id,
        plan_request_id: replan.plan_request_id,
        reason: replan.reason,
        evidence_links: parse_json(&replan.evidence_links_json, "coordinator replan evidence")?,
        classification: replan
            .classification
            .map(|classification| match classification {
                db::CoordinatorReplanClassification::WithinEnvelope => {
                    CoordinatorReplanClassification::WithinEnvelope
                }
                db::CoordinatorReplanClassification::ScopeOrAuthorityChange => {
                    CoordinatorReplanClassification::ScopeOrAuthorityChange
                }
            }),
        classification_facts: replan
            .classification_facts_json
            .map(|facts| parse_json(&facts, "coordinator replan classification facts"))
            .transpose()?,
        human_decision_id: replan.human_decision_id,
        state: match replan.state {
            db::CoordinatorReplanState::Proposed => CoordinatorReplanState::Proposed,
            db::CoordinatorReplanState::Planning => CoordinatorReplanState::Planning,
            db::CoordinatorReplanState::AwaitingHuman => CoordinatorReplanState::AwaitingHuman,
            db::CoordinatorReplanState::Approved => CoordinatorReplanState::Approved,
            db::CoordinatorReplanState::Rejected => CoordinatorReplanState::Rejected,
            db::CoordinatorReplanState::Materialized => CoordinatorReplanState::Materialized,
            db::CoordinatorReplanState::Failed => CoordinatorReplanState::Failed,
        },
        version: replan.version,
        created_at: replan.created_at,
        updated_at: replan.updated_at,
    })
}

pub async fn list_task_fallback_receipts(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PhaseThreeListQuery>,
) -> ApiResult<Json<WorkUnitFallbackReceiptListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = WorkUnitFallbackReceiptRepo::list_work_unit_fallback_receipts_by_task_page(
        &*state.db,
        &task_id,
        &page_request(query.cursor, query.limit),
    )
    .await?;
    Ok(Json(WorkUnitFallbackReceiptListResponse {
        items: page.items.into_iter().map(fallback_response).collect(),
        next_cursor: page.next_cursor,
    }))
}

pub async fn list_task_micro_fixes(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PhaseThreeListQuery>,
) -> ApiResult<Json<MicroFixReceiptListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = MicroFixReceiptRepo::list_micro_fix_receipts_by_task_page(
        &*state.db,
        &task_id,
        &page_request(query.cursor, query.limit),
    )
    .await?;
    Ok(Json(MicroFixReceiptListResponse {
        items: page
            .items
            .into_iter()
            .map(micro_fix_response)
            .collect::<ApiResult<Vec<_>>>()?,
        next_cursor: page.next_cursor,
    }))
}

pub async fn list_task_coordinator_replans(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<PhaseThreeListQuery>,
) -> ApiResult<Json<CoordinatorReplanListResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = TaskCoordinatorReplanRepo::list_coordinator_replans_by_task_page(
        &*state.db,
        &task_id,
        &page_request(query.cursor, query.limit),
    )
    .await?;
    Ok(Json(CoordinatorReplanListResponse {
        items: page
            .items
            .into_iter()
            .map(coordinator_replan_response)
            .collect::<ApiResult<Vec<_>>>()?,
        next_cursor: page.next_cursor,
    }))
}
