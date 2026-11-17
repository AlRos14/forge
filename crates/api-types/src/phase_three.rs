use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PhaseThreeActorKind {
    Human,
    Agent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CostClass {
    Economy,
    Standard,
    Premium,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MicroFixSourceKind {
    ReviewFindings,
    ValidationFindings,
    ExternalFeedback,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MicroFixBoundsOutcome {
    Pending,
    WithinBounds,
    BoundsExceeded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MicroFixBoundsViolation {
    PathsOutsideDeclared,
    ForbiddenGlob,
    TooManyFiles,
    TooManyLines,
    NewDependency,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CoordinatorReplanClassification {
    WithinEnvelope,
    ScopeOrAuthorityChange,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CoordinatorReplanState {
    Proposed,
    Planning,
    AwaitingHuman,
    Approved,
    Rejected,
    Materialized,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkUnitFallbackReceiptResponse {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    #[ts(type = "number")]
    pub work_unit_version_before: i64,
    #[ts(type = "number")]
    pub work_unit_version_after: i64,
    pub origin_execution_id: String,
    /// Durable failure class which caused the fallback decision.
    pub failure_class: String,
    pub route_attempts_digest: String,
    pub task_role_id: String,
    pub from_actor_kind: PhaseThreeActorKind,
    pub from_actor_id: String,
    pub to_actor_kind: PhaseThreeActorKind,
    pub to_actor_id: String,
    #[ts(type = "number")]
    pub chain_index: i64,
    pub cost_class: CostClass,
    #[ts(type = "number")]
    pub authorization_version: i64,
    pub envelope_digest: String,
    pub new_execution_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkUnitFallbackReceiptListResponse {
    pub items: Vec<WorkUnitFallbackReceiptResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct MicroFixReceiptResponse {
    pub id: String,
    pub task_id: String,
    pub request_id: String,
    pub source_kind: MicroFixSourceKind,
    pub finding_refs: Vec<String>,
    pub declared_paths: Vec<String>,
    pub fix_work_unit_id: String,
    pub reviewed_commit_sha: String,
    pub result_commit_sha: Option<String>,
    #[ts(type = "number | null")]
    pub changed_files: Option<i64>,
    #[ts(type = "number | null")]
    pub changed_lines: Option<i64>,
    pub bounds_outcome: MicroFixBoundsOutcome,
    pub bounds_violation: Option<MicroFixBoundsViolation>,
    pub reviewer_cost_class: Option<CostClass>,
    pub reviewer_actor_kind: Option<PhaseThreeActorKind>,
    pub reviewer_actor_id: Option<String>,
    pub envelope_digest: String,
    #[ts(type = "number")]
    pub ordinal: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct MicroFixReceiptListResponse {
    pub items: Vec<MicroFixReceiptResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct CoordinatorReplanResponse {
    pub id: String,
    pub task_id: String,
    #[ts(type = "number")]
    pub ordinal: i64,
    #[ts(type = "number")]
    pub authorization_version: i64,
    pub source_request_id: String,
    pub producer_execution_id: String,
    pub plan_request_id: Option<String>,
    pub reason: String,
    #[ts(type = "unknown")]
    pub evidence_links: Value,
    pub classification: Option<CoordinatorReplanClassification>,
    #[ts(type = "unknown | null")]
    pub classification_facts: Option<Value>,
    pub human_decision_id: Option<String>,
    pub state: CoordinatorReplanState,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct CoordinatorReplanListResponse {
    pub items: Vec<CoordinatorReplanResponse>,
    pub next_cursor: Option<String>,
}
