use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationCheckState {
    Active,
    Retired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationRunOrigin {
    LegacyCommand,
    CoordinatorRequest,
    AfterIntegration,
    Baseline,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationCheckRequestTrigger {
    CoordinatorRequest,
    AfterIntegration,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationCheckSubjectKind {
    WorkUnitResult,
    TaskIntegration,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationCheckRequestState {
    Queued,
    Running,
    Completed,
    Cancelled,
    FailedToStart,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationComparisonClass {
    Regression,
    PreExistingFailure,
    Fixed,
    BothPass,
    Inconclusive,
}

/// One immutable version of a Project (optionally Repo) validation check.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ValidationCheckDefinitionResponse {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub check_key: String,
    #[ts(type = "number")]
    pub version: i64,
    pub command: String,
    #[ts(type = "number")]
    pub timeout_seconds: i64,
    pub env_policy: String,
    pub definition_digest: String,
    pub state: ValidationCheckState,
    pub created_by: String,
    pub created_at: String,
    pub retired_at: Option<String>,
}

/// Publishes the next exact version of `(project, repo, check_key)`. A replay
/// with identical content returns the active version instead of a new one.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateValidationCheckRequest {
    pub repo_id: Option<String>,
    pub check_key: String,
    pub command: String,
    #[ts(type = "number")]
    pub timeout_seconds: i64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RetireValidationCheckRequest {
    pub expected_state: ValidationCheckState,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ValidationCheckListResponse {
    pub items: Vec<ValidationCheckDefinitionResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ValidationCheckRequestResponse {
    pub id: String,
    pub task_id: String,
    #[ts(type = "number")]
    pub authorization_version: i64,
    pub check_definition_id: String,
    pub definition_digest: String,
    pub trigger_kind: ValidationCheckRequestTrigger,
    pub subject_kind: ValidationCheckSubjectKind,
    pub subject_id: String,
    #[ts(type = "number")]
    pub subject_version: i64,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub baseline: bool,
    pub baseline_commit_sha: Option<String>,
    pub requested_by_request_id: Option<String>,
    pub requested_by_execution_id: Option<String>,
    pub reason: Option<String>,
    pub state: ValidationCheckRequestState,
    pub candidate_run_id: Option<String>,
    pub failure_code: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ValidationCheckRequestListResponse {
    pub items: Vec<ValidationCheckRequestResponse>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ValidationComparisonResponse {
    pub id: String,
    pub task_id: String,
    pub check_request_id: String,
    pub candidate_run_id: String,
    pub candidate_status: crate::ValidationRunStatus,
    pub candidate_output_digest: Option<String>,
    pub baseline_commit_sha: String,
    pub baseline_snapshot_digest: String,
    #[ts(type = "Record<string, unknown>")]
    pub baseline_provenance: Value,
    pub baseline_status: crate::ValidationRunStatus,
    #[ts(type = "number | null")]
    pub baseline_exit_code: Option<i64>,
    pub baseline_output_digest: Option<String>,
    pub classification: ValidationComparisonClass,
    pub same_output_digest: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ValidationComparisonListResponse {
    pub items: Vec<ValidationComparisonResponse>,
    pub next_cursor: Option<String>,
}
