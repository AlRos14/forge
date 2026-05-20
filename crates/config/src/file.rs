use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) struct FileConfig {
    pub forge: Option<FileForgePaths>,
    pub server: Option<FileServerConfig>,
    pub workspace: Option<FileWorkspaceConfig>,
    pub agent: Option<FileAgentDefaults>,
    pub project: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileForgePaths {
    pub data_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileServerConfig {
    pub bind: Option<String>,
    pub public_base_url: Option<String>,
    pub mcp_enabled: Option<bool>,
    pub jwt_secret: Option<String>,
    pub bcrypt_cost: Option<u32>,
    pub cors_origins: Option<Vec<String>>,
    pub media_upload_limit_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileWorkspaceConfig {
    pub root: Option<String>,
    pub cleanup_delay_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileAgentDefaults {
    pub max_concurrent_tasks: Option<u32>,
    pub heartbeat_interval_seconds: Option<u64>,
    pub max_missed_heartbeats: Option<u32>,
}
