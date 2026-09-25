use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use std::any::Any;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::{ExecutionOverrides, ExecutorError, ExecutorKind};

/// Runtime-only environment values attached to an in-memory execution
/// snapshot. This key is consumed before candidate provenance is produced.
pub const RUNTIME_ENV_KEY: &str = "runtime_env";

/// Apply runtime-only environment values after adapter config normalization.
/// These values are deliberately excluded from candidate identity, snapshots,
/// capability evidence, and policy evidence.
pub fn with_runtime_environment(
    config: &Value,
    execution_snapshot: &Value,
) -> Result<Value, ExecutorError> {
    let Some(runtime_env) = execution_snapshot.get(RUNTIME_ENV_KEY) else {
        return Ok(config.clone());
    };
    let runtime_env = runtime_env.as_object().ok_or_else(|| {
        ExecutorError::Other("runtime_env in execution context must be an object".to_owned())
    })?;
    if runtime_env.is_empty() {
        return Ok(config.clone());
    }

    let mut resolved = config.clone();
    let object = resolved.as_object_mut().ok_or_else(|| {
        ExecutorError::Other("normalized harness config must be an object".to_owned())
    })?;
    let env = object
        .entry("env")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let env = env.as_object_mut().ok_or_else(|| {
        ExecutorError::Other("normalized harness env must be an object".to_owned())
    })?;
    for (key, value) in runtime_env {
        let value = value
            .as_str()
            .ok_or_else(|| ExecutorError::Other("runtime_env values must be strings".to_owned()))?;
        env.insert(key.clone(), Value::String(value.to_owned()));
    }
    Ok(resolved)
}

/// Shared command override fields embedded in every typed config.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CommandOverrides {
    pub base_command_override: Option<String>,
    pub additional_params: Option<Vec<String>>,
    pub env: Option<HashMap<String, String>>,
}

/// Forge-hosted Agent Runtime configuration.
///
/// Embedded profiles are resolved here so task snapshots have the same typed,
/// deterministic normalization as CLI profiles.  Credential handles and
/// authority grants deliberately do not belong in this config: the native
/// host resolves those from the selected immutable profile and canonical Task
/// scope.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddedConfig {
    pub base_url: Option<String>,
    pub context_tokens: Option<u32>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub runtime_revision: Option<String>,
    pub prompt_template: Option<String>,
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
    /// Select Claude Code's explicit native plan permission mode. This is
    /// integration evidence for planning support; generic Purpose remains a
    /// separate Execution field until its consumer migrates in PR7.
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
    /// Reasoning effort name, forwarded as `--effort`. Populated from the agent's
    /// `reasoning_effort`. Smith validates the value against the selected
    /// provider/model ladder and refuses an unsupported one.
    pub effort: Option<String>,
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
#[cfg(test)]
pub fn deserialize_config(
    kind: ExecutorKind,
    json: &Value,
) -> Result<Box<dyn Any + Send + Sync>, ExecutorError> {
    match kind {
        ExecutorKind::Embedded => deserialize_typed::<EmbeddedConfig>(kind, json),
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
#[cfg(test)]
pub fn resolve_config_value(
    kind: ExecutorKind,
    json: &Value,
    overrides: &ExecutionOverrides,
) -> Result<Value, ExecutorError> {
    let mut merged = json.clone();
    merge_overrides(&mut merged, overrides)?;
    match kind {
        ExecutorKind::Embedded => normalize_typed::<EmbeddedConfig>(kind, &merged),
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

#[cfg(test)]
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

/// Merge generic per-execution overrides and normalize one adapter-owned
/// configuration type. Concrete adapters call this with their own type so
/// routing does not choose a provider schema.
pub fn normalize_harness_config<T>(
    kind: ExecutorKind,
    json: &Value,
    overrides: &ExecutionOverrides,
) -> Result<Value, ExecutorError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let mut merged = json.clone();
    merge_overrides(&mut merged, overrides)?;
    normalize_typed::<T>(kind, &merged)
}

/// Authored agent-config key holding the ordered fallback candidates.
/// Extracted before config normalization (which drops unknown fields).
pub const FALLBACKS_CONFIG_KEY: &str = "fallbacks";

/// Snapshot key carrying the resolved route.
pub const ROUTING_SNAPSHOT_KEY: &str = "routing";

/// The only routing policy currently defined.
pub const ROUTING_POLICY_ORDERED_FALLBACK_V1: &str = "ordered_fallback_v1";

/// Config fields that bind a session to a prior run. Excluded from candidate
/// identity so an injected resume id does not change which candidate a config
/// belongs to.
const SESSION_SCOPED_CONFIG_KEYS: &[&str] = &[
    "resume_session_id",
    "resume_thread_id",
    "resume_thread_in_place",
    "resume_fallback_prompt",
];

/// One executor candidate in an ordered fallback route.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorCandidate {
    pub executor_type: ExecutorKind,
    pub config: Value,
}

/// Outcome of one candidate attempt, persisted for route provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteAttemptOutcome {
    UsageExhausted,
    Unavailable,
    SkippedCooldown,
    Failed,
    Cancelled,
    Completed,
}

impl RouteAttemptOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UsageExhausted => "usage_exhausted",
            Self::Unavailable => "unavailable",
            Self::SkippedCooldown => "skipped_cooldown",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteAttempt {
    pub candidate_key: String,
    pub outcome: RouteAttemptOutcome,
}

/// First-class route carried on the executor config snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorRouting {
    pub policy: String,
    pub candidates: Vec<ExecutorCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_candidate_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<RouteAttempt>,
}

