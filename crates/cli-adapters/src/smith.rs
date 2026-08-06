use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, CodingExecutorAdapter, DiscoverContext,
    DiscoveredOptions, ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError,
    ExecutorKind, LogKind, LogStream, LogWriter, PermissionPolicy, SmithConfig, TokenUsage,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::Mutex as AsyncMutex;

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_SUMMARY_CHARS: usize = 500;

struct RunningExecution {
    child: Arc<AsyncMutex<Child>>,
    cancelled: Arc<AtomicBool>,
}

pub struct SmithAdapter {
    executions: Arc<Mutex<HashMap<String, RunningExecution>>>,
}

impl SmithAdapter {
    pub fn new() -> Self {
        Self {
            executions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn resolve_config(ctx: &ExecutionContext) -> SmithConfig {
        serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default()
    }

    fn build_command(config: &SmithConfig, prompt: &str) -> tokio::process::Command {
        let mut adapter_args = vec![
            "-p".to_owned(),
            prompt.to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
        ];

        if config.yolo.unwrap_or(false) {
            adapter_args.push("--yolo".to_owned());
        } else if let Some(ref approval) = config.approval {
            adapter_args.push("--approval".to_owned());
            adapter_args.push(approval.clone());
        } else if let Some(ref policy) = config.permission_policy {
            match policy {
                PermissionPolicy::Auto => {
                    adapter_args.push("--yolo".to_owned());
                }
                PermissionPolicy::Supervised | PermissionPolicy::Plan => {
                    adapter_args.push("--approval".to_owned());
                    adapter_args.push("ask".to_owned());
                }
            }
        }

        if let Some(ref profile) = config.profile {
            adapter_args.push("--profile".to_owned());
            adapter_args.push(profile.clone());
        }

        if let Some(ref provider) = config.provider {
            adapter_args.push("--provider".to_owned());
            adapter_args.push(provider.clone());
        }

        if let Some(ref model) = config.model {
            adapter_args.push("--model".to_owned());
            adapter_args.push(model.clone());
        }

        if let Some(ref resume) = config.resume_session_id {
            adapter_args.push("--resume".to_owned());
            adapter_args.push(resume.clone());
        }

        let builder = crate::command::CommandBuilder::new("smith")
            .adapter_args(adapter_args)
            .overrides(&config.command_overrides);

        let mut cmd = builder.build();
        cmd.kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        cmd
    }

    fn insert_execution(
        &self,
        execution_id: String,
        execution: RunningExecution,
    ) -> Result<(), ExecutorError> {
        self.executions
            .lock()
            .map_err(|_| ExecutorError::Other("execution map lock poisoned".into()))?
            .insert(execution_id, execution);
        Ok(())
    }

    fn remove_execution(&self, execution_id: &str) -> Result<(), ExecutorError> {
        self.executions
            .lock()
            .map_err(|_| ExecutorError::Other("execution map lock poisoned".into()))?
            .remove(execution_id);
        Ok(())
    }
}

impl Default for SmithAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CodingExecutorAdapter for SmithAdapter {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Smith
    }

    fn check_availability(&self) -> AvailabilityInfo {
        detect_smith_availability()
    }

    async fn discover_options(
        &self,
        _ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError> {
        Ok(DiscoveredOptions {
            models: vec![
                "gemini-3.6-flash".into(),
                "claude-3-7-sonnet".into(),
                "gpt-4o".into(),
            ],
            permission_policies: vec!["auto".into(), "supervised".into()],
            cli_specific: serde_json::json!({}),
        })
    }

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let config = Self::resolve_config(&ctx);
        let prompt = if let Some(template) = &config.prompt_template {
            format!("{template}\n\n{}", ctx.description)
        } else {
            ctx.description.clone()
        };

        let mut cmd = Self::build_command(&config, &prompt);
        cmd.current_dir(&ctx.worktree_path);

        let mut child = cmd.spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExecutorError::Other("failed to capture smith stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ExecutorError::Other("failed to capture smith stderr".into()))?;

        let child_arc = Arc::new(AsyncMutex::new(child));
        let cancelled = Arc::new(AtomicBool::new(false));

        let mut writer = LogWriter::new(
            &ctx.logs_path,
            ctx.execution_id.clone(),
            DEFAULT_MAX_OUTPUT_BYTES,
        );
        if let Some(sender) = ctx.log_sender.clone() {
            writer.set_log_sender(sender);
        }

        writer
            .write(
                LogKind::User,
                LogStream::Main,
                serde_json::json!({
                    "text": prompt.chars().take(200).collect::<String>(),
                    "source": "forge_prompt",
                    "mode": "cli",
                }),
            )
            .await?;

        self.insert_execution(
            ctx.execution_id.clone(),
            RunningExecution {
                child: child_arc.clone(),
                cancelled: cancelled.clone(),
            },
        )?;

        let stream_result = stream_run_output(stdout, stderr, &mut writer).await;
        let status = {
            let mut child = child_arc.lock().await;
            child.wait().await?
        };
        self.remove_execution(&ctx.execution_id)?;

        let stream = stream_result?;

        if let Some(session_id) = &stream.agent_session_id {
            writer
                .write(
                    LogKind::SessionInfo,
                    LogStream::Main,
                    serde_json::json!({
                        "session_id": session_id,
                        "source": "smith_cli",
                        "resumed": config.resume_session_id.is_some(),
                    }),
                )
                .await?;
        }

        if let Some(summary) = &stream.summary {
            writer
                .write(
                    LogKind::Assistant,
                    LogStream::Main,
                    serde_json::json!({
                        "text": summary,
                        "source": "smith_cli",
                        "session_id": stream.agent_session_id.as_deref(),
                    }),
                )
                .await?;
        }

        if cancelled.load(Ordering::SeqCst) {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Cancelled,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: None,
                usage: stream.usage,
            });
        }

        if let Some(error) = stream.error {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: Some(error),
                usage: stream.usage,
            });
        }

        if !status.success() {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: Some(smith_run_error(status, &stream.stderr_tail)),
                usage: stream.usage,
            });
        }

        let after_sha = if let Ok(false) =
            git::is_worktree_clean(Path::new(&ctx.worktree_path)).await
        {
            let subject = crate::commit::build_commit_subject(Some(&ctx.description), &ctx.task_id);
            crate::commit::commit_worktree_changes(Path::new(&ctx.worktree_path), &subject)
                .await
                .map_err(|err| {
                    ExecutorError::Other(format!("failed to commit worktree changes: {err}"))
                })?
        } else {
            None
        };

        Ok(ExecutionResult {
            status: ExecutionOutcome::Completed,
            after_sha,
            agent_session_id: stream.agent_session_id,
            summary: stream.summary,
            error: None,
            usage: stream.usage,
        })
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let running = {
            let executions = self
                .executions
                .lock()
                .map_err(|_| ExecutorError::Other("execution map lock poisoned".into()))?;
            executions.get(execution_id).map(|item| RunningExecution {
                child: item.child.clone(),
                cancelled: item.cancelled.clone(),
            })
        };

        if let Some(running) = running {
            running.cancelled.store(true, Ordering::SeqCst);
            let mut child = running.child.lock().await;
            child.start_kill()?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct StreamResult {
    agent_session_id: Option<String>,
    summary: Option<String>,
    error: Option<String>,
    stderr_tail: String,
    usage: Option<TokenUsage>,
}

async fn stream_run_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    writer: &mut LogWriter,
) -> Result<StreamResult, ExecutorError> {
    let mut stdout_reader = BufReader::new(stdout).lines();
    let mut stderr_reader = BufReader::new(stderr).lines();

    let mut result = StreamResult::default();
    let mut assistant_chunks = Vec::new();
    let mut stderr_lines = Vec::new();

    loop {
        tokio::select! {
            line = stdout_reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        process_smith_stdout_line(&line, writer, &mut result, &mut assistant_chunks).await?;
                    }
                    Ok(None) => break,
                    Err(err) => return Err(ExecutorError::Other(format!("failed to read smith stdout: {err}"))),
                }
            }
            line = stderr_reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        writer.write(LogKind::Stderr, LogStream::Main, serde_json::json!({
                            "text": line,
                            "source": "smith_cli_stderr",
                        })).await?;
                        if stderr_lines.len() >= 20 {
                            stderr_lines.remove(0);
                        }
                        stderr_lines.push(line);
                    }
                    Ok(None) => {}
                    Err(err) => return Err(ExecutorError::Other(format!("failed to read smith stderr: {err}"))),
                }
            }
        }
    }

    // Drain remaining stderr if any
    while let Ok(Some(line)) = stderr_reader.next_line().await {
        writer
            .write(
                LogKind::Stderr,
                LogStream::Main,
                serde_json::json!({
                    "text": line,
                    "source": "smith_cli_stderr",
                }),
            )
            .await?;
        if stderr_lines.len() >= 20 {
            stderr_lines.remove(0);
        }
        stderr_lines.push(line);
    }

    if result.summary.is_none() && !assistant_chunks.is_empty() {
        let full = assistant_chunks.join("");
        result.summary = Some(full.chars().take(MAX_SUMMARY_CHARS).collect());
    }

    result.stderr_tail = stderr_lines.join("\n");
    Ok(result)
}

