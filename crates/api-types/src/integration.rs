use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIntegrationRequest {
    pub platform: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub token_secret_ref: String,
    pub poll_interval_secs: Option<i64>,
    pub sync_filter: Option<Value>,
    pub default_task_state: Option<String>,
    pub default_assignee_type: Option<String>,
    pub default_assignee_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchIntegrationRequest {
    pub platform: Option<String>,
    pub base_url: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub token_secret_ref: Option<String>,
    pub poll_interval_secs: Option<i64>,
    pub sync_filter: Option<Value>,
    pub default_task_state: Option<String>,
    pub default_assignee_type: Option<String>,
    pub default_assignee_id: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateExternalLinkRequest {
    pub remote_issue_number: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationResponse {
    pub id: String,
    pub project_id: String,
    pub platform: String,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub token_secret_ref: String,
    pub poll_interval_secs: i64,
    pub sync_filter: Value,
    pub default_task_state: Option<String>,
    pub default_assignee_type: Option<String>,
    pub default_assignee_id: Option<String>,
    pub enabled: bool,
    pub last_polled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalLinkResponse {
    pub id: String,
    pub task_id: String,
    pub integration_id: String,
    pub platform: String,
    pub remote_owner: String,
    pub remote_repo: String,
    pub remote_issue_number: i64,
    pub remote_url: String,
    pub global_id: String,
    pub synced_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTriggerResponse {
    pub imported: u32,
    pub skipped: u32,
    pub errors: u32,
}
