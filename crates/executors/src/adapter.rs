use crate::{
    config::resolve_config_value, ExecutionContext, ExecutionResult, ExecutorError, TaskExecutor,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of known CLI executor families.
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorKind {
    Shell,
    Codex,
    ClaudeCode,
    Cursor,
    Opencode,
    Gemini,
    Null,
}

impl std::fmt::Display for ExecutorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell => write!(f, "shell"),
            Self::Codex => write!(f, "codex"),
            Self::ClaudeCode => write!(f, "claude_code"),
            Self::Cursor => write!(f, "cursor"),
            Self::Opencode => write!(f, "opencode"),
            Self::Gemini => write!(f, "gemini"),
            Self::Null => write!(f, "null"),
        }
    }
}

impl std::str::FromStr for ExecutorKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "shell" => Ok(Self::Shell),
            "codex" => Ok(Self::Codex),
            "claude_code" => Ok(Self::ClaudeCode),
            "cursor" => Ok(Self::Cursor),
            "opencode" => Ok(Self::Opencode),
            "gemini" => Ok(Self::Gemini),
            "null" => Ok(Self::Null),
            other => Err(format!("unknown executor kind: {other}")),
        }
    }
}

/// Availability state reported by an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStatus {
    Authenticated,
    Installed,
    NotFound,
}

/// Availability info returned by an adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailabilityInfo {
    pub status: AvailabilityStatus,
    pub authenticated_at: Option<String>,
    pub config_path: Option<String>,
}

/// Context for adapter discovery.
#[derive(Debug, Clone)]
pub struct DiscoverContext {
    pub project_path: Option<String>,
}

/// Options discovered by an adapter.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveredOptions {
    pub models: Vec<String>,
    pub permission_policies: Vec<String>,
    pub cli_specific: serde_json::Value,
}

/// Per-execution overrides applied on top of profile config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionOverrides {
    pub model_id: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
}

/// Typed adapter trait for CLI-specific executor implementations.
#[async_trait]
pub trait CodingExecutorAdapter: Send + Sync {
    fn kind(&self) -> ExecutorKind;

    fn check_availability(&self) -> AvailabilityInfo;

    async fn discover_options(
        &self,
        ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError>;

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError>;

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError>;
}

/// Registry mapping ExecutorKind to adapter implementations.
pub struct AdapterRegistry {
    adapters: HashMap<ExecutorKind, Box<dyn CodingExecutorAdapter>>,
}

impl AdapterRegistry {
    pub fn new() -> Self {
        Self {
            adapters: HashMap::new(),
        }
    }

    pub fn register(&mut self, adapter: Box<dyn CodingExecutorAdapter>) {
        let kind = adapter.kind();
        self.adapters.insert(kind, adapter);
    }

    pub fn get(&self, kind: &ExecutorKind) -> Option<&dyn CodingExecutorAdapter> {
        self.adapters.get(kind).map(|a| a.as_ref())
    }

