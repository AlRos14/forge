use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{AuthorType, ReviewConfig, ReviewStatus, TaskResponse, WorkMode, WorkflowDefinition};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiffSummary {
    pub path: String,
    pub status: DiffFileStatus,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStats {
    pub files_changed: u64,
    pub total_additions: u64,
    pub total_deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResponse {
    pub base_ref: String,
    pub head_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<FileDiffSummary>,
    pub stats: DiffStats,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEnvelope {
    pub data: DiffResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub id: String,
    pub name: String,
    pub settings: Value,
    pub default_review_config: Option<ReviewConfig>,
    pub primary_repo_id: Option<String>,
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub workflow_template_name: Option<String>,
    pub paused_at: Option<String>,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowTemplateSummary {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowTemplateResponse {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub is_builtin: bool,
    pub definition: WorkflowDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SaveWorkflowTemplateRequest {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub definition: WorkflowDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateProjectWorkflowRequest {
    #[serde(default)]
    pub template_name: Option<String>,
    #[serde(default)]
    pub definition: Option<WorkflowDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoResponse {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub local_path: Option<String>,
    pub remote_url: String,
    pub default_branch: String,
    pub work_mode: WorkMode,
    pub pr_provider: Option<String>,
    pub pr_provider_status: Option<PrProviderStatus>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrProviderStatus {
    pub provider_type: String,
    pub has_token: bool,
    pub polling_interval_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrSummary {
    pub pr_url: Option<String>,
    pub pr_state: String,
    pub source_branch: String,
    pub target_branch: String,
    pub merge_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_git_repo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListResponse {
    pub path: String,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BranchListResponse {
    pub branches: Vec<String>,
    pub default_branch: Option<String>,
    pub origin_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResultResponse {
    pub index: usize,
    pub command: String,
    pub exit_code: i32,
    pub stderr_tail: String,
    #[serde(default)]
    pub output_tail: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepResultEntry {
    pub index: usize,
    pub command: String,
    pub exit_code: i32,
    pub stderr_tail: String,
    #[serde(default)]
    pub output_tail: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditorVerdictEntry {
    pub verdict: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReviewDetails {
    #[serde(default)]
    pub ci_steps: Vec<StepResultEntry>,
    #[serde(default)]
    pub auditor: Option<AuditorVerdictEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResponse {
    pub id: String,
    pub task_id: String,
    pub execution_id: String,
    pub attempt_number: i64,
    pub status: ReviewStatus,
    pub step_results: Vec<StepResultResponse>,
    pub details: ReviewDetails,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewDecisionResponse {
    pub task: TaskResponse,
    pub review: ReviewResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectReviewRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateCommentRequest {
    pub content: String,
    pub author_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentResponse {
    pub id: String,
    pub task_id: String,
    pub author_type: AuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NotificationResponse {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub event_type: String,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UnreadCountResponse {
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionTaskResponse {
    pub task: TaskResponse,
    pub review: Option<ReviewResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub total_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
    pub request_id: String,
}
