use crate::ActorRef;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CoordinationMode {
    Partitioned,
    Collaborative,
    Independent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RoleMembershipStatus {
    Active,
    Suspended,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct RoleMembershipResponse {
    pub id: String,
    pub task_role_id: String,
    pub actor_ref: ActorRef,
    pub status: RoleMembershipStatus,
    #[ts(type = "number")]
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct TaskRoleResponse {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub coordination_mode: Option<CoordinationMode>,
    #[ts(type = "Record<string, unknown>")]
    pub policy: serde_json::Value,
    #[ts(type = "number")]
    pub version: i64,
    pub members: Vec<RoleMembershipResponse>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateTaskRoleRequest {
    pub role: String,
    pub coordination_mode: CoordinationMode,
    #[serde(default)]
    #[ts(type = "Record<string, unknown> | null")]
    pub policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateTaskRoleRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub coordination_mode: Option<CoordinationMode>,
    #[serde(default)]
    #[ts(type = "Record<string, unknown> | null")]
    pub policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AddRoleMembershipRequest {
    pub actor_ref: ActorRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateRoleMembershipRequest {
    #[ts(type = "number")]
    pub expected_version: i64,
    pub status: RoleMembershipStatus,
}
