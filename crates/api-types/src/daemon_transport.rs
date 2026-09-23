use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::FsEntry;

pub const METHOD_FS_LIST: &str = "fs.list";
pub const METHOD_FS_BRANCHES: &str = "fs.branches";
pub const METHOD_EXECUTION_START: &str = "execution.start";
pub const METHOD_EXECUTION_CANCEL: &str = "execution.cancel";
pub const METHOD_PROTOCOL_CAPABILITIES: &str = "daemon.protocol_capabilities";
pub const DAEMON_PROTOCOL_FEATURE_GENERIC_HARNESS_INVOCATION_V1: &str =
    "generic_harness_invocation_v1";
pub const DAEMON_PROTOCOL_FEATURE_EXECUTION_ROLE_V1: &str = "execution_role_v1";
pub const METHOD_EXECUTION_LOG: &str = "execution.log";
pub const METHOD_EXECUTION_TERMINAL: &str = "execution.terminal";
pub const METHOD_TERMINAL_START: &str = "terminal.start";
pub const METHOD_TERMINAL_INPUT: &str = "terminal.input";
pub const METHOD_TERMINAL_RESIZE: &str = "terminal.resize";
pub const METHOD_TERMINAL_TERMINATE: &str = "terminal.terminate";
pub const METHOD_TERMINAL_OUTPUT: &str = "terminal.output";
pub const METHOD_TERMINAL_EXITED: &str = "terminal.exited";

pub const DAEMON_UNAVAILABLE: &str = "daemon_unavailable";
pub const DAEMON_TIMEOUT: &str = "daemon_timeout";
pub const UNSUPPORTED_METHOD: &str = "unsupported_method";
pub const INVALID_FRAME: &str = "invalid_frame";
pub const INVALID_INPUT: &str = "invalid_input";
pub const PATH_GUARDRAIL: &str = "path_guardrail";
pub const EXECUTION_NOT_FOUND: &str = "execution_not_found";

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DaemonProtocolCapabilitiesRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DaemonProtocolCapabilities {
    pub schema_version: u32,
    pub features: Vec<String>,
}

impl DaemonProtocolCapabilities {
    pub fn supports(&self, feature: &str) -> bool {
        self.schema_version == 1 && self.features.iter().any(|item| item == feature)
    }
}

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
    pub task_id: String,
    pub execution_id: String,
    #[serde(default)]
    pub role: String,
    pub workspace_path: String,
    pub executor_type: String,
    #[ts(type = "unknown")]
    pub executor_config: serde_json::Value,
    #[ts(type = "unknown")]
    pub prompt: serde_json::Value,
    pub invocation: crate::HarnessInvocation,
    pub max_turns: Option<u32>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_stream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionTerminalNotification {
    pub execution_id: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub error: Option<String>,
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<RemoteTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown")]
    pub account_usage: Option<serde_json::Value>,
    /// Structured failure disposition. Absent on older daemons — the server
    /// then falls back to generic executor-failed handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<RemoteExecutionFailureClass>,
    /// RFC3339 time when an unavailable executor route is worth retrying.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_candidate: Option<RemoteResolvedCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_attempts: Option<Vec<RemoteRouteAttempt>>,
}

/// Structured failure class carried across the daemon protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RemoteExecutionFailureClass {
    TaskFailed,
    ExecutorUnavailable,
}

/// The executor candidate that actually ran a remote execution.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteResolvedCandidate {
    pub candidate_key: String,
    pub executor_type: String,
    #[ts(type = "Record<string, unknown>")]
    pub config: serde_json::Value,
    /// Absent on pre-PR3 daemons. The server resolves absence to an explicit
    /// all-Unknown snapshot and never inherits primary-candidate evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_capabilities: Option<crate::HarnessCapabilitiesSnapshot>,
    #[serde(default)]
    pub effective_policy: Option<crate::EffectiveExecutionPolicy>,
}

