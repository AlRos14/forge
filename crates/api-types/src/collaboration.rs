use crate::ActorRef;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactKind {
    Plan,
    ReviewReport,
    ValidationReport,
    Diff,
    Patch,
    Summary,
    DesignDocument,
    Investigation,
    ApiContract,
    TestReport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactStorageKind {
    Inline,
    External,
}

#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum CollaborationTarget {
    Actor { actor: ActorRef },
    Role { role_id: String },
    Task,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CollaborationTargetWire {
    Actor(ActorTargetFields),
    Role(RoleTargetFields),
    Task(TaskTargetFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActorTargetFields {
    kind: ActorTargetTag,
    actor: CollaborationActorRefWire,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ActorTargetTag {
    Actor,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CollaborationActorRefWire {
    Human(HumanActorRefFields),
    Agent(AgentActorRefFields),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanActorRefFields {
    kind: HumanActorRefTag,
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum HumanActorRefTag {
    Human,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentActorRefFields {
    kind: AgentActorRefTag,
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AgentActorRefTag {
    Agent,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleTargetFields {
    kind: RoleTargetTag,
    role_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RoleTargetTag {
    Role,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskTargetFields {
    kind: TaskTargetTag,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskTargetTag {
    Task,
}

impl<'de> Deserialize<'de> for CollaborationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match CollaborationTargetWire::deserialize(deserializer)? {
            CollaborationTargetWire::Actor(fields) => {
                let _ = fields.kind;
                Self::Actor {
                    actor: match fields.actor {
                        CollaborationActorRefWire::Human(fields) => {
                            let _ = fields.kind;
                            ActorRef::Human(fields.id)
                        }
                        CollaborationActorRefWire::Agent(fields) => {
                            let _ = fields.kind;
                            ActorRef::Agent(fields.id)
                        }
                    },
                }
            }
            CollaborationTargetWire::Role(fields) => {
                let _ = fields.kind;
                Self::Role {
                    role_id: fields.role_id,
                }
            }
            CollaborationTargetWire::Task(fields) => {
                let _ = fields.kind;
                Self::Task
            }
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HandoffIntent {
    Rework,
    Delegation,
    Question,
    Answer,
    Investigate,
    DecisionRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HandoffStatus {
    Pending,
    Accepted,
    Completed,
    Declined,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalTargetKind {
    Task,
    Execution,
    Workspace,
    WorkUnit,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProposalTarget {
    pub kind: ProposalTargetKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProposalStatus {
    Open,
    Resolved,
    Withdrawn,
    Superseded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DecisionOutcome {
    Approve,
    Reject,
    Supersede,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateArtifactRequest {
    pub kind: ArtifactKind,
    pub storage_kind: ArtifactStorageKind,
    pub content: Option<String>,
    pub content_ref: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub metadata: Value,
    pub digest: Option<String>,
    pub producer_execution_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ArtifactResponse {
    pub id: String,
    pub task_id: String,
    pub kind: ArtifactKind,
    pub storage_kind: ArtifactStorageKind,
    pub content: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub metadata: Value,
    pub digest: Option<String>,
    pub producer_execution_id: String,
    pub producer: ActorRef,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateMessageRequest {
    pub target: CollaborationTarget,
    pub body: String,
    pub work_unit_id: Option<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct MessageResponse {
    pub id: String,
    pub task_id: String,
    pub sender: ActorRef,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub body: String,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateHandoffRequest {
    pub source_role_id: Option<String>,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub intent: HandoffIntent,
    pub parent_execution_id: Option<String>,
    pub expected_policy_ref: Option<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct HandoffResponse {
    pub id: String,
    pub task_id: String,
    pub created_by: ActorRef,
    pub source_role_id: Option<String>,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub intent: HandoffIntent,
    pub parent_execution_id: Option<String>,
    pub expected_policy_ref: Option<String>,
    pub status: HandoffStatus,
    pub version: i64,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct HandoffTransitionRequest {
    pub status: HandoffStatus,
    pub expected_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateProposalRequest {
    pub target: ProposalTarget,
    pub action: String,
    pub reason: String,
    pub target_version: Option<i64>,
    pub target_digest: Option<String>,
    pub required_policy_ref: Option<String>,
    pub required_policy_version: Option<i64>,
    pub required_policy_digest: Option<String>,
    pub supersedes_proposal_id: Option<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ProposalResponse {
    pub id: String,
    pub task_id: String,
    pub proposer: ActorRef,
    pub target: ProposalTarget,
    pub action: String,
    pub reason: String,
    pub target_version: Option<i64>,
    pub target_digest: Option<String>,
    pub required_policy_ref: Option<String>,
    pub required_policy_version: Option<i64>,
    pub required_policy_digest: Option<String>,
    pub content_version: i64,
    pub status: ProposalStatus,
    pub supersedes_proposal_id: Option<String>,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CreateDecisionRequest {
    pub proposal_id: String,
    pub proposal_version: i64,
    pub outcome: DecisionOutcome,
    pub rationale: String,
    pub policy_ref: Option<String>,
    pub policy_version: Option<i64>,
    pub policy_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct DecisionResponse {
    pub id: String,
    pub task_id: String,
    pub proposal_id: String,
    pub proposal_version: i64,
    pub outcome: DecisionOutcome,
    pub rationale: String,
    pub policy_ref: Option<String>,
    pub policy_version: Option<i64>,
    pub policy_digest: Option<String>,
    pub actors: Vec<ActorRef>,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CollaborationListQuery {
    pub cursor: Option<String>,
    pub limit: Option<i64>,
    pub include_total: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::ActorRef;

    use super::{
        CreateDecisionRequest, CreateMessageRequest, CreateProposalRequest, ProposalTarget,
    };

    #[test]
    fn collaboration_requests_reject_client_selected_actor_fields() {
        let message = serde_json::json!({
            "target": { "kind": "task" },
            "body": "hello",
            "sender_actor_id": "someone-else"
        });
        assert!(serde_json::from_value::<CreateMessageRequest>(message).is_err());

        let proposal = serde_json::json!({
            "target": { "kind": "task", "id": "task-1" },
            "action": "do-the-thing",
            "reason": "because",
            "proposer": { "kind": "human", "id": "someone-else" }
        });
        assert!(serde_json::from_value::<CreateProposalRequest>(proposal).is_err());

        let decision = serde_json::json!({
            "proposal_id": "proposal-1",
            "proposal_version": 1,
            "outcome": "approve",
            "rationale": "yes",
            "actors": [{ "kind": "agent", "id": "spoofed-agent" }]
        });
        assert!(serde_json::from_value::<CreateDecisionRequest>(decision).is_err());

        let future_target = serde_json::json!({"kind":"validation_run", "id":"run-1"});
        assert!(serde_json::from_value::<ProposalTarget>(future_target).is_err());

        assert_eq!(
            serde_json::from_value::<super::CollaborationTarget>(
                serde_json::json!({"kind":"task"})
            )
            .expect("canonical Task target"),
            super::CollaborationTarget::Task
        );
        assert_eq!(
            serde_json::from_value::<super::CollaborationTarget>(serde_json::json!({
                "kind":"actor",
                "actor":{"kind":"human", "id":"user-1"}
            }))
            .expect("canonical Actor target"),
            super::CollaborationTarget::Actor {
                actor: ActorRef::Human("user-1".to_owned())
            }
        );
        assert_eq!(
            serde_json::from_value::<super::CollaborationTarget>(serde_json::json!({
                "kind":"role",
                "role_id":"role-1"
            }))
            .expect("canonical Role target"),
            super::CollaborationTarget::Role {
                role_id: "role-1".to_owned()
            }
        );

        for contradictory in [
            serde_json::json!({
                "kind": "task",
                "actor": {"kind": "human", "id": "user-1"}
            }),
            serde_json::json!({
                "kind": "role",
                "role_id": "role-1",
                "actor": {"kind": "human", "id": "user-1"}
            }),
            serde_json::json!({
                "kind": "actor",
                "actor": {"kind": "human", "id": "user-1"},
                "role_id": "role-1"
            }),
            serde_json::json!({
                "kind": "actor",
                "actor": {"kind": "human", "id": "user-1", "extra": true}
            }),
        ] {
            let described = contradictory.clone();
            assert!(
                serde_json::from_value::<super::CollaborationTarget>(contradictory).is_err(),
                "collaboration targets reject contradictory or extra fields: {described}"
            );
        }
    }
}
