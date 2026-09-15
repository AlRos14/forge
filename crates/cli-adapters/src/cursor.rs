use async_trait::async_trait;
use command_group::{AsyncCommandGroup, AsyncGroupChild};
#[cfg(unix)]
use command_group::{Signal, UnixChildExt};
use executors::{
    AvailabilityInfo, AvailabilityStatus, CodingExecutorAdapter, CursorConfig, DiscoverContext,
    DiscoveredOptions, ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError,
    ExecutorKind, LogKind, LogStream, LogWriter, PermissionPolicy,
};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};
use tempfile::{Builder as TempfileBuilder, NamedTempFile, TempDir};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{Instant, sleep, timeout};
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_SUMMARY_CHARS: usize = 500;
const STATUS_TIMEOUT_SECONDS: u64 = 2;
const LIST_MODELS_TIMEOUT_SECONDS: u64 = 10;
const MAX_DIRECT_PROMPT_BYTES: usize = 32 * 1024;
const CONTROL_DIR_PREFIX: &str = "forge-cursor-control-";
const STALE_CONTROL_DIR_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct CursorAdapter {
    processes: Arc<Mutex<HashMap<String, RunningProcess>>>,
    usage_cache: Arc<AsyncMutex<Option<(std::time::Instant, Value)>>>,
}

struct RuntimePromptFile {
    file: NamedTempFile,
    root: TempDir,
}

impl RuntimePromptFile {
    fn path(&self) -> &Path {
        self.file.path()
    }

    fn workspace_root(&self) -> &Path {
        self.root.path()
    }

    fn instruction(&self) -> String {
        format!(
            "Follow the instructions in the external file `{}` exactly. Do not commit, edit, or delete that control file.",
            self.path().display()
        )
    }
}

#[derive(Clone)]
struct RunningProcess {
    child: Arc<AsyncMutex<AsyncGroupChild>>,
    cancel: CancellationToken,
}

struct CursorStreamResult {
    cancelled: bool,
    agent_session_id: Option<String>,
    summary: Option<String>,
    error: Option<String>,
    stderr_tail: String,
    usage: Option<executors::TokenUsage>,
}

impl CursorAdapter {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
            usage_cache: Arc::new(AsyncMutex::new(None)),
        }
    }

    async fn cached_account_usage(&self, config: &CursorConfig) -> Option<Value> {
        let cached = self.usage_cache.lock().await.clone();
        if let Some((captured, value)) = cached.as_ref()
            && captured.elapsed() < Duration::from_secs(300)
        {
            return Some(value.clone());
        }
        match query_account_usage(config).await {
            Ok(value) => {
                *self.usage_cache.lock().await = Some((std::time::Instant::now(), value.clone()));
                Some(value)
            }
            Err(_) => cached.map(|(_, value)| value),
        }
    }

    fn resolve_config(ctx: &ExecutionContext) -> CursorConfig {
        serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default()
    }

    fn write_runtime_prompt(
        worktree_path: &str,
        execution_id: &str,
        prompt: &str,
    ) -> Result<RuntimePromptFile, ExecutorError> {
        let worktree = fs::canonicalize(worktree_path).map_err(|error| {
            ExecutorError::Other(format!("failed to resolve Cursor worktree: {error}"))
        })?;
        let safe_execution_id = execution_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();

        let control_parent = std::env::temp_dir();
        cleanup_stale_control_dirs_at(&control_parent, SystemTime::now()).map_err(|error| {
            ExecutorError::Other(format!(
                "failed to clean stale Cursor control files: {error}"
            ))
        })?;
        let root = TempfileBuilder::new()
            .prefix(&format!("{CONTROL_DIR_PREFIX}{safe_execution_id}-"))
            .tempdir_in(&control_parent)
            .map_err(|error| {
                ExecutorError::Other(format!(
                    "failed to create Cursor runtime directory: {error}"
                ))
            })?;
        let control_root = fs::canonicalize(root.path()).map_err(|error| {
            ExecutorError::Other(format!(
                "failed to resolve Cursor runtime directory: {error}"
            ))
        })?;
        if control_root.starts_with(&worktree) || worktree.starts_with(&control_root) {
            return Err(ExecutorError::Other(
                "Cursor control storage overlaps the repository worktree".to_owned(),
            ));
        }

        let mut file = NamedTempFile::new_in(root.path()).map_err(|error| {
            ExecutorError::Other(format!("failed to create Cursor runtime prompt: {error}"))
        })?;
        file.write_all(prompt.as_bytes()).map_err(|error| {
            ExecutorError::Other(format!("failed to write Cursor runtime prompt: {error}"))
        })?;
        file.as_file_mut().flush().map_err(|error| {
            ExecutorError::Other(format!("failed to flush Cursor runtime prompt: {error}"))
        })?;
        set_private_runtime_permissions(&root, &file)?;
        Ok(RuntimePromptFile { file, root })
    }

    fn build_command(
        config: &CursorConfig,
        additional_workspace: Option<&Path>,
    ) -> tokio::process::Command {
        let mut adapter_args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
        ];

        if should_force(config) {
            adapter_args.push("--force".to_owned());
        }

        if let Some(model) = &config.model {
            adapter_args.push("--model".to_owned());
            adapter_args.push(model.clone());
        }

        if let Some(session_id) = &config.resume_session_id {
            adapter_args.push("--resume".to_owned());
            adapter_args.push(session_id.clone());
        }

        if let Some(additional_workspace) = additional_workspace {
            adapter_args.push("--add-dir".to_owned());
            adapter_args.push(additional_workspace.to_string_lossy().into_owned());
        }

        let builder = crate::command::CommandBuilder::new("cursor-agent")
            .adapter_args(adapter_args)
            .overrides(&config.command_overrides);

        let mut cmd = builder.build();
        cmd.kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        cmd
    }

    fn insert_process(
        &self,
        execution_id: String,
        running: RunningProcess,
    ) -> Result<(), ExecutorError> {
        self.processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?
            .insert(execution_id, running);
        Ok(())
    }

    fn remove_process(&self, execution_id: &str) -> Result<(), ExecutorError> {
        self.processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?
            .remove(execution_id);
        Ok(())
    }
}

