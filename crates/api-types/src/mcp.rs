use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpAgent {
    Claude,
    Cursor,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpScope {
    Project,
    Local,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpAction {
    Install,
    Uninstall,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct McpConfigQuery {
    pub agent: String,
    pub scope: Option<String>,
    pub project_id: Option<String>,
    pub public_base_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct McpConfigActionRequest {
    pub agent: String,
    #[ts(optional = nullable)]
    pub scope: Option<String>,
    #[ts(optional = nullable)]
    pub project_id: Option<String>,
    #[ts(optional = nullable)]
    pub public_base_url: Option<String>,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct McpConfigResponse {
    pub installed: bool,
    pub url: Option<String>,
    pub expected_url: String,
    pub config_path: String,
    pub agents: Vec<String>,
}