    pub fn kinds(&self) -> Vec<ExecutorKind> {
        self.adapters.keys().cloned().collect()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Supervisor-facing executor that dispatches to a typed CLI adapter.
pub struct AdapterExecutor {
    registry: Arc<AdapterRegistry>,
}

impl AdapterExecutor {
    pub fn new(registry: Arc<AdapterRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl TaskExecutor for AdapterExecutor {
    async fn execute(&self, mut ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let (kind, config) = resolve_context_config(&ctx.agent_config)?;
        let adapter = self.registry.get(&kind).ok_or_else(|| {
            ExecutorError::Other(format!("No adapter registered for executor type: {kind}"))
        })?;

        ctx.agent_config = config;
        adapter.execute(ctx).await
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        for kind in self.registry.kinds() {
            if let Some(adapter) = self.registry.get(&kind) {
                adapter.cancel(execution_id).await?;
            }
        }
        Ok(())
    }
}

fn resolve_context_config(
    agent_config: &serde_json::Value,
) -> Result<(ExecutorKind, serde_json::Value), ExecutorError> {
    let object = agent_config.as_object().ok_or_else(|| {
        ExecutorError::Other("executor config snapshot must be a JSON object".to_owned())
    })?;
    let executor_type = object
        .get("executor_type")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            ExecutorError::Other("executor config snapshot missing executor_type".to_owned())
        })?;
    let kind = executor_type.parse::<ExecutorKind>().map_err(|_| {
        ExecutorError::Other(format!(
            "No adapter registered for executor type: {executor_type}"
        ))
    })?;
    let config = object.get("config").unwrap_or(agent_config);
    let config = resolve_config_value(kind.clone(), config, &ExecutionOverrides::default())?;
    Ok((kind, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_shell_command_plan, ExecutionOutcome, ExecutionResult, ShellConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CapturingAdapter;

    #[async_trait]
    impl CodingExecutorAdapter for CapturingAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Codex
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            assert_eq!(ctx.agent_config["model"], "gpt-5-codex");
            assert_eq!(ctx.agent_config["model_reasoning_effort"], "high");
            assert_eq!(ctx.agent_config["permission_policy"], "auto");
            assert_eq!(ctx.agent_config["sandbox"], "danger-full-access");
            assert!(ctx.agent_config.get("effort").is_none());
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
            })
        }

        async fn cancel(&self, _execution_id: &str) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    struct CancelTrackingAdapter {
        cancel_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl CodingExecutorAdapter for CancelTrackingAdapter {
        fn kind(&self) -> ExecutorKind {
            ExecutorKind::Shell
        }

        fn check_availability(&self) -> AvailabilityInfo {
            AvailabilityInfo {
                status: AvailabilityStatus::Authenticated,
                authenticated_at: None,
                config_path: None,
            }
        }

        async fn discover_options(
            &self,
            _ctx: DiscoverContext,
        ) -> Result<DiscoveredOptions, ExecutorError> {
            Ok(DiscoveredOptions::default())
        }

        async fn execute(&self, _ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
            Ok(ExecutionResult {
                status: ExecutionOutcome::Completed,
                after_sha: None,
                agent_session_id: None,
                summary: None,
                error: None,
                usage: None,
            })
        }

        async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
            assert_eq!(execution_id, "execution-to-cancel");
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn adapter_executor_dispatches_using_snapshot_config() {
        let mut registry = AdapterRegistry::new();
        registry.register(Box::new(CapturingAdapter));
        let executor = AdapterExecutor::new(Arc::new(registry));

        let result = executor
            .execute(ExecutionContext {
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                worktree_path: ".".to_owned(),
                description: "do it".to_owned(),
                agent_config: serde_json::json!({
                    "executor_type": "codex",
                    "config": {
                        "model": "gpt-5-codex",
                        "model_reasoning_effort": "high",
                        "effort": "high",
                        "permission_policy": "auto",
                        "sandbox": "danger-full-access"
                    }
                }),
                logs_path: "logs.jsonl".to_owned(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .expect("dispatch succeeds");

        assert_eq!(result.status, ExecutionOutcome::Completed);
    }

    #[test]
    fn resolve_context_config_builds_shell_command_plan() {
        let snapshot = serde_json::json!({
            "executor_type": "shell",
            "config": {
                "command": "bash",
                "args": ["-lc", "make test"],
                "permission_policy": "supervised",
                "additional_params": ["--verbose"],
                "env": { "CI": "1" }
            }
        });

        let (kind, config) = resolve_context_config(&snapshot).expect("snapshot resolves");
        assert_eq!(kind, ExecutorKind::Shell);

        let shell_config: ShellConfig = serde_json::from_value(config).expect("shell config");
        let plan =
            build_shell_command_plan("ignored", "/tmp/worktree", Some(2), Some(&shell_config));

        assert_eq!(plan.program, "bash");
        assert_eq!(plan.args, vec!["-lc", "make test", "--verbose"]);
        assert_eq!(plan.cwd.to_string_lossy(), "/tmp/worktree");
        assert_eq!(plan.env_set.get("CI").map(String::as_str), Some("1"));
        assert_eq!(
            plan.env_set.get("FORGE_MAX_TURNS").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn resolve_context_config_resolves_codex_snapshot() {
        let snapshot = serde_json::json!({
            "executor_type": "codex",
            "config": {
                "model": "gpt-5-codex",
                "sandbox": "danger-full-access",
                "model_reasoning_effort": "high",
                "permission_policy": "auto",
                "additional_params": ["--verbose"],
                "env": { "CUSTOM": "1" }
            }
        });

        let (kind, config) = resolve_context_config(&snapshot).expect("snapshot resolves");
        assert_eq!(kind, ExecutorKind::Codex);

        let codex_config: crate::CodexConfig =
            serde_json::from_value(config).expect("codex config");
        assert_eq!(codex_config.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(
            codex_config.command_overrides.additional_params.as_deref(),
            Some(["--verbose".to_owned()].as_slice())
        );
    }

    #[tokio::test]
    async fn adapter_executor_cancel_passthrough_reaches_registered_adapter() {
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = AdapterRegistry::new();
        registry.register(Box::new(CancelTrackingAdapter {
            cancel_calls: Arc::clone(&cancel_calls),
        }));
        let executor = AdapterExecutor::new(Arc::new(registry));

        executor
            .cancel("execution-to-cancel")
            .await
            .expect("cancel succeeds");

        assert_eq!(cancel_calls.load(Ordering::SeqCst), 1);
    }
}
