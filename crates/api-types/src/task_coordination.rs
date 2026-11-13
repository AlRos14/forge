use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{
    ActorRef, TaskDispatchIntentResponse, TaskLifecycleResponse, TaskRoleCapability,
    TaskRoleConfiguration,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskCoordinationMode {
    Manual,
    Proposal,
    PrepareAndExecute,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskCoordinationRequestKind {
    WorkUnitPlan,
    ReviewFindings,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskCoordinationRequestState {
    Requested,
    Running,
    PlanReady,
    AwaitingApproval,
    Materialized,
    Triaged,
    Failed,
    Cancelled,
    Uncertain,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskCoordinationPlanState {
    Proposed,
    Approved,
    Rejected,
    Superseded,
    Materialized,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskQualityCategory {
    DeliveryCorrection,
    PlanRevision,
    Specialist,
    HumanDecision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskQualityTargetKind {
    Implementation,
    Orchestrator,
    Specialist,
    Human,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskQualityState {
    Requested,
    Notified,
    WorkCreated,
    Running,
    Resolved,
    RequiresHuman,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskInterventionKind {
    Instruction,
    Decision,
    Pause,
    Resume,
    Cancel,
    RevokeAuthorization,
    RequestReplan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskInterventionState {
    Recorded,
    Applied,
    RequiresHuman,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskAutonomyLimits {
    #[ts(type = "number")]
    pub version: i64,
    #[ts(type = "number")]
    pub max_turns: i64,
    #[ts(type = "number")]
    pub max_quality_requests: i64,
    #[ts(type = "number")]
    pub max_rework_rounds: i64,
    #[ts(type = "number")]
    pub max_route_requests: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateTaskAutonomyLimitsRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub limits: TaskAutonomyLimits,
    pub reason: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskReviewDisposition {
    Correct,
    Recheck,
    Human,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReviewTriageResponse {
    pub review_report_artifact_id: String,
    pub execution_id: String,
    pub disposition: TaskReviewDisposition,
    pub rationale: String,
    pub instructions: String,
    pub review_work_unit_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationCoordinatorResponse {
    pub task_role_id: String,
    pub role: String,
    pub actor_ref: ActorRef,
    pub display_name: String,
    pub harness: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationTeamMemberResponse {
    pub id: String,
    pub actor_ref: ActorRef,
    pub display_name: String,
    pub harness: Option<String>,
    pub status: String,
    #[ts(type = "number")]
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationTeamRoleResponse {
    pub id: String,
    pub role: String,
    pub display_label: Option<String>,
    pub coordination_mode: Option<crate::CoordinationMode>,
    pub capabilities: Vec<TaskRoleCapability>,
    pub configuration: TaskRoleConfiguration,
    #[ts(type = "number")]
    pub version: i64,
    pub members: Vec<TaskCoordinationTeamMemberResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationBlockerResponse {
    pub code: String,
    pub message: String,
    pub task_role_id: Option<String>,
    pub actor_ref: Option<ActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationBriefResponse {
    pub objective: String,
    pub constraints: Option<String>,
    pub acceptance_criteria: Vec<String>,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationRequestResponse {
    pub id: String,
    pub task_id: String,
    pub kind: TaskCoordinationRequestKind,
    pub review_report_artifact_id: Option<String>,
    pub review_report_digest: Option<String>,
    pub review_producer_execution_id: Option<String>,
    pub mode: TaskCoordinationMode,
    pub state: TaskCoordinationRequestState,
    #[ts(type = "number")]
    pub authorization_version: i64,
    pub objective_digest: String,
    pub request_digest: String,
    pub brief: TaskCoordinationBriefResponse,
    pub wake_id: Option<String>,
    pub execution_id: Option<String>,
    pub plan_artifact_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationPlanWorkUnitResponse {
    pub key: String,
    pub title: String,
    pub scope: String,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub parent_key: Option<String>,
    pub depends_on: Vec<String>,
    pub requires_integration: bool,
    pub execution_instructions: Option<String>,
    pub review_subject_key: Option<String>,
    pub acceptance: Vec<String>,
    pub artifact_inputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationMaterializationResponse {
    pub id: String,
    pub plan_artifact_id: String,
    pub plan_digest: String,
    pub work_unit_ids: Vec<String>,
    pub receipt_digest: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationPlanResponse {
    pub artifact_id: String,
    pub task_id: String,
    pub request_id: String,
    pub mode: TaskCoordinationMode,
    pub state: TaskCoordinationPlanState,
    #[ts(type = "number")]
    pub authorization_version: i64,
    #[ts(type = "number")]
    pub task_version: i64,
    pub digest: String,
    pub team_digest: String,
    pub envelope_digest: String,
    pub objective_digest: String,
    pub work_units: Vec<TaskCoordinationPlanWorkUnitResponse>,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    pub materialization: Option<TaskCoordinationMaterializationResponse>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskCoordinationProjectBatchScopeResponse {
    pub batch_id: String,
    pub batch_state: String,
    pub task_state: String,
    pub blocker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskCoordinationOverviewResponse {
    #[serde(default)]
    #[ts(optional)]
    pub review_triage: Option<Vec<TaskReviewTriageResponse>>,
    pub task_id: String,
    #[ts(type = "number")]
    pub task_version: i64,
    pub lifecycle: TaskLifecycleResponse,
    pub dispatch: TaskDispatchIntentResponse,
    pub mode: TaskCoordinationMode,
    #[ts(type = "number")]
    pub authorization_version: i64,
    pub revoked_at: Option<String>,
    pub revoke_reason: Option<String>,
    pub requested_by: Option<ActorRef>,
    pub coordinator: Option<TaskCoordinationCoordinatorResponse>,
    pub team: Vec<TaskCoordinationTeamRoleResponse>,
    pub team_digest: String,
    #[ts(type = "unknown")]
    pub envelope: Value,
    pub envelope_digest: String,
    #[ts(type = "number")]
    pub turn_count: i64,
    #[ts(type = "number")]
    pub max_turns: i64,
    #[ts(type = "number")]
    pub quality_request_count: i64,
    #[ts(type = "number")]
    pub max_quality_requests: i64,
    pub can_authorize: bool,
    pub can_request: bool,
    pub project_plan_batch_scope: Option<TaskCoordinationProjectBatchScopeResponse>,
    pub blockers: Vec<TaskCoordinationBlockerResponse>,
    pub latest_request: Option<TaskCoordinationRequestResponse>,
    pub latest_plan: Option<TaskCoordinationPlanResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AuthorizeTaskCoordinationRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_coordination_version: i64,
    pub mode: TaskCoordinationMode,
    pub coordinator_task_role_id: Option<String>,
    pub coordinator_actor: Option<ActorRef>,
    #[serde(default)]
    #[ts(optional = nullable, type = "unknown")]
    pub envelope: Option<Value>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RevokeTaskCoordinationRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_coordination_version: i64,
    pub idempotency_key: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskCoordinationTurnRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_coordination_version: i64,
    pub objective: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub constraints: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub context: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskCoordinationTurnResponse {
    pub request: TaskCoordinationRequestResponse,
    pub accepted: bool,
    pub blockers: Vec<TaskCoordinationBlockerResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ApproveTaskCoordinationPlanRequest {
    #[ts(type = "number")]
    pub expected_coordination_version: i64,
    pub expected_plan_digest: String,
    pub decision: TaskCoordinationPlanDecision,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub rationale: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskCoordinationPlanDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct MaterializeTaskCoordinationPlanRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_coordination_version: i64,
    pub expected_plan_digest: String,
    #[ts(type = "number")]
    pub expected_plan_version: i64,
    #[ts(type = "number")]
    pub expected_request_version: i64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct MaterializeTaskCoordinationPlanResponse {
    pub plan: TaskCoordinationPlanResponse,
    pub request: TaskCoordinationRequestResponse,
    pub materialization: TaskCoordinationMaterializationResponse,
    pub replayed: bool,
    pub launch_intent_accepted: bool,
    pub launch_blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskQualityRequest {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub source_artifact_id: Option<String>,
    /// Reviewer Execution that produced the exact ReviewReport. When present,
    /// the request is attributed to that Execution's Actor instead of the
    /// calling Human account.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub source_execution_id: Option<String>,
    pub category: TaskQualityCategory,
    pub target_kind: TaskQualityTargetKind,
    pub target_task_role_id: Option<String>,
    pub target_actor: Option<ActorRef>,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    pub resolution_condition: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskQualityRequestResponse {
    pub id: String,
    pub task_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub category: TaskQualityCategory,
    pub target_kind: TaskQualityTargetKind,
    pub target_task_role_id: Option<String>,
    pub target_actor: Option<ActorRef>,
    pub summary: String,
    pub findings: Vec<String>,
    pub resolution_condition: String,
    pub state: TaskQualityState,
    pub handoff_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub coordination_request_id: Option<String>,
    #[ts(type = "number")]
    pub budget_round: i64,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
    pub resolution_kind: Option<String>,
    pub resolution_ref: Option<String>,
    pub resolution_digest: Option<String>,
    pub quality_route: TaskQualityRouteResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskQualityRouteResponse {
    /// Stable identity for the review-line/source chain. It deliberately does
    /// not include quality category or target role.
    pub id: String,
    #[ts(type = "number")]
    pub round: i64,
    #[ts(type = "number")]
    pub limit: i64,
    /// Exact TaskRole that produced the source ReviewReport, when applicable.
    pub source_task_role_id: Option<String>,
    pub escalation: Option<TaskQualityEscalationResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskQualityEscalationResponse {
    pub reason_code: String,
    pub reason: String,
    /// Exact quality-request IDs participating in a detected role cycle.
    pub cycle_request_ids: Vec<String>,
    /// Exact TaskRole IDs in the detected cycle; role labels are not authority.
    pub cycle_task_role_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskQualityRequestsResponse {
    pub items: Vec<TaskQualityRequestResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ResolveTaskQualityRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub resolution: TaskQualityResolution,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskQualityResolution {
    Resolved,
    Cancelled,
    RequiresHuman,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskInterventionRequest {
    pub kind: TaskInterventionKind,
    #[ts(type = "number | null")]
    pub expected_task_version: Option<i64>,
    #[ts(type = "number | null")]
    pub expected_coordination_version: Option<i64>,
    #[ts(type = "number | null")]
    pub expected_lifecycle_version: Option<i64>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub instruction: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub plan_artifact_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub decision: Option<TaskCoordinationPlanDecision>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub rationale: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub reason: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub brief: Option<CreateTaskCoordinationTurnRequest>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskInterventionResponse {
    pub id: String,
    pub task_id: String,
    pub kind: TaskInterventionKind,
    pub state: TaskInterventionState,
    pub source_kind: String,
    pub source_id: String,
    pub expected_task_version: Option<i64>,
    pub expected_coordination_version: Option<i64>,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    pub coordination_request_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskInterventionsResponse {
    pub items: Vec<TaskInterventionResponse>,
}
