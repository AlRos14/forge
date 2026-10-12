use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// Event names that the REST SSE endpoint is allowed to publish.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub enum PublicEventType {
    #[serde(rename = "task.lifecycle_changed")]
    #[ts(rename = "task.lifecycle_changed")]
    TaskLifecycleChanged,
    #[serde(rename = "gate.created")]
    #[ts(rename = "gate.created")]
    GateCreated,
    #[serde(rename = "gate.policy_revised")]
    #[ts(rename = "gate.policy_revised")]
    GatePolicyRevised,
    #[serde(rename = "gate.evaluated")]
    #[ts(rename = "gate.evaluated")]
    GateEvaluated,
    #[serde(rename = "execution.started")]
    #[ts(rename = "execution.started")]
    ExecutionStarted,
    #[serde(rename = "execution.completed")]
    #[ts(rename = "execution.completed")]
    ExecutionCompleted,
    #[serde(rename = "execution.failed")]
    #[ts(rename = "execution.failed")]
    ExecutionFailed,
    #[serde(rename = "execution.cancelled")]
    #[ts(rename = "execution.cancelled")]
    ExecutionCancelled,
    #[serde(rename = "execution.stalled")]
    #[ts(rename = "execution.stalled")]
    ExecutionStalled,
    #[serde(rename = "validation_run.started")]
    #[ts(rename = "validation_run.started")]
    ValidationRunStarted,
    #[serde(rename = "validation_run.completed")]
    #[ts(rename = "validation_run.completed")]
    ValidationRunCompleted,
    #[serde(rename = "evidence.created")]
    #[ts(rename = "evidence.created")]
    EvidenceCreated,
    #[serde(rename = "artifact.created")]
    #[ts(rename = "artifact.created")]
    ArtifactCreated,
    #[serde(rename = "message.created")]
    #[ts(rename = "message.created")]
    MessageCreated,
    #[serde(rename = "handoff.created")]
    #[ts(rename = "handoff.created")]
    HandoffCreated,
    #[serde(rename = "handoff.status_changed")]
    #[ts(rename = "handoff.status_changed")]
    HandoffStatusChanged,
    #[serde(rename = "proposal.created")]
    #[ts(rename = "proposal.created")]
    ProposalCreated,
    #[serde(rename = "proposal.withdrawn")]
    #[ts(rename = "proposal.withdrawn")]
    ProposalWithdrawn,
    #[serde(rename = "decision.recorded")]
    #[ts(rename = "decision.recorded")]
    DecisionRecorded,
    #[serde(rename = "project.created")]
    #[ts(rename = "project.created")]
    ProjectCreated,
    #[serde(rename = "project.updated")]
    #[ts(rename = "project.updated")]
    ProjectUpdated,
    #[serde(rename = "project.deleted")]
    #[ts(rename = "project.deleted")]
    ProjectDeleted,
    #[serde(rename = "project.paused")]
    #[ts(rename = "project.paused")]
    ProjectPaused,
    #[serde(rename = "project.resumed")]
    #[ts(rename = "project.resumed")]
    ProjectResumed,
    #[serde(rename = "project_hook.run_changed")]
    #[ts(rename = "project_hook.run_changed")]
    ProjectHookRunChanged,
    #[serde(rename = "notification.created")]
    #[ts(rename = "notification.created")]
    NotificationCreated,
    #[serde(rename = "operations.status_changed")]
    #[ts(rename = "operations.status_changed")]
    OperationsStatusChanged,
    #[serde(rename = "events.resync_required")]
    #[ts(rename = "events.resync_required")]
    EventsResyncRequired,
}

impl PublicEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskLifecycleChanged => "task.lifecycle_changed",
            Self::GateCreated => "gate.created",
            Self::GatePolicyRevised => "gate.policy_revised",
            Self::GateEvaluated => "gate.evaluated",
            Self::ExecutionStarted => "execution.started",
            Self::ExecutionCompleted => "execution.completed",
            Self::ExecutionFailed => "execution.failed",
            Self::ExecutionCancelled => "execution.cancelled",
            Self::ExecutionStalled => "execution.stalled",
            Self::ValidationRunStarted => "validation_run.started",
            Self::ValidationRunCompleted => "validation_run.completed",
            Self::EvidenceCreated => "evidence.created",
            Self::ArtifactCreated => "artifact.created",
            Self::MessageCreated => "message.created",
            Self::HandoffCreated => "handoff.created",
            Self::HandoffStatusChanged => "handoff.status_changed",
            Self::ProposalCreated => "proposal.created",
            Self::ProposalWithdrawn => "proposal.withdrawn",
            Self::DecisionRecorded => "decision.recorded",
            Self::ProjectCreated => "project.created",
            Self::ProjectUpdated => "project.updated",
            Self::ProjectDeleted => "project.deleted",
            Self::ProjectPaused => "project.paused",
            Self::ProjectResumed => "project.resumed",
            Self::ProjectHookRunChanged => "project_hook.run_changed",
            Self::NotificationCreated => "notification.created",
            Self::OperationsStatusChanged => "operations.status_changed",
            Self::EventsResyncRequired => "events.resync_required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct PublicEventEnvelope {
    pub event_type: PublicEventType,
    pub entity_id: String,
    pub timestamp: String,
    pub event_id: Option<String>,
    #[ts(type = "number | null")]
    pub sequence: Option<u64>,
    pub entity_type: Option<String>,
    pub scope_type: Option<String>,
    pub scope_id: Option<String>,
    #[ts(type = "Record<string, unknown> | null")]
    pub payload: Option<Value>,
}
