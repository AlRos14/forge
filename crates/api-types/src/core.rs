use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub type TaskStatus = String;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskType {
    Task,
    PlanningTask,
    SubTask,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkMode {
    #[default]
    DirectMerge,
    PullRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Busy,
    Error,
    Offline,
}

pub type ExecutionRole = String;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConversationStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConversationMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConversationMessageStatus {
    Complete,
    Streaming,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum StopReason {
    UserCancelled,
    TaskCancelled,
    RoleReassigned,
    GracefulShutdown,
    CrashRecovery,
    AgentTimeout,
    ExecutionStalled,
    DaemonDisconnected,
    ExecutorCancelled,
    ExecutorFailed,
    LegacyUnknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ResumePolicy {
    Auto,
    Manual,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RecoveryAction {
    ResumeSession,
    Reexecute,
    ResetToInitial,
    CancelTask,
    MarkReviewed,
    RetryHook,
    ResumeProcess,
    UpdateWorkspaceAndRetryHook,
    SkipHookOnce,
    ResetRetryWindow,
    ProceedOnce,
    OpenInteractive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionBehaviorKind {
    ManualLaunch,
    SessionFollowUp,
    WorkflowResume,
    ReExecute,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct ExecutionBehavior {
    pub kind: ExecutionBehaviorKind,
    pub propagates: bool,
    pub cascade_role: Option<String>,
    pub cascade_state: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionActionKind {
    ManualLaunch,
    SessionFollowUp,
    WorkflowResume,
    ReExecute,
    StopExecution,
    CancelTask,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
pub struct ExecutionAction {
    pub action: ExecutionActionKind,
    pub label: String,
    pub enabled: bool,
    pub propagates: bool,
    pub requires_session: bool,
    pub disabled_reason: Option<String>,
    pub target_execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct BlockingArtifact {
    pub kind: String,
    pub id: Option<String>,
    pub log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct TaskBlockingAnnotation {
    #[serde(rename = "type")]
    pub annotation_type: String,
    #[serde(default)]
    pub blocking_reason: String,
    pub blocked_by: Option<String>,
    pub blocked_at: Option<String>,
    pub blocked_execution_id: Option<String>,
    pub artifact: Option<BlockingArtifact>,
    pub message: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "Record<string, unknown> | null")]
    #[ts(optional)]
    pub hook: Option<serde_json::Value>,
    #[serde(default)]
    pub recovery_actions: Vec<RecoveryAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct WorkflowHealthSummary {
    pub kind: WorkflowHealthKind,
    pub label: String,
    pub severity: HealthSeverity,
    pub message: Option<String>,
    pub state: Option<String>,
    pub role: Option<String>,
    pub execution_id: Option<String>,
    pub review_id: Option<String>,
    pub since: Option<String>,
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkflowHealthKind {
    Idle,
    WaitingForAgent,
    Running,
    AwaitingHuman,
    Blocked,
    Failed,
    Stuck,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HealthSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct WorkflowExceptionSummary {
    #[serde(rename = "type")]
    pub exception_type: String,
    pub message: String,
    pub review_id: Option<String>,
    pub execution_id: Option<String>,
    pub state: Option<String>,
    pub role: Option<String>,
    pub target_state: Option<String>,
    pub target_role: Option<String>,
    pub failing_step: Option<FailingStepSummary>,
    #[serde(default)]
    pub related_evidence: Vec<RelatedEvidence>,
    #[serde(default)]
    pub actions: Vec<WorkflowExceptionAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct FailingStepSummary {
    pub index: usize,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub output_tail: Option<String>,
    pub stderr_tail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct RelatedEvidence {
    pub kind: String,
    pub id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct WorkflowExceptionAction {
    pub kind: RecoveryAction,
    pub label: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub requires_reason: bool,
    pub requires_guidance: bool,
    pub propagates: bool,
    pub target_state: Option<String>,
    pub target_role: Option<String>,
    pub target_execution_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct InterruptionMetadata {
    pub reason: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub execution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "Record<string, unknown> | null")]
    #[ts(optional)]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[allow(clippy::large_enum_variant)]
#[serde(untagged)]
#[ts(export)]
pub enum TaskAnnotation {
    Blocking(TaskBlockingAnnotation),
    #[ts(type = "unknown")]
    Legacy(Value),
}

impl std::fmt::Display for TaskAnnotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_string(self) {
            Ok(s) => f.write_str(&s),
            Err(_) => write!(f, "{:?}", self),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Running,
    AwaitingHuman,
    Passed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AuthorType {
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HumanDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct ReviewConfig {
    #[serde(default)]
    pub ci_steps: Vec<String>,
    #[serde(default)]
    pub review_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectAgentLinkResponse {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub linked_by_user_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateProjectAgentLinkRequest {
    pub agent_id: String,
}
