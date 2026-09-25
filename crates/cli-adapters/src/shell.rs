use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, DiscoverContext, DiscoveredOptions, ExecutionContext,
    ExecutionResult, ExecutorError, ExecutorKind, HarnessAdapter, ShellExecutor, TaskExecutor,
};

const REVIEW_RESULT_COMMAND: &str = r#"echo 'FORGE_RESULT: {"schema_version":1,"kind":"review","verdict":"pass","summary":"clear","findings":[],"questions":[]}'"#;

fn command_for_context(ctx: &ExecutionContext) -> &str {
    if ctx.role == "reviewer" {
        REVIEW_RESULT_COMMAND
    } else {
        &ctx.description
    }
}

/// Shell adapter: wraps the existing ShellExecutor.
pub struct ShellAdapter {
    inner: ShellExecutor,
}

impl ShellAdapter {
    pub fn new() -> Self {
        Self {
            inner: ShellExecutor::default(),
        }
    }
}

impl Default for ShellAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HarnessAdapter for ShellAdapter {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Shell
    }

    fn normalize_config(
        &self,
        config: &serde_json::Value,
        overrides: &executors::ExecutionOverrides,
    ) -> Result<serde_json::Value, ExecutorError> {
        executors::normalize_harness_config::<executors::ShellConfig>(
            self.kind(),
            config,
            overrides,
        )
    }

    fn capabilities(&self, _config: &serde_json::Value) -> executors::HarnessCapabilities {
        use executors::CapabilitySupport as S;
        crate::harness_capabilities(
            S::Unsupported,
            S::Emulated,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
            S::Unsupported,
        )
    }

    fn executable_name(&self) -> Option<String> {
        Some("sh".to_owned())
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

    async fn execute(&self, mut ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        if ctx.role == "reviewer" {
            ctx.description = command_for_context(&ctx).to_owned();
        }
        self.inner.execute(ctx).await
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        self.inner.cancel(execution_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use executors::{ExecutionOutcome, LogKind, LogReader};

    #[test]
    fn reviewer_compatibility_command_is_owned_by_shell_adapter() {
        let mut ctx = crate::test_execution_context(
            executors::HarnessInvocation::Start,
            serde_json::json!({}),
        );
        ctx.role = "reviewer".to_owned();
        assert_eq!(command_for_context(&ctx), REVIEW_RESULT_COMMAND);
        ctx.role = "coder".to_owned();
        ctx.description = "printf normal-task".to_owned();
        assert_eq!(command_for_context(&ctx), "printf normal-task");
    }

    #[tokio::test]
    async fn shell_adapter_executes_simple_command_and_writes_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("shell-adapter.jsonl");
        let adapter = ShellAdapter::new();

        let result = adapter
            .execute(ExecutionContext {
                invocation: executors::HarnessInvocation::Start,
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                role: "coder".to_owned(),
                worktree_path: dir.path().to_string_lossy().to_string(),
                description: "printf shell-adapter-ok".to_owned(),
                agent_config: serde_json::json!({}),
                logs_path: log_path.to_string_lossy().to_string(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .expect("shell adapter executes");

        assert_eq!(result.status, ExecutionOutcome::Completed);
        let logs = LogReader::read(&log_path, 0, 100).await.unwrap();
        assert!(logs.entries.iter().any(|entry| {
            entry.kind == LogKind::Stdout
                && entry.payload.get("line").and_then(|line| line.as_str())
                    == Some("shell-adapter-ok")
        }));
    }
}
