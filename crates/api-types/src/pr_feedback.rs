use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PrFeedbackAuthorization {
    HumanApproval,
    Automatic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PrFeedbackSummaryMode {
    HumanApproval,
    Automatic,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct PrFeedbackPolicy {
    #[ts(type = "number")]
    pub schema_version: i64,
    pub ingest: bool,
    pub trusted_author_associations: Vec<String>,
    pub trusted_logins: Vec<String>,
    #[ts(type = "number")]
    pub max_items_per_poll: i64,
    pub push_corrections: PrFeedbackAuthorization,
    pub post_summary: PrFeedbackSummaryMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct PrFeedbackPolicyResponse {
    pub policy: PrFeedbackPolicy,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdatePrFeedbackPolicyRequest {
    #[ts(optional = nullable)]
    pub expected_updated_at: Option<String>,
    pub policy: PrFeedbackPolicy,
}

/// Feedback body is visible only on a Task read. SSE and MCP events publish
/// identifiers, digests and states without this untrusted provider text.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ExternalFeedbackItemResponse {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub provider_item_id: String,
    pub kind: String,
    pub author_login: String,
    pub author_association: String,
    pub review_state: Option<String>,
    pub path: Option<String>,
    #[ts(type = "number | null")]
    pub line: Option<i64>,
    pub commit_sha: Option<String>,
    pub body: String,
    pub body_truncated: bool,
    pub body_digest: String,
    pub provider_url: Option<String>,
    pub trust_class: String,
    pub head_state: String,
    pub disposition: Option<String>,
    pub dismiss_reason: Option<String>,
    pub scope_classification: Option<String>,
    pub human_decision_id: Option<String>,
    pub followup_task_id: Option<String>,
    pub ingested_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ExternalFeedbackItemListResponse {
    pub items: Vec<ExternalFeedbackItemResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct PrReworkSupersessionResponse {
    pub id: String,
    pub task_id: String,
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub metadata_id: String,
    pub request_id: String,
    pub triage_set_digest: String,
    pub provider_pr_id: String,
    pub superseded_head_sha: String,
    pub state: String,
    pub readmission_merge_operation_id: Option<String>,
    pub convergence_decision_id: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct PrReworkListResponse {
    pub items: Vec<PrReworkSupersessionResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct PrExternalActionResponse {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub kind: String,
    pub state: String,
    pub authorization_mode: String,
    pub gate_evaluation_id: Option<String>,
    pub subject_commit_sha: Option<String>,
    pub expected_remote_head_sha: Option<String>,
    pub summary_artifact_id: Option<String>,
    pub body_digest: Option<String>,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct PrExternalActionListResponse {
    pub items: Vec<PrExternalActionResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DecidePrExternalActionRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    #[ts(optional = nullable)]
    pub note: Option<String>,
    pub idempotency_key: String,
}
