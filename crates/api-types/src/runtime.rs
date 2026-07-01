use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorTypeDescriptor {
    #[serde(rename = "type")]
    pub type_name: String,
    pub display_name: String,
    pub config_schema: Value,
    pub default_config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityResponse {
    pub status: String,
    pub authenticated_at: Option<String>,
    pub config_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredOptionsResponse {
    pub models: Vec<String>,
    pub permission_policies: Vec<String>,
    pub cli_specific: Value,
    #[serde(default)]
    pub available_daemons: Vec<DiscoveredDaemonResponse>,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDaemonResponse {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAvailabilityResponse {
    pub available: bool,
    pub effective_status: String,
    pub resolved_daemon_id: Option<String>,
    pub active_task_count: i64,
    pub max_concurrent_tasks: i64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceResponse {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub status: String,
    pub before_sha: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: Option<String>,
    pub executor_type: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub config_json: Option<Value>,
    pub daemon_id: Option<String>,
    pub max_concurrent_tasks: Option<i64>,
    pub heartbeat_interval_seconds: Option<i64>,
    pub max_missed_heartbeats: Option<i64>,
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAgentRequest {
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub model: Option<Option<String>>,
    #[serde(default)]
    pub reasoning_effort: Option<Option<String>>,
    #[serde(default)]
    pub permission_policy: Option<Option<String>>,
    #[serde(default)]
    pub prompt_template: Option<Option<String>>,
    pub capabilities: Option<Vec<String>>,
    pub config_json: Option<Value>,
    #[serde(default)]
    pub daemon_id: Option<Option<String>>,
    pub max_concurrent_tasks: Option<i64>,
    pub is_default: Option<bool>,
    pub paused: Option<bool>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateAgentRequest {
    pub name: String,
}
