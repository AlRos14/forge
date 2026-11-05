use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectCopilotRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProjectCopilotMessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotRun {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    #[ts(type = "number")]
    pub binding_version: i64,
    pub context_task_id: Option<String>,
    pub status: ProjectCopilotRunStatus,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotMessage {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub role: ProjectCopilotMessageRole,
    pub actor_type: String,
    pub actor_id: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotResponse {
    pub project_id: String,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    #[ts(type = "number")]
    pub version: i64,
    pub active_run: Option<ProjectCopilotRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateProjectCopilotRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SendProjectCopilotMessageRequest {
    pub content: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SendProjectCopilotMessageResponse {
    pub run: ProjectCopilotRun,
    pub user_message: ProjectCopilotMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotRunDetailResponse {
    pub run: ProjectCopilotRun,
    pub user_message: ProjectCopilotMessage,
    pub assistant_message: Option<ProjectCopilotMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotMessagesResponse {
    pub items: Vec<ProjectCopilotMessage>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotActivity {
    pub id: String,
    pub run_id: String,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub task_id: Option<String>,
    pub actor_id: String,
    pub requested_by_user_id: String,
    pub tool: Option<String>,
    pub outcome: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCopilotActivityResponse {
    pub items: Vec<ProjectCopilotActivity>,
    pub next_cursor: Option<String>,
}
