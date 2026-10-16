use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentUsageResponse {
    pub available: bool,
    pub executor_type: String,
    /// The exact observed account context, or the configured exact key when
    /// the Agent is pinned/credential-bound. Unpinned remote Agents have no
    /// key until an execution-linked observation exists.
    pub account_key: Option<String>,
    /// Host/daemon that produced the returned observation, when known.
    pub daemon_id: Option<String>,
    pub shared_account: bool,
    pub source: Option<String>,
    #[ts(type = "Record<string, unknown> | null")]
    pub usage: Option<serde_json::Value>,
    pub captured_at: Option<String>,
    pub stale: bool,
    pub message: Option<String>,
}
