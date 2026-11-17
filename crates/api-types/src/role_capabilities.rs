use crate::ActorRef;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The intentionally small set of TaskRole capabilities understood by the
/// current runtime. A capability never grants Workspace or repository access.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRoleCapability {
    FormalReview,
    Orchestration,
    RequestRework,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskRoleReworkMode {
    Manual,
    Automatic,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export, tag = "kind", rename_all = "snake_case")]
pub enum ReworkActorSelection {
    Explicit { actor_ref: ActorRef },
    OriginalContributor,
}

/// Persisted routing policy for a role that can request corrections. Role IDs
/// are local to one Task; Project templates use role names and resolve them
/// when copied into a new Task.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRoleReworkConfiguration {
    pub mode: TaskRoleReworkMode,
    pub allowed_destination_role_ids: Vec<String>,
    pub next_review_role_id: Option<String>,
    pub correction_actor: Option<ReworkActorSelection>,
    pub review_actor: Option<ActorRef>,
}

/// User-editable identity and capability configuration for a TaskRole.
/// `schema_version` is checked by Rust at every write and authority read.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct TaskRoleConfiguration {
    pub schema_version: u32,
    pub display_label: Option<String>,
    pub capabilities: Vec<TaskRoleCapability>,
    pub rework: Option<TaskRoleReworkConfiguration>,
}

impl Default for TaskRoleConfiguration {
    fn default() -> Self {
        Self {
            schema_version: 1,
            display_label: None,
            capabilities: Vec::new(),
            rework: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRoleTemplate {
    pub id: String,
    pub label: String,
    pub default_role: String,
    pub description: String,
    pub default_capabilities: Vec<TaskRoleCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TaskRoleCapabilityCatalog {
    pub schema_version: u32,
    pub capabilities: Vec<TaskRoleCapability>,
    pub templates: Vec<TaskRoleTemplate>,
}

/// Configuration stored in a Project template. References are role names,
/// never TaskRole IDs from another Task.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct ProjectTaskRoleConfiguration {
    pub schema_version: u32,
    pub display_label: Option<String>,
    pub capabilities: Vec<TaskRoleCapability>,
    pub rework: Option<ProjectTaskRoleReworkConfiguration>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub coordinator_actor: Option<ActorRef>,
}

impl Default for ProjectTaskRoleConfiguration {
    fn default() -> Self {
        Self {
            schema_version: 1,
            display_label: None,
            capabilities: Vec::new(),
            rework: None,
            coordinator_actor: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProjectTaskRoleReworkConfiguration {
    pub mode: TaskRoleReworkMode,
    pub allowed_destination_roles: Vec<String>,
    pub next_review_role: Option<String>,
    pub correction_actor: Option<ReworkActorSelection>,
    pub review_actor: Option<ActorRef>,
}
