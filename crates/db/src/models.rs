use std::{fmt, str::FromStr};

use crate::CreateDomainEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub settings: String,
    pub primary_repo_id: Option<String>,
    pub paused_at: Option<String>,
    pub owner_id: Option<String>,
    pub project_hooks_json: String,
    pub project_work_epoch: i64,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTaskRoleDefaults {
    pub project_id: String,
    pub version: i64,
    pub roles: Vec<ProjectTaskRoleDefault>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCopilotBinding {
    pub project_id: String,
    pub agent_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectCopilotRunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl fmt::Display for ProjectCopilotRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        })
    }
}

impl FromStr for ProjectCopilotRunStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(format!("unknown Project Copilot run status: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCopilotRun {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub user_id: String,
    pub binding_version: i64,
    pub agent_version: i64,
    pub agent_snapshot_json: String,
    pub context_task_id: Option<String>,
    pub status: ProjectCopilotRunStatus,
    pub idempotency_key: String,
    pub request_digest: String,
    pub mcp_token_hash: Option<String>,
    pub mcp_token_expires_at: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCopilotMessage {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub role: String,
    pub actor_type: String,
    pub actor_id: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCopilotRunGrant {
    pub run_id: String,
    pub project_id: String,
    pub agent_id: String,
    pub user_id: String,
    pub binding_version: i64,
    pub context_task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCopilotRun {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub user_id: String,
    pub binding_version: i64,
    pub agent_version: i64,
    pub agent_snapshot_json: String,
    pub context_task_id: Option<String>,
    pub idempotency_key: String,
    pub request_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectCopilotMessage {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub role: String,
    pub actor_type: String,
    pub actor_id: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCopilotRunAdmission {
    pub run: ProjectCopilotRun,
    pub user_message: ProjectCopilotMessage,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotMessagePageQuery {
    pub cursor: Option<String>,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTaskRoleDefault {
    pub role: String,
    pub coordination_mode: CoordinationMode,
    pub configuration_json: String,
    pub members: Vec<ActorRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectHookRunStatus {
    Queued,
    Running,
    Dispatched,
    Skipped,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHookRun {
    pub id: String,
    pub project_id: String,
    pub rule_id: String,
    pub trigger_type: String,
    pub dedupe_key: String,
    pub status: ProjectHookRunStatus,
    pub source_task_id: Option<String>,
    pub source_execution_id: Option<String>,
    pub automation_task_id: Option<String>,
    pub execution_id: Option<String>,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectHookRun {
    pub id: String,
    pub project_id: String,
    pub rule_id: String,
    pub trigger_type: String,
    pub dedupe_key: String,
    pub status: ProjectHookRunStatus,
    pub source_task_id: Option<String>,
    pub source_execution_id: Option<String>,
    pub automation_task_id: Option<String>,
    pub execution_id: Option<String>,
    pub agent_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateProjectHookRun {
    pub id: String,
    pub status: ProjectHookRunStatus,
    // Outer Some means "update this column"; inner None writes SQL NULL.
    pub automation_task_id: Option<Option<String>>,
    pub execution_id: Option<Option<String>>,
    pub agent_id: Option<Option<String>>,
    pub reason: Option<Option<String>>,
    pub updated_at: String,
    pub completed_at: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrationPlatform {
    Github,
    Gitea,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIntegration {
    pub id: String,
    pub project_id: String,
    pub platform: IntegrationPlatform,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub token_secret_ref: String,
    pub poll_interval_secs: i64,
    pub sync_filter: String,
    pub default_implementer_kind: Option<String>,
    pub default_implementer_id: Option<String>,
    pub enabled: bool,
    pub last_polled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskExternalLink {
    pub id: String,
    pub task_id: String,
    pub integration_id: String,
    pub platform: String,
    pub remote_owner: String,
    pub remote_repo: String,
    pub remote_issue_number: i64,
    pub remote_url: String,
    pub global_id: String,
    pub synced_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectIntegration {
    pub id: String,
    pub project_id: String,
    pub platform: IntegrationPlatform,
    pub base_url: String,
    pub owner: String,
    pub repo: String,
    pub token_secret_ref: String,
    pub poll_interval_secs: i64,
    pub sync_filter: String,
    pub default_implementer_kind: Option<String>,
    pub default_implementer_id: Option<String>,
    pub enabled: bool,
    pub last_polled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateProjectIntegration {
    pub id: String,
    pub updated_at: String,
    pub project_id: Option<String>,
    pub platform: Option<IntegrationPlatform>,
    pub base_url: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub token_secret_ref: Option<String>,
    pub poll_interval_secs: Option<i64>,
    pub sync_filter: Option<String>,
    pub default_implementer_kind: Option<Option<String>>,
    pub default_implementer_id: Option<Option<String>>,
    pub enabled: Option<bool>,
    pub last_polled_at: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskExternalLink {
    pub id: String,
    pub task_id: String,
    pub integration_id: String,
    pub platform: String,
    pub remote_owner: String,
    pub remote_repo: String,
    pub remote_issue_number: i64,
    pub remote_url: String,
    pub global_id: String,
    pub synced_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub remote_url: String,
    pub local_path: Option<String>,
    pub work_mode: WorkMode,
    pub default_branch: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkMode {
    DirectMerge,
    PullRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrProviderConfig {
    pub id: String,
    pub repo_id: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub polling_interval_seconds: i64,
    /// Retired for PR use (historical data); the PR credential is
    /// `credential_handle_id` (V173, D4).
    pub token_secret_ref: Option<String>,
    /// `credential_handle` (`provider = 'github'`) whose secret authenticates
    /// PR provider calls; always set together with `credential_owner_user_id`.
    pub credential_handle_id: Option<String>,
    pub credential_owner_user_id: Option<String>,
    /// Versioned feedback policy JSON (schema_version 1); see `PrFeedbackPolicy`.
    pub feedback_policy_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrMetadata {
    pub id: String,
    pub task_id: String,
    pub provider_type: String,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub pr_state: String,
    pub merge_status: String,
    pub admission_status: String,
    pub task_merge_operation_id: Option<String>,
    pub publish_operation_id: Option<String>,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Immutable provider, repository, branch, and commit identity captured when
/// a remote TaskMerge is admitted. Mutable observation fields point at the
/// exact provider result event currently attached to that admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrAdmission {
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub metadata_id: String,
    pub task_id: String,
    pub provider_config_id: String,
    pub provider_type: String,
    pub provider_config_revision: String,
    pub provider_config_digest: String,
    pub provider_base_url: Option<String>,
    pub token_secret_ref: Option<String>,
    pub remote_repo_identity: String,
    pub source_branch: String,
    pub target_branch: String,
    pub admitted_source_sha: String,
    /// `Some("superseded_for_rework")` once the admission was paused for
    /// same-PR rework (V175); `state` then stays at the last provider-observed
    /// value and the admission is excluded from every "open admission" query.
    pub rework_outcome: Option<String>,
    pub supersession_id: Option<String>,
    /// The paused `pr_rework_supersession` this admission re-admits (V175).
    pub readmits_supersession_id: Option<String>,
    /// Credential reference frozen at admission (V173). `None` on historical
    /// admissions and when the repo had no credential configured.
    pub credential_handle_id: Option<String>,
    pub credential_owner_user_id: Option<String>,
    /// Feedback policy frozen at admission (V173); `None` on historical
    /// admissions (no ingestion, no external action is authorized for them).
    pub feedback_policy_json: Option<String>,
    pub feedback_policy_digest: Option<String>,
    pub state: String,
    /// The outcome the provider reported, independent from Forge's integrity
    /// classification of whether it affected the admitted subject.
    pub provider_status: Option<String>,
    pub result_classification: Option<String>,
    pub reconciliation_reason: Option<String>,
    pub provider_event_id: Option<String>,
    pub observed_head_sha: Option<String>,
    pub merged_commit_sha: Option<String>,
    pub result_event_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub profile_id: String,
    pub backend_kind: String,
    pub executor_type: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub tool_policy_json: String,
    pub config_json: String,
    pub credential_ref: Option<String>,
    pub daemon_id: Option<String>,
    pub max_concurrent_tasks: i64,
    pub heartbeat_interval_seconds: i64,
    pub max_missed_heartbeats: i64,
    pub status: AgentStatus,
    pub last_heartbeat_at: Option<String>,
    pub is_default: bool,
    pub paused: bool,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub id: String,
    pub identity_id: String,
    pub backend_kind: String,
    pub executor_type: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub tool_policy_json: String,
    pub config_json: String,
    pub credential_ref: Option<String>,
    pub daemon_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialHandle {
    pub id: String,
    pub owner_user_id: String,
    pub provider: String,
    pub label: String,
    pub status: String,
    pub credential_method: String,
    pub metadata_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialUsage {
    pub credential_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub runtime: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAuthorizationOperation {
    pub id: String,
    pub owner_user_id: String,
    pub provider: String,
    pub method: String,
    pub status: String,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub redirect_origin: String,
    pub callback_state_hash: Option<String>,
    pub request_json: String,
    pub poll_interval_seconds: i64,
    pub expires_at: String,
    pub profile_id: Option<String>,
    pub credential_handle_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProviderAuthorizationOperation {
    pub id: String,
    pub owner_user_id: String,
    pub provider: String,
    pub method: String,
    pub status: String,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub redirect_origin: String,
    pub callback_state_hash: Option<String>,
    pub request_json: String,
    pub poll_interval_seconds: i64,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateProviderAuthorizationOperation {
    pub id: String,
    pub expected_version: i64,
    pub status: String,
    pub authorization_url: Option<String>,
    pub user_code: Option<String>,
    pub poll_interval_seconds: i64,
    pub profile_id: Option<String>,
    pub credential_handle_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainEvent {
    pub sequence: i64,
    pub id: String,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub scope_type: String,
    pub scope_id: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub causation_depth: i64,
    pub dedupe_key: Option<String>,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrchestratorWakeState {
    Pending,
    Leased,
    Running,
    AwaitingHuman,
    Completed,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorWake {
    pub id: String,
    pub event_id: String,
    pub event_sequence: i64,
    pub task_id: String,
    pub task_role_id: String,
    pub coordination_mode: Option<CoordinationMode>,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    pub work_unit_id: Option<String>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub causation_depth: i64,
    pub policy_ref: String,
    pub policy_version: i64,
    pub policy_digest: String,
    pub task_role_version: i64,
    pub task_role_policy_json: String,
    pub state: OrchestratorWakeState,
    pub available_at: String,
    pub lease_owner: Option<String>,
    pub lease_until: Option<String>,
    pub attempt_count: i64,
    pub current_attempt: Option<i64>,
    pub last_error: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorWakeExecution {
    pub wake_id: String,
    pub attempt_number: i64,
    pub execution_id: String,
    pub state: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorActionRecord {
    pub execution_id: String,
    pub action_index: i64,
    pub action_type: String,
    pub action_digest: String,
    pub result_id: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventConsumerCursor {
    pub consumer_name: String,
    pub last_sequence: i64,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub status: WorkspaceStatus,
    pub before_sha: Option<String>,
    pub cleanup_after: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceStatus {
    Creating,
    Ready,
    Error,
    Cleaning,
    Cleaned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitStatus {
    Open,
    Completed,
    Cancelled,
}

impl fmt::Display for WorkUnitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        })
    }
}

impl FromStr for WorkUnitStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "open" => Ok(Self::Open),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown WorkUnit status: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnit {
    pub id: String,
    pub task_id: String,
    pub parent_work_unit_id: Option<String>,
    pub title: String,
    pub scope: String,
    pub execution_instructions: Option<String>,
    pub status: WorkUnitStatus,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub requires_integration: bool,
    pub provenance: Option<WorkUnitProvenance>,
    pub review_approval_gate_id: Option<String>,
    pub review_approval_line_id: Option<String>,
    pub created_by: ActorRef,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Typed provenance for a WorkUnit. `LegacyActor` represents a V091 Actor
/// provenance row whose untyped ID could not be resolved unambiguously by
/// V092; new writes must use `Actor(ActorRef)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkUnitProvenance {
    Actor(ActorRef),
    WorkUnit(String),
    Artifact(String),
    External(String),
    LegacyActor(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitDependency {
    pub task_id: String,
    pub work_unit_id: String,
    pub depends_on_work_unit_id: String,
    pub created_by: ActorRef,
    pub created_at: String,
    pub satisfied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkUnitIntegrationOutcome {
    Running,
    Success,
    Conflict,
    Failed,
    Rejected,
}

impl fmt::Display for WorkUnitIntegrationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
        })
    }
}

impl FromStr for WorkUnitIntegrationOutcome {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "conflict" => Ok(Self::Conflict),
            "failed" => Ok(Self::Failed),
            "rejected" => Ok(Self::Rejected),
            other => Err(format!("unknown WorkUnit integration outcome: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskIntegrationOperationKind {
    WorkUnitIntegration,
    TaskMerge,
    PublishPr,
    WorkUnitCreate,
    WorkUnitWorkspacePrepare,
    IntegrationWorkspaceCleanup,
}

impl fmt::Display for TaskIntegrationOperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WorkUnitIntegration => "work_unit_integration",
            Self::TaskMerge => "task_merge",
            Self::PublishPr => "publish_pr",
            Self::WorkUnitCreate => "work_unit_create",
            Self::WorkUnitWorkspacePrepare => "work_unit_workspace_prepare",
            Self::IntegrationWorkspaceCleanup => "integration_workspace_cleanup",
        })
    }
}

impl FromStr for TaskIntegrationOperationKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "work_unit_integration" => Ok(Self::WorkUnitIntegration),
            "task_merge" => Ok(Self::TaskMerge),
            "publish_pr" => Ok(Self::PublishPr),
            "work_unit_create" => Ok(Self::WorkUnitCreate),
            "work_unit_workspace_prepare" => Ok(Self::WorkUnitWorkspacePrepare),
            "integration_workspace_cleanup" => Ok(Self::IntegrationWorkspaceCleanup),
            other => Err(format!("unknown Task integration operation kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskIntegrationOperationStatus {
    Running,
    Succeeded,
    Conflict,
    Failed,
    Abandoned,
}

impl fmt::Display for TaskIntegrationOperationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        })
    }
}

impl FromStr for TaskIntegrationOperationStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "conflict" => Ok(Self::Conflict),
            "failed" => Ok(Self::Failed),
            "abandoned" => Ok(Self::Abandoned),
            other => Err(format!(
                "unknown Task integration operation status: {other}"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIntegrationOperation {
    pub id: String,
    pub task_id: String,
    pub kind: TaskIntegrationOperationKind,
    pub owner_id: String,
    pub status: TaskIntegrationOperationStatus,
    pub gate_evaluation_id: Option<String>,
    pub remote_waiting: bool,
    pub parent_operation_id: Option<String>,
    pub result_event_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitIntegration {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    pub execution_id: String,
    pub source_workspace_id: String,
    pub source_branch: String,
    pub source_sha: String,
    pub target_workspace_id: String,
    pub target_branch: String,
    pub target_before_sha: String,
    pub target_after_sha: Option<String>,
    pub operation_idempotency_key: String,
    pub outcome: WorkUnitIntegrationOutcome,
    pub conflict_metadata_json: Option<String>,
    pub version: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScopeKind {
    Integration,
    WorkUnit,
}

impl fmt::Display for WorkspaceScopeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Integration => "integration",
            Self::WorkUnit => "work_unit",
        })
    }
}

impl FromStr for WorkspaceScopeKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "integration" => Ok(Self::Integration),
            "work_unit" => Ok(Self::WorkUnit),
            other => Err(format!("unknown Workspace scope: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceScope {
    pub workspace_id: String,
    pub task_id: String,
    pub kind: WorkspaceScopeKind,
    pub work_unit_id: Option<String>,
    pub created_at: String,
}

/// Scheduler-issued authority for one assigned Task/repository operation.
/// This row is internal: callers receive neither its capability payload nor
/// any filesystem path or bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLease {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    /// Set only for an explicitly WorkUnit-scoped lease. Legacy Task leases
    /// keep both WorkUnit and Workspace bindings empty.
    pub work_unit_id: Option<String>,
    pub workspace_id: Option<String>,
    /// Exact Task revision admitted for this execution attempt.
    pub task_version: i64,
    /// Execution identity for the attempt; a lease cannot be replayed by a
    /// later execution of the same Task.
    pub execution_id: String,
    /// Stable operation key for replaying one scheduler lease issue.
    pub operation_idempotency_key: String,
    pub repository_binding_id: String,
    pub base_ref: String,
    pub role: String,
    pub capabilities_json: String,
    pub assigned_principal_type: String,
    pub assigned_principal_id: String,
    pub capability_profile_revision: String,
    pub capability_profile_digest: String,
    pub issuing_principal_type: String,
    pub issuing_principal_id: String,
    pub status: String,
    pub issued_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Daemon {
    pub id: String,
    pub machine_id: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: Option<String>,
    pub labels_json: String,
    pub status: DaemonStatus,
    pub last_report_at: Option<String>,
    pub registration_token_hash: Option<String>,
    pub detected_clis_json: String,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStatus {
    Online,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub id: String,
    pub daemon_id: String,
    pub kind: String,
    pub workspace_root: String,
    pub status: RuntimeStatus,
    pub labels_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    Ready,
    Degraded,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Busy,
    Error,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub event_type: String,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateNotification {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub event_type: String,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub lifecycle_state: TaskLifecycleState,
    pub is_automation: bool,
    pub priority: i64,
    pub board_position: f64,
    pub subtask_order: Option<i64>,
    pub merge_config: Option<String>,
    pub metadata_json: Option<String>,
    pub error_annotation: Option<String>,
    pub blocked_json: Option<String>,
    pub failed_json: Option<String>,
    pub deleted_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub actor_kind: Option<ActorKind>,
    pub actor_id: Option<String>,
    pub role: String,
    pub status: ExecutionStatus,
    pub stop_reason: Option<StopReason>,
    pub stopped_by: Option<String>,
    pub resume_policy: Option<ResumePolicy>,
    pub stopped_at: Option<String>,
    pub parent_execution_id: Option<String>,
    pub harness_session_id: Option<String>,
    pub agent_message_id: Option<String>,
    pub last_activity_at: Option<String>,
    pub prompt: Option<String>,
    pub summary: Option<String>,
    pub logs_path: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub error: Option<String>,
    pub executor_config_snapshot_json: Option<String>,
    pub workspace_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub work_unit_version: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Immutable Workspace, commit, and working-tree identity reviewed by one
/// workspace-bound reviewer Execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewExecutionSubject {
    pub execution_id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub base_commit_sha: String,
    pub head_commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateReviewExecutionSubject {
    pub execution_id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub base_commit_sha: String,
    pub head_commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewExecutionSubjectWrite {
    pub execution: Execution,
    pub subject: ReviewExecutionSubject,
    /// Present only when the subject was first frozen with its domain event.
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessSession {
    pub id: String,
    pub agent_id: String,
    pub harness_kind: String,
    pub external_session_id: Option<String>,
    pub profile_id: Option<String>,
    pub profile_snapshot_json: String,
    pub capabilities_snapshot_json: String,
    pub workspace_id: Option<String>,
    pub status: HarnessSessionStatus,
    pub predecessor_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateHarnessSession {
    pub id: String,
    pub agent_id: String,
    pub harness_kind: String,
    pub external_session_id: Option<String>,
    pub profile_id: Option<String>,
    pub profile_snapshot_json: String,
    pub capabilities_snapshot_json: String,
    pub workspace_id: Option<String>,
    pub status: HarnessSessionStatus,
    pub predecessor_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_activity_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateHarnessSession {
    pub id: String,
    pub status: Option<HarnessSessionStatus>,
    pub last_activity_at: Option<Option<String>>,
    pub updated_at: String,
}

impl Execution {
    pub fn actor_ref(&self) -> Option<ActorRef> {
        match (self.actor_kind, self.actor_id.as_deref()) {
            (Some(ActorKind::Human), Some(id)) => Some(ActorRef::Human(id.to_owned())),
            (Some(ActorKind::Agent), Some(id)) => Some(ActorRef::Agent(id.to_owned())),
            _ => None,
        }
    }
}

pub type ExecutionRole = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRunStatus {
    Running,
    Passed,
    Failed,
    Error,
    Cancelled,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRun {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub caused_by_execution_id: Option<String>,
    pub check_identity: String,
    pub command: String,
    pub config_summary_json: String,
    pub config_digest: String,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub idempotency_key: String,
    pub status: ValidationRunStatus,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub logs_ref: Option<String>,
    /// Catalog provenance (V171). `LegacyCommand` rows carry no catalog identity.
    pub catalog_check_id: Option<String>,
    pub definition_digest: Option<String>,
    pub run_origin: ValidationRunOrigin,
    pub check_request_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateValidationRun {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub caused_by_execution_id: Option<String>,
    pub check_identity: String,
    pub command: String,
    pub config_summary_json: String,
    pub config_digest: String,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub idempotency_key: String,
    /// All-or-nothing with `check_request_id`/`definition_digest`; use
    /// `ValidationRunOrigin::LegacyCommand` with `None` for the historical path.
    pub catalog_check_id: Option<String>,
    pub definition_digest: Option<String>,
    pub run_origin: ValidationRunOrigin,
    pub check_request_id: Option<String>,
    pub started_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRunStartWrite {
    pub validation_run: ValidationRun,
    /// `None` means an identical idempotent replay returned the existing run.
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub id: String,
    pub task_id: String,
    pub kind: String,
    pub content_json: String,
    pub digest: String,
    pub producer_validation_run_id: String,
    pub evidence_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateEvidence {
    pub id: String,
    pub task_id: String,
    pub validation_run_id: String,
    pub evidence_key: String,
    pub kind: String,
    pub content_json: String,
    pub digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateValidationRunArtifact {
    pub id: String,
    pub task_id: String,
    pub validation_run_id: String,
    pub content: String,
    pub metadata_json: String,
    pub digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishValidationRun {
    pub id: String,
    pub claim_owner: String,
    pub status: ValidationRunStatus,
    pub exit_code: Option<i32>,
    pub finished_at: String,
    pub logs_ref: String,
    pub evidence: Vec<CreateEvidence>,
    pub validation_report: Option<CreateValidationRunArtifact>,
    pub events: Vec<crate::CreateDomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRunCompletionWrite {
    pub validation_run: ValidationRun,
    pub evidence: Vec<Evidence>,
    pub validation_report: Option<Artifact>,
    /// Events are returned only when this call committed the terminal result.
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionEvidenceInput {
    pub execution_id: String,
    pub evidence_id: String,
    pub task_id: String,
    pub digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessSessionStatus {
    Pending,
    Active,
    Ended,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumePolicy {
    Auto,
    Manual,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentAuthorType {
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskComment {
    pub id: String,
    pub task_id: String,
    pub author_type: CommentAuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMedia {
    pub id: String,
    pub task_id: String,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub storage_key: String,
    pub author_type: CommentAuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub created_at: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskMedia {
    pub id: String,
    pub task_id: String,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub storage_key: String,
    pub author_type: CommentAuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub created_at: String,
}

/// Project-owned metadata for one existing task media blob.
///
/// The `id`, `storage_key`, and legacy task media reference are deliberately
/// kept separate from the Project evidence attachment.  This lets Project
/// evidence and immutable release pins reuse the exact bytes without changing
/// the historical Task media API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAsset {
    pub id: String,
    pub project_id: String,
    pub legacy_task_media_id: Option<String>,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub storage_key: String,
    pub checksum: Option<String>,
    pub availability: String,
    pub gc_state: String,
    pub gc_candidate_at: Option<String>,
    pub gc_lease_owner: Option<String>,
    pub gc_lease_expires_at: Option<String>,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Project media upload metadata.  The bytes are written by the API's
/// storage adapter; this record makes the metadata insert and its durable
/// idempotency event one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMediaAsset {
    pub id: String,
    pub project_id: String,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub storage_key: String,
    pub checksum: String,
    pub idempotency_key: String,
    pub mutation_fingerprint: String,
    pub expected_project_version: i64,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub authorization_event_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginProjectMediaUpload {
    pub project_id: String,
    pub idempotency_key: String,
    pub mutation_fingerprint: String,
    pub expected_project_version: i64,
    pub asset_id: String,
    pub final_storage_key: String,
    pub staging_storage_key: String,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub checksum: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMediaUpload {
    pub project_id: String,
    pub idempotency_key: String,
    pub mutation_fingerprint: String,
    pub expected_project_version: i64,
    pub asset_id: String,
    pub final_storage_key: String,
    pub staging_storage_key: Option<String>,
    pub display_filename: String,
    pub content_type: String,
    pub byte_size: i64,
    pub checksum: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMediaTombstone {
    pub asset_id: String,
    pub project_id: String,
    pub expected_version: i64,
    pub idempotency_key: String,
    pub mutation_fingerprint: String,
    pub target_availability: String,
    pub principal_type: String,
    pub principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub authorization_event_id: String,
    pub authorization_json: String,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectReleaseMediaPin {
    pub id: String,
    pub project_id: String,
    pub release_id: String,
    pub asset_id: String,
    pub attachment_id: Option<String>,
    pub legacy_task_media_id: Option<String>,
    pub asset_checksum: String,
    pub attachment_digest: String,
    pub availability: String,
    pub pin_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReleaseMediaPin {
    pub id: String,
    pub project_id: String,
    pub release_id: String,
    pub asset_id: String,
    pub attachment_id: Option<String>,
    pub legacy_task_media_id: Option<String>,
    pub asset_checksum: String,
    pub attachment_digest: String,
    pub availability: String,
    pub pin_digest: String,
    pub created_at: String,
}

/// Stored immutable Release identity and raw snapshot fields.
///
/// This record is read only. Project OS writers were retired before the
/// historical Release reader was detached from their source tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReleaseRecord {
    pub id: String,
    pub project_id: String,
    pub milestone_id: String,
    pub release_sequence: i64,
    pub release_revision: i64,
    pub release_identifier: String,
    pub milestone_revision_id: String,
    pub readiness_snapshot_id: String,
    pub readiness_digest: String,
    pub baseline_id: String,
    pub baseline_revision_id: String,
    pub baseline_digest: String,
    pub release_policy_revision: String,
    pub release_policy_digest: String,
    pub summary: String,
    pub changelog: String,
    pub known_issues_json: String,
    pub charter_revision_id: Option<String>,
    pub document_revisions_json: String,
    pub decision_ids_json: String,
    pub task_references_json: String,
    pub validation_references_json: String,
    pub git_references_json: String,
    pub evidence_references_json: String,
    pub waivers_json: String,
    pub releasing_principal_type: String,
    pub releasing_principal_id: String,
    pub authorization_basis: String,
    pub authorization_action: String,
    pub authorization_occurred_at: String,
    pub explicit_event: String,
    pub schema_version: String,
    pub snapshot_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalSessionStatus {
    Starting,
    Running,
    Exited,
    Terminated,
    TimedOut,
    Orphaned,
    CleanupTerminated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession {
    pub id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub daemon_id: Option<String>,
    pub status: TerminalSessionStatus,
    pub rows: i64,
    pub cols: i64,
    pub pid: Option<i64>,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<String>,
    pub exit_reason: Option<String>,
    pub created_by_user_id: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub ended_at: Option<String>,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTerminalSession {
    pub id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub daemon_id: Option<String>,
    pub created_by_user_id: String,
    pub rows: i64,
    pub cols: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTerminalSessionStatus {
    pub status: TerminalSessionStatus,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub ended_at: Option<String>,
    pub pid: Option<i64>,
    pub exit_code: Option<i64>,
    pub exit_signal: Option<String>,
    pub exit_reason: Option<String>,
}

macro_rules! enum_strings {
    ($enum:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        impl fmt::Display for $enum {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                let value = match self {
                    $(Self::$variant => $value,)+
                };
                formatter.write_str(value)
            }
        }

        impl FromStr for $enum {
            type Err = String;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(format!("invalid {} value: {value}", stringify!($enum))),
                }
            }
        }
    };
}

enum_strings!(ValidationRunStatus {
    Running => "running",
    Passed => "passed",
    Failed => "failed",
    Error => "error",
    Cancelled => "cancelled",
    Stale => "stale",
});

enum_strings!(OrchestratorWakeState {
    Pending => "pending",
    Leased => "leased",
    Running => "running",
    AwaitingHuman => "awaiting_human",
    Completed => "completed",
    Failed => "failed",
    Uncertain => "uncertain",
});

enum_strings!(WorkMode {
    DirectMerge => "direct_merge",
    PullRequest => "pull_request",
});

enum_strings!(IntegrationPlatform {
    Github => "github",
    Gitea => "gitea",
});

enum_strings!(AgentStatus {
    Idle => "idle",
    Busy => "busy",
    Error => "error",
    Offline => "offline",
});

enum_strings!(WorkspaceStatus {
    Creating => "creating",
    Ready => "ready",
    Error => "error",
    Cleaning => "cleaning",
    Cleaned => "cleaned",
});

enum_strings!(DaemonStatus {
    Online => "online",
    Offline => "offline",
});

enum_strings!(RuntimeStatus {
    Ready => "ready",
    Degraded => "degraded",
    Offline => "offline",
});

enum_strings!(ExecutionStatus {
    Running => "running",
    Completed => "completed",
    Failed => "failed",
    Cancelled => "cancelled",
});

enum_strings!(HarnessSessionStatus {
    Pending => "pending",
    Active => "active",
    Ended => "ended",
    Failed => "failed",
});

enum_strings!(StopReason {
    UserCancelled => "user_cancelled",
    TaskCancelled => "task_cancelled",
    RoleReassigned => "role_reassigned",
    GracefulShutdown => "graceful_shutdown",
    CrashRecovery => "crash_recovery",
    AgentTimeout => "agent_timeout",
    ExecutionStalled => "execution_stalled",
    DaemonDisconnected => "daemon_disconnected",
    ExecutorCancelled => "executor_cancelled",
    ExecutorFailed => "executor_failed",
    LegacyUnknown => "legacy_unknown",
});

enum_strings!(ResumePolicy {
    Auto => "auto",
    Manual => "manual",
    None => "none",
});

enum_strings!(ProjectHookRunStatus {
    Queued => "queued",
    Running => "running",
    Dispatched => "dispatched",
    Skipped => "skipped",
    Failed => "failed",
    Completed => "completed",
});

enum_strings!(CommentAuthorType {
    User => "user",
    Agent => "agent",
    System => "system",
});

enum_strings!(TerminalSessionStatus {
    Starting => "starting",
    Running => "running",
    Exited => "exited",
    Terminated => "terminated",
    TimedOut => "timed_out",
    Orphaned => "orphaned",
    CleanupTerminated => "cleanup_terminated",
});

/// The two persisted Actor identities that may participate in a TaskRole.
/// This is intentionally separate from `api_types::Actor`, whose `System`
/// variant records the source of a transition rather than a durable
/// participant identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ActorRef {
    Human(String),
    Agent(String),
}

impl ActorRef {
    pub fn kind(&self) -> ActorKind {
        match self {
            Self::Human(_) => ActorKind::Human,
            Self::Agent(_) => ActorKind::Agent,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Human(id) | Self::Agent(id) => id,
        }
    }
}

/// Translate transitional workflow labels at the TaskRole boundary.  The
/// workflow engine may continue to use its existing labels during PR1; only
/// the persisted replacement model uses the target responsibility vocabulary.
pub fn canonical_task_role_name(role: &str) -> Option<String> {
    let normalized = role.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    match normalized.as_str() {
        "coder" | "worker" | "assignee" | "executor" => Some("implementer".to_owned()),
        "planner" => Some("planner".to_owned()),
        "reviewer" => Some("reviewer".to_owned()),
        "orchestrator" => Some("orchestrator".to_owned()),
        "interactive" | "merge_fixer" | "system" => None,
        custom => Some(custom.to_owned()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Human,
    Agent,
}

impl fmt::Display for ActorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Human => "human",
            Self::Agent => "agent",
        })
    }
}

impl FromStr for ActorKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "human" => Ok(Self::Human),
            "agent" => Ok(Self::Agent),
            other => Err(format!("unknown actor kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationMode {
    Partitioned,
    Collaborative,
    Independent,
}

impl fmt::Display for CoordinationMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Partitioned => "partitioned",
            Self::Collaborative => "collaborative",
            Self::Independent => "independent",
        })
    }
}

impl FromStr for CoordinationMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "partitioned" => Ok(Self::Partitioned),
            "collaborative" => Ok(Self::Collaborative),
            "independent" => Ok(Self::Independent),
            other => Err(format!("unknown coordination mode: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleMembershipStatus {
    Active,
    Suspended,
    Ended,
}

impl fmt::Display for RoleMembershipStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Ended => "ended",
        })
    }
}

impl FromStr for RoleMembershipStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "ended" => Ok(Self::Ended),
            other => Err(format!("unknown role membership status: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRole {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub coordination_mode: Option<CoordinationMode>,
    pub policy_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRoleCapabilityConfiguration {
    pub task_role_id: String,
    pub task_role_version: i64,
    pub configuration_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionTaskRoleAuthority {
    pub execution_id: String,
    pub task_role_id: String,
    pub task_role_version: Option<i64>,
    pub configuration_json: String,
    pub capabilities_json: String,
    pub membership_id: String,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    pub source: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateTaskRole {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub coordination_mode: Option<CoordinationMode>,
    pub policy_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTaskRole {
    pub id: String,
    pub expected_version: i64,
    pub coordination_mode: Option<Option<CoordinationMode>>,
    pub policy_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTaskRoleWithConfiguration {
    pub id: String,
    pub expected_version: i64,
    pub coordination_mode: Option<Option<CoordinationMode>>,
    pub policy_json: Option<String>,
    pub configuration_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleMembership {
    pub id: String,
    pub task_role_id: String,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    pub status: RoleMembershipStatus,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

impl RoleMembership {
    pub fn actor_ref(&self) -> ActorRef {
        match self.actor_kind {
            ActorKind::Human => ActorRef::Human(self.actor_id.clone()),
            ActorKind::Agent => ActorRef::Agent(self.actor_id.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoleMembership {
    pub id: String,
    pub task_role_id: String,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    pub status: RoleMembershipStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRoleMembership {
    pub id: String,
    pub expected_version: i64,
    pub status: RoleMembershipStatus,
    pub updated_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionUsage {
    pub id: String,
    pub execution_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub display_name: Option<String>,
    pub is_admin: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshToken {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub family_id: String,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClient {
    pub id: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub redirect_uris_json: String,
    pub token_endpoint_auth_method: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOAuthClient {
    pub id: String,
    pub client_id: String,
    pub client_name: Option<String>,
    pub redirect_uris_json: String,
    pub token_endpoint_auth_method: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthAuthorizationCode {
    pub id: String,
    pub code_hash: String,
    pub user_id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: String,
    pub scopes: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOAuthAuthorizationCode {
    pub id: String,
    pub code_hash: String,
    pub user_id: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub resource: String,
    pub scopes: String,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthRefreshToken {
    pub id: String,
    pub token_hash: String,
    pub family_id: String,
    pub user_id: String,
    pub client_id: String,
    pub resource: String,
    pub scopes: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOAuthRefreshToken {
    pub id: String,
    pub token_hash: String,
    pub family_id: String,
    pub user_id: String,
    pub client_id: String,
    pub resource: String,
    pub scopes: String,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalAccessToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub token_hash: String,
    pub prefix: String,
    pub scopes: String,
    pub expires_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePersonalAccessToken {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub token_hash: String,
    pub prefix: String,
    pub scopes: String,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub id: String,
    pub project_id: String,
    pub user_id: String,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectMember {
    pub id: String,
    pub project_id: String,
    pub user_id: String,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStorageKind {
    Inline,
    External,
}

enum_strings!(ArtifactStorageKind {
    Inline => "inline",
    External => "external",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plan,
    WorkUnitPlan,
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

enum_strings!(ArtifactKind {
    Plan => "plan",
    WorkUnitPlan => "work_unit_plan",
    ReviewReport => "review_report",
    ValidationReport => "validation_report",
    Diff => "diff",
    Patch => "patch",
    Summary => "summary",
    DesignDocument => "design_document",
    Investigation => "investigation",
    ApiContract => "api_contract",
    TestReport => "test_report",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationTargetKind {
    Actor,
    Role,
    Task,
}

enum_strings!(CollaborationTargetKind {
    Actor => "actor",
    Role => "role",
    Task => "task",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollaborationTarget {
    Actor(ActorRef),
    Role(String),
    Task,
}

impl CollaborationTarget {
    pub fn kind(&self) -> CollaborationTargetKind {
        match self {
            Self::Actor(_) => CollaborationTargetKind::Actor,
            Self::Role(_) => CollaborationTargetKind::Role,
            Self::Task => CollaborationTargetKind::Task,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffIntent {
    Rework,
    Delegation,
    Question,
    Answer,
    Investigate,
    DecisionRequest,
}

enum_strings!(HandoffIntent {
    Rework => "rework",
    Delegation => "delegation",
    Question => "question",
    Answer => "answer",
    Investigate => "investigate",
    DecisionRequest => "decision_request",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    Pending,
    Accepted,
    Completed,
    Declined,
    Cancelled,
}

enum_strings!(HandoffStatus {
    Pending => "pending",
    Accepted => "accepted",
    Completed => "completed",
    Declined => "declined",
    Cancelled => "cancelled",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalTargetKind {
    Task,
    Execution,
    Workspace,
    WorkUnit,
    /// A `human_approval` PR external action (V177; plan node P3).
    PrExternalAction,
}

enum_strings!(ProposalTargetKind {
    Task => "task",
    Execution => "execution",
    Workspace => "workspace",
    WorkUnit => "work_unit",
    PrExternalAction => "pr_external_action",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalTarget {
    pub kind: ProposalTargetKind,
    /// Task targets use the owning Task id; execution/workspace targets use
    /// their own ids and are validated against that same Task.
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Open,
    Resolved,
    Withdrawn,
    Superseded,
}

enum_strings!(ProposalStatus {
    Open => "open",
    Resolved => "resolved",
    Withdrawn => "withdrawn",
    Superseded => "superseded",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    Approve,
    Reject,
    Supersede,
}

enum_strings!(DecisionOutcome {
    Approve => "approve",
    Reject => "reject",
    Supersede => "supersede",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: String,
    pub task_id: String,
    pub kind: ArtifactKind,
    pub storage_kind: ArtifactStorageKind,
    pub content: Option<String>,
    pub content_ref: Option<String>,
    pub metadata_json: String,
    pub digest: Option<String>,
    pub producer: ArtifactProducer,
    pub created_at: String,
}

impl Artifact {
    pub fn execution_producer(&self) -> Option<(&str, &ActorRef)> {
        match &self.producer {
            ArtifactProducer::Execution {
                execution_id,
                actor,
            } => Some((execution_id.as_str(), actor)),
            ArtifactProducer::ValidationRun { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArtifactProducer {
    Execution {
        execution_id: String,
        actor: ActorRef,
    },
    ValidationRun {
        validation_run_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateArtifact {
    pub id: String,
    pub task_id: String,
    pub kind: ArtifactKind,
    pub storage_kind: ArtifactStorageKind,
    pub content: Option<String>,
    pub content_ref: Option<String>,
    pub metadata_json: String,
    pub digest: Option<String>,
    pub producer_execution_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionArtifactOutputWrite {
    pub artifact: Artifact,
    /// Present only when this call committed the new Artifact and its event.
    /// Replays return the existing exact Artifact without emitting a second event.
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionArtifactInput {
    pub execution_id: String,
    pub artifact_id: String,
    pub task_id: String,
    pub digest: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub task_id: String,
    pub sender: ActorRef,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub body: String,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateMessage {
    pub id: String,
    pub task_id: String,
    pub sender: ActorRef,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub body: String,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateHandoff {
    pub id: String,
    pub task_id: String,
    pub created_by: ActorRef,
    pub source_role_id: Option<String>,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub intent: HandoffIntent,
    pub parent_execution_id: Option<String>,
    pub expected_policy_ref: Option<String>,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionHandoff {
    pub id: String,
    pub expected_version: i64,
    pub status: HandoffStatus,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProposal {
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
    pub supersedes_proposal_id: Option<String>,
    pub artifact_ids: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDecision {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationWrite<T> {
    pub record: T,
    pub event: DomainEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycleState {
    Backlog,
    Ready,
    Active,
    Blocked,
    ReadyToMerge,
    Merging,
    Done,
    Cancelled,
}

enum_strings!(TaskLifecycleState {
    Backlog => "backlog",
    Ready => "ready",
    Active => "active",
    Blocked => "blocked",
    ReadyToMerge => "ready_to_merge",
    Merging => "merging",
    Done => "done",
    Cancelled => "cancelled",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLifecycle {
    pub task_id: String,
    pub state: TaskLifecycleState,
    pub version: i64,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDispatchState {
    Idle,
    Running,
    Paused,
    Cancelled,
}

enum_strings!(TaskDispatchState {
    Idle => "idle",
    Running => "running",
    Paused => "paused",
    Cancelled => "cancelled",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskDispatchOperation {
    Launch,
    Pause,
    Resume,
    Cancel,
}

enum_strings!(TaskDispatchOperation {
    Launch => "launch",
    Pause => "pause",
    Resume => "resume",
    Cancel => "cancel",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDispatch {
    pub task_id: String,
    pub state: TaskDispatchState,
    pub version: i64,
    pub requested_by_kind: String,
    pub requested_by_id: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDispatchReceipt {
    pub id: String,
    pub task_id: String,
    pub operation: TaskDispatchOperation,
    pub idempotency_key: String,
    pub request_digest: String,
    pub intent_version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDispatchWrite {
    pub dispatch: TaskDispatch,
    pub receipt: TaskDispatchReceipt,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionTaskDispatch {
    pub task_id: String,
    pub operation: TaskDispatchOperation,
    pub state: TaskDispatchState,
    pub expected_intent_version: i64,
    pub requested_by_kind: String,
    pub requested_by_id: Option<String>,
    pub reason: Option<String>,
    pub idempotency_key: String,
    pub request_digest: String,
    pub receipt_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCoordinationMode {
    Manual,
    Proposal,
    PrepareAndExecute,
}

enum_strings!(TaskCoordinationMode {
    Manual => "manual",
    Proposal => "proposal",
    PrepareAndExecute => "prepare_and_execute",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCoordinationOperation {
    Authorize,
    Revoke,
}

enum_strings!(TaskCoordinationOperation {
    Authorize => "authorize",
    Revoke => "revoke",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordination {
    pub task_id: String,
    pub mode: TaskCoordinationMode,
    pub coordinator_task_role_id: Option<String>,
    pub coordinator_actor: Option<ActorRef>,
    pub authorization_version: i64,
    pub team_snapshot_json: String,
    pub team_digest: String,
    pub team_stale: bool,
    pub team_generation: i64,
    pub envelope_json: String,
    pub envelope_digest: String,
    pub requested_by: Option<String>,
    pub turn_count: i64,
    pub quality_request_count: i64,
    pub max_turns: i64,
    pub max_quality_requests: i64,
    pub created_at: String,
    pub updated_at: String,
    pub revoked_at: Option<String>,
    pub revoke_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationReceipt {
    pub id: String,
    pub task_id: String,
    pub operation: TaskCoordinationOperation,
    pub idempotency_key: String,
    pub request_digest: String,
    pub authorization_version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationWrite {
    pub coordination: TaskCoordination,
    pub receipt: TaskCoordinationReceipt,
    pub replayed: bool,
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeTaskCoordinationInput {
    pub task_id: String,
    pub expected_coordination_version: i64,
    pub expected_team_generation: i64,
    pub mode: TaskCoordinationMode,
    pub coordinator_task_role_id: Option<String>,
    pub coordinator_actor: Option<ActorRef>,
    pub team_snapshot_json: String,
    pub team_digest: String,
    pub envelope_json: String,
    pub envelope_digest: String,
    pub unconfigured_default_refresh: bool,
    pub requested_by: String,
    pub idempotency_key: String,
    pub request_digest: String,
    pub receipt_id: String,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeTaskCoordinationInput {
    pub task_id: String,
    pub expected_coordination_version: i64,
    pub reason: Option<String>,
    pub requested_by: String,
    pub idempotency_key: String,
    pub request_digest: String,
    pub receipt_id: String,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCoordinationRequestKind {
    WorkUnitPlan,
    ReviewFindings,
    ValidationFindings,
    ExternalFeedback,
    HumanDecisionResume,
}

enum_strings!(TaskCoordinationRequestKind {
    WorkUnitPlan => "work_unit_plan",
    ReviewFindings => "review_findings",
    ValidationFindings => "validation_findings",
    ExternalFeedback => "external_feedback",
    HumanDecisionResume => "human_decision_resume",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCoordinationRequestState {
    Requested,
    Running,
    PlanReady,
    AwaitingApproval,
    Materialized,
    Triaged,
    Failed,
    Cancelled,
    Uncertain,
}

enum_strings!(TaskCoordinationRequestState {
    Requested => "requested",
    Running => "running",
    PlanReady => "plan_ready",
    AwaitingApproval => "awaiting_approval",
    Materialized => "materialized",
    Triaged => "triaged",
    Failed => "failed",
    Cancelled => "cancelled",
    Uncertain => "uncertain",
});

impl TaskCoordinationRequestState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Materialized | Self::Triaged | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationRequest {
    pub id: String,
    pub task_id: String,
    pub kind: TaskCoordinationRequestKind,
    pub review_report_artifact_id: Option<String>,
    pub review_report_digest: Option<String>,
    pub review_producer_execution_id: Option<String>,
    /// Exact non-review source of validation_findings / external_feedback /
    /// human_decision_resume requests (V172). `None` for historical kinds.
    pub source_subject_kind: Option<String>,
    pub source_subject_id: Option<String>,
    pub source_subject_digest: Option<String>,
    /// The request a human_decision_resume request continues (V172).
    pub resumes_request_id: Option<String>,
    pub authorization_version: i64,
    pub mode: TaskCoordinationMode,
    pub brief_json: String,
    pub objective_digest: String,
    pub request_digest: String,
    pub idempotency_key: String,
    pub state: TaskCoordinationRequestState,
    pub wake_id: Option<String>,
    pub execution_id: Option<String>,
    pub plan_artifact_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskCoordinationRequestInput {
    pub id: String,
    pub task_id: String,
    pub kind: TaskCoordinationRequestKind,
    pub review_report_artifact_id: Option<String>,
    pub review_report_digest: Option<String>,
    pub review_producer_execution_id: Option<String>,
    pub source_subject_kind: Option<String>,
    pub source_subject_id: Option<String>,
    pub source_subject_digest: Option<String>,
    pub resumes_request_id: Option<String>,
    pub requested_by: String,
    pub expected_task_version: i64,
    pub authorization_version: i64,
    pub mode: TaskCoordinationMode,
    pub brief_json: String,
    pub objective_digest: String,
    pub request_digest: String,
    pub idempotency_key: String,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

/// Exact Project batch claim that is allowed to create and attach one
/// coordinator request. The repository consumes the coordination turn budget,
/// links the request to the batch Task row, and appends the request event in
/// the same serialized transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlanBatchRequestBinding {
    pub project_id: String,
    pub batch_id: String,
    pub authorized_by: String,
    pub expected_batch_version: i64,
    pub expected_graph_version: i64,
    pub expected_task_version: i64,
    pub expected_coordination_version: i64,
    pub coordination_snapshot_digest: String,
    pub origin: ProjectPlanBatchRequestOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectPlanBatchRequestOrigin {
    Scheduler {
        claim_token: String,
    },
    QualityReplan {
        source_artifact_id: String,
        producer_execution_id: String,
    },
    HumanReplan {
        approved_plan_artifact_id: String,
    },
    ReviewTriage {
        source_artifact_id: String,
        producer_execution_id: String,
    },
    /// The exact source lives in `task_coordination_request.source_subject_*`.
    ValidationTriage,
    ExternalFeedbackTriage,
    HumanDecisionResume,
    CoordinatorReplan {
        producer_execution_id: String,
    },
}

/// Typed binding used while minting TaskCoordination authority from a
/// Project batch. It is persisted in the same transaction as the new
/// authorization version, so a later pin failure or batch revocation cannot
/// turn that grant into a generic Task authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlanBatchAuthorizationBinding {
    pub project_id: String,
    pub batch_id: String,
    pub authorized_by: String,
    pub expected_batch_version: i64,
    pub expected_graph_version: i64,
    pub expected_team_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTaskCoordinationRequestState {
    pub id: String,
    pub expected_version: i64,
    pub state: TaskCoordinationRequestState,
    pub wake_id: Option<String>,
    pub execution_id: Option<String>,
    pub plan_artifact_id: Option<String>,
    pub failure_code: Option<Option<String>>,
    pub failure_message: Option<Option<String>>,
    pub completed_at: Option<String>,
    pub updated_at: String,
    pub event: Option<CreateDomainEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCoordinationPlanState {
    Proposed,
    Approved,
    Rejected,
    Superseded,
    Materialized,
}

enum_strings!(TaskCoordinationPlanState {
    Proposed => "proposed",
    Approved => "approved",
    Rejected => "rejected",
    Superseded => "superseded",
    Materialized => "materialized",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationPlan {
    pub artifact_id: String,
    pub task_id: String,
    pub request_id: String,
    pub authorization_version: i64,
    pub plan_digest: String,
    pub state: TaskCoordinationPlanState,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteTaskCoordinationTurnInput {
    pub request_id: String,
    pub task_id: String,
    pub authorization_version: i64,
    pub artifact_id: String,
    pub plan_digest: String,
    pub artifact_content: String,
    pub artifact_metadata_json: String,
    pub producer_execution_id: String,
    pub producer_actor: ActorRef,
    pub plan_state: TaskCoordinationPlanState,
    pub proposal_id: Option<String>,
    pub request_state: TaskCoordinationRequestState,
    pub created_at: String,
    pub artifact_event: CreateDomainEvent,
    pub plan_event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationPlanWrite {
    pub plan: TaskCoordinationPlan,
    pub request: TaskCoordinationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinationMaterialization {
    pub id: String,
    pub task_id: String,
    pub request_id: String,
    pub plan_artifact_id: String,
    pub plan_digest: String,
    pub authorization_version: i64,
    pub task_version_before: i64,
    pub task_version_after: i64,
    pub work_unit_ids_json: String,
    pub receipt_digest: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedWorkUnit {
    pub id: String,
    pub key: String,
    pub title: String,
    pub scope: String,
    pub execution_instructions: Option<String>,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub parent_work_unit_id: Option<String>,
    pub depends_on_work_unit_ids: Vec<String>,
    pub requires_integration: bool,
    pub artifact_inputs: Vec<MaterializedArtifactInput>,
    pub review_subject_work_unit_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedArtifactInput {
    pub artifact_id: String,
    pub digest: String,
    pub producer_execution_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeTaskCoordinationPlanInput {
    pub receipt_id: String,
    pub task_id: String,
    pub request_id: String,
    pub plan_artifact_id: String,
    pub plan_digest: String,
    pub authorization_version: i64,
    pub expected_team_digest: String,
    pub expected_plan_version: i64,
    pub task_version_before: i64,
    pub expected_request_version: i64,
    pub work_units: Vec<MaterializedWorkUnit>,
    pub receipt_digest: String,
    pub created_by: String,
    pub created_at: String,
    pub events: Vec<CreateDomainEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskQualityCategory {
    DeliveryCorrection,
    PlanRevision,
    Specialist,
    HumanDecision,
}

enum_strings!(TaskQualityCategory {
    DeliveryCorrection => "delivery_correction",
    PlanRevision => "plan_revision",
    Specialist => "specialist",
    HumanDecision => "human_decision",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskQualityTargetKind {
    Implementation,
    Orchestrator,
    Specialist,
    Human,
}

enum_strings!(TaskQualityTargetKind {
    Implementation => "implementation",
    Orchestrator => "orchestrator",
    Specialist => "specialist",
    Human => "human",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskQualityState {
    Requested,
    Notified,
    WorkCreated,
    Running,
    Resolved,
    RequiresHuman,
    Cancelled,
}

enum_strings!(TaskQualityState {
    Requested => "requested",
    Notified => "notified",
    WorkCreated => "work_created",
    Running => "running",
    Resolved => "resolved",
    RequiresHuman => "requires_human",
    Cancelled => "cancelled",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQualityRequest {
    pub id: String,
    pub task_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub category: TaskQualityCategory,
    pub target_kind: TaskQualityTargetKind,
    pub target_task_role_id: Option<String>,
    pub target_actor: Option<ActorRef>,
    pub summary: String,
    pub findings_json: String,
    pub resolution_condition: String,
    pub state: TaskQualityState,
    pub handoff_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub coordination_request_id: Option<String>,
    pub budget_round: i64,
    pub source_digest: String,
    pub request_digest: String,
    pub idempotency_key: String,
    pub created_by: ActorRef,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
    pub resolution_kind: Option<String>,
    pub resolution_ref: Option<String>,
    pub resolution_digest: Option<String>,
    pub route_id: String,
    pub route_round: i64,
    pub route_limit: i64,
    pub source_task_role_id: Option<String>,
    pub requires_human_reason_code: Option<String>,
    pub requires_human_reason: Option<String>,
    pub cycle_request_ids_json: String,
    pub cycle_task_role_ids_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskQualityRequestInput {
    pub id: String,
    pub task_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub category: TaskQualityCategory,
    pub target_kind: TaskQualityTargetKind,
    pub target_task_role_id: Option<String>,
    pub target_actor: Option<ActorRef>,
    pub summary: String,
    pub findings_json: String,
    pub resolution_condition: String,
    pub state: TaskQualityState,
    pub handoff_id: Option<String>,
    pub work_unit_id: Option<String>,
    pub source_digest: String,
    pub route_id: String,
    pub route_kind: String,
    pub route_ref: String,
    pub source_task_role_id: Option<String>,
    pub initial_requires_human_reason_code: Option<String>,
    pub initial_requires_human_reason: Option<String>,
    pub request_digest: String,
    pub idempotency_key: String,
    pub created_by: ActorRef,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone)]
pub struct UpdateTaskQualityRequestState {
    pub id: String,
    pub expected_version: i64,
    pub state: TaskQualityState,
    pub handoff_id: Option<Option<String>>,
    pub work_unit_id: Option<Option<String>>,
    pub coordination_request_id: Option<Option<String>>,
    pub resolution_kind: Option<Option<String>>,
    pub resolution_ref: Option<Option<String>>,
    pub resolution_digest: Option<Option<String>>,
    pub requires_human_reason_code: Option<Option<String>>,
    pub requires_human_reason: Option<Option<String>>,
    pub updated_at: String,
    pub event: Option<CreateDomainEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInterventionKind {
    Instruction,
    Decision,
    Pause,
    Resume,
    Cancel,
    RevokeAuthorization,
    RequestReplan,
}

enum_strings!(TaskInterventionKind {
    Instruction => "instruction",
    Decision => "decision",
    Pause => "pause",
    Resume => "resume",
    Cancel => "cancel",
    RevokeAuthorization => "revoke_authorization",
    RequestReplan => "request_replan",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskInterventionState {
    Recorded,
    Applied,
    RequiresHuman,
    Rejected,
}

enum_strings!(TaskInterventionState {
    Recorded => "recorded",
    Applied => "applied",
    RequiresHuman => "requires_human",
    Rejected => "rejected",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIntervention {
    pub id: String,
    pub task_id: String,
    pub kind: TaskInterventionKind,
    pub state: TaskInterventionState,
    pub source_kind: String,
    pub source_id: String,
    pub payload_json: String,
    pub request_digest: String,
    pub idempotency_key: String,
    pub expected_task_version: Option<i64>,
    pub expected_coordination_version: Option<i64>,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    pub coordination_request_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskInterventionInput {
    pub id: String,
    pub task_id: String,
    pub kind: TaskInterventionKind,
    pub source_kind: String,
    pub source_id: String,
    pub payload_json: String,
    pub request_digest: String,
    pub idempotency_key: String,
    pub expected_task_version: Option<i64>,
    pub expected_coordination_version: Option<i64>,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInterventionWrite {
    pub intervention: TaskIntervention,
    pub event: Option<DomainEvent>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskQualityWrite {
    pub request: TaskQualityRequest,
    pub event: Option<DomainEvent>,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLifecycleMigrationAudit {
    pub task_id: String,
    pub legacy_state: String,
    pub mapped_state: TaskLifecycleState,
    pub mapping_status: String,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub task_state_config: Option<String>,
    pub workflow_definition: String,
    pub details_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateScopeKind {
    Task,
    WorkUnit,
    MergeOperation,
    LifecycleOperation,
}

enum_strings!(GateScopeKind {
    Task => "task",
    WorkUnit => "work_unit",
    MergeOperation => "merge_operation",
    LifecycleOperation => "lifecycle_operation",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateEvaluationOutcome {
    Satisfied,
    Unsatisfied,
    Indeterminate,
}

enum_strings!(GateEvaluationOutcome {
    Satisfied => "satisfied",
    Unsatisfied => "unsatisfied",
    Indeterminate => "indeterminate",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    pub id: String,
    pub task_id: String,
    pub gate_kind: String,
    pub scope_kind: GateScopeKind,
    pub scope_id: String,
    pub active_policy_revision: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatePolicyRevision {
    pub gate_id: String,
    pub revision: i64,
    pub schema_version: i64,
    pub policy_json: String,
    pub policy_digest: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluation {
    pub id: String,
    pub gate_id: String,
    pub task_id: String,
    pub policy_revision: i64,
    pub outcome: GateEvaluationOutcome,
    pub input_digest: String,
    pub result_json: String,
    pub evaluated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluationInput {
    pub evaluation_id: String,
    pub ordinal: i64,
    pub input_kind: String,
    pub input_id: String,
    pub input_version: i64,
    pub input_digest: String,
    pub producer_ref: Option<String>,
    pub subject_json: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateGate {
    pub id: String,
    pub task_id: String,
    pub gate_kind: String,
    pub scope_kind: GateScopeKind,
    pub scope_id: String,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateGatePolicyRevision {
    pub gate_id: String,
    pub expected_active_revision: Option<i64>,
    pub revision: i64,
    pub schema_version: i64,
    pub policy_json: String,
    pub policy_digest: String,
    pub created_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreGateEvaluation {
    pub evaluation: GateEvaluation,
    pub inputs: Vec<GateEvaluationInput>,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateEvaluationWrite {
    pub evaluation: GateEvaluation,
    pub inputs: Vec<GateEvaluationInput>,
    /// `None` means an identical evaluation already existed.
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionTaskLifecycle {
    pub id: String,
    pub task_id: String,
    pub expected_task_version: i64,
    pub expected_lifecycle_version: i64,
    pub expected_state: TaskLifecycleState,
    pub to_state: TaskLifecycleState,
    pub cause_kind: String,
    pub cause_ref: Option<String>,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub gate_evaluation_id: Option<String>,
    pub idempotency_key: String,
    pub updated_at: String,
    pub event: CreateDomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLifecycleTransitionIdentity {
    pub task_id: String,
    pub idempotency_key: String,
    pub expected_task_version: Option<i64>,
    pub expected_lifecycle_version: Option<i64>,
    pub to_state: TaskLifecycleState,
    pub cause_kind: String,
    pub cause_ref: Option<String>,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub gate_evaluation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLifecycleTransitionFact {
    pub id: String,
    pub task_id: String,
    pub from_state: TaskLifecycleState,
    pub to_state: TaskLifecycleState,
    pub from_version: i64,
    pub to_version: i64,
    pub cause_kind: String,
    pub cause_ref: Option<String>,
    pub gate_evaluation_id: Option<String>,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub domain_event_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLifecycleTransitionWrite {
    pub lifecycle: TaskLifecycle,
    pub transition_id: String,
    pub gate_evaluation_id: Option<String>,
    pub replayed: bool,
    pub event: Option<DomainEvent>,
}

// ---------------------------------------------------------------------------
// V171 validation check catalog, requests and comparison; V172 triage/replan;
// V174 human decisions (coordinator-verification-and-pr-loop, Phase 1).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRunOrigin {
    LegacyCommand,
    CoordinatorRequest,
    AfterIntegration,
    /// Reserved by the frozen enum; never inserted (baseline results live in
    /// `validation_comparison`, decision C2).
    Baseline,
}

enum_strings!(ValidationRunOrigin {
    LegacyCommand => "legacy_command",
    CoordinatorRequest => "coordinator_request",
    AfterIntegration => "after_integration",
    Baseline => "baseline",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCheckState {
    Active,
    Retired,
}

enum_strings!(ValidationCheckState {
    Active => "active",
    Retired => "retired",
});

/// Trigger pinned in the coordination envelope (`validation_checks[].trigger`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeCheckTrigger {
    OnRequest,
    AfterIntegration,
}

enum_strings!(EnvelopeCheckTrigger {
    OnRequest => "on_request",
    AfterIntegration => "after_integration",
});

/// Trigger stored on a `validation_check_request` row. `CoordinatorRequest`
/// requires an `on_request` pin; `AfterIntegration` an `after_integration` pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCheckRequestTrigger {
    CoordinatorRequest,
    AfterIntegration,
}

enum_strings!(ValidationCheckRequestTrigger {
    CoordinatorRequest => "coordinator_request",
    AfterIntegration => "after_integration",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCheckSubjectKind {
    WorkUnitResult,
    TaskIntegration,
}

enum_strings!(ValidationCheckSubjectKind {
    WorkUnitResult => "work_unit_result",
    TaskIntegration => "task_integration",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCheckRequestState {
    Queued,
    Running,
    Completed,
    Cancelled,
    FailedToStart,
}

enum_strings!(ValidationCheckRequestState {
    Queued => "queued",
    Running => "running",
    Completed => "completed",
    Cancelled => "cancelled",
    FailedToStart => "failed_to_start",
});

impl ValidationCheckRequestState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::FailedToStart
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationComparisonClass {
    Regression,
    PreExistingFailure,
    Fixed,
    BothPass,
    Inconclusive,
}

enum_strings!(ValidationComparisonClass {
    Regression => "regression",
    PreExistingFailure => "pre_existing_failure",
    Fixed => "fixed",
    BothPass => "both_pass",
    Inconclusive => "inconclusive",
});

impl ValidationComparisonClass {
    /// Deterministic classification enforced by the V171 CHECK constraint.
    pub fn classify(candidate: ValidationRunStatus, baseline: ValidationRunStatus) -> Self {
        match (candidate, baseline) {
            (ValidationRunStatus::Passed, ValidationRunStatus::Passed) => Self::BothPass,
            (ValidationRunStatus::Failed, ValidationRunStatus::Passed) => Self::Regression,
            (ValidationRunStatus::Failed, ValidationRunStatus::Failed) => Self::PreExistingFailure,
            (ValidationRunStatus::Passed, ValidationRunStatus::Failed) => Self::Fixed,
            _ => Self::Inconclusive,
        }
    }
}

/// Input of the frozen `definition_digest`:
/// `sha256(serde_json::to_string(&ValidationCheckDigestInput))` with this exact
/// field order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCheckDigestInput {
    pub check_key: String,
    pub version: i64,
    pub command: String,
    pub timeout_seconds: i64,
    pub env_policy: String,
}

/// Frozen digest of a catalog definition version (sha256, lowercase hex).
pub fn validation_check_definition_digest(input: &ValidationCheckDigestInput) -> String {
    use sha2::{Digest, Sha256};
    // Serializing plain strings and integers cannot fail.
    let json = serde_json::to_string(input).unwrap_or_default();
    hex::encode(Sha256::digest(json.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckDefinition {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub check_key: String,
    pub version: i64,
    pub command: String,
    pub timeout_seconds: i64,
    pub env_policy: String,
    pub definition_digest: String,
    pub state: ValidationCheckState,
    pub created_by: String,
    pub created_at: String,
    pub retired_at: Option<String>,
}

/// Publishes the next exact version of `(project_id, repo_id, check_key)`. The
/// repository assigns `version`, computes `definition_digest`, and retires the
/// previous active version in the same `BEGIN IMMEDIATE` transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishValidationCheckDefinition {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub check_key: String,
    pub command: String,
    pub timeout_seconds: i64,
    pub env_policy: String,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckRequest {
    pub id: String,
    pub task_id: String,
    pub authorization_version: i64,
    pub check_definition_id: String,
    pub definition_digest: String,
    pub trigger_kind: ValidationCheckRequestTrigger,
    pub subject_kind: ValidationCheckSubjectKind,
    pub subject_id: String,
    pub subject_version: i64,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub baseline: bool,
    pub baseline_commit_sha: Option<String>,
    pub requested_by_request_id: Option<String>,
    pub requested_by_execution_id: Option<String>,
    pub reason: Option<String>,
    pub idempotency_key: String,
    pub state: ValidationCheckRequestState,
    pub candidate_run_id: Option<String>,
    pub failure_code: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateValidationCheckRequest {
    pub id: String,
    pub task_id: String,
    pub authorization_version: i64,
    pub check_definition_id: String,
    pub definition_digest: String,
    pub trigger_kind: ValidationCheckRequestTrigger,
    pub subject_kind: ValidationCheckSubjectKind,
    pub subject_id: String,
    pub subject_version: i64,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub baseline: bool,
    pub requested_by_request_id: Option<String>,
    pub requested_by_execution_id: Option<String>,
    pub reason: Option<String>,
    pub idempotency_key: String,
    pub created_at: String,
}

/// Forward-only transition guarded by `expected_version` (typed conflict).
/// `candidate_run_id`/`baseline_commit_sha` may be set once while open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionValidationCheckRequest {
    pub id: String,
    pub expected_version: i64,
    pub to_state: ValidationCheckRequestState,
    pub candidate_run_id: Option<String>,
    pub baseline_commit_sha: Option<String>,
    pub failure_code: Option<String>,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationCheckRequestWrite {
    pub request: ValidationCheckRequest,
    /// `false` means an identical idempotent replay returned the existing row.
    pub created: bool,
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationComparison {
    pub id: String,
    pub task_id: String,
    pub check_request_id: String,
    pub candidate_run_id: String,
    pub candidate_status: ValidationRunStatus,
    pub candidate_output_digest: Option<String>,
    pub baseline_commit_sha: String,
    pub baseline_snapshot_digest: String,
    pub baseline_provenance_json: String,
    pub baseline_status: ValidationRunStatus,
    pub baseline_exit_code: Option<i64>,
    pub baseline_output_digest: Option<String>,
    pub classification: ValidationComparisonClass,
    pub same_output_digest: bool,
    pub created_at: String,
}

/// `classification` and `same_output_digest` are derived by the repository from
/// the statuses and digests (never trusted from the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateValidationComparison {
    pub id: String,
    pub task_id: String,
    pub check_request_id: String,
    pub candidate_run_id: String,
    pub candidate_output_digest: Option<String>,
    pub baseline_commit_sha: String,
    pub baseline_snapshot_digest: String,
    pub baseline_provenance_json: String,
    pub baseline_status: ValidationRunStatus,
    pub baseline_exit_code: Option<i64>,
    pub baseline_output_digest: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationComparisonWrite {
    pub comparison: ValidationComparison,
    pub created: bool,
    pub event: Option<DomainEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationTriageDisposition {
    Correct,
    Recheck,
    Human,
}

enum_strings!(ValidationTriageDisposition {
    Correct => "correct",
    Recheck => "recheck",
    Human => "human",
});

/// Immutable decision of a `validation_findings` coordination request (V172).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskValidationTriage {
    pub request_id: String,
    pub task_id: String,
    pub validation_run_id: String,
    pub comparison_id: Option<String>,
    pub run_digest: String,
    pub execution_id: String,
    pub authorization_version: i64,
    pub disposition: ValidationTriageDisposition,
    pub rationale: String,
    pub instructions: String,
    pub decision_digest: String,
    pub review_work_unit_id: Option<String>,
    pub rework_ref: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorReplanState {
    Proposed,
    Planning,
    AwaitingHuman,
    Approved,
    Rejected,
    Materialized,
    Failed,
}

enum_strings!(CoordinatorReplanState {
    Proposed => "proposed",
    Planning => "planning",
    AwaitingHuman => "awaiting_human",
    Approved => "approved",
    Rejected => "rejected",
    Materialized => "materialized",
    Failed => "failed",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorReplanClassification {
    WithinEnvelope,
    ScopeOrAuthorityChange,
}

enum_strings!(CoordinatorReplanClassification {
    WithinEnvelope => "within_envelope",
    ScopeOrAuthorityChange => "scope_or_authority_change",
});

/// Hard D6 limit: at most `MAX_COORDINATOR_REPLANS_PER_TASK` rows per Task.
pub const MAX_COORDINATOR_REPLANS_PER_TASK: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCoordinatorReplan {
    pub id: String,
    pub task_id: String,
    pub ordinal: i64,
    pub authorization_version: i64,
    pub source_request_id: String,
    pub producer_execution_id: String,
    pub plan_request_id: Option<String>,
    pub reason: String,
    pub evidence_links_json: String,
    pub classification: Option<CoordinatorReplanClassification>,
    pub classification_facts_json: Option<String>,
    pub human_decision_id: Option<String>,
    pub state: CoordinatorReplanState,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// The repository assigns `ordinal = max + 1` inside `BEGIN IMMEDIATE`; a third
/// proposal fails with `DbError::CoordinatorReplanLimitReached`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateCoordinatorReplan {
    pub id: String,
    pub task_id: String,
    pub authorization_version: i64,
    pub source_request_id: String,
    pub producer_execution_id: String,
    pub reason: String,
    pub evidence_links_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionCoordinatorReplan {
    pub id: String,
    pub expected_version: i64,
    pub to_state: CoordinatorReplanState,
    /// Each optional field may be set once (`None` keeps the stored value).
    pub plan_request_id: Option<String>,
    pub classification: Option<CoordinatorReplanClassification>,
    pub classification_facts_json: Option<String>,
    pub human_decision_id: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanDecisionState {
    Open,
    Answered,
    Superseded,
    Cancelled,
}

enum_strings!(HumanDecisionState {
    Open => "open",
    Answered => "answered",
    Superseded => "superseded",
    Cancelled => "cancelled",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanDecisionSourceKind {
    CoordinationRequest,
    QualityRequest,
    GateEvaluation,
    ValidationRun,
    ExternalFeedback,
    CoordinatorReplan,
}

enum_strings!(HumanDecisionSourceKind {
    CoordinationRequest => "coordination_request",
    QualityRequest => "quality_request",
    GateEvaluation => "gate_evaluation",
    ValidationRun => "validation_run",
    ExternalFeedback => "external_feedback",
    CoordinatorReplan => "coordinator_replan",
});

/// Ordered: `None < Scope < Authority`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityEffect {
    None,
    Scope,
    Authority,
}

enum_strings!(AuthorityEffect {
    None => "none",
    Scope => "scope",
    Authority => "authority",
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanDecisionRequest {
    pub id: String,
    pub task_id: String,
    pub source_kind: HumanDecisionSourceKind,
    pub source_id: String,
    pub raised_by_execution_id: Option<String>,
    pub authorization_version: Option<i64>,
    pub question: String,
    pub options_json: String,
    pub recommended_option_id: Option<String>,
    pub recommendation_rationale: Option<String>,
    pub evidence_links_json: String,
    pub max_authority_effect: AuthorityEffect,
    pub state: HumanDecisionState,
    pub answered_by_user_id: Option<String>,
    pub answered_option_id: Option<String>,
    pub answer_note: Option<String>,
    pub answered_at: Option<String>,
    pub resume_request_id: Option<String>,
    pub resume_binding_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateHumanDecisionRequest {
    pub id: String,
    pub task_id: String,
    pub source_kind: HumanDecisionSourceKind,
    pub source_id: String,
    pub raised_by_execution_id: Option<String>,
    pub authorization_version: Option<i64>,
    pub question: String,
    pub options_json: String,
    pub recommended_option_id: Option<String>,
    pub recommendation_rationale: Option<String>,
    pub evidence_links_json: String,
    pub max_authority_effect: AuthorityEffect,
    pub resume_binding_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerHumanDecision {
    pub id: String,
    pub expected_version: i64,
    pub option_id: String,
    pub answered_by_user_id: String,
    pub note: Option<String>,
    pub answered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanDecisionWrite {
    pub decision: HumanDecisionRequest,
    /// `false` means an identical idempotent replay returned the existing row.
    pub created: bool,
    pub event: Option<DomainEvent>,
}

// ---------------------------------------------------------------------------
// V173 (coordinator-verification-and-pr-loop, Phase 2): PR feedback policy,
// external feedback ledger and triage, PR external actions.
// ---------------------------------------------------------------------------

/// Default feedback policy JSON stored on every `pr_provider_config` row.
pub const DEFAULT_PR_FEEDBACK_POLICY_JSON: &str = r#"{"schema_version":1,"ingest":false,"trusted_author_associations":["OWNER","MEMBER","COLLABORATOR"],"trusted_logins":[],"max_items_per_poll":50,"push_corrections":"human_approval","post_summary":"off"}"#;

/// Maximum number of items one `external_feedback` coordination request triages.
pub const MAX_EXTERNAL_FEEDBACK_SET_ITEMS: usize = 20;

/// Lower-case SHA-256 hex of the exact stored policy JSON (what is frozen in
/// `remote_pr_admission.feedback_policy_digest`).
pub fn pr_feedback_policy_digest(policy_json: &str) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(policy_json.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackAuthorization {
    HumanApproval,
    Automatic,
}

enum_strings!(PrFeedbackAuthorization {
    HumanApproval => "human_approval",
    Automatic => "automatic",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrFeedbackSummaryMode {
    HumanApproval,
    Automatic,
    Off,
}

enum_strings!(PrFeedbackSummaryMode {
    HumanApproval => "human_approval",
    Automatic => "automatic",
    Off => "off",
});

/// Typed view of `feedback_policy_json` (schema_version 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrFeedbackPolicy {
    pub schema_version: i64,
    pub ingest: bool,
    pub trusted_author_associations: Vec<String>,
    pub trusted_logins: Vec<String>,
    pub max_items_per_poll: i64,
    pub push_corrections: PrFeedbackAuthorization,
    pub post_summary: PrFeedbackSummaryMode,
}

impl Default for PrFeedbackPolicy {
    fn default() -> Self {
        Self {
            schema_version: 1,
            ingest: false,
            trusted_author_associations: vec![
                "OWNER".to_owned(),
                "MEMBER".to_owned(),
                "COLLABORATOR".to_owned(),
            ],
            trusted_logins: Vec::new(),
            max_items_per_poll: 50,
            push_corrections: PrFeedbackAuthorization::HumanApproval,
            post_summary: PrFeedbackSummaryMode::Off,
        }
    }
}

impl PrFeedbackPolicy {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|error| format!("invalid feedback policy: {error}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("feedback policy serializes")
    }
}

/// Reference to a `credential_handle` (`provider = 'github'`) and its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrProviderCredentialRef {
    pub handle_id: String,
    pub owner_user_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalFeedbackKind {
    Review,
    ReviewComment,
    IssueComment,
}

enum_strings!(ExternalFeedbackKind {
    Review => "review",
    ReviewComment => "review_comment",
    IssueComment => "issue_comment",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalFeedbackReviewState {
    #[serde(rename = "CHANGES_REQUESTED")]
    ChangesRequested,
    #[serde(rename = "COMMENTED")]
    Commented,
    #[serde(rename = "APPROVED")]
    Approved,
    #[serde(rename = "DISMISSED")]
    Dismissed,
}

enum_strings!(ExternalFeedbackReviewState {
    ChangesRequested => "CHANGES_REQUESTED",
    Commented => "COMMENTED",
    Approved => "APPROVED",
    Dismissed => "DISMISSED",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalFeedbackTrustClass {
    Trusted,
    Untrusted,
    ForgeAuthored,
}

enum_strings!(ExternalFeedbackTrustClass {
    Trusted => "trusted",
    Untrusted => "untrusted",
    ForgeAuthored => "forge_authored",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalFeedbackHeadState {
    Current,
    StaleHead,
}

enum_strings!(ExternalFeedbackHeadState {
    Current => "current",
    StaleHead => "stale_head",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalFeedbackDisposition {
    Correct,
    Recheck,
    Human,
    Dismiss,
}

enum_strings!(ExternalFeedbackDisposition {
    Correct => "correct",
    Recheck => "recheck",
    Human => "human",
    Dismiss => "dismiss",
});

/// Immutable provider-authored item ingested for an open remote PR admission.
/// `body` is untrusted data; never treat it as an instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFeedbackItem {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub provider_item_id: String,
    pub kind: ExternalFeedbackKind,
    pub author_login: String,
    pub author_association: String,
    pub review_state: Option<ExternalFeedbackReviewState>,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub commit_sha: Option<String>,
    pub body: String,
    pub body_truncated: bool,
    pub body_digest: String,
    pub provider_url: Option<String>,
    pub trust_class: ExternalFeedbackTrustClass,
    pub head_state: ExternalFeedbackHeadState,
    pub ingested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateExternalFeedbackItem {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub provider_item_id: String,
    pub kind: ExternalFeedbackKind,
    pub author_login: String,
    pub author_association: String,
    pub review_state: Option<ExternalFeedbackReviewState>,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub commit_sha: Option<String>,
    /// At most 16 KiB; truncate and set `body_truncated` before calling.
    pub body: String,
    pub body_truncated: bool,
    pub body_digest: String,
    pub provider_url: Option<String>,
    pub trust_class: ExternalFeedbackTrustClass,
    pub head_state: ExternalFeedbackHeadState,
    pub ingested_at: String,
}

/// One entry of the exact set an `external_feedback` request triages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFeedbackSetEntry {
    pub feedback_item_id: String,
    pub body_digest: String,
}

/// Immutable coordinator decision about one feedback item of one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFeedbackTriage {
    pub id: String,
    pub task_id: String,
    pub request_id: String,
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: ExternalFeedbackDisposition,
    pub rationale: String,
    pub instructions: String,
    pub decision_digest: String,
    pub execution_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateExternalFeedbackTriage {
    pub id: String,
    pub task_id: String,
    pub request_id: String,
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: ExternalFeedbackDisposition,
    pub rationale: String,
    pub instructions: String,
    pub decision_digest: String,
    pub execution_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrExternalActionKind {
    PushUpdate,
    PostComment,
}

enum_strings!(PrExternalActionKind {
    PushUpdate => "push_update",
    PostComment => "post_comment",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrExternalActionState {
    Proposed,
    Approved,
    Running,
    Succeeded,
    Failed,
    OutcomeUnknown,
    ReconciliationRequired,
    Rejected,
    Abandoned,
}

enum_strings!(PrExternalActionState {
    Proposed => "proposed",
    Approved => "approved",
    Running => "running",
    Succeeded => "succeeded",
    Failed => "failed",
    OutcomeUnknown => "outcome_unknown",
    ReconciliationRequired => "reconciliation_required",
    Rejected => "rejected",
    Abandoned => "abandoned",
});

impl PrExternalActionState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Rejected | Self::Abandoned
        )
    }

    /// States counted by the one-open-action-per-Task unique index.
    pub fn is_open(self) -> bool {
        matches!(
            self,
            Self::Proposed
                | Self::Approved
                | Self::Running
                | Self::OutcomeUnknown
                | Self::ReconciliationRequired
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrExternalAction {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub kind: PrExternalActionKind,
    pub state: PrExternalActionState,
    pub authorization_mode: PrFeedbackAuthorization,
    pub gate_evaluation_id: Option<String>,
    pub subject_commit_sha: Option<String>,
    pub expected_remote_head_sha: Option<String>,
    pub summary_artifact_id: Option<String>,
    pub body_digest: Option<String>,
    pub proposal_id: Option<String>,
    pub decision_id: Option<String>,
    pub idempotency_key: String,
    pub provider_event_id: Option<String>,
    pub provider_result_json: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

/// Birth of an action: always `proposed`, or `approved` for an `automatic`
/// action whose frozen admission policy says so. `push_update` requires the
/// Gate evaluation and both SHAs; `post_comment` the summary Artifact and
/// body digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrExternalAction {
    pub id: String,
    pub task_id: String,
    pub admission_id: String,
    pub kind: PrExternalActionKind,
    pub authorization_mode: PrFeedbackAuthorization,
    pub gate_evaluation_id: Option<String>,
    pub subject_commit_sha: Option<String>,
    pub expected_remote_head_sha: Option<String>,
    pub summary_artifact_id: Option<String>,
    pub body_digest: Option<String>,
    pub idempotency_key: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionPrExternalAction {
    pub id: String,
    pub expected_version: i64,
    pub to_state: PrExternalActionState,
    /// Required for `proposed -> approved|rejected` under `human_approval`.
    pub decision_id: Option<String>,
    pub provider_event_id: Option<String>,
    pub provider_result_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrExternalActionWrite {
    pub action: PrExternalAction,
    /// `false` means an identical idempotent replay returned the existing row.
    pub created: bool,
    pub event: Option<DomainEvent>,
}

// ---------------------------------------------------------------------------
// V175 (coordinator-verification-and-pr-loop, Phase 2, node PX, D3'):
// non-destructive PR pause for same-PR rework and per-item feedback
// dispositions.
// ---------------------------------------------------------------------------

/// `remote_pr_admission.rework_outcome` marker of a paused admission.
pub const REWORK_OUTCOME_SUPERSEDED: &str = "superseded_for_rework";
/// `reason_kind` of the `Merging -> Blocked` and `Blocked -> Active` receipts.
pub const PR_FEEDBACK_REWORK_REASON_KIND: &str = "pr_feedback_rework";
/// Domain event recorded when an open admission is paused for rework.
pub const PR_ADMISSION_SUPERSEDED_EVENT: &str = "pr.admission_superseded";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrReworkState {
    Open,
    Readmitted,
    RemoteMerged,
    RemoteClosed,
    Abandoned,
}

enum_strings!(PrReworkState {
    Open => "open",
    Readmitted => "readmitted",
    RemoteMerged => "remote_merged",
    RemoteClosed => "remote_closed",
    Abandoned => "abandoned",
});

/// Immutable receipt of one paused admission (also the lifecycle receipt
/// `reason_ref` of the `pr_feedback_rework` edges). Only `state`,
/// `readmission_merge_operation_id`, `convergence_decision_id`, `version` and
/// `updated_at` ever change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrReworkSupersession {
    pub id: String,
    pub task_id: String,
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub metadata_id: String,
    pub request_id: String,
    pub triage_set_digest: String,
    pub provider_pr_id: String,
    pub superseded_head_sha: String,
    pub result_event_id: String,
    pub state: PrReworkState,
    pub readmission_merge_operation_id: Option<String>,
    pub convergence_decision_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Input of [`PrReworkRepo::supersede_admission_in_tx`]. Everything else
/// (admission, metadata, request digest, provider PR id, head SHA, event) is
/// read and verified inside the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeAdmission {
    pub id: String,
    pub task_merge_operation_id: String,
    pub request_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeAdmissionWrite {
    pub supersession: PrReworkSupersession,
    /// `false`: an identical earlier supersession was returned (replay).
    pub created: bool,
    /// Domain events committed by this call (`pr.admission_superseded`, the
    /// `task.lifecycle_changed` of `Merging -> Blocked`); empty on replay.
    pub events: Vec<DomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionPrReworkSupersession {
    pub id: String,
    pub expected_version: i64,
    pub to_state: PrReworkState,
    /// Required (and only allowed) for `readmitted`.
    pub readmission_merge_operation_id: Option<String>,
    /// Only allowed for `remote_merged` / `abandoned`; set once.
    pub convergence_decision_id: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackItemDisposition {
    ExtendTask,
    ProposeTask,
    Human,
    Dismiss,
}

enum_strings!(FeedbackItemDisposition {
    ExtendTask => "extend_task",
    ProposeTask => "propose_task",
    Human => "human",
    Dismiss => "dismiss",
});

impl FeedbackItemDisposition {
    /// Mandatory mapping onto the V173 triage row.
    pub fn legacy(self) -> ExternalFeedbackDisposition {
        match self {
            Self::ExtendTask => ExternalFeedbackDisposition::Correct,
            Self::ProposeTask | Self::Human => ExternalFeedbackDisposition::Human,
            Self::Dismiss => ExternalFeedbackDisposition::Dismiss,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackScopeClassification {
    WithinTaskScope,
    ScopeChange,
}

enum_strings!(FeedbackScopeClassification {
    WithinTaskScope => "within_task_scope",
    ScopeChange => "scope_change",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackDismissReason {
    NotActionable,
    Disagree,
    Duplicate,
    AlreadyAddressed,
    OutOfDate,
}

enum_strings!(FeedbackDismissReason {
    NotActionable => "not_actionable",
    Disagree => "disagree",
    Duplicate => "duplicate",
    AlreadyAddressed => "already_addressed",
    OutOfDate => "out_of_date",
});

/// Real per-item disposition recorded next to the V173 triage row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFeedbackDispositionRecord {
    pub id: String,
    pub task_id: String,
    pub triage_id: String,
    pub request_id: String,
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: FeedbackItemDisposition,
    pub legacy_disposition: ExternalFeedbackDisposition,
    pub scope_classification: Option<FeedbackScopeClassification>,
    pub classification_facts_json: Option<String>,
    pub dismiss_reason: Option<FeedbackDismissReason>,
    pub proposal_json: Option<String>,
    pub human_decision_id: Option<String>,
    pub followup_task_id: Option<String>,
    pub created_at: String,
}

/// `legacy_disposition` is derived from `disposition` and written to the
/// matching V173 triage row (`triage_id`) by the caller in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateExternalFeedbackDisposition {
    pub id: String,
    pub task_id: String,
    pub triage_id: String,
    pub request_id: String,
    pub feedback_item_id: String,
    pub body_digest: String,
    pub disposition: FeedbackItemDisposition,
    pub scope_classification: Option<FeedbackScopeClassification>,
    pub classification_facts_json: Option<String>,
    pub dismiss_reason: Option<FeedbackDismissReason>,
    pub proposal_json: Option<String>,
    pub created_at: String,
}
