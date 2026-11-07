use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ActorRef, TaskLifecycleResponse, WorkUnitReadinessResponse, WorkUnitStatus};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskDispatchState {
    Idle,
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskDispatchIntentResponse {
    pub task_id: String,
    pub state: TaskDispatchState,
    #[ts(type = "number")]
    pub version: i64,
    pub requested_by: Option<ActorRef>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskLaunchBlockerResponse {
    pub code: String,
    pub message: String,
    pub work_unit_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkUnitDispatchResponse {
    pub work_unit_id: String,
    pub title: String,
    pub status: WorkUnitStatus,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub readiness: WorkUnitReadinessResponse,
    pub state: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskLaunchPreflightResponse {
    pub task_id: String,
    #[ts(type = "number")]
    pub task_version: i64,
    pub lifecycle: TaskLifecycleResponse,
    pub dispatch: TaskDispatchIntentResponse,
    pub can_launch: bool,
    pub can_pause: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub blockers: Vec<TaskLaunchBlockerResponse>,
    pub work_units: Vec<WorkUnitDispatchResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskDispatchActionRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_lifecycle_version: i64,
    pub idempotency_key: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskDispatchActionResponse {
    pub task: crate::TaskResponse,
    pub dispatch: TaskDispatchIntentResponse,
    pub preflight: TaskLaunchPreflightResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskLaunchResponse {
    pub admitted: bool,
    pub dispatch: TaskDispatchIntentResponse,
    pub preflight: TaskLaunchPreflightResponse,
    /// Executions admitted by the launch request or by its dispatcher tick.
    pub admitted_execution_ids: Vec<String>,
    /// Executions whose external provider start was accepted in this request.
    pub started_execution_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RequestReworkRequest {
    #[ts(type = "number")]
    pub expected_task_version: i64,
    #[ts(type = "number")]
    pub expected_lifecycle_version: i64,
    /// Exact ReviewReport Artifact whose verdict requested changes.
    pub review_report_artifact_id: String,
    /// WorkUnit revision the report reviewed.
    pub reviewed_work_unit_id: String,
    #[ts(type = "number")]
    pub reviewed_work_unit_version: i64,
    pub correction_title: String,
    pub correction_scope: String,
    pub correction_role_id: String,
    pub correction_actor: Option<ActorRef>,
    pub review_title: String,
    pub review_scope: String,
    pub review_actor: Option<ActorRef>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct RequestReworkResponse {
    pub correction_work_unit: crate::WorkUnitResponse,
    pub review_work_unit: crate::WorkUnitResponse,
    pub review_subject: Option<ReviewSubjectResponse>,
    #[ts(type = "number")]
    pub rounds_remaining: i64,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskReworkAutomationState {
    Pending,
    Completed,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReworkAutomationIntentResponse {
    pub id: String,
    pub review_report_artifact_id: String,
    pub reviewed_work_unit_id: String,
    #[ts(type = "number")]
    pub reviewed_work_unit_version: i64,
    pub state: TaskReworkAutomationState,
    pub block_reason: Option<String>,
    pub correction_work_unit_id: Option<String>,
    pub review_work_unit_id: Option<String>,
    /// Exact manual receipt used to reconcile this automatic intent, when one
    /// was committed under a different idempotency key.
    pub reconciled_manual_receipt_id: Option<String>,
    #[ts(type = "number | null")]
    pub rework_round: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskReworkAutomationIntentsResponse {
    pub items: Vec<TaskReworkAutomationIntentResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewSubjectResponse {
    /// Primary key of the durable review-subject relation (the review WorkUnit id).
    pub id: String,
    pub review_work_unit_id: String,
    pub reviewed_work_unit_id: String,
    #[ts(type = "number | null")]
    pub reviewed_work_unit_version: Option<i64>,
    pub result_execution_id: Option<String>,
    pub result_artifact_id: Option<String>,
    pub result_commit_sha: Option<String>,
    pub result_workspace_id: Option<String>,
    pub workspace_snapshot_digest: Option<String>,
}
