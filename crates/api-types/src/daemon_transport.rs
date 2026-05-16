use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::FsEntry;

pub const METHOD_FS_LIST: &str = "fs.list";
pub const METHOD_FS_BRANCHES: &str = "fs.branches";
pub const METHOD_EXECUTION_START: &str = "execution.start";
pub const METHOD_EXECUTION_CANCEL: &str = "execution.cancel";
pub const METHOD_EXECUTION_LOG: &str = "execution.log";
pub const METHOD_EXECUTION_TERMINAL: &str = "execution.terminal";

pub const DAEMON_UNAVAILABLE: &str = "daemon_unavailable";
pub const DAEMON_TIMEOUT: &str = "daemon_timeout";
pub const UNSUPPORTED_METHOD: &str = "unsupported_method";
pub const INVALID_FRAME: &str = "invalid_frame";
pub const PATH_GUARDRAIL: &str = "path_guardrail";
pub const EXECUTION_NOT_FOUND: &str = "execution_not_found";

pub const DEFAULT_DAEMON_COMMAND_TIMEOUT_SECS: u64 = 30;
pub const DAEMON_HEARTBEAT_INTERVAL_SECS: u64 = 20;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "lowercase")]
#[ts(export)]
pub enum DaemonFrame {
    Request {
        id: String,
        method: String,
        #[ts(type = "unknown")]
        params: serde_json::Value,
    },
    Response {
        id: String,
        #[ts(type = "unknown")]
        result: serde_json::Value,
    },
    Error {
        id: Option<String>,
        error: DaemonErrorPayload,
    },
    Notification {
        method: String,
        #[ts(type = "unknown")]
        params: serde_json::Value,
    },
    Heartbeat {
        seq: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListResult {
    pub path: String,
    pub entries: Vec<FsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsBranchesParams {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsBranchesResult {
    pub branches: Vec<String>,
    pub default_branch: Option<String>,
    pub origin_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionStartParams {
    pub execution_id: String,
    pub workspace_path: String,
    pub executor_type: String,
    #[ts(type = "unknown")]
    pub executor_config: serde_json::Value,
    #[ts(type = "unknown")]
    pub prompt: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionStartResult {
    pub execution_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionCancelParams {
    pub execution_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionCancelResult {
    pub execution_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionLogNotification {
    pub execution_id: String,
    pub seq: u64,
    pub stream: String,
    pub line: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionTerminalNotification {
    pub execution_id: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub error: Option<String>,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DaemonErrorPayload {
    pub code: String,
    pub message: String,
    #[ts(type = "unknown")]
    pub details: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::{DaemonErrorPayload, DaemonFrame};

    #[test]
    fn request_frame_round_trips() {
        let frame = DaemonFrame::Request {
            id: "req-1".to_owned(),
            method: "fs.list".to_owned(),
            params: serde_json::json!({ "path": "/tmp" }),
        };

        let json = serde_json::to_value(&frame).expect("serialize request frame");
        assert_eq!(json["type"], "request");
        assert!(json.get("id").is_some());
        assert!(json.get("method").is_some());
        assert!(json.get("params").is_some());

        let decoded: DaemonFrame = serde_json::from_value(json).expect("deserialize request frame");
        assert!(matches!(decoded, DaemonFrame::Request { .. }));
    }

    #[test]
    fn response_frame_round_trips() {
        let frame = DaemonFrame::Response {
            id: "req-1".to_owned(),
            result: serde_json::json!({ "ok": true }),
        };

        let json = serde_json::to_value(&frame).expect("serialize response frame");
        assert_eq!(json["type"], "response");

        let decoded: DaemonFrame =
            serde_json::from_value(json).expect("deserialize response frame");
        assert!(matches!(decoded, DaemonFrame::Response { .. }));
    }

    #[test]
    fn error_frame_round_trips() {
        let frame = DaemonFrame::Error {
            id: Some("req-1".to_owned()),
            error: DaemonErrorPayload {
                code: "daemon_timeout".to_owned(),
                message: "daemon timed out".to_owned(),
                details: None,
            },
        };

        let json = serde_json::to_value(&frame).expect("serialize error frame");
        assert_eq!(json["type"], "error");

        let decoded: DaemonFrame = serde_json::from_value(json).expect("deserialize error frame");
        assert!(matches!(decoded, DaemonFrame::Error { .. }));
    }

    #[test]
    fn notification_frame_round_trips() {
        let frame = DaemonFrame::Notification {
            method: "execution.log".to_owned(),
            params: serde_json::json!({
                "execution_id": "exec-1",
                "seq": 1,
                "stream": "stdout",
                "line": "started",
                "ts": "2026-05-14T00:00:00Z"
            }),
        };

        let json = serde_json::to_value(&frame).expect("serialize notification frame");
        assert_eq!(json["type"], "notification");

        let decoded: DaemonFrame =
            serde_json::from_value(json).expect("deserialize notification frame");
        assert!(matches!(decoded, DaemonFrame::Notification { .. }));
    }
}
