use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Row};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub settings: String,
    pub workflow_definition: String,
    pub workflow_template_name: Option<String>,
    pub primary_repo_id: Option<String>,
    pub paused_at: Option<String>,
    pub owner_id: Option<String>,
    pub project_hooks_json: String,
    pub project_work_epoch: i64,
    pub created_at: String,
    pub updated_at: String,
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
    pub default_task_state: Option<String>,
    pub default_assignee_type: Option<String>,
    pub default_assignee_id: Option<String>,
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
    pub default_task_state: Option<String>,
    pub default_assignee_type: Option<String>,
    pub default_assignee_id: Option<String>,
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
    pub default_task_state: Option<Option<String>>,
    pub default_assignee_type: Option<Option<String>>,
    pub default_assignee_id: Option<Option<String>>,
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
    pub token_secret_ref: Option<String>,
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
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub executor_type: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub config_json: String,
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
    pub assignee_type: Option<String>,
    pub assignee_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub task_type: String,
    pub status: TaskStatus,
    pub is_automation: bool,
    pub priority: i64,
    pub board_position: f64,
    pub subtask_order: Option<i64>,
    pub task_state_config: Option<String>,
    pub merge_config: Option<String>,
    pub metadata_json: Option<String>,
    pub plan: Option<String>,
    pub error_annotation: Option<String>,
    pub blocked_json: Option<String>,
    pub failed_json: Option<String>,
    pub entry_barrier_json: Option<String>,
    pub review_passed_at: Option<String>,
    pub archived_at: Option<String>,
    pub deleted_at: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub type TaskStatus = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveTaskIdentity {
    pub project_id: String,
    pub task_id: String,
    pub task_version: i64,
    pub board_revision: i64,
    pub target_status: String,
    pub before_id: Option<String>,
    pub after_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CompareAndMoveTask {
    pub operation_id: String,
    pub project_id: String,
    pub task_id: String,
    pub task_version: i64,
    pub board_revision: i64,
    pub target_status: String,
    pub target_column_statuses: Vec<String>,
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub entry_barrier_json: Option<String>,
    pub transition_log_id: String,
    pub trigger_name: Option<String>,
    pub triggered_by: String,
    pub trigger_reason: String,
    pub rejection: bool,
    pub updated_at: String,
}

impl CompareAndMoveTask {
    pub fn identity(&self) -> MoveTaskIdentity {
        MoveTaskIdentity {
            project_id: self.project_id.clone(),
            task_id: self.task_id.clone(),
            task_version: self.task_version,
            board_revision: self.board_revision,
            target_status: self.target_status.clone(),
            before_id: self.before_id.clone(),
            after_id: self.after_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveTaskResult {
    pub task: Task,
    pub board_revision: i64,
    pub operation_id: String,
    pub old_status: String,
    pub old_board_position: f64,
    pub before_id: Option<String>,
    pub after_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MoveTaskPersistence {
    Committed {
        result: Box<MoveTaskResult>,
        transition_log: Box<TransitionLog>,
    },
    Replayed(Box<MoveTaskResult>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub role: String,
    pub status: ExecutionStatus,
    pub stop_reason: Option<StopReason>,
    pub stopped_by: Option<String>,
    pub resume_policy: Option<ResumePolicy>,
    pub stopped_at: Option<String>,
    pub parent_execution_id: Option<String>,
    pub agent_session_id: Option<String>,
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
    pub created_at: String,
    pub updated_at: String,
}

pub type ExecutionRole = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    pub project_id: String,
    pub agent_id: Option<String>,
    pub title: String,
    pub status: ConversationStatus,
    pub system_prompt: Option<String>,
    pub message_count: i64,
    pub last_message_at: Option<String>,
    pub agent_session_id: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: ConversationMessageRole,
    pub content: String,
    pub status: ConversationMessageStatus,
    pub model: Option<String>,
    pub token_usage_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
    pub sequence: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationMessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationMessageStatus {
    Complete,
    Streaming,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryKind {
    Observation,
    Decision,
    Handoff,
    Failure,
    ReviewResult,
    ExecutionSummary,
    Comment,
    Transition,
    ConversationMessage,
    Artifact,
    Lesson,
    ContextPack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemorySourceType {
    Execution,
    Review,
    Comment,
    Transition,
    Conversation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryConfidence {
    Confirmed,
    Partial,
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryItem {
    pub row_id: i64,
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub execution_id: Option<String>,
    pub conversation_id: Option<String>,
    pub source_type: String,
    pub kind: String,
    pub title: String,
    pub summary: Option<String>,
    pub body: String,
    pub metadata_json: String,
    pub confidence: Option<String>,
    pub quality_score: Option<i64>,
    pub created_by_type: Option<String>,
    pub created_by_id: Option<String>,
    pub created_at: String,
}

impl MemoryItem {
    pub fn from_row(row: &SqliteRow) -> std::result::Result<MemoryItem, sqlx::Error> {
        Ok(MemoryItem {
            row_id: row.try_get("row_id")?,
            id: row.try_get("id")?,
            project_id: row.try_get("project_id")?,
            task_id: row.try_get("task_id")?,
            execution_id: row.try_get("execution_id")?,
            conversation_id: row.try_get("conversation_id")?,
            source_type: row.try_get("source_type")?,
            kind: row.try_get("kind")?,
            title: row.try_get("title")?,
            summary: row.try_get("summary")?,
            body: row.try_get("body")?,
            metadata_json: row.try_get("metadata_json")?,
            confidence: row.try_get("confidence")?,
            quality_score: row.try_get("quality_score")?,
            created_by_type: row.try_get("created_by_type")?,
            created_by_id: row.try_get("created_by_id")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
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
pub struct Review {
    pub id: String,
    pub task_id: String,
    pub execution_id: String,
    pub attempt_number: i64,
    pub status: ReviewStatus,
    pub step_results_json: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewStatus {
    Running,
    AwaitingHuman,
    Passed,
    Failed,
    Cancelled,
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

enum_strings!(ConversationStatus {
    Active => "active",
    Archived => "archived",
});

enum_strings!(ConversationMessageRole {
    User => "user",
    Assistant => "assistant",
    System => "system",
});

enum_strings!(ConversationMessageStatus {
    Complete => "complete",
    Streaming => "streaming",
    Failed => "failed",
    Cancelled => "cancelled",
});

enum_strings!(MemoryKind {
    Observation => "observation",
    Decision => "decision",
    Handoff => "handoff",
    Failure => "failure",
    ReviewResult => "review_result",
    ExecutionSummary => "execution_summary",
    Comment => "comment",
    Transition => "transition",
    ConversationMessage => "conversation_message",
    Artifact => "artifact",
    Lesson => "lesson",
    ContextPack => "context_pack",
});

enum_strings!(MemorySourceType {
    Execution => "execution",
    Review => "review",
    Comment => "comment",
    Transition => "transition",
    Conversation => "conversation",
});

enum_strings!(MemoryConfidence {
    Confirmed => "confirmed",
    Partial => "partial",
    Unconfirmed => "unconfirmed",
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

enum_strings!(ReviewStatus {
    Running => "running",
    AwaitingHuman => "awaiting_human",
    Passed => "passed",
    Failed => "failed",
    Cancelled => "cancelled",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssigneeKind {
    Agent,
    User,
}

enum_strings!(AssigneeKind {
    Agent => "agent",
    User => "user",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRoleAssignment {
    pub id: String,
    pub task_id: String,
    pub role_name: String,
    pub assignee_type: Option<AssigneeKind>,
    pub assignee_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionLog {
    pub id: String,
    pub task_id: String,
    pub from_state: String,
    pub to_state: String,
    pub trigger_name: Option<String>,
    pub triggered_by: String,
    pub trigger_reason: String,
    pub hook_results_json: Option<String>,
    pub rejection: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRoleAssignment {
    pub id: String,
    pub task_id: String,
    pub role_name: String,
    pub assignee_type: Option<AssigneeKind>,
    pub assignee_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTransitionLog {
    pub id: String,
    pub task_id: String,
    pub from_state: String,
    pub to_state: String,
    pub trigger_name: Option<String>,
    pub triggered_by: String,
    pub trigger_reason: String,
    pub hook_results_json: Option<String>,
    pub rejection: bool,
    pub created_at: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectAgentLink {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub linked_by_user_id: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProjectAgentLink {
    pub id: String,
    pub project_id: String,
    pub agent_id: String,
    pub linked_by_user_id: String,
    pub created_at: String,
    pub updated_at: String,
}
