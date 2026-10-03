use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskGateRequest {
    pub gate_kind: String,
    #[ts(type = "Record<string, unknown>")]
    pub policy: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ReviseGatePolicyRequest {
    pub expected_active_revision: Option<i64>,
    #[ts(type = "Record<string, unknown>")]
    pub policy: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskGateResponse {
    pub id: String,
    pub task_id: String,
    pub gate_kind: String,
    pub scope_kind: String,
    pub scope_id: String,
    pub active_policy_revision: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskLifecycleState {
    Backlog,
    Ready,
    Active,
    Blocked,
    ReadyToMerge,
    Merging,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct TaskLifecycleResponse {
    pub task_id: String,
    pub state: TaskLifecycleState,
    pub version: i64,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq)]
#[ts(export)]
pub struct GatePolicyRevisionResponse {
    pub gate_id: String,
    pub revision: i64,
    pub schema_version: i64,
    #[ts(type = "Record<string, unknown>")]
    pub policy: Value,
    pub policy_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq)]
#[ts(export)]
pub struct GateResponse {
    pub gate: TaskGateResponse,
    pub active_policy: Option<GatePolicyRevisionResponse>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct GateEvaluationInputResponse {
    pub ordinal: i64,
    pub input_kind: String,
    pub input_id: String,
    pub input_version: i64,
    pub input_digest: String,
    pub producer_ref: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub subject: Value,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq)]
#[ts(export)]
pub struct GateEvaluationResponse {
    pub id: String,
    pub gate_id: String,
    pub task_id: String,
    pub policy_revision: i64,
    pub outcome: String,
    pub input_digest: String,
    #[ts(type = "Record<string, unknown>")]
    pub result: Value,
    pub evaluated_at: String,
    pub inputs: Vec<GateEvaluationInputResponse>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct MergeAfterGateRequest {
    pub gate_evaluation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct MergeAfterGateResponse {
    pub outcome: String,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub target_branch: Option<String>,
    pub details: Option<String>,
    pub files: Vec<String>,
}
