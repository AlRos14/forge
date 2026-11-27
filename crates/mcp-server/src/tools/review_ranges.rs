use std::path::PathBuf;

use api_types::{
    ActorRef as ApiActorRef, ReviewCoverageRangeResponse, ReviewCoverageReportRef,
    ReviewPolicyDocument, ReviewReportVerdict, ReviewSeverityCounts, TaskReviewCoverageResponse,
    TaskReviewPolicyResponse,
};
use db::{
    PageRequest, ReviewCheckpoint, ReviewCheckpointRepo, ReviewFindingFilter, ReviewLedgerRepo,
    ReviewPolicyRepo, SortBy, SortOrder, TaskRepo, TaskRepositoryRole,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::McpToolError, protocol::McpContext, state::AppState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskIdParams {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetPolicyParams {
    task_id: String,
    #[serde(default)]
    expected_revision: Option<i64>,
    policy: ReviewPolicyDocument,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestRangeParams {
    task_id: String,
    repo_id: String,
    reason: String,
    idempotency_key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaiveFindingParams {
    task_id: String,
    finding_id: String,
    expected_coverage_revision: i64,
    rationale: String,
    reference: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListCheckpointsParams {
    task_id: String,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryCheckpointParams {
    task_id: String,
    checkpoint_id: String,
    expected_version: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributeCommitsParams {
    task_id: String,
    checkpoint_id: String,
    expected_version: i64,
    attributions: Vec<AttributionInput>,
    rationale: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributionInput {
    commit_sha: String,
    actor: ApiActorRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RebaselineParams {
    task_id: String,
    repo_id: String,
    expected_coverage_revision: i64,
    rationale: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListFindingsParams {
    task_id: String,
    #[serde(default)]
    repo_id: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    open: Option<bool>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

fn user_id(context: &McpContext) -> Result<&str, McpToolError> {
    context
        .user_id
        .as_deref()
        .ok_or_else(|| McpToolError::new(-32003, "authenticated server identity is required"))
}

fn workspace_root() -> PathBuf {
    std::env::var("FORGE_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("forge").join("worktrees"))
}

pub(super) async fn project_response_value(
    state: &AppState,
    project: db::Project,
) -> Result<Value, McpToolError> {
    let project_id = project.id.clone();
    let policy = ReviewPolicyRepo::get_project_review_policy(&*state.db, &project_id).await?;
    let mut value = crate::values::project_value(project)?;
    if let Some(policy) = policy {
        value["default_review_policy"] = json!({
            "version": policy.version,
            "policy": policy.policy.map(policy_from_db),
            "digest": policy.digest,
        });
    }
    Ok(value)
}

fn checkpoint_state(state: db::ReviewCheckpointState) -> &'static str {
    match state {
        db::ReviewCheckpointState::Queued => "queued",
        db::ReviewCheckpointState::Pending => "pending",
        db::ReviewCheckpointState::InReview => "in_review",
        db::ReviewCheckpointState::Reviewed => "reviewed",
        db::ReviewCheckpointState::Blocked => "blocked",
        db::ReviewCheckpointState::Superseded => "superseded",
    }
}

fn checkpoint_reason(reason: db::ReviewCheckpointReason) -> &'static str {
    match reason {
        db::ReviewCheckpointReason::Baseline => "baseline",
        db::ReviewCheckpointReason::Delta => "delta",
        db::ReviewCheckpointReason::AfterWorkUnit => "after_work_unit",
        db::ReviewCheckpointReason::SizeThreshold => "size_threshold",
        db::ReviewCheckpointReason::Requested => "requested",
        db::ReviewCheckpointReason::Final => "final",
        db::ReviewCheckpointReason::Recheck => "recheck",
    }
}

fn actor_value(actor: db::ActorRef) -> Value {
    match actor {
        db::ActorRef::Human(id) => json!({"kind": "human", "id": id}),
        db::ActorRef::Agent(id) => json!({"kind": "agent", "id": id}),
    }
}

fn actor_to_db(actor: ApiActorRef) -> db::ActorRef {
    match actor {
        ApiActorRef::Human(id) => db::ActorRef::Human(id),
        ApiActorRef::Agent(id) => db::ActorRef::Agent(id),
    }
}

async fn checkpoint_value(
    state: &AppState,
    checkpoint: ReviewCheckpoint,
) -> Result<Value, McpToolError> {
    let review_work_unit_ids = sqlx::query_scalar::<_, String>(
        "SELECT review_work_unit_id FROM work_unit_review_range WHERE checkpoint_id = ? ORDER BY review_work_unit_id",
    )
    .bind(&checkpoint.id)
    .fetch_all(state.db.pool())
    .await
    .map_err(db::DbError::from)?;
    let blocked_commits = checkpoint
        .blocked_detail_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.get("commits").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    Ok(json!({
        "id": checkpoint.id,
        "task_id": checkpoint.task_id,
        "repo_id": checkpoint.repo_id,
        "state": checkpoint_state(checkpoint.state),
        "reasons": checkpoint.reasons.into_iter().map(checkpoint_reason).collect::<Vec<_>>(),
        "attempt": checkpoint.attempt,
        "base_commit": checkpoint.base_commit,
        "head_commit": checkpoint.head_commit,
        "range_id": checkpoint.range_id,
        "review_work_unit_ids": review_work_unit_ids,
        "blocked_code": checkpoint.blocked_code,
        "blocked_commits": blocked_commits,
        "version": checkpoint.version,
        "created_at": checkpoint.created_at,
        "updated_at": checkpoint.updated_at,
    }))
}

fn coverage_status(
    status: services::review_range::ReviewCoverageStatus,
) -> api_types::ReviewCoverageStatus {
    match status {
        services::review_range::ReviewCoverageStatus::NotRequired => {
            api_types::ReviewCoverageStatus::NotRequired
        }
        services::review_range::ReviewCoverageStatus::Unchanged => {
            api_types::ReviewCoverageStatus::Unchanged
        }
        services::review_range::ReviewCoverageStatus::Uncovered => {
            api_types::ReviewCoverageStatus::Uncovered
        }
        services::review_range::ReviewCoverageStatus::Partial => {
            api_types::ReviewCoverageStatus::Partial
        }
        services::review_range::ReviewCoverageStatus::InReview => {
            api_types::ReviewCoverageStatus::InReview
        }
        services::review_range::ReviewCoverageStatus::FindingsOpen => {
            api_types::ReviewCoverageStatus::FindingsOpen
        }
        services::review_range::ReviewCoverageStatus::HistoryRewritten => {
            api_types::ReviewCoverageStatus::HistoryRewritten
        }
        services::review_range::ReviewCoverageStatus::Blocked => {
            api_types::ReviewCoverageStatus::Blocked
        }
        services::review_range::ReviewCoverageStatus::Satisfied => {
            api_types::ReviewCoverageStatus::Satisfied
        }
    }
}

pub(crate) async fn get_review_coverage(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskIdParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    super::require_task_access(state, &params.task_id, user_id(context)?).await?;
    let service =
        services::review_range::ReviewCoverageService::new(state.db.clone(), workspace_root());
    let repositories = TaskRepo::list_repositories(&*state.db, &params.task_id)
        .await?
        .into_iter()
        .filter(|repo| {
            matches!(
                repo.role,
                TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
            )
        })
        .map(|repo| repo.repo_id)
        .collect::<Vec<_>>();
    let mut responses = Vec::with_capacity(repositories.len());
    let mut policy_revision = None;
    for repo_id in repositories {
        let coverage = service.compute(&params.task_id, &repo_id, None).await?;
        let rows =
            ReviewLedgerRepo::load_review_coverage_rows(&*state.db, &params.task_id, &repo_id)
                .await?;
        policy_revision = rows
            .policy
            .as_ref()
            .map(|policy| policy.revision)
            .or(policy_revision);
        let ranges = coverage
            .ranges
            .into_iter()
            .map(|range| {
                let reports = range
                    .report_artifact_ids
                    .iter()
                    .filter_map(|artifact_id| {
                        rows.reports
                            .iter()
                            .find(|report| report.report_artifact_id == *artifact_id)
                            .map(|report| ReviewCoverageReportRef {
                                report_artifact_id: artifact_id.clone(),
                                actor: match report.actor.clone() {
                                    db::ActorRef::Human(id) => api_types::ActorRef::Human(id),
                                    db::ActorRef::Agent(id) => api_types::ActorRef::Agent(id),
                                },
                                verdict: match report.verdict {
                                    db::ReviewVerdict::Pass => ReviewReportVerdict::Pass,
                                    db::ReviewVerdict::RequestChanges => {
                                        ReviewReportVerdict::RequestChanges
                                    }
                                    db::ReviewVerdict::Questions => ReviewReportVerdict::Questions,
                                },
                            })
                    })
                    .collect();
                ReviewCoverageRangeResponse {
                    range_id: range.range_id,
                    base_commit: range.base_commit,
                    head_commit: range.head_commit,
                    valid_edge: range.valid_edge,
                    reports,
                }
            })
            .collect();
        responses.push(api_types::RepoReviewCoverageResponse {
            repo_id: coverage.repo_id,
            status: coverage_status(coverage.status),
            reason_code: coverage.reason_code,
            revision: coverage.revision,
            base_commit: coverage.base_commit,
            candidate_commit: coverage.candidate_commit,
            frontier_commit: coverage.frontier_commit,
            unreviewed_commits: coverage.unreviewed_commits,
            ranges,
            open_findings: ReviewSeverityCounts {
                p0: coverage.open_findings.p0,
                p1: coverage.open_findings.p1,
                p2: coverage.open_findings.p2,
                p3: coverage.open_findings.p3,
            },
            open_finding_ids: coverage.open_finding_ids,
            active_checkpoint_id: coverage.active_checkpoint_id,
            snapshot_digest: coverage.snapshot_digest,
        });
    }
    serde_json::to_value(TaskReviewCoverageResponse {
        task_id: params.task_id,
        review_policy_revision: policy_revision,
        repositories: responses,
    })
    .map_err(|error| McpToolError::new(-32603, error.to_string()))
}

pub(crate) async fn set_task_review_policy(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to set review policy",
        ));
    }
    let params: SetPolicyParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    let task = super::require_task_access(state, &params.task_id, user_id).await?;
    super::require_project_admin_access(state, &task.project_id, user_id).await?;
    let revision =
        services::review_range::ReviewPolicyService::new(state.db.clone(), workspace_root())
            .set(
                user_id,
                &params.task_id,
                params.expected_revision,
                policy_to_db(params.policy),
            )
            .await?;
    let response = TaskReviewPolicyResponse {
        task_id: params.task_id,
        policy: Some(api_types::TaskReviewPolicyRevisionResponse {
            revision: revision.revision,
            digest: revision.digest,
            source: match revision.source {
                db::ReviewPolicySource::ProjectDefault => {
                    api_types::ReviewPolicySource::ProjectDefault
                }
                db::ReviewPolicySource::Owner => api_types::ReviewPolicySource::Owner,
            },
            policy: policy_from_db(revision.policy),
            created_by_user_id: revision.created_by_user_id,
            created_at: revision.created_at,
        }),
    };
    serde_json::to_value(response).map_err(|error| McpToolError::new(-32603, error.to_string()))
}

pub(crate) async fn get_task_review_policy(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: TaskIdParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    super::require_task_access(state, &params.task_id, user_id(context)?).await?;
    let current =
        ReviewPolicyRepo::get_current_task_review_policy(&*state.db, &params.task_id).await?;
    let policy = current.map(|revision| api_types::TaskReviewPolicyRevisionResponse {
        revision: revision.revision,
        digest: revision.digest,
        source: match revision.source {
            db::ReviewPolicySource::ProjectDefault => api_types::ReviewPolicySource::ProjectDefault,
            db::ReviewPolicySource::Owner => api_types::ReviewPolicySource::Owner,
        },
        policy: policy_from_db(revision.policy),
        created_by_user_id: revision.created_by_user_id,
        created_at: revision.created_at,
    });
    serde_json::to_value(TaskReviewPolicyResponse {
        task_id: params.task_id,
        policy,
    })
    .map_err(|error| McpToolError::new(-32603, error.to_string()))
}

pub(crate) async fn request_range_review(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to request a range review",
        ));
    }
    let params: RequestRangeParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    let task = super::require_task_access(state, &params.task_id, user_id).await?;
    super::require_project_write_access(state, &task.project_id, user_id).await?;
    let checkpoint =
        services::review_range::ReviewScheduler::new(state.db.clone(), workspace_root())
            .with_event_bus(state.event_bus.clone())
            .request(
                services::review_range::ReviewRequestSource::Human {
                    user_id: user_id.to_owned(),
                },
                &params.task_id,
                &params.repo_id,
                params.reason,
                &params.idempotency_key,
            )
            .await?;
    checkpoint_value(state, checkpoint).await
}

pub(crate) async fn list_review_checkpoints(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListCheckpointsParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    super::require_task_access(state, &params.task_id, user_id(context)?).await?;
    let page = ReviewCheckpointRepo::list_review_checkpoints(
        &*state.db,
        &params.task_id,
        params.repo_id.as_deref(),
        PageRequest {
            cursor: params.cursor,
            limit: params.limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut items = Vec::with_capacity(page.items.len());
    for checkpoint in page.items {
        items.push(checkpoint_value(state, checkpoint).await?);
    }
    Ok(json!({"items": items, "next_cursor": page.next_cursor}))
}

pub(crate) async fn retry_review_checkpoint(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to retry a review checkpoint",
        ));
    }
    let params: RetryCheckpointParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    super::require_task_access(state, &params.task_id, user_id).await?;
    let checkpoint = ReviewCheckpointRepo::get_review_checkpoint(&*state.db, &params.checkpoint_id)
        .await?
        .filter(|checkpoint| checkpoint.task_id == params.task_id)
        .ok_or_else(|| {
            McpToolError::not_found("review_checkpoint", params.checkpoint_id.clone())
        })?;
    let retried = services::review_range::ReviewScheduler::new(state.db.clone(), workspace_root())
        .retry(user_id, &checkpoint.id, params.expected_version)
        .await?;
    checkpoint_value(state, retried).await
}

pub(crate) async fn attribute_review_commits(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to attribute review commits",
        ));
    }
    let params: AttributeCommitsParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    super::require_task_access(state, &params.task_id, user_id).await?;
    let checkpoint =
        services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root())
            .attribute_commits(
                user_id,
                &params.task_id,
                &params.checkpoint_id,
                params.expected_version,
                params
                    .attributions
                    .into_iter()
                    .map(|attribution| (attribution.commit_sha, actor_to_db(attribution.actor)))
                    .collect(),
                params.rationale,
            )
            .await?;
    checkpoint_value(state, checkpoint).await
}

