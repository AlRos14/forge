use crate::ActorRef;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkUnitStatus {
    Open,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export, tag = "kind", rename_all = "snake_case")]
pub enum WorkUnitProvenance {
    Actor {
        actor: ActorRef,
    },
    WorkUnit {
        id: String,
    },
    Artifact {
        id: String,
    },
    External {
        id: String,
    },
    /// A V091 record whose untyped Actor ID could not be resolved when V092
    /// added ActorRef provenance. This response-only form cannot be created.
    LegacyActor {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export, tag = "kind", rename_all = "snake_case")]
pub enum CreateWorkUnitProvenance {
    Actor { actor: ActorRef },
    WorkUnit { id: String },
    Artifact { id: String },
    External { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateWorkUnitRequest {
    pub title: String,
    pub scope: String,
    pub role: String,
    pub parent_work_unit_id: Option<String>,
    pub assigned_actor: Option<ActorRef>,
    pub requires_integration: bool,
    pub provenance: Option<CreateWorkUnitProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct UpdateWorkUnitRequest {
    pub expected_version: i64,
    pub title: Option<String>,
    pub scope: Option<String>,
    pub requires_integration: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AllocateWorkUnitRequest {
    pub expected_version: i64,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct TransitionWorkUnitRequest {
    pub expected_version: i64,
    pub status: WorkUnitStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct AddWorkUnitDependencyRequest {
    pub expected_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkUnitDependencyResponse {
    pub work_unit_id: String,
    pub depends_on_work_unit_id: String,
    pub satisfied: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkUnitReadinessResponse {
    pub runnable: bool,
    pub ready_for_allocation: bool,
    pub active_execution_ids: Vec<String>,
    pub unsatisfied_dependency_ids: Vec<String>,
    pub awaiting_integration: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkUnitResponse {
    pub id: String,
    pub task_id: String,
    pub parent_work_unit_id: Option<String>,
    pub title: String,
    pub scope: String,
    pub status: WorkUnitStatus,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub requires_integration: bool,
    pub provenance: Option<WorkUnitProvenance>,
    pub created_by: ActorRef,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub readiness: WorkUnitReadinessResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkUnitIntegrationRequest {
    pub execution_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkUnitIntegrationOutcome {
    Running,
    Success,
    Conflict,
    Failed,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct WorkUnitIntegrationResponse {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    pub execution_id: String,
    pub source_workspace_id: String,
    pub source_sha: String,
    pub target_workspace_id: String,
    pub target_before_sha: String,
    pub target_after_sha: Option<String>,
    pub outcome: WorkUnitIntegrationOutcome,
    pub version: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{CreateWorkUnitProvenance, CreateWorkUnitRequest, WorkUnitIntegrationRequest};

    #[test]
    fn create_work_unit_request_rejects_authority_and_unknown_fields() {
        let forged = serde_json::json!({
            "title": "implement",
            "scope": "bounded task",
            "role": "implementer",
            "parent_work_unit_id": null,
            "assigned_actor": null,
            "requires_integration": true,
            "provenance": null,
            "created_by": {"kind":"human","id":"other-user"}
        });
        assert!(serde_json::from_value::<CreateWorkUnitRequest>(forged).is_err());
        assert!(
            serde_json::from_value::<WorkUnitIntegrationRequest>(serde_json::json!({
                "execution_id": "execution-1",
                "idempotency_key": "same-operation",
                "source_sha": "caller-selected"
            }))
            .is_err()
        );
    }

    #[test]
    fn work_unit_actor_provenance_requires_a_typed_actor_ref() {
        let valid = serde_json::json!({
            "kind": "actor",
            "actor": {"kind": "human", "id": "user-1"}
        });
        assert!(serde_json::from_value::<CreateWorkUnitProvenance>(valid).is_ok());

        for invalid in [
            serde_json::json!({"kind":"actor","id":"user-1"}),
            serde_json::json!({
                "kind":"actor",
                "actor":{"kind":"human","id":"user-1"},
                "id":"agent-1"
            }),
            serde_json::json!({"kind":"legacy_actor","id":"old-id"}),
        ] {
            assert!(serde_json::from_value::<CreateWorkUnitProvenance>(invalid).is_err());
        }
    }
}
