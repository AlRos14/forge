use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRepositoryRole {
    Primary,
    Affected,
    Context,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryInput {
    pub repo_id: String,
    pub role: TaskRepositoryRole,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRepositoryMergeState {
    Pending,
    Integrating,
    Integrated,
    Publishing,
    Open,
    Merged,
    Conflict,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryMergeResponse {
    pub batch_id: String,
    pub state: TaskRepositoryMergeState,
    pub integration_id: Option<String>,
    pub task_merge_operation_id: Option<String>,
    pub pr_metadata_id: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryPullRequestResponse {
    pub pr_metadata_id: String,
    pub provider_type: String,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub pr_state: String,
    pub merge_status: String,
    pub admission_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryResponse {
    pub repo_id: String,
    pub role: TaskRepositoryRole,
    #[ts(type = "number")]
    pub position: i64,
    pub merge: Option<TaskRepositoryMergeResponse>,
    pub pull_request: Option<TaskRepositoryPullRequestResponse>,
}