/// Stable identity of a candidate: executor kind + human-readable
/// discriminators + a hash over the session-stripped config. Keys ordering,
/// sticky selection, and session compatibility.
pub fn candidate_key(kind: &ExecutorKind, config: &Value) -> String {
    let mut discriminators = vec![kind.to_string()];
    for field in ["profile", "provider", "model"] {
        if let Some(value) = config.get(field).and_then(Value::as_str) {
            discriminators.push(format!("{field}={value}"));
        }
    }
    format!(
        "{}#{:08x}",
        discriminators.join(":"),
        stable_config_hash(config)
    )
}

/// Identity of the quota pool a candidate consumes inside one executor host.
/// Candidates sharing an account key share cooldowns. For Smith the pool is
/// the provider (Smith rotates that provider's credentials natively); for
/// Codex it is the lexical credential context; other executors have one
/// machine-level account. An executable override is deliberately not an
/// account identity: two wrappers may launch the same account, and the
/// wrapper may change without changing the credentials behind it. Absolute
/// CODEX_HOME values are normalized lexically without filesystem access;
/// relative and `~` values remain opaque because their meaning belongs to the
/// execution host.
///
/// This function is intentionally host-neutral. Callers that persist usage
/// observations must use [`account_key_for_context`] so host-local credentials
/// on different daemons cannot be merged accidentally.
pub fn account_key(kind: &ExecutorKind, config: &Value) -> String {
    let discriminator = match kind {
        ExecutorKind::Smith => {
            nonempty_str(config.get("provider")).or_else(|| nonempty_str(config.get("profile")))
        }
        ExecutorKind::Codex => {
            let home = nonempty_str(config.get("env").and_then(|env| env.get("CODEX_HOME")))
                .map(|home| format!("home={}", normalize_account_path(Path::new(&home))));
            home.or_else(|| {
                nonempty_str(config.get("profile")).map(|profile| format!("profile={profile}"))
            })
        }
        _ => None,
    };
    match discriminator {
        Some(value) => format!("{kind}:{value}"),
        None => kind.to_string(),
    }
}