/// One candidate attempt outcome from a remote execution's fallback route.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteRouteAttempt {
    pub candidate_key: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteTokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalStartParams {
    pub session_id: String,
    pub workspace_path: String,
    pub rows: u16,
    pub cols: u16,
    pub shell: Option<String>,
    pub env: Option<Vec<(String, String)>>,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalStartResult {
    pub session_id: String,
    pub pid: Option<u32>,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalInputParams {
    pub session_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalInputResult {
    pub session_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalResizeParams {
    pub session_id: String,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalResizeResult {
    pub session_id: String,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalTerminateParams {
    pub session_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalTerminateResult {
    pub session_id: String,
    pub terminated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalOutputNotification {
    pub session_id: String,
    pub data: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalExitedNotification {
    pub session_id: String,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub reason: Option<String>,
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
    use super::{
        DaemonErrorPayload, DaemonFrame, ExecutionStartParams, RemoteResolvedCandidate,
        TerminalOutputNotification,
    };
    use crate::HarnessInvocation;
    use serde::Deserialize;

    fn execution_start_params(invocation: HarnessInvocation) -> ExecutionStartParams {
        ExecutionStartParams {
            task_id: "task-1".to_owned(),
            execution_id: "execution-1".to_owned(),
            role: "coder".to_owned(),
            workspace_path: "/tmp/worktree".to_owned(),
            executor_type: "codex".to_owned(),
            executor_config: serde_json::json!({"model":"test"}),
            prompt: serde_json::json!({"description":"continue"}),
            invocation,
            max_turns: None,
        }
    }

    #[test]
    fn generic_start_and_resume_survive_daemon_transport_serialization() {
        for invocation in [
            HarnessInvocation::Start,
            HarnessInvocation::Resume {
                external_session_id: "external-session-abc".to_owned(),
            },
        ] {
            let params = execution_start_params(invocation.clone());
            let encoded = serde_json::to_value(&params).expect("start params serialize");
            let decoded: ExecutionStartParams =
                serde_json::from_value(encoded).expect("start params deserialize");
            assert_eq!(decoded.invocation, invocation);
            assert_eq!(decoded.role, "coder");
        }
    }

    #[test]
    fn pre_pr3_execution_start_without_invocation_is_rejected() {
        let legacy_start_payload = serde_json::json!({
            "task_id": "task-1",
            "execution_id": "execution-1",
            "workspace_path": "/tmp/worktree",
            "executor_type": "codex",
            "executor_config": {},
            "prompt": { "description": "start" },
            "max_turns": null
        });
        assert!(serde_json::from_value::<ExecutionStartParams>(legacy_start_payload).is_err());

        for (executor_type, key) in [
            ("codex", "resume_thread_id"),
            ("claude", "resume_session_id"),
            ("cursor", "resume_session_id"),
            ("smith", "resume_session_id"),
        ] {
            let mut executor_config = serde_json::Map::new();
            executor_config.insert(key.to_owned(), serde_json::json!("session-123"));
            let payload = serde_json::json!({
                "task_id": "task-1",
                "execution_id": "execution-1",
                "workspace_path": "/tmp/worktree",
                "executor_type": executor_type,
                "executor_config": executor_config,
                "prompt": { "description": "continue" },
                "max_turns": null
            });

            let error = serde_json::from_value::<ExecutionStartParams>(payload)
                .expect_err("pre-PR3 payload must not silently default to Start");
            assert!(error.to_string().contains("invocation"));
        }
    }

    #[test]
    fn remote_resolved_candidate_carries_effective_harness_capabilities() {
        let candidate = RemoteResolvedCandidate {
            candidate_key: "cursor:profile=work#1234".to_owned(),
            executor_type: "cursor".to_owned(),
            config: serde_json::json!({"profile":"work"}),
            harness_capabilities: Some(crate::HarnessCapabilities::unknown().snapshot()),
            effective_policy: None,
        };
        let encoded = serde_json::to_value(candidate).expect("candidate serializes");
        assert_eq!(encoded["harness_capabilities"]["schema_version"], 1);
        assert_eq!(
            encoded["harness_capabilities"]["capabilities"]["resume"],
            "unknown"
        );
    }

    #[test]
    fn old_daemon_winner_omission_is_not_inherited_as_capability_evidence() {
        let candidate: RemoteResolvedCandidate = serde_json::from_value(serde_json::json!({
            "candidate_key":"codex#primary",
            "executor_type":"codex",
            "config":{"model":"model-a"}
        }))
        .expect("legacy resolved candidate remains decodable");
        assert!(candidate.harness_capabilities.is_none());
        assert!(candidate.effective_policy.is_none());
    }

    #[test]
    fn new_server_start_payload_remains_safe_for_old_daemon() {
        #[derive(Deserialize)]
        struct OldExecutionStartParams {
            task_id: String,
            execution_id: String,
            workspace_path: String,
            executor_type: String,
            executor_config: serde_json::Value,
            prompt: serde_json::Value,
            max_turns: Option<u32>,
        }

        let new_payload = serde_json::to_value(ExecutionStartParams {
            task_id: "task-1".to_owned(),
            execution_id: "execution-1".to_owned(),
            role: "worker".to_owned(),
            workspace_path: "/work".to_owned(),
            executor_type: "codex".to_owned(),
            executor_config: serde_json::json!({"executor_type":"codex","config":{}}),
            prompt: serde_json::json!({"description":"start"}),
            invocation: crate::HarnessInvocation::Start,
            max_turns: None,
        })
        .expect("Start payload serializes");
        let old_decoded: OldExecutionStartParams = serde_json::from_value(new_payload)
            .expect("old daemon ignores additive Start intent");
        assert_eq!(old_decoded.task_id, "task-1");
        assert_eq!(old_decoded.execution_id, "execution-1");
        assert_eq!(old_decoded.workspace_path, "/work");
        assert_eq!(old_decoded.executor_type, "codex");
        assert_eq!(old_decoded.executor_config["executor_type"], "codex");
        assert_eq!(old_decoded.prompt["description"], "start");
        assert_eq!(old_decoded.max_turns, None);
    }

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

    #[test]
    fn terminal_output_notification_round_trips() {
        let notification = TerminalOutputNotification {
            session_id: "term-1".to_owned(),
            data: "hello\r\n".to_owned(),
            ts: "2026-05-20T00:00:00Z".to_owned(),
        };

        let json = serde_json::to_value(&notification).expect("serialize terminal output");
        assert_eq!(json["session_id"], "term-1");
        assert_eq!(json["data"], "hello\r\n");

        let decoded: TerminalOutputNotification =
            serde_json::from_value(json).expect("deserialize terminal output");
        assert_eq!(decoded.session_id, "term-1");
        assert_eq!(decoded.data, "hello\r\n");
        assert_eq!(decoded.ts, "2026-05-20T00:00:00Z");
    }
}
