use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HumanDecisionState {
    Open,
    Answered,
    Superseded,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HumanDecisionSourceKind {
    CoordinationRequest,
    QualityRequest,
    GateEvaluation,
    ValidationRun,
    ExternalFeedback,
    CoordinatorReplan,
}

/// What an option may change. `scope` and `authority` need a Project
/// owner/admin to answer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AuthorityEffect {
    None,
    Scope,
    Authority,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct HumanDecisionOptionResponse {
    pub id: String,
    pub label: String,
    pub description: String,
    pub consequence: String,
    pub authority_effect: AuthorityEffect,
}

/// Exact evidence reference (kind, id, optional sha256 digest); never a body.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct HumanDecisionEvidenceLink {
    pub kind: String,
    pub id: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct HumanDecisionResponse {
    pub id: String,
    pub task_id: String,
    pub source_kind: HumanDecisionSourceKind,
    pub source_id: String,
    pub raised_by_execution_id: Option<String>,
    #[ts(type = "number | null")]
    pub authorization_version: Option<i64>,
    pub question: String,
    pub options: Vec<HumanDecisionOptionResponse>,
    pub recommended_option_id: Option<String>,
    pub recommendation_rationale: Option<String>,
    pub evidence_links: Vec<HumanDecisionEvidenceLink>,
    pub max_authority_effect: AuthorityEffect,
    pub state: HumanDecisionState,
    pub answered_by_user_id: Option<String>,
    pub answered_option_id: Option<String>,
    pub answer_note: Option<String>,
    pub answered_at: Option<String>,
    // The paused coordination request this answer resumes.
    pub resumes_request_id: Option<String>,
    // The `human_decision_resume` request created by the answer, if any yet.
    pub resume_request_id: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct HumanDecisionListResponse {
    pub items: Vec<HumanDecisionResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AnswerHumanDecisionRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub option_id: String,
    pub note: Option<String>,
    pub idempotency_key: String,
}

/// `created`: the exact resume request exists; `already_bound`: it already did;
/// `deferred`: the answer is durable and the reconcile scanner retries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HumanDecisionResumeStatus {
    Created,
    AlreadyBound,
    Deferred,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct HumanDecisionResumeResponse {
    pub status: HumanDecisionResumeStatus,
    pub request_id: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct AnswerHumanDecisionResponse {
    pub decision: HumanDecisionResponse,
    pub resume: HumanDecisionResumeResponse,
}
