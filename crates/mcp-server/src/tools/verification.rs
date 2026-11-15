//! Read-only verification tools: validation catalog, check requests,
//! baseline comparisons and human decisions (plan
//! coordinator-verification-and-pr-loop, phase 1).
//!
//! No tool here answers a decision, publishes a catalog version or enqueues a
//! check: those are human REST operations. Task and Project access is checked
//! by `apply_project_scope` before dispatch; the Project Copilot allowlist does
//! not include these tools.

use std::sync::Arc;

use db::{PageRequest, SortBy, SortOrder, ValidationCheckRequestRepo, ValidationComparisonRepo};
use serde::Deserialize;
use serde_json::{json, Value};
use services::{human_decision_service::HumanDecisionService, ValidationCatalogService};

use crate::{error::McpToolError, params::parse_params, protocol::McpContext, state::AppState};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListChecksParams {
    project_id: String,
    repo_id: Option<String>,
    state: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListParams {
    task_id: String,
    state: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

fn user_id(context: &McpContext) -> Result<&str, McpToolError> {
    context
        .user_id
        .as_deref()
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))
}

fn page(cursor: Option<String>, limit: Option<i64>) -> PageRequest {
    PageRequest {
        cursor,
        limit: limit.unwrap_or(20).clamp(1, 100),
        include_total: false,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    }
}

fn unsupported_state(kind: &str) -> McpToolError {
    McpToolError::new(-32602, format!("{kind} state filter is unsupported"))
}

