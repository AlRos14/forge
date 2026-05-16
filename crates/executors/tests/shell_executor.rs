use executors::{
    ExecutionContext, ExecutionOutcome, LogKind, LogReader, ShellExecutor, TaskExecutor,
};

#[tokio::test]
async fn shell_executor_runs_echo_and_writes_logs() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("shell.jsonl");

    let executor = ShellExecutor::default();
    let result = executor
        .execute(ExecutionContext {
            task_id: "task-1".to_string(),
            execution_id: "exec-1".to_string(),
            worktree_path: dir.path().to_string_lossy().to_string(),
            description: "echo hello world".to_string(),
            agent_config: serde_json::json!({}),
            logs_path: log_path.to_string_lossy().to_string(),
            heartbeat_interval_seconds: 1,
            max_turns: None,
            log_sender: None,
        })
        .await
        .unwrap();

    assert_eq!(result.status, ExecutionOutcome::Completed);

    let logs = LogReader::read(&log_path, 0, 100).await.unwrap();
    assert!(!logs.entries.is_empty());
    assert!(logs.entries.iter().any(|entry| {
        entry.kind == LogKind::Stdout
            && entry.payload.get("line").and_then(|line| line.as_str()) == Some("hello world")
    }));
}
