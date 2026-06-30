use cli_adapters::ClaudeCodeAdapter;
use executors::{CodingExecutorAdapter, ExecutionContext, ExecutionOutcome};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn claude_adapter_writes_file_in_live_repo() -> TestResult {
    if which::which("npx").is_err() {
        println!("skipping Claude E2E: npx not installed");
        return Ok(());
    }

    if !claude_auth_path().exists() {
        println!("skipping Claude E2E: ~/.claude.json missing");
        return Ok(());
    }

    if !npx_package_available_offline([
        "--offline",
        "-y",
        "@anthropic-ai/claude-code@2.1.150",
        "code",
        "--version",
    ]) {
        println!(
            "skipping Claude E2E: @anthropic-ai/claude-code npx package not available offline"
        );
        return Ok(());
    }

    let tempdir = tempfile::tempdir()?;
    init_git_repo(tempdir.path())?;

    let logs_path = tempdir.path().join("log.jsonl");
    let ctx = ExecutionContext {
        worktree_path: tempdir.path().to_string_lossy().into_owned(),
        task_id: "smoke".to_owned(),
        execution_id: "smoke-exec".to_owned(),
        description: "Create a file called HELLO.md in the repo root with the single word banana."
            .to_owned(),
        agent_config: json!({
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "permission_policy": "supervised"
        }),
        logs_path: logs_path.to_string_lossy().into_owned(),
        heartbeat_interval_seconds: 30,
        max_turns: None,
        log_sender: None,
    };

    let adapter = ClaudeCodeAdapter::new();
    let result = tokio::time::timeout(Duration::from_secs(120), adapter.execute(ctx)).await??;

    assert_eq!(
        result.status,
        ExecutionOutcome::Completed,
        "unexpected Claude result: {result:?}"
    );
    assert!(
        result.agent_session_id.is_some(),
        "expected Claude session id to be captured"
    );

    let hello_path = tempdir.path().join("HELLO.md");
    if let Err(error) = assert_hello_contains_banana(&hello_path) {
        if let Some(message) = last_assistant_message(&logs_path) {
            println!("last Claude assistant message: {message}");
        }
        return Err(error);
    }

    Ok(())
}

fn claude_auth_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".claude.json")
}

fn init_git_repo(path: &Path) -> TestResult {
    run_git(path, ["init", "-b", "main"])?;
    run_git(path, ["config", "user.email", "forge-e2e@example.com"])?;
    run_git(path, ["config", "user.name", "Forge E2E"])?;
    fs::write(path.join("README.md"), "smoke\n")?;
    run_git(path, ["add", "README.md"])?;
    run_git(path, ["commit", "-m", "initial commit"])?;
    Ok(())
}

fn run_git<const N: usize>(path: &Path, args: [&str; N]) -> TestResult {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::other(format!("git command failed: {stderr}")).into())
    }
}

fn npx_package_available_offline<const N: usize>(args: [&str; N]) -> bool {
    Command::new("npx")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn assert_hello_contains_banana(path: &Path) -> TestResult {
    assert!(path.exists(), "expected HELLO.md to exist");
    let contents = fs::read_to_string(path)?;
    assert!(
        contents.to_ascii_lowercase().contains("banana"),
        "expected HELLO.md to contain banana, got: {contents:?}"
    );
    Ok(())
}

fn last_assistant_message(logs_path: &Path) -> Option<String> {
    let logs = fs::read_to_string(logs_path).ok()?;
    logs.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|entry| assistant_message_from_payload(entry.get("payload")?))
        .next_back()
}

fn assistant_message_from_payload(payload: &Value) -> Option<String> {
    let content = payload
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| payload.get("content"))?;

    match content {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => Some(payload.to_string()),
    }
}