fn cleanup_stale_control_dirs_at(parent: &Path, now: SystemTime) -> std::io::Result<()> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(CONTROL_DIR_PREFIX) || !entry.file_type()?.is_dir() {
            continue;
        }
        let modified = entry.metadata()?.modified().unwrap_or(now);
        let stale = now
            .duration_since(modified)
            .unwrap_or_default()
            .gt(&STALE_CONTROL_DIR_TTL);
        if stale {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn set_private_runtime_permissions(
    root: &TempDir,
    file: &NamedTempFile,
) -> Result<(), ExecutorError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).map_err(|error| {
            ExecutorError::Other(format!(
                "failed to protect Cursor runtime directory: {error}"
            ))
        })?;
        fs::set_permissions(file.path(), fs::Permissions::from_mode(0o600)).map_err(|error| {
            ExecutorError::Other(format!("failed to protect Cursor runtime prompt: {error}"))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (root, file);
    }
    Ok(())
}

impl Default for CursorAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CodingExecutorAdapter for CursorAdapter {
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Cursor
    }

    fn check_availability(&self) -> AvailabilityInfo {
        detect_cursor_availability()
    }

    async fn discover_options(
        &self,
        _ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError> {
        Ok(DiscoveredOptions {
            models: discover_cursor_models().await,
            permission_policies: vec!["auto".into(), "supervised".into(), "plan".into()],
            cli_specific: serde_json::json!({
                "output_formats": ["text", "json", "stream-json"],
            }),
        })
    }

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let config = Self::resolve_config(&ctx);
        let prompt = if let Some(template) = &config.prompt_template {
            format!("{template}\n\n{}", ctx.description)
        } else {
            ctx.description.clone()
        };

        let runtime_prompt = if prompt.len() > MAX_DIRECT_PROMPT_BYTES {
            Some(Self::write_runtime_prompt(
                &ctx.worktree_path,
                &ctx.execution_id,
                &prompt,
            )?)
        } else {
            None
        };
        let mut command = Self::build_command(
            &config,
            runtime_prompt
                .as_ref()
                .map(RuntimePromptFile::workspace_root),
        );
        command.arg(
            runtime_prompt
                .as_ref()
                .map(RuntimePromptFile::instruction)
                .unwrap_or_else(|| prompt.clone()),
        );
        command.current_dir(&ctx.worktree_path);
        let mut child = command.group_spawn()?;

        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                signal_child(&mut child);
                let _ = child.wait().await;
                return Err(ExecutorError::Other(
                    "failed to capture cursor stdout".to_owned(),
                ));
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                signal_child(&mut child);
                let _ = child.wait().await;
                return Err(ExecutorError::Other(
                    "failed to capture cursor stderr".to_owned(),
                ));
            }
        };

        let child = Arc::new(AsyncMutex::new(child));
        let cancel = CancellationToken::new();
        self.insert_process(
            ctx.execution_id.clone(),
            RunningProcess {
                child: child.clone(),
                cancel: cancel.clone(),
            },
        )?;

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
                LogKind::System,
                LogStream::Main,
                serde_json::json!({
                    "type": "cursor_adapter_started",
                    "worktree_path": ctx.worktree_path,
                    "model": config.model.as_deref(),
                    "prompt_bytes": prompt.len(),
                    "force": should_force(&config),
                }),
            )
            .await?;

        let stream_result = stream_child_output(stdout, stderr, &mut writer, cancel.clone()).await;
        let stream_failed = stream_result.is_err();
        let status = {
            let mut child = child.lock().await;
            if stream_failed {
                signal_child(&mut child);
            }
            child.wait().await?
        };
        self.remove_process(&ctx.execution_id)?;

        let stream = stream_result?;
        if stream.cancelled {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Cancelled,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: None,
                usage: stream.usage,
                ..Default::default()
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
                ..Default::default()
            });
        }

        if !status.success() {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: Some(cursor_run_error(status, &stream.stderr_tail)),
                usage: stream.usage,
                ..Default::default()
            });
        }

        let after_sha = if let Ok(false) =
            git::is_worktree_clean(Path::new(&ctx.worktree_path)).await
        {
            let subject = crate::commit::build_commit_subject(Some(&ctx.description), &ctx.task_id);
            crate::commit::commit_worktree_changes(Path::new(&ctx.worktree_path), &subject)
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let after_sha = match after_sha {
            Some(sha) => Some(sha),
            None => git::get_current_sha(Path::new(&ctx.worktree_path))
                .await
                .ok(),
        };

        let account_usage = self.cached_account_usage(&config).await;
        Ok(ExecutionResult {
            status: ExecutionOutcome::Completed,
            after_sha,
            agent_session_id: stream.agent_session_id,
            summary: stream.summary,
            error: None,
            usage: stream.usage,
            account_usage,
            ..Default::default()
        })
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let running = {
            self.processes
                .lock()
                .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?
                .get(execution_id)
                .cloned()
        };

        if let Some(running) = running {
            running.cancel.cancel();
            let mut child = running.child.lock().await;
            signal_child(&mut child);
        }

        Ok(())
    }
}

