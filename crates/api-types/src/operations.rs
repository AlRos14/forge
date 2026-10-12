use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::TaskLifecycleState;

// Persisted orchestration operation identifiers shared by Agent action
// admission, deterministic materialization, and API projections.
pub const MAIN_CHARTER_READ_OPERATION: &str = "charter.read";
pub const MAIN_CHARTER_DRAFT_OPERATION: &str = "charter.draft";
pub const MAIN_CHARTER_READINESS_OPERATION: &str = "charter.readiness";
pub const MAIN_CHARTER_DIFF_OPERATION: &str = "charter.diff";
pub const MAIN_CHARTER_APPROVAL_TARGET_OPERATION: &str = "charter.approval_target";
pub const MAIN_PROJECT_CREATE_OPERATION: &str = "project.create";
pub const PROJECT_CURRENT_STATE_OPERATION: &str = "project.current_state";
pub const PROJECT_CHARTER_ADOPTION_OPERATION: &str = "project.charter.adoption";
pub const PROJECT_DOCUMENT_OPERATION: &str = "project.document";
pub const PROJECT_DECISION_OPERATION: &str = "project.decision";
pub const PROJECT_EXECUTION_BASELINE_OPERATION: &str = "project.execution_baseline";
pub const PROJECT_MILESTONE_OPERATION: &str = "project.milestone";
pub const PROJECT_EVIDENCE_OPERATION: &str = "project.evidence";
pub const PROJECT_READINESS_OPERATION: &str = "project.readiness";
pub const PROJECT_RELEASE_OPERATION: &str = "project.release.request";

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OperatorSeverity {
    Healthy,
    Attention,
    Blocked,
    Error,
}

impl OperatorSeverity {
    fn rank(&self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Attention => 1,
            Self::Blocked => 2,
            Self::Error => 3,
        }
    }
}

impl fmt::Display for OperatorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Healthy => "healthy",
            Self::Attention => "attention",
            Self::Blocked => "blocked",
            Self::Error => "error",
        })
    }
}

impl FromStr for OperatorSeverity {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "healthy" => Ok(Self::Healthy),
            "attention" => Ok(Self::Attention),
            "blocked" => Ok(Self::Blocked),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown operator severity: {other}")),
        }
    }
}

impl PartialOrd for OperatorSeverity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OperatorSeverity {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperatorStatusResponse {
    pub overall_severity: OperatorSeverity,
    pub active_executions: Vec<ActiveExecutionSummary>,
    pub blocked_tasks: Vec<BlockedTaskSummary>,
    pub daemon_issues: Vec<DaemonIssueSummary>,
    pub daemon_pressure: Vec<DaemonPressureSummary>,
    pub agent_pressure: Vec<AgentPressureSummary>,
    pub workspace_cleanup: Vec<WorkspaceCleanupSummary>,
    pub retry_pressure: Vec<RetryPressureSummary>,
    pub usage_summary: Option<UsageSummary>,
    pub recent_errors: Vec<RecentErrorSummary>,
    pub computed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActiveExecutionSummary {
    pub execution_id: String,
    pub task_id: String,
    pub task_title: Option<String>,
    pub role: String,
    pub agent_id: Option<String>,
    pub agent_name: Option<String>,
    pub daemon_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_path: Option<String>,
    pub harness_session_id: Option<String>,
    pub started_at: String,
    pub runtime_seconds: f64,
    pub elapsed_seconds: f64,
    pub latest_event: Option<String>,
    pub last_event: Option<String>,
    pub last_event_time: Option<String>,
    #[ts(type = "number")]
    pub turn_count: u32,
    pub token_totals: Option<TokenTotalsSummary>,
    #[ts(type = "Record<string, unknown> | null")]
    pub rate_limit_snapshot: Option<serde_json::Value>,
    pub effective_policy: Option<EffectiveExecutionPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockedTaskSummary {
    pub task_id: String,
    pub title: String,
    pub lifecycle_state: TaskLifecycleState,
    #[ts(type = "number")]
    pub lifecycle_version: i64,
    pub transition_id: Option<String>,
    pub reason_kind: Option<String>,
    pub reason_ref: Option<String>,
    pub blocked_since: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DaemonIssueSummary {
    pub daemon_id: String,
    pub hostname: Option<String>,
    pub issue: String,
    pub severity: OperatorSeverity,
    pub detected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DaemonPressureSummary {
    pub daemon_id: String,
    pub hostname: Option<String>,
    #[ts(type = "number")]
    pub active_execution_count: u32,
    #[ts(type = "number | null")]
    pub max_sessions: Option<u32>,
    pub at_capacity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentPressureSummary {
    pub agent_id: String,
    pub agent_name: String,
    pub daemon_id: Option<String>,
    #[ts(type = "number")]
    pub active_execution_count: u32,
    #[ts(type = "number")]
    pub max_concurrent_tasks: u32,
    pub at_capacity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceCleanupSummary {
    pub workspace_id: String,
    pub task_id: String,
    pub worktree_path: Option<String>,
    pub cleanup_after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RetryPressureSummary {
    pub retry_receipt_id: String,
    pub task_id: String,
    pub title: String,
    pub failure_kind: String,
    pub failure_ref: String,
    pub source_event_id: String,
    pub receipt_event_id: String,
    #[ts(type = "number")]
    pub attempt_number: u32,
    #[ts(type = "number")]
    pub retry_budget: u32,
    pub disposition: String,
    pub lifecycle_state: TaskLifecycleState,
    #[ts(type = "number")]
    pub lifecycle_version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenTotalsSummary {
    #[ts(type = "number")]
    pub input_tokens: i64,
    #[ts(type = "number")]
    pub output_tokens: i64,
    #[ts(type = "number")]
    pub cache_read_tokens: i64,
    #[ts(type = "number")]
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OperationsRefreshResponse {
    #[ts(type = "number")]
    pub dispatched_tasks: u64,
    pub refreshed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageSummary {
    pub available: bool,
    #[ts(type = "number | null")]
    pub total_input_tokens: Option<i64>,
    #[ts(type = "number | null")]
    pub total_output_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    #[ts(type = "number")]
    pub active_execution_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RecentErrorSummary {
    pub entity_type: String,
    pub entity_id: String,
    pub error: String,
    pub occurred_at: String,
    pub severity: OperatorSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanArtifactDetail {
    pub artifact_id: String,
    pub task_id: String,
    pub producer_execution_id: String,
    pub producer: crate::ActorRef,
    pub content_digest: Option<String>,
    pub markdown: String,
    pub items: Vec<PlanChecklistItem>,
    pub warnings: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskPlanHistoryResponse {
    pub artifacts: Vec<PlanArtifactDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanChecklistItem {
    pub checked: bool,
    pub label: String,
    #[ts(type = "number")]
    pub nesting_level: u32,
    #[ts(type = "number")]
    pub line_number: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EffectiveExecutionPolicy {
    pub executor_kind: String,
    pub permission_policy: String,
    pub isolation_posture: String,
    pub is_high_risk: bool,
    pub effective_cwd: Option<String>,
    pub workspace_root: Option<String>,
    pub environment_posture: String,
    pub scoped_tools: Vec<String>,
    pub mcp_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanProgressSummary {
    #[ts(type = "number")]
    pub total: u32,
    #[ts(type = "number")]
    pub completed: u32,
    #[ts(type = "number")]
    pub remaining: u32,
    pub available: bool,
    pub warnings: Vec<String>,
}
