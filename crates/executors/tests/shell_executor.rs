use executors::{
    build_shell_command_plan, is_pid_alive, ExecutionContext, ExecutionOutcome, ExecutorError,
    LogKind, LogReader, ShellConfig, ShellExecutor, TaskExecutor,
};
use std::time::{Duration, Instant};
use tokio::time::sleep;

fn execution_context(
    dir: &tempfile::TempDir,
    log_name: &str,
    execution_id: &str,
    description: &str,
) -> ExecutionContext {
    let log_path = dir.path().join(log_name);
    ExecutionContext {
        task_id: "task-1".to_string(),
        execution_id: execution_id.to_string(),
        worktree_path: dir.path().to_string_lossy().to_string(),
        description: description.to_string(),
        agent_config: serde_json::json!({}),
        logs_path: log_path.to_string_lossy().to_string(),
        heartbeat_interval_seconds: 60,
        max_turns: None,
        log_sender: None,
    }
}

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
    assert!(!executor.has_process("exec-1"));
}

#[tokio::test]
async fn shell_executor_cancel_while_running_returns_cancelled_and_reaps_child() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ShellExecutor::default().with_cancel_grace_period(Duration::from_millis(200));
    let ctx = execution_context(&dir, "cancel.jsonl", "exec-cancel", "sleep 30");

    let child_pid = {
        let running_executor = executor.clone();
        let ctx = ctx.clone();
        let handle = tokio::spawn(async move { running_executor.execute(ctx).await });

        sleep(Duration::from_millis(200)).await;
        assert!(executor.has_process("exec-cancel"));
        let pid = executor
            .running_child_pid("exec-cancel")
            .await
            .expect("child pid while running");

        executor
            .cancel("exec-cancel")
            .await
            .expect("cancel succeeds");
        let result = handle.await.expect("join").expect("execute result");
        assert_eq!(result.status, ExecutionOutcome::Cancelled);
        pid
    };

    sleep(Duration::from_millis(500)).await;
    assert!(!is_pid_alive(child_pid));
    assert!(!executor.has_process("exec-cancel"));
}

#[tokio::test]
async fn shell_executor_cancel_escalates_to_sigkill_after_grace() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ShellExecutor::default().with_cancel_grace_period(Duration::from_millis(200));
    let ctx = execution_context(
        &dir,
        "sigkill.jsonl",
        "exec-sigkill",
        r#"sh -c 'trap "" TERM; sleep 30'"#,
    );

    let started = Instant::now();
    let handle = {
        let executor = executor.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move { executor.execute(ctx).await })
    };

    sleep(Duration::from_millis(200)).await;
    executor
        .cancel("exec-sigkill")
        .await
        .expect("cancel succeeds");

    let result = handle.await.expect("join").expect("execute result");
    assert_eq!(result.status, ExecutionOutcome::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel should not wait for the full sleep"
    );
    assert!(!executor.has_process("exec-sigkill"));
}

#[tokio::test]
async fn shell_executor_non_zero_exit_is_failure_with_stderr_logged() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("failure.jsonl");
    let executor = ShellExecutor::default();
    let result = executor
        .execute(ExecutionContext {
            task_id: "task-1".to_string(),
            execution_id: "exec-fail".to_string(),
            worktree_path: dir.path().to_string_lossy().to_string(),
            description: "sh -c 'echo err >&2; exit 3'".to_string(),
            agent_config: serde_json::json!({}),
            logs_path: log_path.to_string_lossy().to_string(),
            heartbeat_interval_seconds: 60,
            max_turns: None,
            log_sender: None,
        })
        .await
        .unwrap();

    assert_eq!(result.status, ExecutionOutcome::Failed);
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("exit status")),
        "expected shell status in error message"
    );

    let logs = LogReader::read(&log_path, 0, 100).await.unwrap();
    assert!(logs.entries.iter().any(|entry| {
        entry.kind == LogKind::Stderr
            && entry.payload.get("line").and_then(|line| line.as_str()) == Some("err")
    }));
    assert!(!executor.has_process("exec-fail"));
}

#[tokio::test]
async fn shell_executor_nonexistent_binary_returns_io_error_without_leak() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ShellExecutor::default().with_shell_program("/definitely/missing/shell-binary");
    let ctx = execution_context(&dir, "missing.jsonl", "exec-missing", "echo hi");

    let error = executor.execute(ctx).await.expect_err("spawn should fail");
    assert!(matches!(error, ExecutorError::Io(_)));
    assert!(!executor.has_process("exec-missing"));
}

#[tokio::test]
async fn shell_executor_cancel_unknown_execution_id_is_idempotent() {
    let executor = ShellExecutor::default();
    executor
        .cancel("missing-execution")
        .await
        .expect("cancel of an already-finished execution must succeed");
}

#[tokio::test]
async fn shell_command_plan_from_snapshot_matches_executor_defaults() {
    let config: ShellConfig = serde_json::from_value(serde_json::json!({
        "permission_policy": "plan",
        "command": "sh",
        "args": ["-c", "echo configured"]
    }))
    .unwrap();

    let plan = build_shell_command_plan("ignored", "/tmp/worktree", None, Some(&config));
    assert_eq!(plan.program, "sh");
    assert_eq!(plan.args, vec!["-c", "echo configured"]);
    assert_eq!(plan.cwd.to_string_lossy(), "/tmp/worktree");
}