async fn process_smith_stdout_line(
    line: &str,
    writer: &mut LogWriter,
    result: &mut StreamResult,
    assistant_chunks: &mut Vec<String>,
) -> Result<(), ExecutorError> {
    let parsed: serde_json::Value = match serde_json::from_str(line) {
        Ok(val) => val,
        Err(_) => {
            // Unstructured stdout line
            writer
                .write(
                    LogKind::Stdout,
                    LogStream::Main,
                    serde_json::json!({
                        "text": line,
                        "source": "smith_cli_stdout",
                    }),
                )
                .await?;
            return Ok(());
        }
    };

    let line_type = parsed.get("type").and_then(|v| v.as_str());

    match line_type {
        Some("runtime_event") => {
            if let Some(event) = parsed.get("event") {
                let payload = event.get("payload");
                if result.agent_session_id.is_none() {
                    result.agent_session_id = event
                        .get("session")
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned);
                }

                if let Some(payload) = payload {
                    let event_kind = payload.get("event").and_then(|v| v.as_str());
                    match event_kind {
                        Some("text_delta") => {
                            if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                                assistant_chunks.push(text.to_owned());
                                writer
                                    .write(
                                        LogKind::Assistant,
                                        LogStream::Main,
                                        serde_json::json!({
                                            "text": text,
                                            "source": "smith_event",
                                        }),
                                    )
                                    .await?;
                            }
                        }
                        Some("tool_call_started") | Some("tool_call_finished") => {
                            writer
                                .write(
                                    LogKind::System,
                                    LogStream::Main,
                                    serde_json::json!({
                                        "event": event_kind,
                                        "payload": payload,
                                        "source": "smith_tool_event",
                                    }),
                                )
                                .await?;
                        }
                        _ => {
                            writer
                                .write(
                                    LogKind::System,
                                    LogStream::Main,
                                    serde_json::json!({
                                        "event": payload,
                                        "source": "smith_runtime_event",
                                    }),
                                )
                                .await?;
                        }
                    }
                }
            }
        }
        Some("result") => {
            if let Some(session_id) = parsed.get("session_id").and_then(|v| v.as_str()) {
                result.agent_session_id = Some(session_id.to_owned());
            }

            if let Some(output) = parsed.get("output").and_then(|v| v.as_str()) {
                result.summary = Some(output.chars().take(MAX_SUMMARY_CHARS).collect());
            }

            let status = parsed.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status == "approval_required" {
                let approval_details = parsed
                    .get("approval_required")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "Approval required for tool execution".to_string());
                result.error = Some(format!("smith execution halted: {approval_details}"));
            } else if status != "ok" {
                result.error = Some(format!("smith returned non-ok status: {status}"));
            }

            if let Some(usage_json) = parsed.get("usage") {
                let current_turn = usage_json.get("current_turn");
                if let Some(turn) = current_turn {
                    let input = turn
                        .get("input_uncached")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let output = turn.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
                    result.usage = Some(TokenUsage {
                        input_tokens: input,
                        output_tokens: output,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                        cost_usd: None,
                        model: parsed
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                    });
                }
            }

            writer
                .write(
                    LogKind::System,
                    LogStream::Main,
                    serde_json::json!({
                        "result": parsed,
                        "source": "smith_result",
                    }),
                )
                .await?;
        }
        _ => {
            writer
                .write(
                    LogKind::Stdout,
                    LogStream::Main,
                    serde_json::json!({
                        "text": line,
                        "source": "smith_cli_stdout",
                    }),
                )
                .await?;
        }
    }

    Ok(())
}