pub(super) async fn forge_list_validation_checks(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListChecksParams = parse_params(params)?;
    let user_id = user_id(context)?;
    let state_filter = match params.state.as_deref() {
        None => None,
        Some("active") => Some(db::ValidationCheckState::Active),
        Some("retired") => Some(db::ValidationCheckState::Retired),
        Some(_) => return Err(unsupported_state("validation check")),
    };
    let result = ValidationCatalogService::new(Arc::clone(&state.db))
        .list(
            &params.project_id,
            user_id,
            params.repo_id.as_deref(),
            state_filter,
            &page(params.cursor, params.limit),
        )
        .await?;
    let items = result
        .items
        .into_iter()
        .map(|definition| {
            json!({
                "id": definition.id,
                "project_id": definition.project_id,
                "repo_id": definition.repo_id,
                "check_key": definition.check_key,
                "version": definition.version,
                "command": definition.command,
                "timeout_seconds": definition.timeout_seconds,
                "env_policy": definition.env_policy,
                "definition_digest": definition.definition_digest,
                "state": definition.state.to_string(),
                "created_by": definition.created_by,
                "created_at": definition.created_at,
                "retired_at": definition.retired_at,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"items": items, "next_cursor": result.next_cursor}))
}

pub(super) async fn forge_list_task_validation_check_requests(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: TaskListParams = parse_params(params)?;
    let state_filter = match params.state.as_deref() {
        None => None,
        Some("queued") => Some(db::ValidationCheckRequestState::Queued),
        Some("running") => Some(db::ValidationCheckRequestState::Running),
        Some("completed") => Some(db::ValidationCheckRequestState::Completed),
        Some("cancelled") => Some(db::ValidationCheckRequestState::Cancelled),
        Some("failed_to_start") => Some(db::ValidationCheckRequestState::FailedToStart),
        Some(_) => return Err(unsupported_state("validation check request")),
    };
    let result = ValidationCheckRequestRepo::list_validation_check_requests_by_task(
        &*state.db,
        &params.task_id,
        state_filter,
        &page(params.cursor, params.limit),
    )
    .await?;
    let items = result
        .items
        .into_iter()
        .map(|request| {
            json!({
                "id": request.id,
                "task_id": request.task_id,
                "authorization_version": request.authorization_version,
                "check_definition_id": request.check_definition_id,
                "definition_digest": request.definition_digest,
                "trigger_kind": request.trigger_kind.to_string(),
                "subject_kind": request.subject_kind.to_string(),
                "subject_id": request.subject_id,
                "subject_version": request.subject_version,
                "workspace_id": request.workspace_id,
                "commit_sha": request.commit_sha,
                "workspace_snapshot_digest": request.workspace_snapshot_digest,
                "baseline": request.baseline,
                "baseline_commit_sha": request.baseline_commit_sha,
                "requested_by_request_id": request.requested_by_request_id,
                "requested_by_execution_id": request.requested_by_execution_id,
                "reason": request.reason,
                "state": request.state.to_string(),
                "candidate_run_id": request.candidate_run_id,
                "failure_code": request.failure_code,
                "version": request.version,
                "created_at": request.created_at,
                "updated_at": request.updated_at,
                "completed_at": request.completed_at,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"items": items, "next_cursor": result.next_cursor}))
}

pub(super) async fn forge_list_task_validation_comparisons(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: TaskListParams = parse_params(params)?;
    if params.state.is_some() {
        return Err(McpToolError::new(
            -32602,
            "validation comparisons have no state filter",
        ));
    }
    let result = ValidationComparisonRepo::list_validation_comparisons_by_task(
        &*state.db,
        &params.task_id,
        &page(params.cursor, params.limit),
    )
    .await?;
    let mut items = Vec::with_capacity(result.items.len());
    for comparison in result.items {
        let provenance = serde_json::from_str::<Value>(&comparison.baseline_provenance_json)
            .map_err(|error| {
                McpToolError::new(
                    -32603,
                    format!("invalid stored comparison provenance: {error}"),
                )
            })?;
        items.push(json!({
            "id": comparison.id,
            "task_id": comparison.task_id,
            "check_request_id": comparison.check_request_id,
            "candidate_run_id": comparison.candidate_run_id,
            "candidate_status": comparison.candidate_status.to_string(),
            "candidate_output_digest": comparison.candidate_output_digest,
            "baseline_commit_sha": comparison.baseline_commit_sha,
            "baseline_snapshot_digest": comparison.baseline_snapshot_digest,
            "baseline_provenance": provenance,
            "baseline_status": comparison.baseline_status.to_string(),
            "baseline_exit_code": comparison.baseline_exit_code,
            "baseline_output_digest": comparison.baseline_output_digest,
            "classification": comparison.classification.to_string(),
            "same_output_digest": comparison.same_output_digest,
            "created_at": comparison.created_at,
        }));
    }
    Ok(json!({"items": items, "next_cursor": result.next_cursor}))
}

pub(super) async fn forge_list_task_human_decisions(
    state: &AppState,
    params: Value,
) -> Result<Value, McpToolError> {
    let params: TaskListParams = parse_params(params)?;
    let state_filter = match params.state.as_deref() {
        None => None,
        Some("open") => Some(db::HumanDecisionState::Open),
        Some("answered") => Some(db::HumanDecisionState::Answered),
        Some("superseded") => Some(db::HumanDecisionState::Superseded),
        Some("cancelled") => Some(db::HumanDecisionState::Cancelled),
        Some(_) => return Err(unsupported_state("human decision")),
    };
    let result = HumanDecisionService::new(Arc::clone(&state.db), Arc::clone(&state.event_bus))
        .list_by_task(
            &params.task_id,
            state_filter,
            &page(params.cursor, params.limit),
        )
        .await?;
    let mut items = Vec::with_capacity(result.items.len());
    for decision in result.items {
        let options = serde_json::from_str::<Value>(&decision.options_json).map_err(|error| {
            McpToolError::new(-32603, format!("invalid stored decision options: {error}"))
        })?;
        let evidence_links =
            serde_json::from_str::<Value>(&decision.evidence_links_json).map_err(|error| {
                McpToolError::new(-32603, format!("invalid stored decision evidence: {error}"))
            })?;
        let resumes_request_id = serde_json::from_str::<Value>(&decision.resume_binding_json)
            .ok()
            .and_then(|binding| {
                binding
                    .get("request_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        items.push(json!({
            "id": decision.id,
            "task_id": decision.task_id,
            "source_kind": decision.source_kind.to_string(),
            "source_id": decision.source_id,
            "raised_by_execution_id": decision.raised_by_execution_id,
            "authorization_version": decision.authorization_version,
            "question": decision.question,
            "options": options,
            "recommended_option_id": decision.recommended_option_id,
            "recommendation_rationale": decision.recommendation_rationale,
            "evidence_links": evidence_links,
            "max_authority_effect": decision.max_authority_effect.to_string(),
            "state": decision.state.to_string(),
            "answered_by_user_id": decision.answered_by_user_id,
            "answered_option_id": decision.answered_option_id,
            "answer_note": decision.answer_note,
            "answered_at": decision.answered_at,
            "resumes_request_id": resumes_request_id,
            "resume_request_id": decision.resume_request_id,
            "version": decision.version,
            "created_at": decision.created_at,
            "updated_at": decision.updated_at,
        }));
    }
    Ok(json!({"items": items, "next_cursor": result.next_cursor}))
}