/// Read Cursor quota through its native interactive `/usage` command without
/// starting a model turn. The result is explicitly a polling observation; it
/// is not advertised as a native streaming capability.
pub async fn query_account_usage(config: &CursorConfig) -> Result<Value, ExecutorError> {
    query_account_usage_with_cancel(config, CancellationToken::new()).await
}

/// Cancellable form used by the bounded live-usage probe around an active
/// execution. Every wait has a deadline, and cancellation drops a
/// `kill_on_drop` child so a probe cannot survive its execution.
pub async fn query_account_usage_with_cancel(
    config: &CursorConfig,
    cancel: CancellationToken,
) -> Result<Value, ExecutorError> {
    let builder =
        crate::command::CommandBuilder::new("cursor-agent").overrides(&config.command_overrides);
    let program = builder
        .resolve_executable()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            config
                .command_overrides
                .base_command_override
                .clone()
                .unwrap_or_else(|| "cursor-agent".to_owned())
        });
    let extra_args = config
        .command_overrides
        .additional_params
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|arg| sh_single_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let shell = if extra_args.is_empty() {
        format!("stty cols 100 rows 40; exec {}", sh_single_quote(&program))
    } else {
        format!(
            "stty cols 100 rows 40; exec {} {extra_args}",
            sh_single_quote(&program)
        )
    };
    let mut command = tokio::process::Command::new("script");
    command
        .args(["-qec", &shell, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("NO_COLOR", "1")
        .kill_on_drop(true);
    if let Some(env) = &config.command_overrides.env {
        for (key, value) in env {
            command.env(key, value);
        }
    }
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ExecutorError::Other("Cursor usage PTY has no stdin".to_owned()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExecutorError::Other("Cursor usage PTY has no stdout".to_owned()))?;
    let mut output = Vec::new();
    read_until(
        &mut stdout,
        &mut output,
        Duration::from_secs(10),
        |text| text.contains("Ready"),
        &cancel,
    )
    .await
    .map_err(|error| ExecutorError::Other(format!("Cursor did not become ready: {error}")))?;
    write_or_cancel(&mut stdin, b"/usage\r", &cancel).await?;
    sleep_or_cancel(Duration::from_millis(500), &cancel).await?;
    write_or_cancel(&mut stdin, b"\r", &cancel).await?;
    read_until(
        &mut stdout,
        &mut output,
        Duration::from_secs(20),
        |text| {
            text.contains("Monthly plan and on-demand usage") && text.contains("View in dashboard")
        },
        &cancel,
    )
    .await
    .map_err(|error| ExecutorError::Other(format!("Cursor /usage did not load: {error}")))?;
    write_or_cancel(&mut stdin, b"\x1b", &cancel).await?;
    sleep_or_cancel(Duration::from_millis(250), &cancel).await?;
    write_or_cancel(&mut stdin, b"/quit\r", &cancel).await?;
    sleep_or_cancel(Duration::from_millis(250), &cancel).await?;
    write_or_cancel(&mut stdin, b"\r", &cancel).await?;
    drop(stdin);
    tokio::select! {
        _ = cancel.cancelled() => {
            return Err(ExecutorError::Other("Cursor usage probe cancelled".to_owned()));
        }
        result = timeout(Duration::from_secs(3), stdout.read_to_end(&mut output)) => {
            result
                .map_err(|_| ExecutorError::Other("Cursor /usage output timed out".to_owned()))??;
        }
    }
    tokio::select! {
        _ = cancel.cancelled() => {
            return Err(ExecutorError::Other("Cursor usage probe cancelled".to_owned()));
        }
        result = timeout(Duration::from_secs(5), child.wait()) => {
            result
                .map_err(|_| ExecutorError::Other("Cursor /usage timed out".to_owned()))??;
        }
    }
    parse_cursor_usage(&String::from_utf8_lossy(&output))
}

fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn write_or_cancel<W>(
    writer: &mut W,
    bytes: &[u8],
    cancel: &CancellationToken,
) -> Result<(), ExecutorError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    tokio::select! {
        _ = cancel.cancelled() => Err(ExecutorError::Other("Cursor usage probe cancelled".to_owned())),
        result = writer.write_all(bytes) => result.map_err(ExecutorError::from),
    }
}

