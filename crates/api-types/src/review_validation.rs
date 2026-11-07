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
    pub task_role_id: String,
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
    #[serde(default)]
    #[ts(optional = nullable)]
    pub reviewed_work_unit_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    #[ts(type = "number | null")]
    pub reviewed_work_unit_version: Option<i64>,
}

/// Submit the assigned Human's review of a prepared WorkUnit. Forge creates
/// the technical attempt and binds its subject; no Execution selection is needed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SubmitWorkUnitReviewRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub report: SubmitReviewReportRequest,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewLineRevisionResponse {
    pub line_id: String,
    pub task_id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub work_unit_id: String,
    #[ts(type = "number")]
    pub work_unit_version: i64,
    pub result_execution_id: Option<String>,
    pub result_artifact_id: Option<String>,
    pub result_commit_sha: Option<String>,
    pub result_workspace_id: Option<String>,
    pub workspace_snapshot_digest: Option<String>,
    pub manifest_id: Option<String>,
    pub review_report_artifact_id: Option<String>,
    pub verdict: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ReviewLineResponse {
    pub id: String,
    pub task_id: String,
    pub root_work_unit_id: String,
    pub current_work_unit_id: String,
    #[ts(type = "number")]
    pub current_work_unit_version: i64,
    pub current_result_execution_id: Option<String>,
    pub current_result_artifact_id: Option<String>,
    pub current_commit_sha: Option<String>,
    pub current_workspace_id: Option<String>,
    pub current_snapshot_digest: Option<String>,
    pub current_manifest_id: Option<String>,
    pub latest_report_artifact_id: Option<String>,
    pub latest_verdict: Option<String>,
    #[ts(type = "number")]
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
    pub history: Vec<ReviewLineRevisionResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct WorkUnitArtifactInputResponse {
    pub artifact_id: String,
    pub digest: String,
    pub producer_execution_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct WorkUnitArtifactManifestResponse {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    #[ts(type = "number")]
    pub work_unit_version: i64,
    pub result_execution_id: String,
    pub delivery_artifact_id: String,
    pub delivery_artifact_digest: String,
    #[ts(type = "Record<string, unknown>")]
    pub manifest: Value,
    pub manifest_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct WorkUnitArtifactPackageResponse {
    pub work_unit_id: String,
    pub inputs: Vec<WorkUnitArtifactInputResponse>,
    pub manifests: Vec<WorkUnitArtifactManifestResponse>,
}