/// Reject route candidates that would make one Agent Execution impersonate a
/// different harness or configured native account. Generic command override
/// channels are opaque: adapters may use them to select accounts, config
/// directories, wrappers, or native modes, so same-Agent candidates must keep
/// them identical. Explicit run settings such as model, effort, sandbox, and
/// approval remain routable when the harness and account key stay the same.
/// Agent-level profile and credential references are immutable for the whole
/// route and are not route inputs.
pub fn validate_same_agent_candidate(
    primary_kind: &ExecutorKind,
    primary_config: &Value,
    candidate_kind: &ExecutorKind,
    candidate_config: &Value,
) -> Result<(), ExecutorError> {
    if primary_kind != candidate_kind {
        return Err(ExecutorError::Other(format!(
            "fallback candidate changes Agent harness identity from {primary_kind} to {candidate_kind}; select or reassign a separate Agent"
        )));
    }
    let primary_account = account_key(primary_kind, primary_config);
    let candidate_account = account_key(candidate_kind, candidate_config);
    if primary_account != candidate_account {
        return Err(ExecutorError::Other(format!(
            "fallback candidate changes Agent native account identity from {primary_account} to {candidate_account}; select or reassign a separate Agent"
        )));
    }
    for field in ["base_command_override", "additional_params", "env"] {
        let primary_value = primary_config.get(field).filter(|value| !value.is_null());
        let candidate_value = candidate_config.get(field).filter(|value| !value.is_null());
        if primary_value != candidate_value {
            return Err(ExecutorError::Other(format!(
                "fallback candidate changes opaque command configuration field {field}; same-Agent fallback cannot prove that this preserves native harness identity"
            )));
        }
    }
    Ok(())
}

/// Build the account key used for a usage observation when credentials are
/// host-local. `host_identity` is supplied by the execution environment (for
/// example a daemon id or the local server marker), never resolved through the
/// server filesystem.
pub fn account_key_for_context(
    kind: &ExecutorKind,
    config: &Value,
    host_identity: &str,
    credential_ref: Option<&str>,
) -> String {
    if let Some(credential_ref) = credential_ref
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!("{kind}:credential={credential_ref}");
    }
    let base = account_key(kind, config);
    let host = host_identity.trim();
    if host.is_empty() {
        return base;
    }
    match base.split_once(':') {
        Some((kind_name, discriminator)) => format!("{kind_name}@{host}:{discriminator}"),
        None => format!("{base}@{host}"),
    }
}