async fn sleep_or_cancel(
    duration: Duration,
    cancel: &CancellationToken,
) -> Result<(), ExecutorError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(ExecutorError::Other("Cursor usage probe cancelled".to_owned())),
        _ = sleep(duration) => Ok(()),
    }
}

async fn read_until<R, F>(
    reader: &mut R,
    output: &mut Vec<u8>,
    duration: Duration,
    predicate: F,
    cancel: &CancellationToken,
) -> std::result::Result<(), &'static str>
where
    R: AsyncRead + Unpin,
    F: Fn(&str) -> bool,
{
    let deadline = Instant::now() + duration;
    let mut chunk = [0_u8; 4096];
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        if predicate(&strip_ansi(&String::from_utf8_lossy(output))) {
            return Ok(());
        }
        let read = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled"),
            result = timeout(
                deadline.saturating_duration_since(Instant::now()),
                reader.read(&mut chunk),
            ) => result
                .map_err(|_| "timed out")?
                .map_err(|_| "terminal output could not be read")?,
        };
        if read == 0 {
            return Err("process exited early");
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn parse_cursor_usage(output: &str) -> Result<Value, ExecutorError> {
    parse_cursor_usage_panel(output).or_else(|_| parse_cursor_usage_pools(output))
}

fn parse_cursor_usage_panel(output: &str) -> Result<Value, ExecutorError> {
    let text = strip_ansi(output);
    let lines = text.lines().map(str::trim).collect::<Vec<_>>();
    let usage_start = lines
        .iter()
        .rposition(|line| line.contains("Monthly plan and on-demand usage"))
        .map(|index| index.saturating_sub(1))
        .ok_or_else(|| {
            ExecutorError::Other("Cursor /usage returned no usage summary".to_owned())
        })?;
    let pools = lines[usage_start..]
        .iter()
        .copied()
        .take_while(|line| !line.contains("View in dashboard"))
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("usage")
                || lower.contains("included")
                || lower.contains("auto")
                || lower.contains("api")
                || lower.contains("on-demand")
                || lower.contains("reset")
        })
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if pools.is_empty() {
        return Err(ExecutorError::Other(
            "Cursor /usage returned no recognizable quota pools".to_owned(),
        ));
    }
    let header = pools.iter().find(|line| line.starts_with("Usage"));
    let plan = header
        .and_then(|line| line.split('•').nth(1))
        .and_then(|tail| {
            tail.split_once("Resets")
                .map(|(plan, _)| plan.trim().to_owned())
        });
    let resets_at = header.and_then(|line| {
        line.split_once("Resets")
            .map(|(_, reset)| reset.trim().to_owned())
    });
    let percentage = |category: &str| {
        pools
            .iter()
            .find(|line| line.starts_with(category))
            .and_then(|line| line.split_whitespace().find(|part| part.ends_with('%')))
            .and_then(|part| part.trim_end_matches('%').parse::<u8>().ok())
    };
    let on_demand_enabled = pools
        .iter()
        .find(|line| line.starts_with("On-Demand"))
        .map(|line| !line.to_ascii_lowercase().contains("disabled"));
    Ok(serde_json::json!({
        "plan": plan,
        "resets_at": resets_at,
        "categories": {
            "included": percentage("Included"),
            "auto": percentage("Auto"),
            "api": percentage("API"),
        },
        "on_demand_enabled": on_demand_enabled,
        "pools": pools,
        "raw_kind": "cursor_interactive_usage"
    }))
}

