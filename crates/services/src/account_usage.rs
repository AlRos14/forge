use serde_json::Value;

use crate::{Result, ServiceError};

pub async fn refresh_codex_usage(config_json: &str) -> Result<Value> {
    let config = serde_json::from_str::<executors::CodexConfig>(config_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid Codex agent configuration: {error}"))
    })?;
    cli_adapters::codex::query_account_usage(&config)
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to query Codex usage: {error}"))
        })
}

pub async fn refresh_cursor_usage(config_json: &str) -> Result<Value> {
    let config = serde_json::from_str::<executors::CursorConfig>(config_json).unwrap_or_default();
    cli_adapters::cursor::query_account_usage(&config)
        .await
        .map_err(|error| {
            ServiceError::invalid_operation(format!("failed to query Cursor usage: {error}"))
        })
}
