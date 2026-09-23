use serde_json::Value;

use crate::{Result, ServiceError};

async fn observe_usage(executor_type: &str, config_json: &str) -> Result<Value> {
    let kind = executor_type
        .parse::<executors::ExecutorKind>()
        .map_err(ServiceError::invalid_operation)?;
    let raw_config = serde_json::from_str::<Value>(config_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid {executor_type} agent configuration: {error}"))
    })?;
    let registry = cli_adapters::default_registry();
    let adapter = registry.get(&kind).ok_or_else(|| {
        ServiceError::invalid_operation(format!("no HarnessAdapter registered for {executor_type}"))
    })?;
    let config = adapter
        .normalize_config(&raw_config, &executors::ExecutionOverrides::default())
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
    let capabilities = adapter.capabilities(&config);
    if !capabilities.account_usage_observation.is_available() {
        return Err(ServiceError::invalid_operation(format!(
            "{executor_type} account usage observation is {:?}",
            capabilities.account_usage_observation
        )));
    }
    adapter
        .observe_usage(&config, tokio_util::sync::CancellationToken::new())
        .await
        .map_err(|error| ServiceError::invalid_operation(error.to_string()))?
        .map(|observation| observation.value)
        .ok_or_else(|| ServiceError::invalid_operation("HarnessAdapter returned no usage observation"))
}

pub async fn refresh_codex_usage(config_json: &str) -> Result<Value> {
    observe_usage("codex", config_json).await
}

pub async fn refresh_cursor_usage(config_json: &str) -> Result<Value> {
    observe_usage("cursor", config_json).await
}