fn parse_cursor_usage_pools(output: &str) -> Result<Value, ExecutorError> {
    let text = strip_ansi(output);
    let pools = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("plan")
                || lower.contains("included")
                || lower.contains("auto")
                || lower.contains("api")
                || lower.contains("on-demand")
                || lower.contains("reset")
        })
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if pools.is_empty() {
        return Err(ExecutorError::Other(
            "Cursor /usage returned no quota pools".to_owned(),
        ));
    }
    Ok(serde_json::json!({ "pools": pools, "raw_kind": "cursor_interactive_usage" }))
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else if ch != '\r' {
            output.push(ch);
        }
    }
    output
}

async fn stream_child_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    writer: &mut LogWriter,
    cancel: CancellationToken,
) -> Result<CursorStreamResult, ExecutorError> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut cancelled = false;
    let mut agent_session_id = None;
    let mut summary = None;
    let mut assistant_text = String::new();
    let mut error = None;
    let mut stderr_tail = String::new();
    let mut usage = None;

    while !stdout_done || !stderr_done {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                cancelled = true;
                break;
            }
            line = stdout_lines.next_line(), if !stdout_done => {
                match line? {
                    Some(line) => {
                        if let Ok(event) = serde_json::from_str::<Value>(&line) {
                            capture_cursor_event(
                                &event,
                                &mut agent_session_id,
                                &mut summary,
                                &mut assistant_text,
                                &mut error,
                                &mut usage,
                            );
                            writer
                                .write(classify_cursor_event(&event), LogStream::Main, event)
                                .await?;
                        } else {
                            writer
                                .write(
                                    LogKind::Stdout,
                                    LogStream::Main,
                                    serde_json::json!({ "line": line }),
                                )
                                .await?;
                        }
                    }
                    None => stdout_done = true,
                }
            }
            line = stderr_lines.next_line(), if !stderr_done => {
                match line? {
                    Some(line) => {
                        push_tail(&mut stderr_tail, &line, 2000);
                        writer
                            .write(
                                LogKind::Stderr,
                                LogStream::Main,
                                serde_json::json!({ "line": line }),
                            )
                            .await?;
                    }
                    None => stderr_done = true,
                }
            }
        }
    }

    Ok(CursorStreamResult {
        cancelled,
        agent_session_id,
        summary,
        error,
        stderr_tail,
        usage,
    })
}

fn capture_cursor_event(
    event: &Value,
    agent_session_id: &mut Option<String>,
    summary: &mut Option<String>,
    assistant_text: &mut String,
    error: &mut Option<String>,
    usage: &mut Option<executors::TokenUsage>,
) {
    if agent_session_id.is_none() {
        *agent_session_id = extract_session_id(event);
    }

    match event_type(event) {
        "assistant" => {
            if let Some(text) = extract_message_text(event) {
                assistant_text.push_str(&text);
                *summary = Some(truncate_summary(assistant_text));
            }
        }
        "result" => {
            if let Some(value) = event.get("usage") {
                *usage = Some(executors::TokenUsage {
                    input_tokens: cursor_i64(value, &["inputTokens", "input_tokens"]),
                    output_tokens: cursor_i64(value, &["outputTokens", "output_tokens"]),
                    cache_read_tokens: cursor_i64(value, &["cacheReadTokens", "cache_read_tokens"]),
                    cache_write_tokens: cursor_i64(
                        value,
                        &["cacheWriteTokens", "cache_write_tokens"],
                    ),
                    cost_usd: None,
                    model: event
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                });
            }
            if let Some(text) = event.get("result").and_then(Value::as_str) {
                *summary = Some(truncate_summary(text));
            }
            if event
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                *error = Some(
                    event
                        .get("result")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or("cursor-agent result marked error")
                        .to_owned(),
                );
            }
        }
        _ => {}
    }
}

fn cursor_i64(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
        .unwrap_or_default()
}

fn classify_cursor_event(event: &Value) -> LogKind {
    match event_type(event) {
        "assistant" => LogKind::AssistantDelta,
        "tool_call" => {
            if event.get("subtype").and_then(Value::as_str) == Some("completed") {
                LogKind::ToolResult
            } else {
                LogKind::ToolCall
            }
        }
        "user" => LogKind::User,
        "system" | "result" => LogKind::SessionInfo,
        _ => LogKind::Stdout,
    }
}

