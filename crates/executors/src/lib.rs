#![forbid(unsafe_code)]

pub mod adapter;
pub mod config;
pub mod effective_policy;
pub mod log_reader;
pub mod log_schema;
pub mod log_writer;
pub mod shell;

pub use adapter::{
    AdapterExecutor, AdapterRegistry, AvailabilityInfo, AvailabilityStatus, CodingExecutorAdapter,
    DiscoverContext, DiscoveredOptions, ExecutionOverrides, ExecutorKind,
};
pub use config::{
    deserialize_config, merge_overrides, resolve_config_value, ClaudeCodeConfig, CodexConfig,
    CommandOverrides, GeminiConfig, NullConfig, OpencodeConfig, PermissionPolicy, ShellConfig,
};
pub use log_reader::{LogReadResult, LogReader};
pub use log_schema::{LogEntry, LogKind, LogStream};
pub use log_writer::LogWriter;
pub use shell::ShellExecutor;

use async_trait::async_trait;

/// Context passed to an executor when running a task.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub task_id: String,
    pub execution_id: String,
    pub worktree_path: String,
    pub description: String,
    pub agent_config: serde_json::Value,
    pub logs_path: String,
    pub heartbeat_interval_seconds: u64,
    pub max_turns: Option<u32>,
    pub log_sender: Option<tokio::sync::mpsc::UnboundedSender<LogEntry>>,
}

/// Accumulated token usage from an executor run.
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
    pub model: Option<String>,
}

/// Result from an executor run.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub status: ExecutionOutcome,
    pub after_sha: Option<String>,
    pub agent_session_id: Option<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionOutcome {
    Completed,
    Failed,
    Cancelled,
}

#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError>;
    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("executor error: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_write_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("test.jsonl");

        let mut writer = LogWriter::new(&log_path, "exec-1".to_string(), 1024 * 1024);

        for i in 0..100 {
            writer
                .write(
                    LogKind::Stdout,
                    LogStream::Main,
                    serde_json::json!({"line": format!("line {i}")}),
                )
                .await
                .unwrap();
        }

        assert_eq!(writer.sequence(), 100);

        // Read from_sequence=50, limit=10
        let result = LogReader::read(&log_path, 50, 10).await.unwrap();
        assert_eq!(result.entries.len(), 10);
        assert_eq!(result.entries[0].sequence, 50);
        assert_eq!(result.entries[9].sequence, 59);
        assert!(result.has_more);
        assert_eq!(result.next_sequence, Some(60));

        // Tail last 5
        let tail_result = LogReader::tail(&log_path, 5).await.unwrap();
        assert_eq!(tail_result.entries.len(), 5);
        assert_eq!(tail_result.entries[0].sequence, 95);
        assert_eq!(tail_result.entries[4].sequence, 99);
        assert!(tail_result.has_more);
        assert_eq!(tail_result.next_sequence, Some(100));

        writer
            .write(
                LogKind::SessionInfo,
                LogStream::Main,
                serde_json::json!({"method": "thread/started"}),
            )
            .await
            .unwrap();
        writer
            .write(
                LogKind::User,
                LogStream::Main,
                serde_json::json!({"text": "follow-up"}),
            )
            .await
            .unwrap();
        for i in 0..10 {
            writer
                .write(
                    LogKind::Stdout,
                    LogStream::Main,
                    serde_json::json!({"line": format!("follow-up line {i}")}),
                )
                .await
                .unwrap();
        }

        let turn_tail_result = LogReader::tail(&log_path, 5).await.unwrap();
        assert_eq!(turn_tail_result.entries[0].sequence, 101);
        assert!(turn_tail_result.has_more);
    }

    #[tokio::test]
    async fn log_read_empty_delta_preserves_requested_next_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("empty-delta.jsonl");

        let mut writer = LogWriter::new(&log_path, "exec-1".to_string(), 1024 * 1024);
        writer
            .write(
                LogKind::Stdout,
                LogStream::Main,
                serde_json::json!({"line": "hello"}),
            )
            .await
            .unwrap();

        let result = LogReader::read(&log_path, 1, 10).await.unwrap();
        assert!(result.entries.is_empty());
        assert!(!result.has_more);
        assert_eq!(result.next_sequence, Some(1));
    }

    #[tokio::test]
    async fn log_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("truncated.jsonl");

        // Very small max to trigger truncation quickly
        let mut writer = LogWriter::new(&log_path, "exec-2".to_string(), 500);

        for i in 0..100 {
            writer
                .write(
                    LogKind::Stdout,
                    LogStream::Main,
                    serde_json::json!({"line": format!("line {i}")}),
                )
                .await
                .unwrap();
        }

        assert!(writer.is_truncated());
        assert!(writer.sequence() < 100); // Should have stopped early

        // Last entry should be truncated
        let result = LogReader::tail(&log_path, 1).await.unwrap();
        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].truncated);
    }

    #[tokio::test]
    async fn log_writer_appends_after_existing_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("append.jsonl");

        let mut first = LogWriter::new(&log_path, "exec-1".to_string(), 1024 * 1024);
        first
            .write(
                LogKind::User,
                LogStream::Main,
                serde_json::json!({"text": "first turn"}),
            )
            .await
            .unwrap();
        first
            .write(
                LogKind::Assistant,
                LogStream::Main,
                serde_json::json!({"text": "first response"}),
            )
            .await
            .unwrap();

        let mut second = LogWriter::new(&log_path, "exec-1".to_string(), 1024 * 1024);
        assert_eq!(second.sequence(), 2);
        second
            .write(
                LogKind::User,
                LogStream::Main,
                serde_json::json!({"text": "follow up"}),
            )
            .await
            .unwrap();

        let result = LogReader::read(&log_path, 0, 10).await.unwrap();
        let sequences = result
            .entries
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![0, 1, 2]);
    }
}
