#![forbid(unsafe_code)]

mod defaults;
mod error;
mod file;
mod loader;
mod path;
#[cfg(test)]
mod tests;
mod types;

pub use defaults::{
    DEFAULT_AGENT_HEARTBEAT_INTERVAL_SECONDS, DEFAULT_AGENT_MAX_CONCURRENT_TASKS,
    DEFAULT_AGENT_MAX_MISSED_HEARTBEATS, DEFAULT_BCRYPT_COST, DEFAULT_CORS_ORIGIN,
    DEFAULT_SERVER_BIND, DEFAULT_WORKSPACE_CLEANUP_DELAY_SECONDS,
};
pub use error::ConfigError;
pub use path::{default_config_path, default_data_dir, default_workspace_root};
pub use types::{
    AgentDefaults, ConfigOverrides, ForgeConfig, ForgePaths, ProjectSettings, ServerConfig,
    WorkspaceConfig,
};