fn should_force(config: &CursorConfig) -> bool {
    config.force.unwrap_or({
        !matches!(
            config.permission_policy.as_ref(),
            Some(PermissionPolicy::Plan)
        )
    })
}

fn cursor_run_error(status: ExitStatus, stderr_tail: &str) -> String {
    let mut message = format!("cursor-agent exited with status {status}");
    let trimmed = stderr_tail.trim();
    if !trimmed.is_empty() {
        message.push_str("\nstderr:\n");
        message.push_str(trimmed);
    }
    message
}

fn event_type(event: &Value) -> &str {
    event.get("type").and_then(Value::as_str).unwrap_or("")
}

fn extract_session_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["session_id", "sessionId", "sessionID"] {
                if let Some(id) = map.get(key).and_then(Value::as_str)
                    && !id.trim().is_empty()
                {
                    return Some(id.to_owned());
                }
            }
            for key in ["message", "data", "result"] {
                if let Some(id) = map.get(key).and_then(extract_session_id) {
                    return Some(id);
                }
            }
            None
        }
        Value::Array(values) => values.iter().find_map(extract_session_id),
        _ => None,
    }
}

fn extract_message_text(value: &Value) -> Option<String> {
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))?;
    match content {
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Value::String(text) if !text.trim().is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

fn push_tail(buffer: &mut String, line: &str, max_chars: usize) {
    if !buffer.is_empty() {
        buffer.push('\n');
    }
    buffer.push_str(line);
    let len = buffer.chars().count();
    if len > max_chars {
        *buffer = buffer.chars().skip(len - max_chars).collect();
    }
}

fn truncate_summary(content: &str) -> String {
    if content.chars().count() <= MAX_SUMMARY_CHARS {
        content.to_owned()
    } else {
        content.chars().take(MAX_SUMMARY_CHARS).collect()
    }
}

fn default_cursor_models() -> Vec<String> {
    vec![
        "auto".into(),
        "composer-2.5".into(),
        "composer-2.5-fast".into(),
        "cursor-grok-4.6-medium-fast".into(),
        "cursor-grok-4.6-high-fast".into(),
        "cursor-grok-4.6-xhigh-fast".into(),
        "claude-sonnet-5-thinking-high".into(),
        "claude-opus-5-thinking-high".into(),
        "gpt-5.6-sol-high".into(),
        "gpt-5.3-codex".into(),
    ]
}

fn parse_cursor_list_models(output: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("available models") {
            continue;
        }
        let Some((id, _)) = line.split_once(" - ") else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.contains(' ') {
            continue;
        }
        if !models.iter().any(|existing| existing == id) {
            models.push(id.to_owned());
        }
    }
    models
}

async fn discover_cursor_models() -> Vec<String> {
    if executable_in_path("cursor-agent") {
        let mut command = tokio::process::Command::new("cursor-agent");
        command.arg("--list-models");
        if let Some(output) = crate::command::output_with_timeout(
            command,
            Duration::from_secs(LIST_MODELS_TIMEOUT_SECONDS),
        )
        .await
            && output.status.success()
        {
            let parsed = parse_cursor_list_models(&String::from_utf8_lossy(&output.stdout));
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }
    default_cursor_models()
}

fn detect_cursor_availability() -> AvailabilityInfo {
    if std::env::var("CURSOR_API_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: None,
            config_path: None,
        };
    }

    if !executable_in_path("cursor-agent") {
        return AvailabilityInfo {
            status: AvailabilityStatus::NotFound,
            authenticated_at: None,
            config_path: None,
        };
    }

    if cursor_status_authenticated() {
        return AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: None,
            config_path: None,
        };
    }

    AvailabilityInfo {
        status: AvailabilityStatus::Installed,
        authenticated_at: None,
        config_path: None,
    }
}

fn cursor_status_authenticated() -> bool {
    let mut command = std::process::Command::new("cursor-agent");
    command.arg("status");
    let Some(output) = command_output_timeout(command, Duration::from_secs(STATUS_TIMEOUT_SECONDS))
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }

    let mut combined = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    combined.push_str(&String::from_utf8_lossy(&output.stderr).to_ascii_lowercase());
    if combined.contains("not authenticated")
        || combined.contains("authenticated: false")
        || combined.contains("authenticated false")
    {
        return false;
    }
    combined.contains("authenticated: true")
        || combined.contains("authenticated true")
        || combined.contains("logged in")
        || combined.contains("signed in")
}

