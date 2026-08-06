use api_types::{DiscoveredDaemonResponse, DiscoveredOptionsResponse, ExecutorTypeDescriptor};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use db::DaemonRepo;
use executors::{
    ClaudeCodeConfig, CodexConfig, CursorConfig, DiscoverContext, ExecutorKind, GeminiConfig,
    OpencodeConfig, ShellConfig, SmithConfig,
};
use schemars::{schema_for, JsonSchema};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::{
    errors::{ApiError, ApiResult},
    routes::auth::AuthenticatedUser,
    state::AppState,
};

pub async fn list_executor_types() -> Json<Vec<ExecutorTypeDescriptor>> {
    Json(vec![
        descriptor::<ShellConfig>("shell", "Shell", schema_to_value(schema_for!(ShellConfig))),
        descriptor::<CodexConfig>("codex", "Codex", schema_to_value(schema_for!(CodexConfig))),
        descriptor::<ClaudeCodeConfig>(
            "claude_code",
            "Claude Code",
            schema_to_value(schema_for!(ClaudeCodeConfig)),
        ),
        descriptor::<CursorConfig>(
            "cursor",
            "Cursor",
            schema_to_value(schema_for!(CursorConfig)),
        ),
        descriptor::<OpencodeConfig>(
            "opencode",
            "OpenCode",
            schema_to_value(schema_for!(OpencodeConfig)),
        ),
        descriptor::<GeminiConfig>(
            "gemini",
            "Gemini",
            schema_to_value(schema_for!(GeminiConfig)),
        ),
        descriptor::<SmithConfig>("smith", "Smith", schema_to_value(schema_for!(SmithConfig))),
    ])
}

pub async fn executor_type_discovered_options(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(type_name): Path<String>,
    Query(params): Query<DiscoveryParams>,
) -> ApiResult<Json<DiscoveredOptionsResponse>> {
    let kind = parse_executor_kind(&type_name)?;
    let adapter = state.adapter_registry.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "No adapter registered for executor type: {type_name}"
        ))
    })?;
    let daemons = if user.is_admin {
        DaemonRepo::list_available_for_executor(&*state.db, &type_name).await?
    } else {
        Vec::new()
    };
    let discovered = adapter
        .discover_options(DiscoverContext {
            project_path: params.project_id,
        })
        .await
        .map_err(|error| ApiError::bad_request(error.to_string()))?;

    Ok(Json(DiscoveredOptionsResponse {
        models: discovered.models,
        permission_policies: discovered.permission_policies,
        cli_specific: discovered.cli_specific,
        available_daemons: daemons
            .into_iter()
            .map(|daemon| DiscoveredDaemonResponse {
                id: daemon.id,
                name: daemon.hostname,
                status: daemon.status.to_string(),
            })
            .collect(),
        warning: None,
    }))
}

fn descriptor<T>(
    type_name: &str,
    display_name: &str,
    config_schema: Value,
) -> ExecutorTypeDescriptor
where
    T: Default + JsonSchema + Serialize,
{
    ExecutorTypeDescriptor {
        type_name: type_name.to_owned(),
        display_name: display_name.to_owned(),
        config_schema,
        default_config: serde_json::to_value(T::default()).unwrap_or(Value::Null),
    }
}

fn schema_to_value<T>(schema: T) -> Value
where
    T: Serialize,
{
    serde_json::to_value(schema).unwrap_or(Value::Null)
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryParams {
    pub project_id: Option<String>,
}

fn parse_executor_kind(value: &str) -> ApiResult<ExecutorKind> {
    value
        .parse()
        .map_err(|_| ApiError::bad_request(format!("invalid executor_type: {value}")))
}
