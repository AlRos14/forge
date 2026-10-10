use std::str::FromStr;

use api_types::{ExecutionPurpose, ProjectHookRule};
use db::{AgentStatus, PageRequest, SortBy, SortOrder, TaskLifecycleState};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::error::McpToolError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolCallParams {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) arguments: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTaskParams {
    pub(crate) project_id: String,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) parent_task_id: Option<String>,
    #[serde(default, rename = "type")]
    pub(crate) task_type: Option<String>,
    pub(crate) priority: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListTasksParams {
    pub(crate) project_id: String,
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<i64>,
    #[serde(default)]
    pub(crate) lifecycle_state: LifecycleStateFilter,
    pub(crate) sort_by: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct LifecycleStateFilter(Vec<TaskLifecycleState>);
impl LifecycleStateFilter {
    pub(crate) fn into_vec(self) -> Vec<TaskLifecycleState> {
        self.0
    }
}
impl<'de> Deserialize<'de> for LifecycleStateFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let values = match value {
            Value::Null => Vec::new(),
            Value::String(value) => {
                parse_lifecycle_state_list(&value).map_err(serde::de::Error::custom)?
            }
            Value::Array(values) => values
                .into_iter()
                .map(|value| match value {
                    Value::String(value) => TaskLifecycleState::from_str(&value)
                        .map_err(|_| format!("invalid lifecycle_state: {value}")),
                    _ => Err("lifecycle_state array must contain strings".to_owned()),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(serde::de::Error::custom)?,
            _ => {
                return Err(serde::de::Error::custom(
                    "lifecycle_state must be a string or array",
                ))
            }
        };
        Ok(Self(values))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetTaskParams {
    pub(crate) task_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListExecutionsParams {
    pub(crate) task_id: String,
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartExecutionParams {
    pub(crate) task_id: String,
    pub(crate) agent_id: String,
    pub(crate) role: String,
    pub(crate) purpose: ExecutionPurpose,
    pub(crate) prompt: String,
    #[serde(default)]
    pub(crate) input_artifact_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTaskRoleParams {
    pub(crate) task_id: String,
    pub(crate) role: String,
    pub(crate) coordination_mode: api_types::CoordinationMode,
    pub(crate) policy: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddTaskRoleMemberParams {
    pub(crate) task_id: String,
    pub(crate) role: String,
    pub(crate) actor_ref: api_types::ActorRef,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTaskParams {
    pub(crate) task_id: String,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) priority: Option<i64>,
    pub(crate) version: i64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisterAgentParams {
    pub(crate) name: String,
    pub(crate) executor_type: String,
    pub(crate) daemon_id: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListAgentsParams {
    pub(crate) status: Option<AgentStatusParam>,
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<i64>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListProjectsParams {
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<i64>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateProjectParams {
    pub(crate) name: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetProjectParams {
    pub(crate) project_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateProjectParams {
    pub(crate) project_id: String,
    pub(crate) name: Option<String>,
    pub(crate) paused: Option<bool>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateProjectHooksParams {
    pub(crate) project_id: String,
    pub(crate) version: i64,
    pub(crate) project_hooks: Vec<ProjectHookRule>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateSubTasksParams {
    pub(crate) parent_task_id: String,
    pub(crate) subtasks: Vec<SubTaskInput>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubTaskInput {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddTaskDependencyParams {
    pub(crate) task_id: String,
    pub(crate) depends_on_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoveTaskDependencyParams {
    pub(crate) task_id: String,
    pub(crate) depends_on_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListTaskDependenciesParams {
    pub(crate) task_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListAgentProfilesParams {
    pub(crate) identity_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionTaskLifecycleParams {
    pub(crate) task_id: String,
    pub(crate) to_state: TaskLifecycleState,
    pub(crate) expected_lifecycle_version: i64,
    pub(crate) idempotency_key: String,
    pub(crate) gate_evaluation_id: Option<String>,
    pub(crate) reason_kind: Option<String>,
    pub(crate) reason_ref: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateTaskParams {
    pub(crate) task_id: String,
    pub(crate) gate_kind: String,
    pub(crate) policy: Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateIdParams {
    pub(crate) gate_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviseGatePolicyParams {
    pub(crate) gate_id: String,
    pub(crate) expected_active_revision: Option<i64>,
    pub(crate) policy: Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateEvaluationParams {
    pub(crate) evaluation_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewTaskParams {
    pub(crate) task_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewExecutionParams {
    pub(crate) execution_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationTaskParams {
    pub(crate) task_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationRunParams {
    pub(crate) validation_run_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceParams {
    pub(crate) evidence_id: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationListParams {
    pub(crate) task_id: String,
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct AgentStatusParam(AgentStatus);
impl<'de> Deserialize<'de> for AgentStatusParam {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        AgentStatus::from_str(&value)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
impl From<AgentStatusParam> for AgentStatus {
    fn from(value: AgentStatusParam) -> Self {
        value.0
    }
}

pub(crate) fn parse_params<T>(params: Value) -> Result<T, McpToolError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(params).map_err(|error| {
        McpToolError::new(-32602, "invalid params").with_data(json!({"details":error.to_string()}))
    })
}

fn parse_lifecycle_state_list(value: &str) -> Result<Vec<TaskLifecycleState>, String> {
    value
        .split(',')
        .filter(|state| !state.trim().is_empty())
        .map(|state| {
            TaskLifecycleState::from_str(state.trim())
                .map_err(|_| format!("invalid lifecycle_state: {}", state.trim()))
        })
        .collect()
}

pub(crate) fn page_request(
    cursor: Option<String>,
    limit: Option<i64>,
    sort_by: Option<String>,
) -> Result<PageRequest, McpToolError> {
    Ok(PageRequest {
        cursor,
        limit: limit.unwrap_or(20).clamp(1, 100),
        include_total: false,
        sort_by: parse_sort_by(sort_by.as_deref())?,
        sort_order: SortOrder::Desc,
    })
}
pub(crate) fn task_page_request(
    cursor: Option<String>,
    limit: Option<i64>,
    sort_by: Option<String>,
) -> Result<PageRequest, McpToolError> {
    if sort_by.is_none() {
        return Ok(PageRequest {
            cursor,
            limit: limit.unwrap_or(20).clamp(1, 100),
            include_total: false,
            sort_by: SortBy::BoardPosition,
            sort_order: SortOrder::Asc,
        });
    }
    page_request(cursor, limit, sort_by)
}
fn parse_sort_by(value: Option<&str>) -> Result<SortBy, McpToolError> {
    match value.unwrap_or("created_at") {
        "created_at" => Ok(SortBy::CreatedAt),
        "updated_at" => Ok(SortBy::UpdatedAt),
        "priority" => Ok(SortBy::Priority),
        "board_position" => Ok(SortBy::BoardPosition),
        "lifecycle_state" => Ok(SortBy::LifecycleState),
        "id" => Ok(SortBy::Id),
        value => Err(McpToolError::new(
            -32602,
            format!("invalid sort_by: {value}"),
        )),
    }
}
