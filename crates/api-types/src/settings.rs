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
    pub forge: Option<UpdateForgePathsRequest>,
    pub server: Option<UpdateServerSettingsRequest>,
    pub workspace: Option<UpdateWorkspaceSettingsRequest>,
    pub agent: Option<UpdateAgentSettingsRequest>,
    pub project: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateForgePathsRequest {
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateServerSettingsRequest {
    pub bind: Option<String>,
    pub mcp_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateWorkspaceSettingsRequest {
    pub root: Option<String>,
    pub cleanup_delay_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub struct UpdateAgentSettingsRequest {
    pub max_concurrent_tasks: Option<u32>,
    pub heartbeat_interval_seconds: Option<u64>,
    pub max_missed_heartbeats: Option<u32>,
}
