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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRepositoryChangeState {
    #[default]
    Pending,
    Changed,
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryIntegrationResponse {
    pub repo_id: String,
    pub integration_id: Option<String>,
    pub outcome: String,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRepositoryMergeBatchState {
    Integrating,
    Publishing,
    Open,
    Completed,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryMergeResponse {
    #[serde(default)]
    pub batch_id: String,
    #[serde(default)]
    pub repo_id: String,
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
    #[serde(default)]
    pub repo_id: String,
    #[serde(default)]
    pub admission_id: Option<String>,
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
    #[serde(default)]
    pub change_state: TaskRepositoryChangeState,
    #[serde(default)]
    pub integration: Option<TaskRepositoryIntegrationResponse>,
    #[serde(default)]
    pub merge: Option<TaskRepositoryMergeResponse>,
    #[serde(default)]
    pub pull_request: Option<TaskRepositoryPullRequestResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRepositoryMergeBatchResponse {
    pub id: String,
    pub task_id: String,
    pub gate_evaluation_id: String,
    #[ts(type = "number")]
    pub repository_set_revision: i64,
    pub state: TaskRepositoryMergeBatchState,
    #[ts(type = "number")]
    pub version: i64,
    pub repositories: Vec<TaskRepositoryMergeResponse>,
    pub created_at: String,
    pub updated_at: String,
}
