use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SettingsResponse {
    pub config_path: String,
    pub restart_required: bool,
    pub settings: Vec<ForgeSettingResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForgeSettingResponse {
    pub key: String,
    #[ts(type = "unknown")]
    pub value: Value,
    #[ts(type = "unknown")]
    pub effective_value: Value,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateSettingsRequest {
    #[ts(optional = nullable)]
    pub forge: Option<UpdateForgePathsRequest>,
    #[ts(optional = nullable)]
    pub server: Option<UpdateServerSettingsRequest>,
    #[ts(optional = nullable)]
    pub workspace: Option<UpdateWorkspaceSettingsRequest>,
    #[ts(optional = nullable)]
    pub agent: Option<UpdateAgentSettingsRequest>,
    #[ts(optional = nullable)]
    pub project: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateForgePathsRequest {
    #[ts(optional = nullable)]
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateServerSettingsRequest {
    #[ts(optional = nullable)]
    pub bind: Option<String>,
    #[ts(optional = nullable)]
    pub mcp_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateWorkspaceSettingsRequest {
    #[ts(optional = nullable)]
    pub root: Option<String>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub cleanup_delay_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateAgentSettingsRequest {
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub max_concurrent_tasks: Option<u32>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub heartbeat_interval_seconds: Option<u64>,
    #[ts(type = "number | null")]
    #[ts(optional = nullable)]
    pub max_missed_heartbeats: Option<u32>,
}