fn executable_in_path(name: &str) -> bool {
    which::which(name).is_ok()
}

fn smith_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|dir| dir.join(".smith"))
}

fn smith_run_error(status: std::process::ExitStatus, stderr_tail: &str) -> String {
    let mut message = format!("smith run exited with status {status}");
    if !stderr_tail.trim().is_empty() {
        message.push_str("\nstderr tail:\n");
        message.push_str(stderr_tail.trim());
    }
    message
}

fn detect_smith_availability() -> AvailabilityInfo {
    let config_dir = smith_config_dir();

    if executable_in_path("smith") {
        let auth_or_config_exists = config_dir
            .as_ref()
            .map(|d| d.join("config.toml").exists() || d.join("auth.json").exists())
            .unwrap_or(false);

        let env_key_set = std::env::vars().any(|(k, _)| k.starts_with("SMITH_"));

        let status = if auth_or_config_exists || env_key_set {
            AvailabilityStatus::Authenticated
        } else {
            AvailabilityStatus::Installed
        };

        return AvailabilityInfo {
            status,
            authenticated_at: None,
            config_path: config_dir
                .filter(|p| p.exists())
                .map(|p| p.to_string_lossy().into_owned()),
        };
    }

    AvailabilityInfo {
        status: AvailabilityStatus::NotFound,
        authenticated_at: None,
        config_path: None,
    }
}

mod git {
    use std::path::Path;
    use tokio::process::Command;

    pub async fn is_worktree_clean(worktree: &Path) -> Result<bool, std::io::Error> {
        let output = Command::new("git")
            .arg("status")
            .arg("--porcelain")
            .current_dir(worktree)
            .output()
            .await?;

        Ok(output.stdout.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command() {
        let config = SmithConfig {
            model: Some("gemini-3.6-flash".into()),
            profile: Some("work".into()),
            yolo: Some(true),
            ..SmithConfig::default()
        };

        let cmd = SmithAdapter::build_command(&config, "test prompt");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        assert_eq!(cmd.as_std().get_program(), "smith");
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"test prompt".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--yolo".to_string()));
        assert!(args.contains(&"--profile".to_string()));
        assert!(args.contains(&"work".to_string()));
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gemini-3.6-flash".to_string()));
    }
}
