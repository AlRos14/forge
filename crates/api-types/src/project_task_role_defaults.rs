use crate::{ActorRef, CoordinationMode};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
#[ts(export)]
pub struct ProjectTaskRoleDefault {
    pub role: String,
    pub coordination_mode: CoordinationMode,
    pub members: Vec<ActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct ProjectTaskRoleDefaultsResponse {
    pub project_id: String,
    #[ts(type = "number")]
    pub version: i64,
    pub roles: Vec<ProjectTaskRoleDefault>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateProjectTaskRoleDefaultsRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub roles: Vec<ProjectTaskRoleDefault>,
}
