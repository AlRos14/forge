use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{project_hooks::ProjectHookRule, AuthorType, WorkMode};

fn default_project_version() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileDiffSummary {
    pub path: String,
    pub status: DiffFileStatus,
    #[ts(type = "number")]
    pub additions: u64,
    #[ts(type = "number")]
    pub deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffStats {
    #[ts(type = "number")]
    pub files_changed: u64,
    #[ts(type = "number")]
    pub total_additions: u64,
    #[ts(type = "number")]
    pub total_deletions: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffResponse {
    pub base_ref: String,
    pub head_ref: String,
    pub base_sha: String,
    pub head_sha: String,
    pub files: Vec<FileDiffSummary>,
    pub stats: DiffStats,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffEnvelope {
    pub data: DiffResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectResponse {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub project_hooks: Vec<ProjectHookRule>,
    #[serde(default)]
    pub primary_repo_id: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub paused_at: Option<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "default_project_version")]
    #[ts(type = "number")]
    pub version: i64,
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
    #[ts(type = "number")]
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
pub struct TaskMediaResponse {
    pub id: String,
    pub task_id: String,
    pub filename: String,
    pub content_type: String,
    #[ts(type = "number")]
    pub byte_size: i64,
    pub url: String,
    pub author_type: AuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub created_at: String,
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
    #[ts(type = "number")]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub total_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
    #[ts(type = "Record<string, unknown> | null")]
    pub details: Option<Value>,
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::ProjectResponse;

    #[test]
    fn project_response_deserializes_target_fields() {
        let response: ProjectResponse = serde_json::from_value(serde_json::json!({
            "id": "project-1",
            "name": "Forge",
            "created_at": "2026-05-27T00:00:00Z",
            "updated_at": "2026-05-27T00:00:00Z"
        }))
        .expect("project response should deserialize without compatibility fields");

        assert_eq!(response.id, "project-1");
        assert!(response.project_hooks.is_empty());
        assert!(!response.paused);
        assert_eq!(response.version, 1);
    }
}
