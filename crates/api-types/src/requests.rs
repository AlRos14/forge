use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{
    project_hooks::{parse_project_hooks_json, ProjectHookRule},
    ExecutionPurpose,
};
use crate::{TaskType, WorkMode};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskRequest {
    pub title: String,
    #[ts(optional = nullable)]
    pub description: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub parent_task_id: Option<String>,
    #[ts(optional = nullable)]
    pub task_type: Option<TaskType>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ReorderSubtasksRequest {
    pub ordered_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateTaskRequest {
    #[ts(optional = nullable)]
    pub title: Option<String>,
    #[ts(optional = nullable)]
    pub description: Option<String>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub parent_task_id: Option<Option<String>>,
    #[ts(type = "number")]
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ClaimOverrides {
    pub model_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimTaskRequest {
    pub agent_id: String,
    pub overrides: Option<ClaimOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct StartExecutionRequest {
    pub agent_id: String,
    pub role: String,
    pub purpose: ExecutionPurpose,
    pub prompt: String,
    #[serde(default)]
    pub input_artifact_ids: Vec<String>,
}

pub type ExecutionOverridesRequest = ClaimOverrides;

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[ts(export)]
pub struct FollowUpRequest {
    pub message: String,
    pub agent_id: String,
    #[ts(optional = nullable)]
    pub overrides: Option<ExecutionOverridesRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDependency {
    pub task_id: String,
    pub depends_on_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDependencyRequest {
    pub depends_on_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskActionRequest {
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    #[ts(type = "number | null")]
    pub version: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveGateRequest {
    pub reason: Option<String>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnswerTaskDecisionRequest {
    #[ts(type = "Record<string, unknown>")]
    pub answers: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskDecisionRequestResponse {
    pub id: String,
    pub task_id: String,
    pub execution_id: String,
    pub role: String,
    pub authority_scope: String,
    #[ts(type = "Array<Record<string, unknown>>")]
    pub questions: Value,
    pub context: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectGateRequest {
    pub reason: String,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateProjectRequest {
    #[ts(type = "number")]
    pub version: i64,
    #[ts(optional = nullable)]
    pub name: Option<String>,
    #[ts(optional = nullable)]
    pub paused: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_project_hooks")]
    #[ts(optional = nullable)]
    pub project_hooks: Option<Vec<ProjectHookRule>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateRepoRequest {
    pub remote_url: String,
    #[ts(optional = nullable)]
    pub local_path: Option<String>,
    #[ts(optional = nullable)]
    pub name: Option<String>,
    #[ts(optional = nullable)]
    pub default_branch: Option<String>,
    #[ts(optional = nullable)]
    pub work_mode: Option<WorkMode>,
    #[ts(optional = nullable)]
    pub pr_provider: Option<String>,
    #[ts(optional = nullable)]
    pub pr_provider_config: Option<PrProviderConfigRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateRepoRequest {
    #[ts(optional = nullable)]
    pub name: Option<String>,
    #[ts(optional = nullable)]
    pub remote_url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub local_path: Option<Option<String>>,
    #[ts(optional = nullable)]
    pub default_branch: Option<String>,
    #[ts(optional = nullable)]
    pub work_mode: Option<WorkMode>,
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub pr_provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(optional)]
    pub pr_provider_config: Option<Option<UpdatePrProviderConfigRequest>>,
}

fn deserialize_optional_update_field<'de, D, T>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn deserialize_project_hooks<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<ProjectHookRule>>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let json = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
    parse_project_hooks_json(&json)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrProviderConfigRequest {
    #[ts(optional = nullable)]
    pub base_url: Option<String>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub polling_interval_seconds: Option<i64>,
    #[ts(optional = nullable)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdatePrProviderConfigRequest {
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub base_url: Option<Option<String>>,
    #[ts(type = "number")]
    #[ts(optional)]
    pub polling_interval_seconds: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_update_field")]
    #[ts(type = "string | null")]
    #[ts(optional)]
    pub token: Option<Option<String>>,
}
