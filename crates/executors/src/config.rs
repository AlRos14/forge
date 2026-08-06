use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::any::Any;
use std::collections::HashMap;

use crate::{ExecutionOverrides, ExecutorError, ExecutorKind};

/// Shared command override fields embedded in every typed config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CommandOverrides {
    pub base_command_override: Option<String>,
    pub additional_params: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
}

/// Cross-executor permission abstraction.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    Auto,
    #[default]
    Supervised,
    Plan,
}

impl std::fmt::Display for PermissionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Supervised => write!(f, "supervised"),
            Self::Plan => write!(f, "plan"),
        }
    }
}

impl std::str::FromStr for PermissionPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "supervised" => Ok(Self::Supervised),
            "plan" => Ok(Self::Plan),
            other => Err(format!("unknown permission policy: {other}")),
        }
    }
}

/// Shell executor config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShellConfig {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub timeout_seconds: Option<u64>,
    pub permission_policy: Option<PermissionPolicy>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Codex executor config. Field names compatible with Vibe Kanban.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CodexConfig {
    pub model: Option<String>,
    pub sandbox: Option<String>,
    pub ask_for_approval: Option<String>,
    pub model_reasoning_effort: Option<String>,
    pub model_reasoning_summary: Option<String>,
    pub profile: Option<String>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub include_apply_patch_tool: Option<bool>,
    pub resume_thread_id: Option<String>,
    /// Start the next turn on `resume_thread_id` instead of forking a derived thread.
    ///
    /// Coding/chat follow-ups should keep the same agent session so Codex can reuse
    /// thread history and cache state. Review-style runs may intentionally omit this
    /// and fork from the source thread to inspect the prior work in a separate run.
    pub resume_thread_in_place: Option<bool>,
    /// Prompt used only when an in-place resume cannot find the stored Codex thread.
    pub resume_fallback_prompt: Option<String>,
    pub auto_commit: Option<bool>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Claude Code executor config. Field names compatible with Vibe Kanban.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ClaudeCodeConfig {
    pub model: Option<String>,
    pub plan: Option<bool>,
    pub approvals: Option<String>,
    pub effort: Option<String>,
    pub agent: Option<String>,
    /// Claude Code session id used for follow-up turns.
    pub resume_session_id: Option<String>,
    pub dangerously_skip_permissions: Option<bool>,
    pub claude_code_router: Option<bool>,
    pub disable_api_key: Option<bool>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Cursor Agent CLI executor config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CursorConfig {
    pub model: Option<String>,
    pub force: Option<bool>,
    pub resume_session_id: Option<String>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// OpenCode executor config. Field names compatible with Vibe Kanban.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct OpencodeConfig {
    pub model: Option<String>,
    pub variant: Option<String>,
    pub agent: Option<String>,
    pub auto_approve: Option<bool>,
    pub auto_compact: Option<bool>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    pub resume_session_id: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Gemini CLI executor config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GeminiConfig {
    pub model: Option<String>,
    pub sandbox: Option<String>,
    pub yolo: Option<bool>,
    pub check_every_n: Option<u32>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Smith CLI executor config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SmithConfig {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub profile: Option<String>,
    pub yolo: Option<bool>,
    pub approval: Option<String>,
    pub resume_session_id: Option<String>,
    pub permission_policy: Option<PermissionPolicy>,
    pub prompt_template: Option<String>,
    #[serde(flatten)]
    pub command_overrides: CommandOverrides,
}

/// Null executor config. Completes after a configurable delay.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NullConfig {
    #[serde(default = "default_delay_seconds")]
    pub delay_seconds: u64,
}

fn default_delay_seconds() -> u64 {
    5
}

impl Default for NullConfig {
    fn default() -> Self {
        Self {
            delay_seconds: default_delay_seconds(),
        }
    }
}

/// Deserialize a raw JSON config into the typed config struct for an executor kind.
pub fn deserialize_config(
    kind: ExecutorKind,
    json: &Value,
) -> Result<Box<dyn Any + Send + Sync>, ExecutorError> {
    match kind {
        ExecutorKind::Shell => deserialize_typed::<ShellConfig>(kind, json),
        ExecutorKind::Codex => deserialize_typed::<CodexConfig>(kind, json),
        ExecutorKind::ClaudeCode => deserialize_typed::<ClaudeCodeConfig>(kind, json),
        ExecutorKind::Cursor => deserialize_typed::<CursorConfig>(kind, json),
        ExecutorKind::Opencode => deserialize_typed::<OpencodeConfig>(kind, json),
        ExecutorKind::Gemini => deserialize_typed::<GeminiConfig>(kind, json),
        ExecutorKind::Smith => deserialize_typed::<SmithConfig>(kind, json),
        ExecutorKind::Null => deserialize_typed::<NullConfig>(kind, json),
    }
}

/// Apply per-execution overrides to a config JSON object in-place.
pub fn merge_overrides(
    config: &mut Value,
    overrides: &ExecutionOverrides,
) -> Result<(), ExecutorError> {
    let Value::Object(map) = config else {
        return Err(ExecutorError::Other(
            "profile config_json must be a JSON object".to_owned(),
        ));
    };

    if let Some(model_id) = &overrides.model_id {
        map.insert("model".to_owned(), Value::String(model_id.clone()));
    }
    if let Some(reasoning_effort) = &overrides.reasoning_effort {
        map.insert(
            "model_reasoning_effort".to_owned(),
            Value::String(reasoning_effort.clone()),
        );
        map.insert("effort".to_owned(), Value::String(reasoning_effort.clone()));
    }
    if let Some(permission_policy) = &overrides.permission_policy {
        map.insert(
            "permission_policy".to_owned(),
            Value::String(permission_policy.clone()),
        );
    }

    Ok(())
}

/// Resolve config JSON by applying overrides, deserializing into the typed struct,
/// and serializing back to normalized JSON.
pub fn resolve_config_value(
    kind: ExecutorKind,
    json: &Value,
    overrides: &ExecutionOverrides,
) -> Result<Value, ExecutorError> {
    let mut merged = json.clone();
    merge_overrides(&mut merged, overrides)?;
    match kind {
        ExecutorKind::Shell => normalize_typed::<ShellConfig>(kind, &merged),
        ExecutorKind::Codex => normalize_typed::<CodexConfig>(kind, &merged),
        ExecutorKind::ClaudeCode => normalize_typed::<ClaudeCodeConfig>(kind, &merged),
        ExecutorKind::Cursor => normalize_typed::<CursorConfig>(kind, &merged),
        ExecutorKind::Opencode => normalize_typed::<OpencodeConfig>(kind, &merged),
        ExecutorKind::Gemini => normalize_typed::<GeminiConfig>(kind, &merged),
        ExecutorKind::Smith => normalize_typed::<SmithConfig>(kind, &merged),
        ExecutorKind::Null => normalize_typed::<NullConfig>(kind, &merged),
    }
}

fn deserialize_typed<T>(
    kind: ExecutorKind,
    json: &Value,
) -> Result<Box<dyn Any + Send + Sync>, ExecutorError>
where
    T: for<'de> Deserialize<'de> + Send + Sync + 'static,
{
    serde_json::from_value::<T>(json.clone())
        .map(|config| Box::new(config) as Box<dyn Any + Send + Sync>)
        .map_err(|error| {
            ExecutorError::Other(format!("Failed to deserialize {} config: {error}", kind))
        })
}

fn normalize_typed<T>(kind: ExecutorKind, json: &Value) -> Result<Value, ExecutorError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let config = serde_json::from_value::<T>(json.clone()).map_err(|error| {
        ExecutorError::Other(format!("Failed to deserialize {} config: {error}", kind))
    })?;
    serde_json::to_value(config).map_err(|error| {
        ExecutorError::Other(format!("Failed to serialize {} config: {error}", kind))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_config_round_trips_and_drops_unknown_fields() {
        let value = serde_json::json!({
            "model": "o3",
            "sandbox": "danger-full-access",
            "resume_thread_id": "thread-1",
            "auto_commit": false,
            "unknown_field": true,
            "additional_params": ["--verbose"]
        });

        let resolved =
            resolve_config_value(ExecutorKind::Codex, &value, &ExecutionOverrides::default())
                .expect("config resolves");

        assert_eq!(resolved["model"], "o3");
        assert_eq!(resolved["sandbox"], "danger-full-access");
        assert_eq!(resolved["resume_thread_id"], "thread-1");
        assert_eq!(resolved["auto_commit"], false);
        assert!(resolved.get("unknown_field").is_none());
        assert_eq!(resolved["additional_params"][0], "--verbose");
    }

    #[test]
    fn override_merge_preserves_unset_fields() {
        let value = serde_json::json!({
            "model": "o3",
            "sandbox": "danger-full-access"
        });
        let overrides = ExecutionOverrides {
            model_id: Some("o3-mini".to_owned()),
            reasoning_effort: None,
            permission_policy: Some("supervised".to_owned()),
        };

        let resolved =
            resolve_config_value(ExecutorKind::Codex, &value, &overrides).expect("config resolves");

        assert_eq!(resolved["model"], "o3-mini");
        assert_eq!(resolved["sandbox"], "danger-full-access");
        assert_eq!(resolved["permission_policy"], "supervised");
    }

    #[test]
    fn reasoning_override_resolves_to_executor_specific_key() {
        let overrides = ExecutionOverrides {
            model_id: None,
            reasoning_effort: Some("high".to_owned()),
            permission_policy: None,
        };

        let codex = resolve_config_value(ExecutorKind::Codex, &serde_json::json!({}), &overrides)
            .expect("codex config resolves");
        assert_eq!(codex["model_reasoning_effort"], "high");
        assert!(codex.get("effort").is_none());

        let claude =
            resolve_config_value(ExecutorKind::ClaudeCode, &serde_json::json!({}), &overrides)
                .expect("claude config resolves");
        assert_eq!(claude["effort"], "high");
        assert!(claude.get("model_reasoning_effort").is_none());
    }

    #[test]
    fn shell_config_accepts_permission_policy_override() {
        let overrides = ExecutionOverrides {
            model_id: None,
            reasoning_effort: None,
            permission_policy: Some("auto".to_owned()),
        };

        let resolved =
            resolve_config_value(ExecutorKind::Shell, &serde_json::json!({}), &overrides)
                .expect("shell config resolves");

        assert_eq!(resolved["permission_policy"], "auto");
    }

    #[test]
    fn invalid_permission_policy_is_rejected() {
        let value = serde_json::json!({ "permission_policy": "root" });

        let error = resolve_config_value(
            ExecutorKind::ClaudeCode,
            &value,
            &ExecutionOverrides::default(),
        )
        .expect_err("invalid policy rejects");

        assert!(error
            .to_string()
            .contains("Failed to deserialize claude_code config"));
    }
}