pub(crate) async fn rebaseline_review(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to re-baseline review coverage",
        ));
    }
    let params: RebaselineParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    super::require_task_access(state, &params.task_id, user_id).await?;
    let base =
        services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root())
            .rebaseline(
                user_id,
                &params.task_id,
                &params.repo_id,
                params.expected_coverage_revision,
                params.rationale,
            )
            .await?;
    serde_json::to_value(api_types::ReviewBaseResponse {
        task_id: base.task_id,
        repo_id: base.repo_id,
        revision: base.revision,
        base_commit: base.base_commit,
        source: match base.source {
            db::ReviewBaseSource::FirstIntegration => api_types::ReviewBaseSource::FirstIntegration,
            db::ReviewBaseSource::WorkspacePrepare => api_types::ReviewBaseSource::WorkspacePrepare,
            db::ReviewBaseSource::HumanRebaseline => api_types::ReviewBaseSource::HumanRebaseline,
        },
        decision_id: base.decision_id,
        pinned_at: base.pinned_at,
    })
    .map_err(|error| McpToolError::new(-32603, error.to_string()))
}

pub(crate) async fn list_review_findings(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    let params: ListFindingsParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    super::require_task_access(state, &params.task_id, user_id(context)?).await?;
    let severity = params
        .severity
        .as_deref()
        .map(|value| match value {
            "P0" => Ok(db::ReviewFindingSeverity::P0),
            "P1" => Ok(db::ReviewFindingSeverity::P1),
            "P2" => Ok(db::ReviewFindingSeverity::P2),
            "P3" => Ok(db::ReviewFindingSeverity::P3),
            _ => Err(McpToolError::new(
                -32602,
                "severity must be P0, P1, P2, or P3",
            )),
        })
        .transpose()?;
    let service =
        services::review_range::ReviewCoverageService::new(state.db.clone(), workspace_root());
    let repo_ids = if let Some(repo_id) = params.repo_id.as_ref() {
        vec![repo_id.clone()]
    } else {
        TaskRepo::list_repositories(&*state.db, &params.task_id)
            .await?
            .into_iter()
            .filter(|repository| {
                matches!(
                    repository.role,
                    TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
                )
            })
            .map(|repository| repository.repo_id)
            .collect()
    };
    let mut open_ids = std::collections::BTreeSet::new();
    let mut all_ids = std::collections::BTreeSet::new();
    for repo_id in repo_ids {
        open_ids.extend(
            service
                .compute(&params.task_id, &repo_id, None)
                .await?
                .open_finding_ids,
        );
        all_ids.extend(
            ReviewLedgerRepo::load_review_coverage_rows(&*state.db, &params.task_id, &repo_id)
                .await?
                .findings
                .into_iter()
                .map(|finding| finding.id),
        );
    }
    let finding_ids = params.open.map(|is_open| {
        if is_open {
            open_ids.iter().cloned().collect()
        } else {
            all_ids.difference(&open_ids).cloned().collect()
        }
    });
    let page = ReviewLedgerRepo::list_review_findings(
        &*state.db,
        &params.task_id,
        ReviewFindingFilter {
            repo_id: params.repo_id,
            severity,
            finding_ids,
        },
        PageRequest {
            cursor: params.cursor,
            limit: params.limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut items = Vec::new();
    for finding in page.items {
        let is_open = open_ids.contains(&finding.id);
        let locations = ReviewLedgerRepo::list_review_finding_locations(&*state.db, &finding.id)
            .await?
            .into_iter()
            .map(|location| {
                json!({
                    "path": location.path,
                    "line_start": location.line_start,
                    "line_end": location.line_end,
                    "attribution_kind": location.attribution.kind.to_string(),
                    "work_unit_id": location.attribution.work_unit_id,
                    "execution_id": location.attribution.execution_id,
                    "actor": location.attribution.actor.map(actor_value),
                })
            })
            .collect::<Vec<_>>();
        let resolutions =
            ReviewLedgerRepo::list_review_finding_resolutions(&*state.db, &finding.id)
                .await?
                .into_iter()
                .map(|resolution| {
                    json!({
                        "id": resolution.id,
                        "outcome": resolution.outcome.to_string(),
                        "anchor_commit": resolution.anchor_commit,
                        "report_artifact_id": resolution.report_artifact_id,
                        "decision_id": resolution.decision_id,
                        "reference": resolution.waiver_reference,
                        "actor": actor_value(resolution.actor),
                        "created_at": resolution.created_at,
                    })
                })
                .collect::<Vec<_>>();
        let disposition = ReviewLedgerRepo::list_review_finding_triage(&*state.db, &finding.id)
            .await?
            .into_iter()
            .max_by(|left, right| {
                (&left.created_at, &left.request_id).cmp(&(&right.created_at, &right.request_id))
            })
            .map(|triage| match triage.disposition {
                db::ReviewFindingDisposition::Correct => "correct",
                db::ReviewFindingDisposition::Recheck => "recheck",
                db::ReviewFindingDisposition::Human => "human",
                db::ReviewFindingDisposition::Defer => "defer",
            });
        let mut correction_work_unit_ids =
            db::ReviewLedgerRepo::list_review_correction_links(&*state.db, &finding.id)
                .await?
                .into_iter()
                .map(|link| link.correction_work_unit_id)
                .collect::<Vec<_>>();
        correction_work_unit_ids.sort();
        correction_work_unit_ids.dedup();
        items.push(json!({
            "id": finding.id,
            "task_id": finding.task_id,
            "repo_id": finding.repo_id,
            "range_id": finding.range_id,
            "report_artifact_id": finding.report_artifact_id,
            "label": finding.label,
            "severity": finding.severity.to_string(),
            "title": finding.title,
            "evidence": finding.evidence,
            "invariant": finding.invariant,
            "anchor_commit": finding.anchor_commit,
            "open": is_open,
            "locations": locations,
            "disposition": disposition,
            "correction_work_unit_ids": correction_work_unit_ids,
            "resolutions": resolutions,
            "created_at": finding.created_at,
        }));
    }
    Ok(json!({"items": items, "next_cursor": page.next_cursor}))
}

pub(crate) async fn waive_review_finding(
    state: &AppState,
    params: Value,
    context: &McpContext,
) -> Result<Value, McpToolError> {
    if context.copilot_run_id.is_some() {
        return Err(McpToolError::new(
            -32003,
            "Human authentication is required to waive a review finding",
        ));
    }
    let params: WaiveFindingParams = serde_json::from_value(params)
        .map_err(|error| McpToolError::new(-32602, error.to_string()))?;
    let user_id = user_id(context)?;
    super::require_task_access(state, &params.task_id, user_id).await?;
    let resolution =
        services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root())
            .waive(
                user_id,
                &params.task_id,
                &params.finding_id,
                params.expected_coverage_revision,
                params.rationale,
                params.reference,
            )
            .await?;
    Ok(
        json!({"finding_id": resolution.finding_id, "outcome": "waived", "reference": resolution.waiver_reference, "created_at": resolution.created_at}),
    )
}

fn policy_to_db(policy: ReviewPolicyDocument) -> db::ReviewPolicyDocument {
    db::ReviewPolicyDocument {
        schema_version: policy.schema_version,
        risk_class: match policy.risk_class {
            api_types::ReviewRiskClass::R0 => db::ReviewRiskClass::R0,
            api_types::ReviewRiskClass::R1 => db::ReviewRiskClass::R1,
            api_types::ReviewRiskClass::R2 => db::ReviewRiskClass::R2,
        },
        coverage_required: policy.coverage_required,
        trigger_mode: match policy.trigger_mode {
            api_types::ReviewTriggerMode::Auto => db::ReviewTriggerMode::Auto,
            api_types::ReviewTriggerMode::Manual => db::ReviewTriggerMode::Manual,
        },
        reviewer_role: policy.reviewer_role,
        required_reviewers: policy.required_reviewers,
        require_human: policy.require_human,
        size_threshold: db::ReviewSizeThreshold {
            changed_lines: policy.size_threshold.changed_lines,
            changed_files: policy.size_threshold.changed_files,
        },
        coordinator_may_request: policy.coordinator_may_request,
        max_checkpoints_per_repo: policy.max_checkpoints_per_repo,
    }
}

fn policy_from_db(policy: db::ReviewPolicyDocument) -> ReviewPolicyDocument {
    ReviewPolicyDocument {
        schema_version: policy.schema_version,
        risk_class: match policy.risk_class {
            db::ReviewRiskClass::R0 => api_types::ReviewRiskClass::R0,
            db::ReviewRiskClass::R1 => api_types::ReviewRiskClass::R1,
            db::ReviewRiskClass::R2 => api_types::ReviewRiskClass::R2,
        },
        coverage_required: policy.coverage_required,
        trigger_mode: match policy.trigger_mode {
            db::ReviewTriggerMode::Auto => api_types::ReviewTriggerMode::Auto,
            db::ReviewTriggerMode::Manual => api_types::ReviewTriggerMode::Manual,
        },
        reviewer_role: policy.reviewer_role,
        required_reviewers: policy.required_reviewers,
        require_human: policy.require_human,
        size_threshold: api_types::ReviewSizeThreshold {
            changed_lines: policy.size_threshold.changed_lines,
            changed_files: policy.size_threshold.changed_files,
        },
        coordinator_may_request: policy.coordinator_may_request,
        max_checkpoints_per_repo: policy.max_checkpoints_per_repo,
    }
}