fn nonempty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_account_path(path: &Path) -> String {
    // CODEX_HOME is interpreted by the process that actually launches Codex.
    // The server cannot safely expand a relative or `~` path for a remote
    // daemon, so preserve non-absolute values as opaque lexical context.
    if !path.is_absolute() {
        return format!("relative:{}", path.to_string_lossy());
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized.to_string_lossy().into_owned()
}

/// FNV-1a over a canonical (key-sorted, session-stripped) rendering of the
/// config. Deliberately not `DefaultHasher`, whose output may change across
/// releases — these keys persist in execution snapshots.
fn stable_config_hash(config: &Value) -> u32 {
    fn canonicalize(value: &Value, out: &mut String, strip_session_keys: bool) {
        match value {
            Value::Object(map) => {
                out.push('{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for key in keys {
                    if strip_session_keys && SESSION_SCOPED_CONFIG_KEYS.contains(&key.as_str()) {
                        continue;
                    }
                    out.push_str(key);
                    out.push(':');
                    canonicalize(&map[key], out, false);
                    out.push(',');
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for item in items {
                    canonicalize(item, out, false);
                    out.push(',');
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }

    let mut canonical = String::new();
    canonicalize(config, &mut canonical, true);
    let mut hash: u32 = 0x811c9dc5;
    for byte in canonical.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Build and validate an ordered-fallback route from a normalized primary
/// candidate plus the raw authored `fallbacks` entries.
#[cfg(test)]
pub fn build_ordered_fallback_routing(
    primary_kind: ExecutorKind,
    primary_config: Value,
    fallbacks: &[Value],
) -> Result<ExecutorRouting, ExecutorError> {
    if primary_kind == ExecutorKind::Embedded {
        return Err(ExecutorError::Other(
            "embedded executor is hosted by Forge and cannot use CLI fallback routing".to_owned(),
        ));
    }
    let mut candidates = vec![ExecutorCandidate {
        executor_type: primary_kind,
        config: primary_config,
    }];
    for (index, entry) in fallbacks.iter().enumerate() {
        let object = entry.as_object().ok_or_else(|| {
            ExecutorError::Other(format!("fallbacks[{index}] must be a JSON object"))
        })?;
        let executor_type = object
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ExecutorError::Other(format!("fallbacks[{index}] is missing executor_type"))
            })?;
        let kind = executor_type.parse::<ExecutorKind>().map_err(|_| {
            ExecutorError::Other(format!(
                "fallbacks[{index}] names unknown executor type: {executor_type}"
            ))
        })?;
        if kind == ExecutorKind::Embedded {
            return Err(ExecutorError::Other(
                "embedded executor is hosted by Forge and cannot be a CLI fallback candidate"
                    .to_owned(),
            ));
        }
        let raw_config = object
            .get("config")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        if !raw_config.is_object() {
            return Err(ExecutorError::Other(format!(
                "fallbacks[{index}] config must be a JSON object"
            )));
        }
        let normalized =
            resolve_config_value(kind.clone(), &raw_config, &ExecutionOverrides::default())?;
        candidates.push(ExecutorCandidate {
            executor_type: kind,
            config: normalized,
        });
    }

    validate_ordered_fallback_routing(candidates)
}

/// Validate and snapshot candidates that were already normalized by their
/// registered HarnessAdapters.
pub fn validate_ordered_fallback_routing(
    candidates: Vec<ExecutorCandidate>,
) -> Result<ExecutorRouting, ExecutorError> {
    if candidates.is_empty() {
        return Err(ExecutorError::Other(
            "fallback routing requires a primary candidate".to_owned(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let primary = &candidates[0];
    for candidate in &candidates {
        validate_same_agent_candidate(
            &primary.executor_type,
            &primary.config,
            &candidate.executor_type,
            &candidate.config,
        )?;
        let key = candidate_key(&candidate.executor_type, &candidate.config);
        if !seen.insert(key.clone()) {
            return Err(ExecutorError::Other(format!(
                "duplicate executor candidate: {key}"
            )));
        }
    }

    Ok(ExecutorRouting {
        policy: ROUTING_POLICY_ORDERED_FALLBACK_V1.to_owned(),
        candidates,
        selected_candidate_key: None,
        attempts: Vec::new(),
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
    fn embedded_config_round_trips_without_cli_fields() {
        let value = serde_json::json!({
            "base_url": "https://api.example.test/v1",
            "context_tokens": 32_000,
            "max_input_tokens": 24_000,
            "max_output_tokens": 4_000,
            "runtime_revision": "agent-runtime-rev",
            "model": "model-from-agent",
            "credential_ref": "opaque-handle",
            "permission_policy": "scoped_proposals",
        });
        let resolved = resolve_config_value(
            ExecutorKind::Embedded,
            &value,
            &ExecutionOverrides::default(),
        )
        .expect("embedded config resolves");

        assert_eq!(resolved["base_url"], "https://api.example.test/v1");
        assert_eq!(resolved["context_tokens"], 32_000);
        assert_eq!(resolved["runtime_revision"], "agent-runtime-rev");
        assert!(resolved.get("model").is_none());
        assert!(resolved.get("credential_ref").is_none());
        assert!(resolved.get("permission_policy").is_none());
    }

    #[test]
    fn embedded_executor_is_not_admitted_to_cli_fallback_routes() {
        let error = build_ordered_fallback_routing(
            ExecutorKind::Shell,
            serde_json::json!({}),
            &[serde_json::json!({
                "executor_type": "embedded",
                "config": {}
            })],
        )
        .expect_err("embedded fallback should be rejected");
        assert!(error.to_string().contains("hosted by Forge"));

        let error =
            build_ordered_fallback_routing(ExecutorKind::Embedded, serde_json::json!({}), &[])
                .expect_err("embedded primary should not enter CLI routing");
        assert!(error.to_string().contains("hosted by Forge"));
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
    fn routing_normalizes_each_candidate_and_preserves_order() {
        let routing = build_ordered_fallback_routing(
            ExecutorKind::Smith,
            serde_json::json!({"profile": "acct-1", "model": "model-a"}),
            &[
                serde_json::json!({"executor_type": "smith", "config": {"profile": "acct-1", "model": "model-b", "unknown_field": true}}),
            ],
        )
        .expect("routing builds");

        assert_eq!(routing.policy, ROUTING_POLICY_ORDERED_FALLBACK_V1);
        assert_eq!(routing.candidates.len(), 2);
        assert_eq!(routing.candidates[0].executor_type, ExecutorKind::Smith);
        assert_eq!(routing.candidates[1].config["profile"], "acct-1");
        assert_eq!(routing.candidates[1].config["model"], "model-b");
        assert!(routing.candidates[1].config.get("unknown_field").is_none());
    }

    #[test]
    fn same_agent_routing_rejects_harness_or_identity_bearing_account_changes() {
        let codex_a = serde_json::json!({"env":{"CODEX_HOME":"/accounts/a"}});
        let codex_b = serde_json::json!({"env":{"CODEX_HOME":"/accounts/b"}});
        assert!(validate_same_agent_candidate(
            &ExecutorKind::Codex,
            &codex_a,
            &ExecutorKind::Codex,
            &codex_b,
        )
        .is_err());
        assert!(validate_same_agent_candidate(
            &ExecutorKind::Codex,
            &serde_json::json!({}),
            &ExecutorKind::Cursor,
            &serde_json::json!({}),
        )
        .is_err());
    }

    #[test]
    fn claude_fallback_cannot_change_config_dir_identity() {
        let result = build_ordered_fallback_routing(
            ExecutorKind::ClaudeCode,
            serde_json::json!({"env":{"CLAUDE_CONFIG_DIR":"/accounts/a"}}),
            &[serde_json::json!({
                "executor_type":"claude_code",
                "config":{"env":{"CLAUDE_CONFIG_DIR":"/accounts/b"}}
            })],
        );

        let error = result.expect_err("a different Claude config directory changes identity");
        assert!(error.to_string().contains("env"));
    }

    #[test]
    fn same_agent_fallback_cannot_change_command_environment() {
        let primary = serde_json::json!({"env":{"ANTHROPIC_API_KEY":"account-a"}});
        let candidate = serde_json::json!({"env":{"ANTHROPIC_API_KEY":"account-b"}});

        let error = validate_same_agent_candidate(
            &ExecutorKind::ClaudeCode,
            &primary,
            &ExecutorKind::ClaudeCode,
            &candidate,
        )
        .expect_err("environment overrides can select another native account");
        assert!(error.to_string().contains("env"));
    }

    #[test]
    fn same_agent_fallback_cannot_change_opaque_command_arguments() {
        let primary = serde_json::json!({"additional_params":["--account", "account-a"]});
        let candidate = serde_json::json!({"additional_params":["--account", "account-b"]});

        let error = validate_same_agent_candidate(
            &ExecutorKind::Opencode,
            &primary,
            &ExecutorKind::Opencode,
            &candidate,
        )
        .expect_err("opaque CLI arguments may select another native identity");
        assert!(error.to_string().contains("additional_params"));
    }

    #[test]
    fn same_agent_fallback_cannot_change_wrapper_command() {
        let primary = serde_json::json!({"base_command_override":"/accounts/a/claude"});
        let candidate = serde_json::json!({"base_command_override":"/accounts/b/claude"});

        let error = validate_same_agent_candidate(
            &ExecutorKind::ClaudeCode,
            &primary,
            &ExecutorKind::ClaudeCode,
            &candidate,
        )
        .expect_err("a wrapper override is opaque identity-bearing configuration");
        assert!(error.to_string().contains("base_command_override"));
    }

    #[test]
    fn same_agent_fallback_allows_model_effort_sandbox_variation() {
        let primary = serde_json::json!({
            "profile": "agent-profile",
            "model": "model-a",
            "effort": "low",
            "sandbox": "workspace-write"
        });
        let candidate = serde_json::json!({
            "profile": "agent-profile",
            "model": "model-b",
            "effort": "high",
            "sandbox": "read-only"
        });

        validate_same_agent_candidate(
            &ExecutorKind::ClaudeCode,
            &primary,
            &ExecutorKind::ClaudeCode,
            &candidate,
        )
        .expect("run settings may vary for the same harness/account identity");
    }

    #[test]
    fn routing_rejects_unknown_executor_type_and_non_object_config() {
        let unknown = build_ordered_fallback_routing(
            ExecutorKind::Smith,
            serde_json::json!({}),
            &[serde_json::json!({"executor_type": "warp", "config": {}})],
        )
        .expect_err("unknown type rejects");
        assert!(unknown.to_string().contains("unknown executor type"));

        let non_object = build_ordered_fallback_routing(
            ExecutorKind::Smith,
            serde_json::json!({}),
            &[serde_json::json!({"executor_type": "smith", "config": "profile"})],
        )
        .expect_err("non-object config rejects");
        assert!(non_object.to_string().contains("must be a JSON object"));
    }

    #[test]
    fn routing_rejects_duplicate_candidates() {
        let error = build_ordered_fallback_routing(
            ExecutorKind::Smith,
            resolve_config_value(
                ExecutorKind::Smith,
                &serde_json::json!({"profile": "acct-1"}),
                &ExecutionOverrides::default(),
            )
            .expect("primary resolves"),
            &[serde_json::json!({"executor_type": "smith", "config": {"profile": "acct-1"}})],
        )
        .expect_err("duplicate rejects");
        assert!(error.to_string().contains("duplicate executor candidate"));
    }

    #[test]
    fn candidate_key_ignores_session_scoped_fields() {
        let base = resolve_config_value(
            ExecutorKind::Smith,
            &serde_json::json!({"profile": "acct-1"}),
            &ExecutionOverrides::default(),
        )
        .expect("config resolves");
        let mut with_session = base.clone();
        with_session["resume_session_id"] = serde_json::json!("session-9");

        assert_eq!(
            candidate_key(&ExecutorKind::Smith, &base),
            candidate_key(&ExecutorKind::Smith, &with_session)
        );
        assert!(candidate_key(&ExecutorKind::Smith, &base).starts_with("smith:profile=acct-1#"));
    }

    #[test]
    fn account_key_pools_by_provider_for_smith() {
        let glm_sonnet = serde_json::json!({"provider": "zai", "model": "glm-5"});
        let glm_flash = serde_json::json!({"provider": "zai", "model": "glm-5-flash"});
        let other = serde_json::json!({"provider": "google", "model": "gemini-3.6-flash"});

        assert_eq!(
            account_key(&ExecutorKind::Smith, &glm_sonnet),
            account_key(&ExecutorKind::Smith, &glm_flash)
        );
        assert_ne!(
            account_key(&ExecutorKind::Smith, &glm_sonnet),
            account_key(&ExecutorKind::Smith, &other)
        );
        assert_eq!(
            account_key(
                &ExecutorKind::ClaudeCode,
                &serde_json::json!({"model": "opus"})
            ),
            "claude_code"
        );
    }

    #[test]
    fn account_key_lexically_normalizes_absolute_codex_home_not_wrapper() {
        let with_home = serde_json::json!({
            "env": { "CODEX_HOME": "/tmp/forge-codex-account/../forge-codex-account" },
            "base_command_override": "/home/user/bin/codex-work"
        });
        let equivalent_home = serde_json::json!({
            "env": { "CODEX_HOME": "/tmp/forge-codex-account" },
            "base_command_override": "/home/user/bin/another-codex-wrapper"
        });

        assert_eq!(
            account_key(&ExecutorKind::Codex, &with_home),
            "codex:home=/tmp/forge-codex-account"
        );
        assert_eq!(
            account_key(&ExecutorKind::Codex, &with_home),
            account_key(&ExecutorKind::Codex, &equivalent_home)
        );
    }

    #[test]
    fn account_key_separates_different_homes_with_the_same_wrapper() {
        let first = serde_json::json!({
            "env": { "CODEX_HOME": "/tmp/forge-codex-first" },
            "base_command_override": "/home/user/bin/codex-work"
        });
        let second = serde_json::json!({
            "env": { "CODEX_HOME": "/tmp/forge-codex-second" },
            "base_command_override": "/home/user/bin/codex-work"
        });

        assert_ne!(
            account_key_for_context(&ExecutorKind::Codex, &first, "daemon-a", None),
            account_key_for_context(&ExecutorKind::Codex, &second, "daemon-a", None)
        );
    }

    #[test]
    fn account_key_separates_host_local_credentials() {
        let config = serde_json::json!({
            "env": { "CODEX_HOME": "/home/alex/.codex/../.codex" },
            "base_command_override": "/home/alex/bin/codex-wrapper"
        });
        assert_eq!(
            account_key_for_context(&ExecutorKind::Codex, &config, "daemon-a", None),
            "codex@daemon-a:home=/home/alex/.codex"
        );
        assert_ne!(
            account_key_for_context(&ExecutorKind::Codex, &config, "daemon-a", None),
            account_key_for_context(&ExecutorKind::Codex, &config, "daemon-b", None)
        );
    }

    #[test]
    fn account_key_does_not_resolve_remote_paths_through_server_filesystem() {
        let config = serde_json::json!({
            "env": { "CODEX_HOME": "~/.codex/../.codex" }
        });
        let key = account_key_for_context(&ExecutorKind::Codex, &config, "daemon-a", None);
        assert!(key.contains("home=relative:~/.codex/../.codex"));
        assert!(!key.contains(&std::env::var("HOME").unwrap_or_default()));
    }

    #[test]
    fn account_key_preserves_relative_codex_home_semantics() {
        let parent = serde_json::json!({
            "env": { "CODEX_HOME": "../account" }
        });
        let child = serde_json::json!({
            "env": { "CODEX_HOME": "account" }
        });
        let traversal = serde_json::json!({
            "env": { "CODEX_HOME": "foo/../../bar" }
        });
        let collapsed = serde_json::json!({
            "env": { "CODEX_HOME": "bar" }
        });

        assert_ne!(
            account_key(&ExecutorKind::Codex, &parent),
            account_key(&ExecutorKind::Codex, &child)
        );
        assert_ne!(
            account_key(&ExecutorKind::Codex, &traversal),
            account_key(&ExecutorKind::Codex, &collapsed)
        );
    }

    #[test]
    fn explicit_credential_reference_can_prove_cross_host_sharing() {
        let config = serde_json::json!({
            "env": { "CODEX_HOME": "/different/on/every/host" }
        });
        assert_eq!(
            account_key_for_context(
                &ExecutorKind::Codex,
                &config,
                "daemon-a",
                Some("credential-1")
            ),
            account_key_for_context(
                &ExecutorKind::Codex,
                &config,
                "daemon-b",
                Some("credential-1")
            )
        );
        assert_ne!(
            account_key_for_context(
                &ExecutorKind::Codex,
                &config,
                "daemon-a",
                Some("credential-1")
            ),
            account_key_for_context(
                &ExecutorKind::Codex,
                &config,
                "daemon-a",
                Some("credential-2")
            )
        );
    }

    #[test]
    fn account_key_falls_back_to_profile_without_codex_home() {
        assert_eq!(
            account_key(
                &ExecutorKind::Codex,
                &serde_json::json!({ "profile": "work" })
            ),
            "codex:profile=work"
        );
        assert_eq!(
            account_key(&ExecutorKind::Codex, &serde_json::json!({})),
            "codex"
        );
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
