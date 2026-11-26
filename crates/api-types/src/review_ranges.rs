//! Wire contracts for commit-range review and per-repository coverage.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ActorRef, ReviewReportVerdict, TaskReviewDisposition};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewRiskClass {
    R0,
    R1,
    R2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewTriggerMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ReviewSizeThreshold {
    #[ts(type = "number")]
    pub changed_lines: u64,
    #[ts(type = "number")]
    pub changed_files: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ReviewPolicyDocument {
    pub schema_version: u32,
    pub risk_class: ReviewRiskClass,
    pub coverage_required: bool,
    pub trigger_mode: ReviewTriggerMode,
    pub reviewer_role: String,
    pub required_reviewers: u32,
    pub require_human: bool,
    pub size_threshold: ReviewSizeThreshold,
    pub coordinator_may_request: bool,
    pub max_checkpoints_per_repo: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub enum ReviewFindingSeverity {
    P0,
    P1,
    P2,
    P3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewCheckpointState {
    Queued,
    Pending,
    InReview,
    Reviewed,
    Blocked,
    Superseded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewCheckpointReason {
    Baseline,
    Delta,
    AfterWorkUnit,
    SizeThreshold,
    Requested,
    Final,
    Recheck,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewCoverageStatus {
    NotRequired,
    Unchanged,
    Uncovered,
    Partial,
    InReview,
    FindingsOpen,
    HistoryRewritten,
    Blocked,
    Satisfied,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewResolutionOutcome {
    Fixed,
    NotReproduced,
    Waived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewAttributionKind {
    WorkUnit,
    Preexisting,
    Upstream,
    Unattributed,
}

/// What a reviewer may claim about a frozen input finding. A waiver is a
/// separate Human operation and cannot be claimed by a report.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewResolutionClaim {
    Fixed,
    NotReproduced,
    StillOpen,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewPolicySource {
    ProjectDefault,
    Owner,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewBaseSource {
    FirstIntegration,
    WorkspacePrepare,
    HumanRebaseline,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReviewPolicyResponse {
    pub task_id: String,
    pub policy: Option<TaskReviewPolicyRevisionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReviewPolicyRevisionResponse {
    #[ts(type = "number")]
    pub revision: i64,
    pub digest: String,
    pub source: ReviewPolicySource,
    pub policy: ReviewPolicyDocument,
    pub created_by_user_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SetTaskReviewPolicyRequest {
    #[serde(default)]
    #[ts(optional = nullable)]
    #[ts(type = "number | null")]
    pub expected_revision: Option<i64>,
    pub policy: ReviewPolicyDocument,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ProjectReviewPolicyResponse {
    #[ts(type = "number")]
    pub version: i64,
    pub policy: Option<ReviewPolicyDocument>,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewRangeResponse {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    #[ts(type = "number")]
    pub repository_set_revision: i64,
    pub base_commit: String,
    pub head_commit: String,
    pub base_tree: String,
    pub head_tree: String,
    pub range_digest: String,
}

/// The exact range subject of a reviewer WorkUnit.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkUnitReviewRangeResponse {
    pub review_work_unit_id: String,
    pub checkpoint_id: String,
    pub range: ReviewRangeResponse,
    pub input_finding_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewSeverityCounts {
    pub p0: u32,
    pub p1: u32,
    pub p2: u32,
    pub p3: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewCoverageReportRef {
    pub report_artifact_id: String,
    pub actor: ActorRef,
    pub verdict: ReviewReportVerdict,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewCoverageRangeResponse {
    pub range_id: String,
    pub base_commit: String,
    pub head_commit: String,
    pub valid_edge: bool,
    pub reports: Vec<ReviewCoverageReportRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct RepoReviewCoverageResponse {
    pub repo_id: String,
    pub status: ReviewCoverageStatus,
    pub reason_code: Option<String>,
    #[ts(type = "number")]
    pub revision: i64,
    pub base_commit: Option<String>,
    pub candidate_commit: Option<String>,
    pub frontier_commit: Option<String>,
    pub unreviewed_commits: u32,
    pub ranges: Vec<ReviewCoverageRangeResponse>,
    pub open_findings: ReviewSeverityCounts,
    pub open_finding_ids: Vec<String>,
    pub active_checkpoint_id: Option<String>,
    pub snapshot_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReviewCoverageResponse {
    pub task_id: String,
    #[ts(type = "number | null")]
    pub review_policy_revision: Option<i64>,
    pub repositories: Vec<RepoReviewCoverageResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewCheckpointResponse {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub state: ReviewCheckpointState,
    pub reasons: Vec<ReviewCheckpointReason>,
    #[ts(type = "number")]
    pub attempt: i64,
    pub base_commit: Option<String>,
    pub head_commit: String,
    pub range_id: Option<String>,
    pub review_work_unit_ids: Vec<String>,
    pub blocked_code: Option<String>,
    pub blocked_commits: Vec<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ListReviewCheckpointsResponse {
    pub items: Vec<ReviewCheckpointResponse>,
    pub next_cursor: Option<String>,
}

/// Request a range review. Forge resolves both commits; callers cannot.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateReviewCheckpointRequest {
    pub repo_id: String,
    pub reason: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RetryReviewCheckpointRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewFindingLocationResponse {
    pub path: String,
    #[ts(type = "number | null")]
    pub line_start: Option<i64>,
    #[ts(type = "number | null")]
    pub line_end: Option<i64>,
    pub attribution_kind: ReviewAttributionKind,
    pub work_unit_id: Option<String>,
    pub execution_id: Option<String>,
    pub actor: Option<ActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewFindingResolutionResponse {
    pub id: String,
    pub outcome: ReviewResolutionOutcome,
    pub anchor_commit: String,
    pub report_artifact_id: Option<String>,
    pub decision_id: Option<String>,
    pub reference: Option<String>,
    pub actor: ActorRef,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewFindingResponse {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub range_id: String,
    pub report_artifact_id: String,
    pub label: String,
    pub severity: ReviewFindingSeverity,
    pub title: String,
    pub evidence: String,
    pub invariant: Option<String>,
    pub anchor_commit: String,
    /// Evaluated against the current candidate commit.
    pub open: bool,
    pub locations: Vec<ReviewFindingLocationResponse>,
    pub disposition: Option<TaskReviewDisposition>,
    pub correction_work_unit_ids: Vec<String>,
    pub resolutions: Vec<ReviewFindingResolutionResponse>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ListReviewFindingsResponse {
    pub items: Vec<ReviewFindingResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WaiveReviewFindingRequest {
    #[ts(type = "number")]
    pub expected_coverage_revision: i64,
    pub rationale: String,
    pub reference: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RebaselineReviewRequest {
    pub repo_id: String,
    #[ts(type = "number")]
    pub expected_coverage_revision: i64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewBaseResponse {
    pub task_id: String,
    pub repo_id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub base_commit: String,
    pub source: ReviewBaseSource,
    pub decision_id: Option<String>,
    pub pinned_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ReviewCommitAttributionInput {
    pub commit_sha: String,
    pub actor: ActorRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AttributeReviewCommitsRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub attributions: Vec<ReviewCommitAttributionInput>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct StructuredReviewLocation {
    pub path: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    #[ts(type = "number | null")]
    pub line_start: Option<i64>,
    #[serde(default)]
    #[ts(optional = nullable)]
    #[ts(type = "number | null")]
    pub line_end: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct StructuredReviewFinding {
    pub label: String,
    pub severity: ReviewFindingSeverity,
    pub title: String,
    pub evidence: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub invariant: Option<String>,
    pub locations: Vec<StructuredReviewLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ReviewResolutionInput {
    pub finding_id: String,
    pub outcome: ReviewResolutionClaim,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PlanReviewCheckpoint {
    pub key: String,
    pub after_work_unit_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<String>>", optional)]
    pub repository_ids: Vec<String>,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use serde::{de::DeserializeOwned, Serialize};
    use serde_json::{json, Value};

    use super::*;
    use crate::{
        ProposalTargetKind, PublicEventType, SubmitReviewReportRequest,
        TaskCoordinationPlanResponse, UpdateProjectRequest,
    };

    fn round_trip<T>(wire: Value)
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let parsed: T = serde_json::from_value(wire.clone()).expect("deserialize wire value");
        let encoded = serde_json::to_value(&parsed).expect("serialize wire value");
        assert_eq!(encoded, wire);
        assert_eq!(serde_json::from_value::<T>(encoded).unwrap(), parsed);
    }

    fn wire_strings<T>(values: &[&str])
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        for value in values {
            round_trip::<T>(json!(value));
        }
    }

    fn policy_wire() -> Value {
        json!({
            "schema_version": 1,
            "risk_class": "r1",
            "coverage_required": true,
            "trigger_mode": "auto",
            "reviewer_role": "reviewer",
            "required_reviewers": 1,
            "require_human": false,
            "size_threshold": {"changed_lines": 800, "changed_files": 25},
            "coordinator_may_request": true,
            "max_checkpoints_per_repo": 12
        })
    }

    #[test]
    fn review_range_enums_use_frozen_wire_strings() {
        wire_strings::<ReviewRiskClass>(&["r0", "r1", "r2"]);
        wire_strings::<ReviewTriggerMode>(&["auto", "manual"]);
        wire_strings::<ReviewFindingSeverity>(&["P0", "P1", "P2", "P3"]);
        wire_strings::<ReviewCheckpointState>(&[
            "queued",
            "pending",
            "in_review",
            "reviewed",
            "blocked",
            "superseded",
        ]);
        wire_strings::<ReviewCheckpointReason>(&[
            "baseline",
            "delta",
            "after_work_unit",
            "size_threshold",
            "requested",
            "final",
            "recheck",
        ]);
        wire_strings::<ReviewCoverageStatus>(&[
            "not_required",
            "unchanged",
            "uncovered",
            "partial",
            "in_review",
            "findings_open",
            "history_rewritten",
            "blocked",
            "satisfied",
        ]);
        wire_strings::<ReviewResolutionOutcome>(&["fixed", "not_reproduced", "waived"]);
        wire_strings::<ReviewResolutionClaim>(&["fixed", "not_reproduced", "still_open"]);
        wire_strings::<ReviewAttributionKind>(&[
            "work_unit",
            "preexisting",
            "upstream",
            "unattributed",
        ]);
        wire_strings::<ReviewPolicySource>(&["project_default", "owner"]);
        wire_strings::<ReviewBaseSource>(&[
            "first_integration",
            "workspace_prepare",
            "human_rebaseline",
        ]);
        wire_strings::<TaskReviewDisposition>(&["correct", "recheck", "human", "defer"]);
        wire_strings::<ProposalTargetKind>(&["review_finding", "review_base", "review_commit"]);
    }

    #[test]
    fn review_range_policy_requests_round_trip_and_reject_unknown_fields() {
        round_trip::<ReviewPolicyDocument>(policy_wire());
        round_trip::<SetTaskReviewPolicyRequest>(json!({
            "expected_revision": 2,
            "policy": policy_wire()
        }));
        let first: SetTaskReviewPolicyRequest =
            serde_json::from_value(json!({"policy": policy_wire()})).unwrap();
        assert_eq!(first.expected_revision, None);

        let mut forged = policy_wire();
        forged["extra"] = json!(true);
        assert!(serde_json::from_value::<ReviewPolicyDocument>(forged).is_err());
        let mut forged = policy_wire();
        forged["size_threshold"]["extra"] = json!(1);
        assert!(serde_json::from_value::<ReviewPolicyDocument>(forged).is_err());
    }

    #[test]
    fn review_range_requests_reject_caller_selected_commits() {
        let checkpoint = json!({
            "repo_id": "repo-1",
            "reason": "contract changed",
            "idempotency_key": "request-1"
        });
        round_trip::<CreateReviewCheckpointRequest>(checkpoint.clone());
        let mut forged = checkpoint;
        forged["head_commit"] = json!("caller-selected");
        assert!(serde_json::from_value::<CreateReviewCheckpointRequest>(forged).is_err());

        let rebaseline = json!({
            "repo_id": "repo-1",
            "expected_coverage_revision": 4,
            "rationale": "history rewritten"
        });
        round_trip::<RebaselineReviewRequest>(rebaseline.clone());
        let mut forged = rebaseline;
        forged["base_commit"] = json!("caller-selected");
        assert!(serde_json::from_value::<RebaselineReviewRequest>(forged).is_err());

        round_trip::<RetryReviewCheckpointRequest>(json!({"expected_version": 3}));
        round_trip::<WaiveReviewFindingRequest>(json!({
            "expected_coverage_revision": 4,
            "rationale": "accepted",
            "reference": "ticket-1"
        }));
        round_trip::<AttributeReviewCommitsRequest>(json!({
            "expected_version": 2,
            "attributions": [
                {"commit_sha": "abc", "actor": {"kind": "human", "id": "user-1"}}
            ],
            "rationale": "manual commit"
        }));
    }

    #[test]
    fn review_range_report_request_extensions_round_trip_and_stay_omitted() {
        let result_report = json!({
            "verdict": "pass",
            "summary": "ok",
            "criteria": [],
            "findings": [],
            "questions": [],
            "evidence_ids": [],
            "artifact_ids": [],
            "reviewed_work_unit_id": null,
            "reviewed_work_unit_version": null
        });
        round_trip::<SubmitReviewReportRequest>(result_report.clone());

        let mut range_report = result_report;
        range_report["structured_findings"] = json!([{
            "label": "F-1",
            "severity": "P1",
            "title": "tenant scope",
            "evidence": "missing filter",
            "invariant": null,
            "locations": [{"path": "src/a.rs", "line_start": 120, "line_end": 131}]
        }]);
        range_report["resolutions"] = json!([{
            "finding_id": "finding-1",
            "outcome": "still_open",
            "evidence": "unchanged"
        }]);
        round_trip::<SubmitReviewReportRequest>(range_report.clone());

        range_report["resolutions"][0]["outcome"] = json!("waived");
        assert!(serde_json::from_value::<SubmitReviewReportRequest>(range_report).is_err());
        assert!(serde_json::from_value::<StructuredReviewFinding>(json!({
            "label": "F-1",
            "severity": "P1",
            "title": "t",
            "evidence": "e"
        }))
        .is_err());
    }

    #[test]
    fn review_range_plan_checkpoint_omits_empty_repository_ids() {
        round_trip::<PlanReviewCheckpoint>(json!({
            "key": "after-contract",
            "after_work_unit_keys": ["contract"],
            "reason": "dependents build on the contract"
        }));
        round_trip::<PlanReviewCheckpoint>(json!({
            "key": "after-contract",
            "after_work_unit_keys": ["contract"],
            "repository_ids": ["repo-1"],
            "reason": "dependents build on the contract"
        }));

        let plan = json!({
            "artifact_id": "artifact-1",
            "task_id": "task-1",
            "request_id": "request-1",
            "mode": "proposal",
            "state": "proposed",
            "authorization_version": 1,
            "task_version": 1,
            "digest": "d",
            "team_digest": "t",
            "envelope_digest": "e",
            "objective_digest": "o",
            "work_units": [],
            "proposal_id": null,
            "decision_id": null,
            "materialization": null,
            "version": 1,
            "created_at": "2026-11-26T00:00:00Z",
            "updated_at": "2026-11-26T00:00:00Z"
        });
        round_trip::<TaskCoordinationPlanResponse>(plan);
    }

    #[test]
    fn review_range_project_default_distinguishes_absent_clear_and_set() {
        let absent: UpdateProjectRequest = serde_json::from_value(json!({"version": 1})).unwrap();
        assert_eq!(absent.default_review_policy, None);
        assert!(serde_json::to_value(&absent)
            .unwrap()
            .get("default_review_policy")
            .is_none());

        let clear: UpdateProjectRequest =
            serde_json::from_value(json!({"version": 1, "default_review_policy": null})).unwrap();
        assert_eq!(clear.default_review_policy, Some(None));
        assert_eq!(
            serde_json::to_value(&clear).unwrap()["default_review_policy"],
            Value::Null
        );

        let set: UpdateProjectRequest =
            serde_json::from_value(json!({"version": 1, "default_review_policy": policy_wire()}))
                .unwrap();
        assert!(matches!(set.default_review_policy, Some(Some(_))));
        assert_eq!(
            serde_json::to_value(&set).unwrap()["default_review_policy"],
            policy_wire()
        );
    }

    #[test]
    fn review_range_responses_round_trip() {
        round_trip::<TaskReviewPolicyResponse>(json!({
            "task_id": "task-1",
            "policy": {
                "revision": 1,
                "digest": "d",
                "source": "owner",
                "policy": policy_wire(),
                "created_by_user_id": "user-1",
                "created_at": "2026-11-26T00:00:00Z"
            }
        }));
        round_trip::<ProjectReviewPolicyResponse>(json!({
            "version": 2,
            "policy": null,
            "digest": null
        }));
        round_trip::<WorkUnitReviewRangeResponse>(json!({
            "review_work_unit_id": "work-unit-1",
            "checkpoint_id": "checkpoint-1",
            "range": {
                "id": "range-1",
                "task_id": "task-1",
                "repo_id": "repo-1",
                "repository_set_revision": 1,
                "base_commit": "a",
                "head_commit": "b",
                "base_tree": "c",
                "head_tree": "d",
                "range_digest": "e"
            },
            "input_finding_ids": ["finding-1"]
        }));
        round_trip::<TaskReviewCoverageResponse>(json!({
            "task_id": "task-1",
            "review_policy_revision": 1,
            "repositories": [{
                "repo_id": "repo-1",
                "status": "partial",
                "reason_code": null,
                "revision": 3,
                "base_commit": "a",
                "candidate_commit": "c",
                "frontier_commit": "b",
                "unreviewed_commits": 1,
                "ranges": [{
                    "range_id": "range-1",
                    "base_commit": "a",
                    "head_commit": "b",
                    "valid_edge": true,
                    "reports": [{
                        "report_artifact_id": "artifact-1",
                        "actor": {"kind": "agent", "id": "agent-1"},
                        "verdict": "request_changes"
                    }]
                }],
                "open_findings": {"p0": 0, "p1": 1, "p2": 0, "p3": 0},
                "open_finding_ids": ["finding-1"],
                "active_checkpoint_id": null,
                "snapshot_digest": "s"
            }]
        }));
        round_trip::<ListReviewCheckpointsResponse>(json!({
            "items": [{
                "id": "checkpoint-1",
                "task_id": "task-1",
                "repo_id": "repo-1",
                "state": "blocked",
                "reasons": ["baseline", "size_threshold"],
                "attempt": 1,
                "base_commit": null,
                "head_commit": "b",
                "range_id": null,
                "review_work_unit_ids": [],
                "blocked_code": "review.range.unrecorded_commits",
                "blocked_commits": ["x"],
                "version": 2,
                "created_at": "2026-11-26T00:00:00Z",
                "updated_at": "2026-11-26T00:00:00Z"
            }],
            "next_cursor": null
        }));
        round_trip::<ListReviewFindingsResponse>(json!({
            "items": [{
                "id": "finding-1",
                "task_id": "task-1",
                "repo_id": "repo-1",
                "range_id": "range-1",
                "report_artifact_id": "artifact-1",
                "label": "F-1",
                "severity": "P2",
                "title": "t",
                "evidence": "e",
                "invariant": null,
                "anchor_commit": "b",
                "open": false,
                "locations": [{
                    "path": "src/a.rs",
                    "line_start": 1,
                    "line_end": 2,
                    "attribution_kind": "work_unit",
                    "work_unit_id": "work-unit-1",
                    "execution_id": "execution-1",
                    "actor": {"kind": "agent", "id": "agent-1"}
                }],
                "disposition": "defer",
                "correction_work_unit_ids": [],
                "resolutions": [{
                    "id": "resolution-1",
                    "outcome": "waived",
                    "anchor_commit": "b",
                    "report_artifact_id": null,
                    "decision_id": "decision-1",
                    "reference": "ticket-1",
                    "actor": {"kind": "human", "id": "user-1"},
                    "created_at": "2026-11-26T00:00:00Z"
                }],
                "created_at": "2026-11-26T00:00:00Z"
            }],
            "next_cursor": "cursor-1"
        }));
        round_trip::<ReviewBaseResponse>(json!({
            "task_id": "task-1",
            "repo_id": "repo-1",
            "revision": 2,
            "base_commit": "a",
            "source": "human_rebaseline",
            "decision_id": "decision-1",
            "pinned_at": "2026-11-26T00:00:00Z"
        }));
    }

    #[test]
    fn review_range_public_event_names_match_serde() {
        for (event, name) in [
            (
                PublicEventType::TaskReviewPolicyUpdated,
                "task.review_policy_updated",
            ),
            (
                PublicEventType::ReviewCheckpointCreated,
                "review.checkpoint_created",
            ),
            (
                PublicEventType::ReviewCheckpointStateChanged,
                "review.checkpoint_state_changed",
            ),
            (
                PublicEventType::ReviewRangeReported,
                "review.range_reported",
            ),
            (
                PublicEventType::ReviewFindingsTriaged,
                "review.findings_triaged",
            ),
            (
                PublicEventType::ReviewFindingResolved,
                "review.finding_resolved",
            ),
            (
                PublicEventType::ReviewCoverageChanged,
                "review.coverage_changed",
            ),
        ] {
            assert_eq!(event.as_str(), name);
            assert_eq!(serde_json::to_value(event).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<PublicEventType>(json!(name)).unwrap(),
                event
            );
        }
    }
}
