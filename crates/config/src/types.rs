use crate::{
    default_data_dir, default_workspace_root, DEFAULT_AGENT_HEARTBEAT_INTERVAL_SECONDS,
    DEFAULT_AGENT_MAX_CONCURRENT_TASKS, DEFAULT_AGENT_MAX_MISSED_HEARTBEATS, DEFAULT_BCRYPT_COST,
    DEFAULT_CORS_ORIGIN, DEFAULT_MEDIA_UPLOAD_LIMIT_BYTES, DEFAULT_SERVER_BIND,
    DEFAULT_WORKSPACE_CLEANUP_DELAY_SECONDS,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeConfig {
    pub forge: ForgePaths,
    pub server: ServerConfig,
    pub workspace: WorkspaceConfig,
    pub agent: AgentDefaults,
    #[serde(default)]
    pub terminal: TerminalConfig,
    pub project: ProjectSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgePaths {
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind: String,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default = "default_mcp_enabled")]
    pub mcp_enabled: bool,
    pub jwt_secret: Option<String>,
    #[serde(default = "default_bcrypt_cost")]
    pub bcrypt_cost: u32,
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
    #[serde(default = "default_media_upload_limit_bytes")]
    pub media_upload_limit_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub cleanup_delay_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDefaults {
    pub max_concurrent_tasks: u32,
    pub heartbeat_interval_seconds: u64,
    pub max_missed_heartbeats: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub enabled: bool,
    pub max_sessions_per_task: u32,
    pub max_sessions_per_user: u32,
    pub idle_timeout_secs: u64,
    pub max_lifetime_secs: u64,
    pub attach_token_ttl_secs: u64,
    pub reconnect_scrollback_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigOverrides {
    pub server_bind: Option<String>,
    pub server_public_base_url: Option<String>,
    pub mcp_enabled: Option<bool>,
    pub data_dir: Option<PathBuf>,
    pub workspace_root: Option<PathBuf>,
    pub workspace_cleanup_delay_seconds: Option<u64>,
    pub agent_max_concurrent_tasks: Option<u32>,
    pub agent_heartbeat_interval_seconds: Option<u64>,
    pub agent_max_missed_heartbeats: Option<u32>,
    pub jwt_secret: Option<String>,
    pub bcrypt_cost: Option<u32>,
    pub cors_origins: Option<Vec<String>>,
    pub media_upload_limit_bytes: Option<u64>,
}

impl ForgeConfig {
    #[must_use]
    pub fn db_path(&self) -> PathBuf {
        self.forge.data_dir.join("forge.db")
    }

    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.forge.data_dir.join("sessions")
    }

    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.sessions_dir()
    }

    #[must_use]
    pub fn trusted_origin(&self) -> String {
        self.server
            .public_base_url
            .as_deref()
            .and_then(parse_trusted_origin)
            .unwrap_or_else(|| format!("http://{}", self.server.bind))
    }

    #[must_use]
    pub fn mcp_resource_url(&self) -> String {
        format!("{}/mcp", self.trusted_origin())
    }

    #[must_use]
    pub fn workflows_dir(&self) -> PathBuf {
        self.forge.data_dir.join("workflows")
    }

    pub fn ensure_workflows_dir(&self) -> std::io::Result<PathBuf> {
        let dir = self.workflows_dir();
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[must_use]
    pub fn jwt_secret_path(&self) -> PathBuf {
        self.forge.data_dir.join("jwt_secret.bin")
    }
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            forge: ForgePaths {
                data_dir: default_data_dir(),
            },
            server: ServerConfig {
                bind: DEFAULT_SERVER_BIND.to_owned(),
                public_base_url: None,
                mcp_enabled: true,
                jwt_secret: None,
                bcrypt_cost: DEFAULT_BCRYPT_COST,
                cors_origins: vec![DEFAULT_CORS_ORIGIN.to_owned()],
                media_upload_limit_bytes: DEFAULT_MEDIA_UPLOAD_LIMIT_BYTES,
            },
            workspace: WorkspaceConfig {
                root: default_workspace_root(),
                cleanup_delay_seconds: DEFAULT_WORKSPACE_CLEANUP_DELAY_SECONDS,
            },
            agent: AgentDefaults {
                max_concurrent_tasks: DEFAULT_AGENT_MAX_CONCURRENT_TASKS,
                heartbeat_interval_seconds: DEFAULT_AGENT_HEARTBEAT_INTERVAL_SECONDS,
                max_missed_heartbeats: DEFAULT_AGENT_MAX_MISSED_HEARTBEATS,
            },
            terminal: TerminalConfig::default(),
            project: ProjectSettings::default(),
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_sessions_per_task: 2,
            max_sessions_per_user: 4,
            idle_timeout_secs: 1800,
            max_lifetime_secs: 28800,
            attach_token_ttl_secs: 60,
            reconnect_scrollback_bytes: 65536,
        }
    }
}

fn default_mcp_enabled() -> bool {
    true
}

fn default_bcrypt_cost() -> u32 {
    DEFAULT_BCRYPT_COST
}

fn default_cors_origins() -> Vec<String> {
    vec![DEFAULT_CORS_ORIGIN.to_owned()]
}

fn default_media_upload_limit_bytes() -> u64 {
    DEFAULT_MEDIA_UPLOAD_LIMIT_BYTES
}

fn parse_trusted_origin(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    let origin = url.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}
