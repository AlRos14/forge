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

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum CollaborationTarget {
    Actor { actor: ActorRef },
    Role { role_id: String },
    Task,
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

        let future_target = serde_json::json!({"kind":"work_unit", "id":"wu-1"});
        assert!(serde_json::from_value::<ProposalTarget>(future_target).is_err());
    }
}
