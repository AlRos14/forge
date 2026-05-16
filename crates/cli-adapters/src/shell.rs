use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, CodingExecutorAdapter, DiscoverContext,
    DiscoveredOptions, ExecutionContext, ExecutionResult, ExecutorError, ExecutorKind,
    ShellExecutor, TaskExecutor,
};

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
impl CodingExecutorAdapter for ShellAdapter {
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

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
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

    #[tokio::test]
    async fn shell_adapter_executes_simple_command_and_writes_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("shell-adapter.jsonl");
        let adapter = ShellAdapter::new();

        let result = adapter
            .execute(ExecutionContext {
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
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
