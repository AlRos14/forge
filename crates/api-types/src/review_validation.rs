use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{ArtifactResponse, ExecutionResponse};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewReportVerdict {
    Pass,
    RequestChanges,
    Questions,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct StartReviewExecutionRequest {
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SubmitReviewReportRequest {
    pub verdict: ReviewReportVerdict,
    pub summary: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewExecutionResponse {
    #[ts(type = "import('../api').Execution")]
    pub execution: ExecutionResponse,
    pub report: Option<ArtifactResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SubmitReviewReportResponse {
    pub review_execution: ReviewExecutionResponse,
    pub report: ArtifactResponse,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ValidationRunStatus {
    Running,
    Passed,
    Failed,
    Error,
    Cancelled,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ValidationRunResponse {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub caused_by_execution_id: Option<String>,
    pub check_identity: String,
    pub command: String,
    #[ts(type = "Record<string, unknown>")]
    pub config_summary: Value,
    pub config_digest: String,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub status: ValidationRunStatus,
    #[ts(type = "number | null")]
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub logs_ref: Option<String>,
    pub evidence_ids: Vec<String>,
    pub validation_report_artifact_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EvidenceResponse {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    #[ts(type = "Record<string, unknown>")]
    pub content: Value,
    pub digest: String,
    pub producer_validation_run_id: String,
    pub evidence_key: String,
    pub created_at: String,
}
