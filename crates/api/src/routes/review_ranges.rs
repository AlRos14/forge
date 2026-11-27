use std::collections::BTreeSet;

use api_types::{
    AttributeReviewCommitsRequest, CreateReviewCheckpointRequest, ListReviewCheckpointsResponse,
    ListReviewFindingsResponse, RebaselineReviewRequest, RepoReviewCoverageResponse,
    ReviewAttributionKind, ReviewBaseResponse, ReviewBaseSource, ReviewCheckpointReason,
    ReviewCheckpointResponse, ReviewCheckpointState, ReviewCoverageRangeResponse,
    ReviewCoverageReportRef, ReviewFindingLocationResponse, ReviewFindingResolutionResponse,
    ReviewFindingResponse, ReviewFindingSeverity, ReviewPolicyDocument, ReviewPolicySource,
    ReviewRangeResponse, ReviewResolutionOutcome, ReviewSeverityCounts, SetTaskReviewPolicyRequest,
    TaskReviewCoverageResponse, TaskReviewPolicyResponse, TaskReviewPolicyRevisionResponse,
    WaiveReviewFindingRequest,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::{
    ActorRef, PageRequest, ProjectReviewPolicy, ReviewCheckpoint,
    ReviewCheckpointReason as DbCheckpointReason, ReviewCheckpointRepo,
    ReviewCheckpointState as DbCheckpointState, ReviewFinding, ReviewFindingDisposition,
    ReviewFindingFilter, ReviewLedgerRepo, ReviewPolicyDocument as DbReviewPolicyDocument,
    ReviewPolicyRepo, ReviewRange, ReviewResolutionOutcome as DbResolutionOutcome, SortBy,
    SortOrder, TaskRepo, TaskRepositoryRole, TaskRepositorySetRepo, TaskReviewBase,
    TaskReviewPolicyRevision,
};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    errors::{ApiError, ApiResult},
    routes::{auth::AuthenticatedUser, tasks},
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct ReviewCheckpointListQuery {
    pub repo_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewFindingListQuery {
    pub repo_id: Option<String>,
    pub severity: Option<String>,
    pub open: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

fn workspace_root(state: &AppState) -> std::path::PathBuf {
    state.cleanup_scheduler.workspace_root().to_path_buf()
}

pub(super) fn policy_to_db(policy: ReviewPolicyDocument) -> DbReviewPolicyDocument {
    DbReviewPolicyDocument {
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

pub(super) fn policy_from_db(policy: DbReviewPolicyDocument) -> ReviewPolicyDocument {
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

fn task_policy_response(
    task_id: &str,
    revision: Option<TaskReviewPolicyRevision>,
) -> TaskReviewPolicyResponse {
    TaskReviewPolicyResponse {
        task_id: task_id.to_owned(),
        policy: revision.map(|revision| TaskReviewPolicyRevisionResponse {
            revision: revision.revision,
            digest: revision.digest,
            source: match revision.source {
                db::ReviewPolicySource::ProjectDefault => ReviewPolicySource::ProjectDefault,
                db::ReviewPolicySource::Owner => ReviewPolicySource::Owner,
            },
            policy: policy_from_db(revision.policy),
            created_by_user_id: revision.created_by_user_id,
            created_at: revision.created_at,
        }),
    }
}

pub(super) fn range_response(range: ReviewRange) -> ReviewRangeResponse {
    ReviewRangeResponse {
        id: range.id,
        task_id: range.task_id,
        repo_id: range.repo_id,
        repository_set_revision: range.repository_set_revision,
        base_commit: range.base_commit,
        head_commit: range.head_commit,
        base_tree: range.base_tree,
        head_tree: range.head_tree,
        range_digest: range.range_digest,
    }
}

fn checkpoint_state(state: DbCheckpointState) -> ReviewCheckpointState {
    match state {
        DbCheckpointState::Queued => ReviewCheckpointState::Queued,
        DbCheckpointState::Pending => ReviewCheckpointState::Pending,
        DbCheckpointState::InReview => ReviewCheckpointState::InReview,
        DbCheckpointState::Reviewed => ReviewCheckpointState::Reviewed,
        DbCheckpointState::Blocked => ReviewCheckpointState::Blocked,
        DbCheckpointState::Superseded => ReviewCheckpointState::Superseded,
    }
}

fn checkpoint_reason(reason: DbCheckpointReason) -> ReviewCheckpointReason {
    match reason {
        DbCheckpointReason::Baseline => ReviewCheckpointReason::Baseline,
        DbCheckpointReason::Delta => ReviewCheckpointReason::Delta,
        DbCheckpointReason::AfterWorkUnit => ReviewCheckpointReason::AfterWorkUnit,
        DbCheckpointReason::SizeThreshold => ReviewCheckpointReason::SizeThreshold,
        DbCheckpointReason::Requested => ReviewCheckpointReason::Requested,
        DbCheckpointReason::Final => ReviewCheckpointReason::Final,
        DbCheckpointReason::Recheck => ReviewCheckpointReason::Recheck,
    }
}

async fn checkpoint_response(
    state: &AppState,
    checkpoint: ReviewCheckpoint,
) -> ApiResult<ReviewCheckpointResponse> {
    let review_work_unit_ids = sqlx::query_scalar::<_, String>(
        "SELECT review_work_unit_id FROM work_unit_review_range WHERE checkpoint_id = ? ORDER BY review_work_unit_id",
    )
    .bind(&checkpoint.id)
    .fetch_all(state.db.pool())
    .await?;
    let blocked_commits = checkpoint
        .blocked_detail_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.get("commits").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    Ok(ReviewCheckpointResponse {
        id: checkpoint.id,
        task_id: checkpoint.task_id,
        repo_id: checkpoint.repo_id,
        state: checkpoint_state(checkpoint.state),
        reasons: checkpoint
            .reasons
            .into_iter()
            .map(checkpoint_reason)
            .collect(),
        attempt: checkpoint.attempt,
        base_commit: checkpoint.base_commit,
        head_commit: checkpoint.head_commit,
        range_id: checkpoint.range_id,
        review_work_unit_ids,
        blocked_code: checkpoint.blocked_code,
        blocked_commits,
        version: checkpoint.version,
        created_at: checkpoint.created_at,
        updated_at: checkpoint.updated_at,
    })
}

fn actor_to_api(actor: ActorRef) -> api_types::ActorRef {
    match actor {
        ActorRef::Human(id) => api_types::ActorRef::Human(id),
        ActorRef::Agent(id) => api_types::ActorRef::Agent(id),
    }
}

fn actor_to_db(actor: api_types::ActorRef) -> ActorRef {
    match actor {
        api_types::ActorRef::Human(id) => ActorRef::Human(id),
        api_types::ActorRef::Agent(id) => ActorRef::Agent(id),
    }
}

fn api_verdict(verdict: db::ReviewVerdict) -> api_types::ReviewReportVerdict {
    match verdict {
        db::ReviewVerdict::Pass => api_types::ReviewReportVerdict::Pass,
        db::ReviewVerdict::RequestChanges => api_types::ReviewReportVerdict::RequestChanges,
        db::ReviewVerdict::Questions => api_types::ReviewReportVerdict::Questions,
    }
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

fn coverage_response(
    coverage: services::review_range::RepoCoverage,
    rows: db::ReviewCoverageRows,
) -> RepoReviewCoverageResponse {
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
                        .find(|report| &report.report_artifact_id == artifact_id)
                        .map(|report| ReviewCoverageReportRef {
                            report_artifact_id: artifact_id.clone(),
                            actor: actor_to_api(report.actor.clone()),
                            verdict: api_verdict(report.verdict),
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
    RepoReviewCoverageResponse {
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
    }
}

fn finding_severity(value: &str) -> Option<db::ReviewFindingSeverity> {
    match value {
        "P0" => Some(db::ReviewFindingSeverity::P0),
        "P1" => Some(db::ReviewFindingSeverity::P1),
        "P2" => Some(db::ReviewFindingSeverity::P2),
        "P3" => Some(db::ReviewFindingSeverity::P3),
        _ => None,
    }
}

fn api_severity(value: db::ReviewFindingSeverity) -> ReviewFindingSeverity {
    match value {
        db::ReviewFindingSeverity::P0 => ReviewFindingSeverity::P0,
        db::ReviewFindingSeverity::P1 => ReviewFindingSeverity::P1,
        db::ReviewFindingSeverity::P2 => ReviewFindingSeverity::P2,
        db::ReviewFindingSeverity::P3 => ReviewFindingSeverity::P3,
    }
}

fn api_disposition(value: ReviewFindingDisposition) -> api_types::TaskReviewDisposition {
    match value {
        ReviewFindingDisposition::Correct => api_types::TaskReviewDisposition::Correct,
        ReviewFindingDisposition::Recheck => api_types::TaskReviewDisposition::Recheck,
        ReviewFindingDisposition::Human => api_types::TaskReviewDisposition::Human,
        ReviewFindingDisposition::Defer => api_types::TaskReviewDisposition::Defer,
    }
}

fn api_resolution(value: DbResolutionOutcome) -> ReviewResolutionOutcome {
    match value {
        DbResolutionOutcome::Fixed => ReviewResolutionOutcome::Fixed,
        DbResolutionOutcome::NotReproduced => ReviewResolutionOutcome::NotReproduced,
        DbResolutionOutcome::Waived => ReviewResolutionOutcome::Waived,
    }
}

fn api_attribution(value: db::ReviewAttributionKind) -> ReviewAttributionKind {
    match value {
        db::ReviewAttributionKind::WorkUnit => ReviewAttributionKind::WorkUnit,
        db::ReviewAttributionKind::Preexisting => ReviewAttributionKind::Preexisting,
        db::ReviewAttributionKind::Upstream => ReviewAttributionKind::Upstream,
        db::ReviewAttributionKind::Unattributed => ReviewAttributionKind::Unattributed,
    }
}

async fn finding_response(
    state: &AppState,
    finding: ReviewFinding,
    open_ids: &BTreeSet<String>,
) -> ApiResult<ReviewFindingResponse> {
    let locations = db::ReviewLedgerRepo::list_review_finding_locations(&*state.db, &finding.id)
        .await?
        .into_iter()
        .map(|location| ReviewFindingLocationResponse {
            path: location.path,
            line_start: location.line_start,
            line_end: location.line_end,
            attribution_kind: api_attribution(location.attribution.kind),
            work_unit_id: location.attribution.work_unit_id,
            execution_id: location.attribution.execution_id,
            actor: location.attribution.actor.map(actor_to_api),
        })
        .collect();
    let resolutions =
        db::ReviewLedgerRepo::list_review_finding_resolutions(&*state.db, &finding.id)
            .await?
            .into_iter()
            .map(|resolution| ReviewFindingResolutionResponse {
                id: resolution.id,
                outcome: api_resolution(resolution.outcome),
                anchor_commit: resolution.anchor_commit,
                report_artifact_id: resolution.report_artifact_id,
                decision_id: resolution.decision_id,
                reference: resolution.waiver_reference,
                actor: actor_to_api(resolution.actor),
                created_at: resolution.created_at,
            })
            .collect();
    let disposition = db::ReviewLedgerRepo::list_review_finding_triage(&*state.db, &finding.id)
        .await?
        .into_iter()
        .max_by(|left, right| {
            (&left.created_at, &left.request_id).cmp(&(&right.created_at, &right.request_id))
        })
        .map(|triage| api_disposition(triage.disposition));
    let mut correction_work_unit_ids =
        db::ReviewLedgerRepo::list_review_correction_links(&*state.db, &finding.id)
            .await?
            .into_iter()
            .map(|link| link.correction_work_unit_id)
            .collect::<Vec<_>>();
    correction_work_unit_ids.sort();
    correction_work_unit_ids.dedup();
    Ok(ReviewFindingResponse {
        id: finding.id.clone(),
        task_id: finding.task_id,
        repo_id: finding.repo_id,
        range_id: finding.range_id,
        report_artifact_id: finding.report_artifact_id,
        label: finding.label,
        severity: api_severity(finding.severity),
        title: finding.title,
        evidence: finding.evidence,
        invariant: finding.invariant,
        anchor_commit: finding.anchor_commit,
        open: open_ids.contains(&finding.id),
        locations,
        disposition,
        correction_work_unit_ids,
        resolutions,
        created_at: finding.created_at,
    })
}

fn base_response(base: TaskReviewBase) -> ReviewBaseResponse {
    ReviewBaseResponse {
        task_id: base.task_id,
        repo_id: base.repo_id,
        revision: base.revision,
        base_commit: base.base_commit,
        source: match base.source {
            db::ReviewBaseSource::FirstIntegration => ReviewBaseSource::FirstIntegration,
            db::ReviewBaseSource::WorkspacePrepare => ReviewBaseSource::WorkspacePrepare,
            db::ReviewBaseSource::HumanRebaseline => ReviewBaseSource::HumanRebaseline,
        },
        decision_id: base.decision_id,
        pinned_at: base.pinned_at,
    }
}

pub async fn get_task_review_policy(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskReviewPolicyResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let revision =
        db::ReviewPolicyRepo::get_current_task_review_policy(&*state.db, &task_id).await?;
    Ok(Json(task_policy_response(&task_id, revision)))
}

pub async fn set_task_review_policy(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<SetTaskReviewPolicyRequest>,
) -> ApiResult<Json<TaskReviewPolicyResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let revision =
        services::review_range::ReviewPolicyService::new(state.db.clone(), workspace_root(&state))
            .set(
                &user.user_id,
                &task_id,
                request.expected_revision,
                policy_to_db(request.policy),
            )
            .await
            .map_err(ApiError::from)?;
    Ok(Json(task_policy_response(&task_id, Some(revision))))
}

pub async fn get_task_review_coverage(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
) -> ApiResult<Json<TaskReviewCoverageResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let service = services::review_range::ReviewCoverageService::new(
        state.db.clone(),
        workspace_root(&state),
    );
    let mut repositories = TaskRepo::list_repositories(&*state.db, &task_id)
        .await?
        .into_iter()
        .filter(|repository| {
            matches!(
                repository.role,
                TaskRepositoryRole::Primary | TaskRepositoryRole::Affected
            )
        })
        .map(|repository| repository.repo_id)
        .collect::<Vec<_>>();
    repositories.sort();
    let mut rows = Vec::with_capacity(repositories.len());
    let mut policy_revision = None;
    for repo_id in repositories {
        let coverage = service
            .compute(&task_id, &repo_id, None)
            .await
            .map_err(ApiError::from)?;
        let source_rows =
            db::ReviewLedgerRepo::load_review_coverage_rows(&*state.db, &task_id, &repo_id).await?;
        policy_revision = source_rows
            .policy
            .as_ref()
            .map(|policy| policy.revision)
            .or(policy_revision);
        rows.push(coverage_response(coverage, source_rows));
    }
    Ok(Json(TaskReviewCoverageResponse {
        task_id,
        review_policy_revision: policy_revision,
        repositories: rows,
    }))
}

pub async fn list_task_review_checkpoints(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<ReviewCheckpointListQuery>,
) -> ApiResult<Json<ListReviewCheckpointsResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let page = db::ReviewCheckpointRepo::list_review_checkpoints(
        &*state.db,
        &task_id,
        query.repo_id.as_deref(),
        PageRequest {
            cursor: query.cursor,
            limit: query.limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    Ok(Json(ListReviewCheckpointsResponse {
        items: futures_util::future::try_join_all(
            page.items
                .into_iter()
                .map(|checkpoint| checkpoint_response(&state, checkpoint)),
        )
        .await?,
        next_cursor: page.next_cursor,
    }))
}

pub async fn create_task_review_checkpoint(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<CreateReviewCheckpointRequest>,
) -> ApiResult<Json<ReviewCheckpointResponse>> {
    tasks::require_task_mutable(&state, &task_id, &user).await?;
    let checkpoint =
        services::review_range::ReviewScheduler::new(state.db.clone(), workspace_root(&state))
            .with_event_bus(state.event_bus.clone())
            .request(
                services::review_range::ReviewRequestSource::Human {
                    user_id: user.user_id,
                },
                &task_id,
                &request.repo_id,
                request.reason,
                &request.idempotency_key,
            )
            .await
            .map_err(ApiError::from)?;
    Ok(Json(checkpoint_response(&state, checkpoint).await?))
}

pub async fn retry_task_review_checkpoint(
    State(state): State<AppState>,
    Path((task_id, checkpoint_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<api_types::RetryReviewCheckpointRequest>,
) -> ApiResult<Json<ReviewCheckpointResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let existing = db::ReviewCheckpointRepo::get_review_checkpoint(&*state.db, &checkpoint_id)
        .await?
        .filter(|checkpoint| checkpoint.task_id == task_id)
        .ok_or_else(|| ApiError::not_found("review checkpoint", checkpoint_id.clone()))?;
    let checkpoint =
        services::review_range::ReviewScheduler::new(state.db.clone(), workspace_root(&state))
            .retry(&user.user_id, &checkpoint_id, request.expected_version)
            .await
            .map_err(ApiError::from)?;
    debug_assert_eq!(checkpoint.task_id, existing.task_id);
    Ok(Json(checkpoint_response(&state, checkpoint).await?))
}

pub async fn attribute_task_review_commits(
    State(state): State<AppState>,
    Path((task_id, checkpoint_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<AttributeReviewCommitsRequest>,
) -> ApiResult<Json<ReviewCheckpointResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let checkpoint =
        services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root(&state))
            .attribute_commits(
                &user.user_id,
                &task_id,
                &checkpoint_id,
                request.expected_version,
                request
                    .attributions
                    .into_iter()
                    .map(|attribution| (attribution.commit_sha, actor_to_db(attribution.actor)))
                    .collect(),
                request.rationale,
            )
            .await
            .map_err(ApiError::from)?;
    Ok(Json(checkpoint_response(&state, checkpoint).await?))
}

pub async fn rebaseline_task_review(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Json(request): Json<RebaselineReviewRequest>,
) -> ApiResult<Json<ReviewBaseResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let base =
        services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root(&state))
            .rebaseline(
                &user.user_id,
                &task_id,
                &request.repo_id,
                request.expected_coverage_revision,
                request.rationale,
            )
            .await
            .map_err(ApiError::from)?;
    Ok(Json(base_response(base)))
}

pub async fn list_task_review_findings(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    user: AuthenticatedUser,
    Query(query): Query<ReviewFindingListQuery>,
) -> ApiResult<Json<ListReviewFindingsResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    let severity = query
        .severity
        .as_deref()
        .map(|value| {
            finding_severity(value)
                .ok_or_else(|| ApiError::bad_request("severity must be one of P0, P1, P2, or P3"))
        })
        .transpose()?;
    let service = services::review_range::ReviewCoverageService::new(
        state.db.clone(),
        workspace_root(&state),
    );
    let repo_ids = if let Some(repo_id) = query.repo_id.as_ref() {
        vec![repo_id.clone()]
    } else {
        TaskRepo::list_repositories(&*state.db, &task_id)
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
    let mut open_ids = BTreeSet::new();
    let mut all_ids = BTreeSet::new();
    for repo_id in repo_ids {
        open_ids.extend(
            service
                .compute(&task_id, &repo_id, None)
                .await
                .map_err(ApiError::from)?
                .open_finding_ids,
        );
        all_ids.extend(
            db::ReviewLedgerRepo::load_review_coverage_rows(&*state.db, &task_id, &repo_id)
                .await?
                .findings
                .into_iter()
                .map(|finding| finding.id),
        );
    }
    let filter_ids = query.open.map(|is_open| {
        if is_open {
            open_ids.iter().cloned().collect()
        } else {
            all_ids.difference(&open_ids).cloned().collect()
        }
    });
    let page = db::ReviewLedgerRepo::list_review_findings(
        &*state.db,
        &task_id,
        ReviewFindingFilter {
            repo_id: query.repo_id,
            severity,
            finding_ids: filter_ids,
        },
        PageRequest {
            cursor: query.cursor,
            limit: query.limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let findings = page
        .items
        .into_iter()
        .map(|finding| finding_response(&state, finding, &open_ids));
    Ok(Json(ListReviewFindingsResponse {
        items: futures_util::future::try_join_all(findings).await?,
        next_cursor: page.next_cursor,
    }))
}

pub async fn waive_task_review_finding(
    State(state): State<AppState>,
    Path((task_id, finding_id)): Path<(String, String)>,
    user: AuthenticatedUser,
    Json(request): Json<WaiveReviewFindingRequest>,
) -> ApiResult<Json<ReviewFindingResponse>> {
    tasks::require_task_visible(&state, &task_id, &user).await?;
    services::review_range::ReviewFindingService::new(state.db.clone(), workspace_root(&state))
        .waive(
            &user.user_id,
            &task_id,
            &finding_id,
            request.expected_coverage_revision,
            request.rationale,
            request.reference,
        )
        .await
        .map_err(ApiError::from)?;
    let finding = db::ReviewLedgerRepo::get_review_finding(&*state.db, &finding_id)
        .await?
        .ok_or_else(|| ApiError::not_found("review finding", finding_id.clone()))?;
    let open_ids = services::review_range::ReviewCoverageService::new(
        state.db.clone(),
        workspace_root(&state),
    )
    .compute(&task_id, &finding.repo_id, None)
    .await
    .map_err(ApiError::from)?
    .open_finding_ids
    .into_iter()
    .collect();
    Ok(Json(finding_response(&state, finding, &open_ids).await?))
}

// Kept here to make the DTO conversion explicit and exhaustively checked.
pub(super) fn project_policy_response(
    policy: ProjectReviewPolicy,
) -> api_types::ProjectReviewPolicyResponse {
    api_types::ProjectReviewPolicyResponse {
        version: policy.version,
        policy: policy.policy.map(policy_from_db),
        digest: policy.digest,
    }
}
