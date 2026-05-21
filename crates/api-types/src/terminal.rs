use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const TERMINAL_DISABLED: &str = "terminal_disabled";
pub const TERMINAL_WORKSPACE_NOT_READY: &str = "terminal_workspace_not_ready";
pub const TERMINAL_SESSION_LIMIT: &str = "terminal_session_limit";
pub const TERMINAL_USER_LIMIT: &str = "terminal_user_limit";
pub const TERMINAL_DAEMON_UNAVAILABLE: &str = "terminal_daemon_unavailable";
pub const TERMINAL_ACTIVE_EXECUTION: &str = "terminal_active_execution";
pub const TERMINAL_ATTACH_TOKEN_INVALID: &str = "terminal_attach_token_invalid";
pub const TERMINAL_PATH_GUARDRAIL: &str = "terminal_path_guardrail";
pub const TERMINAL_NOT_FOUND: &str = "terminal_not_found";
pub const TERMINAL_INVALID_INPUT: &str = "invalid_input";

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TerminalSessionStatus {
    Starting,
    Running,
    Exited,
    Terminated,
    TimedOut,
    Orphaned,
    CleanupTerminated,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalSessionResponse {
    pub id: String,
    pub task_id: String,
    pub workspace_id: String,
    pub daemon_id: Option<String>,
    pub status: TerminalSessionStatus,
    pub rows: u16,
    pub cols: u16,
    pub exit_code: Option<i32>,
    pub exit_signal: Option<String>,
    pub exit_reason: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub last_activity_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_by_user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateTerminalSessionRequest {
    pub rows: Option<u16>,
    pub cols: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateTerminalSessionResponse {
    pub session: TerminalSessionResponse,
    pub attach: TerminalAttachTokenResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResizeTerminalSessionRequest {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalAttachTokenResponse {
    pub attach_token: String,
    pub expires_at: String,
    pub ws_url: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalAvailability {
    pub enabled: bool,
    pub workspace_ready: bool,
    pub daemon_reachable: bool,
    pub active_execution: bool,
    pub session_count_for_task: u32,
    pub session_count_for_user: u32,
    pub max_sessions_per_task: u32,
    pub max_sessions_per_user: u32,
    pub can_create: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum TerminalClientFrame {
    Input { data: String },
    Resize { rows: u16, cols: u16 },
    Ping {},
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum TerminalServerFrame {
    Output {
        data: String,
    },
    Exit {
        exit_code: Option<i32>,
        signal: Option<String>,
        reason: Option<String>,
    },
    Error {
        code: String,
        message: String,
    },
    Pong {},
}