fn command_output_timeout(mut command: std::process::Command, timeout: Duration) -> Option<Output> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(command.output());
    });
    rx.recv_timeout(timeout).ok()?.ok()
}

fn executable_in_path(name: &str) -> bool {
    which::which(name).is_ok()
}

fn signal_child(child: &mut AsyncGroupChild) {
    #[cfg(unix)]
    {
        let _ = child.signal(Signal::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use executors::CommandOverrides;

    #[test]
    fn command_builder_maps_cursor_args() {
        let config = CursorConfig {
            model: Some("gpt-5".to_owned()),
            resume_session_id: Some("session-123".to_owned()),
            permission_policy: Some(PermissionPolicy::Supervised),
            command_overrides: CommandOverrides::default(),
            ..CursorConfig::default()
        };

        let cmd = CursorAdapter::build_command(&config, None);
        assert_eq!(cmd.as_std().get_program(), "cursor-agent");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--force",
                "--model",
                "gpt-5",
                "--resume",
                "session-123",
            ]
        );
    }

    #[test]
    fn small_prompt_stays_in_argv_without_runtime_storage() {
        let worktree = tempfile::tempdir().expect("temp worktree");
        let prompt = "small prompt";
        assert!(prompt.len() <= MAX_DIRECT_PROMPT_BYTES);
        let command = CursorAdapter::build_command(&CursorConfig::default(), None);
        assert!(!command.as_std().get_args().any(|arg| arg == "--add-dir"));
        assert!(!worktree.path().join(".forge").exists());
    }

    #[test]
    fn large_prompt_is_external_private_consumable_and_cleaned_without_worktree_changes() {
        let worktree = tempfile::tempdir().expect("temp worktree");
        let prompt = "x".repeat(400_000);
        let runtime = CursorAdapter::write_runtime_prompt(
            worktree.path().to_str().expect("worktree path"),
            "exec-1",
            &prompt,
        )
        .expect("write runtime prompt");
        let pointer = runtime.instruction();

        assert!(pointer.len() < 500);
        assert!(!runtime.path().starts_with(worktree.path()));
        assert!(!runtime.workspace_root().starts_with(worktree.path()));
        assert_eq!(
            std::fs::read_to_string(runtime.path()).expect("read prompt"),
            prompt
        );
        assert!(runtime.path().exists());
        let command =
            CursorAdapter::build_command(&CursorConfig::default(), Some(runtime.workspace_root()));
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let add_dir = args
            .iter()
            .position(|arg| arg == "--add-dir")
            .expect("large prompt grants Cursor access to its private directory");
        let expected_root = runtime.workspace_root().display().to_string();
        assert_eq!(
            args.get(add_dir + 1).map(String::as_str),
            Some(expected_root.as_str())
        );

        let mut fixture = std::process::Command::new("sh");
        fixture.args([
            "-c",
            "test -r \"$1\" && test \"$(wc -c < \"$1\")\" -eq \"$2\"",
            "cursor-prompt-fixture",
        ]);
        fixture.arg(runtime.path()).arg(prompt.len().to_string());
        assert!(fixture.status().expect("prompt fixture runs").success());

        drop(runtime);
        assert!(!worktree.path().join(".forge").exists());
    }

    #[cfg(unix)]
    #[test]
    fn large_prompt_runtime_storage_is_private_and_randomized() {
        use std::os::unix::fs::PermissionsExt;

        let worktree = tempfile::tempdir().expect("temp worktree");
        let first = CursorAdapter::write_runtime_prompt(
            worktree.path().to_str().expect("worktree path"),
            "same-execution",
            "first",
        )
        .expect("first runtime prompt");
        let second = CursorAdapter::write_runtime_prompt(
            worktree.path().to_str().expect("worktree path"),
            "same-execution",
            "second",
        )
        .expect("second runtime prompt");

        assert_eq!(
            first.root.path().metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            first
                .file
                .as_file()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_ne!(first.root.path(), second.root.path());
        assert_ne!(first.path(), second.path());
    }

    #[tokio::test]
    async fn large_prompt_runtime_storage_is_removed_when_launch_fails() {
        let worktree = tempfile::tempdir().expect("temp worktree");
        let execution_id = format!(
            "launch-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        );
        let prefix = format!("{CONTROL_DIR_PREFIX}{execution_id}-");
        let missing_program = worktree.path().join("missing-cursor-agent");
        let adapter = CursorAdapter::new();

        let result = adapter
            .execute(ExecutionContext {
                task_id: "task-1".to_owned(),
                execution_id,
                worktree_path: worktree.path().display().to_string(),
                description: "x".repeat(MAX_DIRECT_PROMPT_BYTES + 1),
                agent_config: serde_json::json!({
                    "base_command_override": missing_program.display().to_string()
                }),
                logs_path: worktree
                    .path()
                    .join("execution.jsonl")
                    .display()
                    .to_string(),
                heartbeat_interval_seconds: 30,
                max_turns: None,
                log_sender: None,
            })
            .await;

        assert!(result.is_err());
        let leftovers = fs::read_dir(std::env::temp_dir())
            .expect("runtime directory readable")
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix));
        assert!(!leftovers, "launch failure left a Cursor control directory");
    }

    #[test]
    fn stale_runtime_directories_are_removed_but_recent_directories_survive() {
        let parent = tempfile::tempdir().expect("control parent");
        let stale = parent.path().join(format!("{CONTROL_DIR_PREFIX}stale"));
        let recent = parent.path().join(format!("{CONTROL_DIR_PREFIX}recent"));
        std::fs::create_dir(&stale).expect("stale dir");
        std::fs::create_dir(&recent).expect("recent dir");
        let now = SystemTime::now();
        let old = now - STALE_CONTROL_DIR_TTL - Duration::from_secs(1);
        std::fs::File::open(&stale)
            .expect("open stale dir")
            .set_modified(old)
            .expect("age stale dir");

        cleanup_stale_control_dirs_at(parent.path(), now).expect("cleanup runs");
        assert!(!stale.exists());
        assert!(recent.exists());
    }

    #[test]
    fn plan_policy_omits_force_unless_explicit() {
        let config = CursorConfig {
            permission_policy: Some(PermissionPolicy::Plan),
            command_overrides: CommandOverrides::default(),
            ..CursorConfig::default()
        };
        let cmd = CursorAdapter::build_command(&config, None);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(!args.contains(&"--force".to_owned()));

        let config = CursorConfig {
            force: Some(true),
            permission_policy: Some(PermissionPolicy::Plan),
            command_overrides: CommandOverrides::default(),
            ..CursorConfig::default()
        };
        let cmd = CursorAdapter::build_command(&config, None);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--force".to_owned()));
    }

    #[test]
    fn parses_cursor_list_models_text() {
        let models = parse_cursor_list_models(
            "Available models\n\nauto - Auto (default)\ncursor-grok-4.6-medium-fast - Cursor Grok 4.6 Medium Fast\n",
        );
        assert_eq!(models, vec!["auto", "cursor-grok-4.6-medium-fast"]);
    }

    #[test]
    fn default_cursor_models_are_non_empty() {
        assert!(default_cursor_models().contains(&"auto".to_owned()));
        assert!(default_cursor_models().contains(&"cursor-grok-4.6-medium-fast".to_owned()));
    }

    #[test]
    fn captures_cursor_stream_fields() {
        let mut session_id = None;
        let mut summary = None;
        let mut assistant_text = String::new();
        let mut error = None;
        let mut usage = None;

        capture_cursor_event(
            &serde_json::json!({
                "type": "system",
                "session_id": "session-1"
            }),
            &mut session_id,
            &mut summary,
            &mut assistant_text,
            &mut error,
            &mut usage,
        );
        capture_cursor_event(
            &serde_json::json!({
                "type": "assistant",
                "message": {
                    "content": [
                        { "type": "text", "text": "hello" },
                        { "type": "text", "text": " world" }
                    ]
                },
                "session_id": "session-1"
            }),
            &mut session_id,
            &mut summary,
            &mut assistant_text,
            &mut error,
            &mut usage,
        );
        capture_cursor_event(
            &serde_json::json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "final text",
                "session_id": "session-1",
                "usage": {
                    "inputTokens": 10,
                    "outputTokens": 4,
                    "cacheReadTokens": 3,
                    "cacheWriteTokens": 2
                }
            }),
            &mut session_id,
            &mut summary,
            &mut assistant_text,
            &mut error,
            &mut usage,
        );

        assert_eq!(session_id.as_deref(), Some("session-1"));
        assert_eq!(summary.as_deref(), Some("final text"));
        assert!(error.is_none());
        assert_eq!(usage.expect("usage").input_tokens, 10);
    }

    #[test]
    fn parses_cursor_interactive_quota_pools_without_inventing_numbers() {
        let usage = parse_cursor_usage(
            "\u{1b}[32mPlan: Pro\u{1b}[0m\r\nIncluded usage: 37%\r\nAuto requests reset Sep 1\r\n",
        )
        .expect("quota pools parse");

        assert_eq!(
            usage["pools"],
            serde_json::json!([
                "Plan: Pro",
                "Included usage: 37%",
                "Auto requests reset Sep 1"
            ])
        );
        assert_eq!(usage["raw_kind"], "cursor_interactive_usage");
    }
}
