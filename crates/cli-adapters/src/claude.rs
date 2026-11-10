mod normalize;

use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, ClaudeCodeConfig, DiscoverContext, DiscoveredOptions,
    ExecutionContext, ExecutionOutcome, ExecutionResult, ExecutorError, ExecutorKind,
    HarnessAdapter, LogKind, LogStream, LogWriter, PermissionPolicy, ProcessGroupChild,
};
use normalize::NormalizedEntry;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const PROMPT_SEND_TIMEOUT_SECONDS: u64 = 10;
const FIRST_OUTPUT_TIMEOUT_SECONDS: u64 = 300;
const WAITING_FOR_OUTPUT_LOG_INTERVAL_SECONDS: u64 = 30;

pub struct ClaudeCodeAdapter {
    processes: Arc<Mutex<HashMap<String, Arc<AsyncMutex<RunningProcess>>>>>,
}

struct RunningProcess {
    child: ProcessGroupChild,
    cancel: CancellationToken,
}

struct ProcessGroupKillOnDrop {
    process: Arc<AsyncMutex<RunningProcess>>,
    armed: bool,
}

impl ProcessGroupKillOnDrop {
    fn new(process: Arc<AsyncMutex<RunningProcess>>) -> Self {
        Self {
            process,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ProcessGroupKillOnDrop {
    fn drop(&mut self) {
        if self.armed
            && let Ok(mut process) = self.process.try_lock()
        {
            let _ = process.child.start_kill();
        }
    }
}

struct StreamResult {
    cancelled: bool,
    agent_session_id: Option<String>,
    assistant_output: Option<String>,
    summary: Option<String>,
    usage: Option<executors::TokenUsage>,
    availability: AvailabilitySignals,
}

/// Availability signals collected from error channels only (stderr lines and
/// `is_error` result events). Assistant output text is never an input.
#[derive(Default)]
struct AvailabilitySignals {
    limit_retry_after: Option<Option<Duration>>,
    auth_failure: Option<String>,
    saw_error_result: bool,
}

impl AvailabilitySignals {
    /// Classify one line from an error channel. Recognizes the structured
    /// API error JSON claude-code echoes (`error.type`) and the CLI's fixed
    /// usage-limit signature (`... usage limit reached|<epoch-seconds>`).
    fn classify_error_channel_line(&mut self, line: &str) {
        let structured_kind = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.get("type"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            });
        if let Some(kind) = structured_kind {
            match kind.as_str() {
                "rate_limit_error" => {
                    self.limit_retry_after.get_or_insert(None);
                }
                "authentication_error" | "permission_error" => {
                    self.auth_failure.get_or_insert(kind);
                }
                _ => {}
            }
            return;
        }

        let lowered = line.to_ascii_lowercase();
        if lowered.contains("usage limit reached") {
            let retry_after = line.rsplit('|').next().and_then(|suffix| {
                let epoch_seconds = suffix.trim().parse::<u64>().ok()?;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs();
                Some(Duration::from_secs(epoch_seconds.saturating_sub(now)))
            });
            // A later line with an epoch hint may upgrade an earlier bare one.
            match &mut self.limit_retry_after {
                Some(existing) if existing.is_none() => *existing = retry_after,
                Some(_) => {}
                slot @ None => *slot = Some(retry_after),
            }
        } else if lowered.contains("invalid api key")
            || lowered.contains("please run /login")
            || lowered.contains("oauth token has expired")
        {
            self.auth_failure
                .get_or_insert_with(|| line.trim().to_owned());
        }
    }

    /// Classify a raw stdout stream-json event. Only `is_error` result
    /// events participate; assistant/tool events are ignored by design.
    fn classify_stdout_event(&mut self, line: &str) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("result") {
            return;
        }
        let is_error = value
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || value
                .get("subtype")
                .and_then(|v| v.as_str())
                .is_some_and(|subtype| subtype != "success");
        if !is_error {
            return;
        }
        self.saw_error_result = true;
        if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
            self.classify_error_channel_line(text);
        }
    }

    fn into_availability_error(
        self,
        exit_ok: bool,
        usage: Option<executors::TokenUsage>,
    ) -> Option<ExecutorError> {
        if exit_ok && !self.saw_error_result {
            return None;
        }
        if let Some(retry_after) = self.limit_retry_after {
            return Some(ExecutorError::UsageExhausted { retry_after, usage });
        }
        self.auth_failure.map(|reason| {
            ExecutorError::Unavailable(format!("claude-code authentication failure: {reason}"))
        })
    }
}

impl ClaudeCodeAdapter {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn resolve_config(ctx: &ExecutionContext) -> ClaudeCodeConfig {
        let mut config: ClaudeCodeConfig =
            serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default();
        crate::command::clear_session_arguments(
            &mut config.command_overrides,
            &["--resume"],
            &["--continue"],
        );
        config.resume_session_id = ctx.invocation.external_session_id().map(str::to_owned);
        if ctx.invocation.is_planning() {
            config.plan = Some(true);
        }
        config
    }

    #[cfg(test)]
    fn build_command(
        config: &ClaudeCodeConfig,
        resume_session_id: Option<&str>,
    ) -> tokio::process::Command {
        Self::build_command_for_cwd(config, resume_session_id, None, false)
    }

    /// Build a command for the exact requested session. Claude reports a
    /// missing session as an execution error; Forge must not turn it into Start.
    fn build_command_for_cwd(
        config: &ClaudeCodeConfig,
        resume_session_id: Option<&str>,
        cwd: Option<&Path>,
        project_copilot: bool,
    ) -> tokio::process::Command {
        Self::build_command_inner(config, resume_session_id, cwd, project_copilot)
    }

    fn build_command_inner(
        config: &ClaudeCodeConfig,
        resume_session_id: Option<&str>,
        cwd: Option<&Path>,
        project_copilot: bool,
    ) -> tokio::process::Command {
        let mut adapter_args = vec![
            "-p".to_owned(),
            "--verbose".to_owned(),
            "--output-format=stream-json".to_owned(),
            "--include-partial-messages".to_owned(),
        ];

        if project_copilot {
            // Claude Code 2.1.317 (the locally verified CLI) supports an
            // explicit empty built-in tool set. Strict MCP loading admits only
            // the per-run config created by ProjectCopilotService.
            adapter_args.extend([
                "--setting-sources".to_owned(),
                String::new(),
                "--settings".to_owned(),
                r#"{"disableAllHooks":true}"#.to_owned(),
                "--disable-slash-commands".to_owned(),
                "--tools".to_owned(),
                String::new(),
                "--strict-mcp-config".to_owned(),
                "--mcp-config".to_owned(),
                cwd.unwrap_or_else(|| Path::new("."))
                    .join(".mcp.json")
                    .to_string_lossy()
                    .into_owned(),
                "--permission-mode".to_owned(),
                "dontAsk".to_owned(),
                "--allowedTools".to_owned(),
                "mcp__forge_project__*".to_owned(),
            ]);
        } else if config.dangerously_skip_permissions.unwrap_or(false) {
            adapter_args.push("--dangerously-skip-permissions".to_owned());
        } else if let Some(permission_mode) = claude_permission_mode(config) {
            adapter_args.push("--permission-mode".to_owned());
            adapter_args.push(permission_mode.to_owned());
        }

        if let Some(model) = &config.model {
            adapter_args.push("--model".to_owned());
            adapter_args.push(model.clone());
        }

        if let Some(effort) = &config.effort {
            adapter_args.push("--effort".to_owned());
            adapter_args.push(effort.clone());
        }

        if let Some(prompt_template) = &config.prompt_template {
            adapter_args.push("--append-system-prompt".to_owned());
            adapter_args.push(prompt_template.clone());
        }

        if let Some(session_id) = resume_session_id {
            adapter_args.push("--resume".to_owned());
            adapter_args.push(session_id.to_owned());
        }

        let router_enabled = !project_copilot && config.claude_code_router.unwrap_or(false);
        let (program, default_args) = if router_enabled {
            let args = match config
                .claude_code_router_profile
                .as_deref()
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
            {
                Some(profile) => vec![profile.to_owned(), "cli".to_owned(), "--".to_owned()],
                None => vec!["code".to_owned()],
            };
            ("ccr", args)
        } else {
            ("claude", Vec::new())
        };

        let builder = crate::command::CommandBuilder::new(program)
            .default_args(default_args)
            .adapter_args(adapter_args)
            .overrides(&config.command_overrides);

        let mut cmd = builder.build();
        if project_copilot {
            cmd.env_clear();
            for (key, value) in std::env::vars_os() {
                if key
                    .to_str()
                    .is_some_and(|key| COPILOT_HOST_ENV_ALLOWLIST.contains(&key))
                {
                    cmd.env(key, value);
                }
            }
            if let Some(runtime_env) = &config.command_overrides.env {
                for (key, value) in runtime_env {
                    cmd.env(key, value);
                }
            }
        }
        cmd.kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        if let Some(effort) = &config.effort {
            // Claude Code gives this environment variable precedence over
            // --effort. Keep the explicit Agent selection effective even if
            // the daemon inherited another default.
            cmd.env("CLAUDE_CODE_EFFORT_LEVEL", effort);
        }
        if !project_copilot && config.disable_api_key.unwrap_or(false) {
            cmd.env_remove("ANTHROPIC_API_KEY");
        }
        if project_copilot {
            // User-specific settings and command extensions are checked before
            // this builder is called. Keep the dedicated runtime environment
            // (provider credentials and the single-use Forge MCP token).
            cmd.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        }
        cmd
    }

    async fn run(
        &self,
        ctx: ExecutionContext,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecutorError> {
        let config = Self::resolve_config(&ctx);
        let project_copilot = ctx.role == "project_copilot";
        if project_copilot {
            validate_project_copilot_config(&config)?;
            if !Path::new(&ctx.worktree_path).join(".mcp.json").is_file() {
                return Err(ExecutorError::Other(
                    "Project Copilot MCP configuration is missing".to_owned(),
                ));
            }
            ensure_project_copilot_cli_supported().await?;
        }
        if config.claude_code_router.unwrap_or(false)
            && config.command_overrides.base_command_override.is_none()
            && which::which("ccr").is_err()
        {
            return Err(ExecutorError::Unavailable(
                "Claude Code Router is enabled, but its installed `ccr` executable is not available on this daemon.".to_owned(),
            ));
        }
        let resume_session_id = config.resume_session_id.clone();
        let worktree = Path::new(&ctx.worktree_path);
        let claude_home = dirs::home_dir();
        if let Some(session_id) = resume_session_id.as_deref() {
            ensure_exact_claude_session(
                claude_home.as_deref(),
                worktree,
                session_id,
                config.command_overrides.env.as_ref(),
            )?;
        }
        let mut cmd = Self::build_command_for_cwd(
            &config,
            resume_session_id.as_deref(),
            Some(worktree),
            project_copilot,
        );
        cmd.current_dir(&ctx.worktree_path);
        let run_started_at = std::time::SystemTime::now();

        let hook_path = if ctx.role == "project_copilot" {
            None
        } else {
            install_stop_hook(Path::new(&ctx.worktree_path)).await?
        };

        let mut child = ProcessGroupChild::spawn(&mut cmd)?;
        let stdin = match child.inner().stdin.take() {
            Some(stdin) => stdin,
            None => {
                let cleanup = kill_unstarted_child(child).await;
                return Err(ExecutorError::Other(cleanup.map_or_else(
                    |error| {
                        format!(
                            "failed to capture claude stdin; process-group cleanup failed: {error}"
                        )
                    },
                    |_| "failed to capture claude stdin".to_owned(),
                )));
            }
        };
        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                let cleanup = kill_unstarted_child(child).await;
                return Err(ExecutorError::Other(cleanup.map_or_else(
                    |error| {
                        format!(
                            "failed to capture claude stdout; process-group cleanup failed: {error}"
                        )
                    },
                    |_| "failed to capture claude stdout".to_owned(),
                )));
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                let cleanup = kill_unstarted_child(child).await;
                return Err(ExecutorError::Other(cleanup.map_or_else(
                    |error| {
                        format!(
                            "failed to capture claude stderr; process-group cleanup failed: {error}"
                        )
                    },
                    |_| "failed to capture claude stderr".to_owned(),
                )));
            }
        };

        let process = Arc::new(AsyncMutex::new(RunningProcess {
            child,
            cancel: cancel.clone(),
        }));
        let mut group_kill_guard = ProcessGroupKillOnDrop::new(process.clone());
        if let Err(registration_error) =
            self.insert_process(ctx.execution_id.clone(), process.clone())
        {
            let cleanup_result = {
                let mut process = process.lock().await;
                crate::command::kill_group_and_wait(&mut process.child).await
            };
            if let Err(cleanup_error) = cleanup_result {
                return Err(ExecutorError::Other(format!(
                    "Claude process registration failed ({registration_error}); child termination failed ({cleanup_error})"
                )));
            }
            group_kill_guard.disarm();
            return Err(registration_error);
        }

        let stream_result = stream_child_output(&ctx, stdin, stdout, stderr, cancel).await;
        let status_result = wait_and_kill(process).await;
        if status_result.is_ok() {
            group_kill_guard.disarm();
        }
        if status_result.is_ok() {
            self.remove_process(&ctx.execution_id)?;
        }
        uninstall_stop_hook(hook_path).await;

        let stream = stream_result?;
        let status = status_result?;

        if !stream.cancelled {
            let availability = stream
                .availability
                .into_availability_error(status.success(), stream.usage.clone());
            if let Some(availability) = availability {
                return Err(availability);
            }
        }

        let (status, error) = if stream.cancelled {
            (ExecutionOutcome::Cancelled, None)
        } else if status.success() {
            (ExecutionOutcome::Completed, None)
        } else {
            (
                ExecutionOutcome::Failed,
                Some(format!("claude-code exited with status {status}")),
            )
        };
        // Trust the on-disk session file over the stream-reported session_id. When --resume
        // points at a missing session, claude-code still echoes that id in its first hook
        // events before allocating a fresh session — without this override Forge would
        // record the stale id and the next resume would also fail.
        let agent_session_id = match claude_home.as_deref() {
            Some(home) => resolve_persisted_session_id(
                &claude_sessions_dir(home, worktree),
                stream.agent_session_id.as_deref(),
                run_started_at,
            ),
            None => stream.agent_session_id.clone(),
        };
        crate::require_exact_resumed_session(
            "Claude Code",
            resume_session_id.as_deref(),
            agent_session_id.as_deref(),
        )?;
        let after_sha = if status == ExecutionOutcome::Completed
            && (!ctx.has_repository_workspace || ctx.role == "project_copilot")
        {
            None
        } else if status == ExecutionOutcome::Completed {
            let subject = crate::commit::build_commit_subject(Some(&ctx.description), &ctx.task_id);
            match crate::commit::commit_worktree_changes_for_role(
                &ctx.role,
                Path::new(&ctx.worktree_path),
                &subject,
            )
            .await
            {
                Ok(Some(sha)) => Some(sha),
                Ok(None) => git::get_current_sha(Path::new(&ctx.worktree_path))
                    .await
                    .ok(),
                Err(error) => {
                    return Ok(ExecutionResult {
                        status: ExecutionOutcome::Failed,
                        after_sha: None,
                        agent_session_id: agent_session_id.clone(),
                        summary: stream.summary,
                        assistant_output: stream.assistant_output,
                        error: Some(error.to_string()),
                        usage: stream.usage,
                        ..Default::default()
                    });
                }
            }
        } else {
            None
        };

        Ok(ExecutionResult {
            status,
            after_sha,
            agent_session_id,
            assistant_output: stream.assistant_output,
            summary: stream.summary,
            error,
            usage: stream.usage,
            ..Default::default()
        })
    }

    fn insert_process(
        &self,
        execution_id: String,
        process: Arc<AsyncMutex<RunningProcess>>,
    ) -> Result<(), ExecutorError> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?;
        processes.insert(execution_id, process);
        Ok(())
    }

    fn remove_process(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?;
        processes.remove(execution_id);
        Ok(())
    }
}

const COPILOT_HOST_ENV_ALLOWLIST: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "SYSTEMROOT",
    "WINDIR",
    "PATHEXT",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_API_KEY_HELPER",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "ANTHROPIC_VERTEX_REGION",
    "CLOUD_ML_REGION",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "REQUESTS_CA_BUNDLE",
];

fn validate_project_copilot_config(config: &ClaudeCodeConfig) -> Result<(), ExecutorError> {
    if config.command_overrides.base_command_override.is_some()
        || config
            .command_overrides
            .additional_params
            .as_ref()
            .is_some_and(|args| !args.is_empty())
        || config.claude_code_router == Some(true)
        || config.dangerously_skip_permissions == Some(true)
        || config.plan == Some(true)
        || config.permission_policy != Some(PermissionPolicy::Supervised)
    {
        return Err(ExecutorError::Other(
            "Project Copilot Agent config does not satisfy MCP-only harness policy".to_owned(),
        ));
    }

    let env =
        config.command_overrides.env.as_ref().ok_or_else(|| {
            ExecutorError::Other("Project Copilot MCP token is missing".to_owned())
        })?;
    const RUNTIME_ENV_ALLOWLIST: &[&str] = &[
        "FORGE_COPILOT_MCP_TOKEN",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "XAI_API_KEY",
        "OPENROUTER_API_KEY",
    ];
    if !env
        .get("FORGE_COPILOT_MCP_TOKEN")
        .is_some_and(|token| !token.is_empty())
        || env
            .keys()
            .any(|key| !RUNTIME_ENV_ALLOWLIST.contains(&key.as_str()))
    {
        return Err(ExecutorError::Other(
            "Project Copilot runtime environment contains unsupported values".to_owned(),
        ));
    }
    Ok(())
}

async fn ensure_project_copilot_cli_supported() -> Result<(), ExecutorError> {
    let output = crate::command::output_with_timeout(
        {
            let mut command = Command::new("claude");
            command.arg("--help");
            command
        },
        Duration::from_secs(5),
    )
    .await
    .filter(|output| output.status.success())
    .ok_or_else(|| {
        ExecutorError::Unavailable(
            "Project Copilot could not verify Claude Code's MCP-only CLI options".to_owned(),
        )
    })?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if project_copilot_cli_supports_mcp_only(&help) {
        Ok(())
    } else {
        Err(ExecutorError::Unavailable(
            "Project Copilot requires Claude Code CLI support for MCP-only tool isolation"
                .to_owned(),
        ))
    }
}

fn project_copilot_cli_supports_mcp_only(help: &str) -> bool {
    let normalized_help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    let required = [
        "--setting-sources <sources>",
        "--settings <file-or-json>",
        "--disable-slash-commands",
        "--strict-mcp-config",
        "--tools <tools...>",
        "Use \"\" to disable all tools",
        "--allowedTools",
        "dontAsk",
    ];
    required
        .iter()
        .all(|option| normalized_help.contains(option))
}

impl Default for ClaudeCodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn effort_options_from_help(help: &str) -> Vec<String> {
    let lines: Vec<_> = help.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|line| line.trim_start().starts_with("--effort "))
    else {
        return Vec::new();
    };

    let mut effort_help = lines[start].to_owned();
    for line in lines.iter().skip(start + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('-') {
            break;
        }
        effort_help.push(' ');
        effort_help.push_str(trimmed);
    }

    let Some((_, parenthesized)) = effort_help.split_once('(') else {
        return Vec::new();
    };
    let Some((values, _)) = parenthesized.split_once(')') else {
        return Vec::new();
    };

    let mut options = Vec::new();
    for value in values.split(',').map(str::trim) {
        if !value.is_empty()
            && !value.chars().any(char::is_whitespace)
            && !options.iter().any(|existing| existing == value)
        {
            options.push(value.to_owned());
        }
    }
    options
}

#[async_trait]
impl HarnessAdapter for ClaudeCodeAdapter {
    fn supports_project_copilot(&self) -> bool {
        true
    }

    fn kind(&self) -> ExecutorKind {
        ExecutorKind::ClaudeCode
    }

    fn check_availability(&self) -> AvailabilityInfo {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        availability_from_home(&home)
    }

    fn normalize_config(
        &self,
        config: &serde_json::Value,
        overrides: &executors::ExecutionOverrides,
    ) -> Result<serde_json::Value, ExecutorError> {
        executors::normalize_harness_config::<ClaudeCodeConfig>(self.kind(), config, overrides)
    }

    fn capabilities(&self, _config: &serde_json::Value) -> executors::HarnessCapabilities {
        use executors::CapabilitySupport as S;
        let resume = if claude_config_dir_override(_config) {
            S::Unknown
        } else {
            S::Native
        };
        crate::C {
            resume,
            cancel: S::Emulated,
            structured_events: S::Native,
            usage_reporting: S::Native,
            account_usage_observation: S::Unsupported,
            model_selection: S::Native,
            reasoning_controls: S::Native,
            approval_policy: S::Native,
            sandbox_controls: S::Unsupported,
            planning: S::Native,
            review_mode: S::Unsupported,
            fork: S::Unsupported,
            steer: S::Unsupported,
            pause_resume: S::Unsupported,
            compaction: S::Unsupported,
            subagents: S::Unsupported,
        }
    }

    fn interpret_execution_policy(
        &self,
        config: &serde_json::Value,
    ) -> executors::HarnessPolicyInterpretation {
        let permission = config
            .get("permission_policy")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let isolation = if config
            .get("dangerously_skip_permissions")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            "dangerously_skip_permissions"
        } else if config.get("plan").and_then(serde_json::Value::as_bool) == Some(true)
            || permission == "plan"
        {
            "plan"
        } else {
            "standard"
        };
        executors::HarnessPolicyInterpretation {
            permission_policy: permission.to_owned(),
            isolation_posture: isolation.to_owned(),
        }
    }

    fn executable_name(&self) -> Option<String> {
        Some("claude".to_owned())
    }

    async fn discover_options(
        &self,
        _ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError> {
        let mut command = Command::new("claude");
        command.arg("--help");
        let effort_options = crate::command::output_with_timeout(command, Duration::from_secs(5))
            .await
            .filter(|output| output.status.success())
            .map(|output| {
                let help = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                effort_options_from_help(&help)
            })
            .unwrap_or_default();

        Ok(DiscoveredOptions {
            // Claude Code's supported non-interactive CLI has no model-list
            // operation. Do not approximate its account- and provider-specific
            // /model picker with a list maintained by Forge.
            models: Vec::new(),
            permission_policies: vec!["auto".into(), "supervised".into(), "plan".into()],
            cli_specific: serde_json::json!({"reasoning_efforts": effort_options}),
            warning: Some(
                "Claude Code does not expose a supported non-interactive model catalog. Enter an alias or model ID accepted by the installed CLI.".to_owned(),
            ),
        })
    }

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        self.run(ctx, CancellationToken::new()).await
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let process = {
            let processes = self
                .processes
                .lock()
                .map_err(|_| ExecutorError::Other("process map lock poisoned".to_owned()))?;
            processes.get(execution_id).cloned()
        };

        if let Some(process) = process {
            let mut process = process.lock().await;
            process.cancel.cancel();
            crate::command::kill_group_and_wait(&mut process.child).await?;
        }

        Ok(())
    }
}

async fn stream_child_output(
    ctx: &ExecutionContext,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    cancel: CancellationToken,
) -> Result<StreamResult, ExecutorError> {
    let mut writer = LogWriter::new(
        &ctx.logs_path,
        ctx.execution_id.clone(),
        DEFAULT_MAX_OUTPUT_BYTES,
    )
    .with_runtime_secret_redactions(&ctx.agent_config);
    if let Some(sender) = ctx.log_sender.clone() {
        writer.set_log_sender(sender);
    }
    writer
        .write(
            LogKind::System,
            LogStream::Main,
            serde_json::json!({
                "type": "claude_adapter_started",
                "worktree_path": ctx.worktree_path,
                "model": ctx.agent_config.get("model").and_then(serde_json::Value::as_str),
                "effort": ctx.agent_config.get("effort").and_then(serde_json::Value::as_str),
                "prompt_bytes": ctx.description.len(),
            }),
        )
        .await?;

    match tokio::time::timeout(Duration::from_secs(PROMPT_SEND_TIMEOUT_SECONDS), async {
        stdin.write_all(ctx.description.as_bytes()).await?;
        stdin.shutdown().await
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => {
            writer
                .write(
                    LogKind::System,
                    LogStream::Main,
                    serde_json::json!({
                        "type": "claude_prompt_send_timeout",
                        "timeout_seconds": PROMPT_SEND_TIMEOUT_SECONDS,
                    }),
                )
                .await?;
            return Err(ExecutorError::Other(
                "timed out sending prompt to claude-code".to_owned(),
            ));
        }
    }
    drop(stdin);
    writer
        .write(
            LogKind::System,
            LogStream::Main,
            serde_json::json!({
                "type": "claude_prompt_sent",
                "prompt_bytes": ctx.description.len(),
            }),
        )
        .await?;
    writer
        .write(
            LogKind::User,
            LogStream::Main,
            serde_json::json!({
                "text": &ctx.description,
                "source": "forge_prompt",
            }),
        )
        .await?;

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut agent_session_id = None;
    let mut assistant_output = None;
    let mut summary = None;
    let mut usage: Option<executors::TokenUsage> = None;
    let mut availability = AvailabilitySignals::default();
    let mut cancelled = false;
    let mut saw_child_output = false;
    let mut waiting_elapsed_seconds = 0;
    let first_output_timeout =
        tokio::time::sleep(Duration::from_secs(FIRST_OUTPUT_TIMEOUT_SECONDS));
    let waiting_for_output_log =
        tokio::time::sleep(Duration::from_secs(WAITING_FOR_OUTPUT_LOG_INTERVAL_SECONDS));
    tokio::pin!(first_output_timeout);
    tokio::pin!(waiting_for_output_log);

    while !stdout_done || !stderr_done {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                cancelled = true;
                break;
            }
            _ = &mut first_output_timeout, if !saw_child_output => {
                writer
                    .write(
                        LogKind::System,
                        LogStream::Main,
                        serde_json::json!({
                            "type": "claude_no_output_timeout",
                            "timeout_seconds": FIRST_OUTPUT_TIMEOUT_SECONDS,
                        }),
                    )
                    .await?;
                return Err(ExecutorError::Other(
                    format!(
                        "claude-code produced no stdout or stderr within {FIRST_OUTPUT_TIMEOUT_SECONDS}s"
                    ),
                ));
            }
            line = stdout_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => {
                        saw_child_output = true;
                        availability.classify_stdout_event(&line);
                        if let Some(entry) = normalize::normalize(&line) {
                            write_normalized_entry(
                                &mut writer,
                                entry,
                                &mut agent_session_id,
                                &mut assistant_output,
                                &mut summary,
                                &mut usage,
                            ).await?;
                        }
                    }
                    Ok(None) => stdout_done = true,
                    Err(error) => return Err(error.into()),
                }
            }
            line = stderr_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        saw_child_output = true;
                        availability.classify_error_channel_line(&line);
                        writer
                            .write(
                                LogKind::Stderr,
                                LogStream::Main,
                                serde_json::json!({ "line": line }),
                            )
                            .await?;
                    }
                    Ok(None) => stderr_done = true,
                    Err(error) => return Err(error.into()),
                }
            }
            _ = &mut waiting_for_output_log, if !saw_child_output => {
                waiting_elapsed_seconds += WAITING_FOR_OUTPUT_LOG_INTERVAL_SECONDS;
                writer
                    .write(
                        LogKind::System,
                        LogStream::Main,
                        serde_json::json!({
                            "type": "claude_waiting_for_output",
                            "elapsed_seconds": waiting_elapsed_seconds,
                            "timeout_seconds": FIRST_OUTPUT_TIMEOUT_SECONDS,
                        }),
                    )
                    .await?;
                waiting_for_output_log.as_mut().reset(
                    tokio::time::Instant::now()
                        + Duration::from_secs(WAITING_FOR_OUTPUT_LOG_INTERVAL_SECONDS),
                );
            }
        }
    }

    Ok(StreamResult {
        cancelled,
        agent_session_id,
        assistant_output,
        summary,
        usage,
        availability,
    })
}

async fn write_normalized_entry(
    writer: &mut LogWriter,
    entry: NormalizedEntry,
    agent_session_id: &mut Option<String>,
    assistant_output: &mut Option<String>,
    summary: &mut Option<String>,
    usage: &mut Option<executors::TokenUsage>,
) -> Result<(), ExecutorError> {
    match entry {
        NormalizedEntry::Assistant {
            payload,
            content,
            session_id,
        } => {
            set_first_session_id(writer, agent_session_id, session_id).await?;
            if let Some(content) = content {
                *summary = Some(truncate_summary(&content));
            }
            writer
                .write(LogKind::Assistant, LogStream::Main, payload)
                .await?;
        }
        NormalizedEntry::ToolCall {
            payload,
            session_id,
        } => {
            set_first_session_id(writer, agent_session_id, session_id).await?;
            writer
                .write(LogKind::ToolCall, LogStream::Main, payload)
                .await?;
        }
        NormalizedEntry::ToolResult {
            payload,
            session_id,
        } => {
            set_first_session_id(writer, agent_session_id, session_id).await?;
            writer
                .write(LogKind::ToolResult, LogStream::Main, payload)
                .await?;
        }
        NormalizedEntry::SessionInfo {
            payload,
            session_id,
        } => {
            set_first_session_id(writer, agent_session_id, session_id).await?;
            if payload.get("type").and_then(|v| v.as_str()) == Some("result") {
                *usage = extract_usage_from_result(&payload);
                if payload.get("is_error").and_then(Value::as_bool) != Some(true) {
                    *assistant_output = payload
                        .get("result")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            writer
                .write(LogKind::SessionInfo, LogStream::Main, payload)
                .await?;
        }
        NormalizedEntry::Stderr { payload } => {
            writer
                .write(LogKind::Stderr, LogStream::Main, payload)
                .await?;
        }
    }

    Ok(())
}

fn extract_usage_from_result(payload: &serde_json::Value) -> Option<executors::TokenUsage> {
    let usage_obj = payload.get("usage")?;
    Some(executors::TokenUsage {
        input_tokens: usage_obj
            .get("input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        output_tokens: usage_obj
            .get("output_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        cache_read_tokens: usage_obj
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        cache_write_tokens: usage_obj
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        cost_usd: payload
            .get("total_cost_usd")
            .or_else(|| payload.get("cost_usd"))
            .and_then(|v| v.as_f64()),
        model: payload
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    })
}

async fn set_first_session_id(
    writer: &mut LogWriter,
    agent_session_id: &mut Option<String>,
    session_id: Option<String>,
) -> Result<(), ExecutorError> {
    #[allow(clippy::collapsible_if)] // pre-existing warning, out of scope for this change
    if agent_session_id.is_none() {
        if let Some(session_id) = session_id {
            *agent_session_id = Some(session_id.clone());
            writer
                .write(
                    LogKind::SessionInfo,
                    LogStream::Main,
                    serde_json::json!({ "session_id": session_id }),
                )
                .await?;
        }
    }
    Ok(())
}

async fn wait_and_kill(
    process: Arc<AsyncMutex<RunningProcess>>,
) -> Result<ExitStatus, ExecutorError> {
    let mut process = process.lock().await;
    crate::command::kill_group_and_wait(&mut process.child)
        .await
        .map_err(ExecutorError::Io)
}

async fn kill_unstarted_child(mut child: ProcessGroupChild) -> std::io::Result<ExitStatus> {
    crate::command::kill_group_and_wait(&mut child).await
}

// ---------------------------------------------------------------------------
// Stop hook: blocks Claude from exiting while uncommitted changes remain
// ---------------------------------------------------------------------------

fn stop_hook_settings() -> serde_json::Value {
    let script = r#"STATUS=$(git status --porcelain 2>/dev/null); if [ -n "$STATUS" ]; then printf '{"decision":"block","reason":"You have uncommitted changes in the worktree. Please stage and commit them with a descriptive message before stopping.\n%s"}' "$STATUS"; else printf '{}'; fi"#;
    serde_json::json!({
        "hooks": {
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": script,
                    "timeout": 10
                }]
            }]
        }
    })
}

async fn install_stop_hook(worktree_path: &Path) -> Result<Option<PathBuf>, ExecutorError> {
    let claude_dir = worktree_path.join(".claude");
    let settings_path = claude_dir.join("settings.local.json");

    if settings_path.exists() {
        return Ok(None);
    }

    tokio::fs::create_dir_all(&claude_dir).await?;
    let content = serde_json::to_string_pretty(&stop_hook_settings())
        .map_err(|e| ExecutorError::Other(e.to_string()))?;
    tokio::fs::write(&settings_path, content).await?;

    Ok(Some(settings_path))
}

async fn uninstall_stop_hook(path: Option<PathBuf>) {
    if let Some(path) = path {
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// Map a worktree path to the directory claude-code uses for its session JSONL files,
/// e.g. `/Volumes/Data/.../subtask-app` → `~/.claude/projects/-Volumes-Data-...-subtask-app`.
/// Mirrors claude-code's encoding: replace `/` and `.` with `-`.
fn claude_sessions_dir(home: &Path, cwd: &Path) -> PathBuf {
    let cwd_str = cwd.to_string_lossy();
    let encoded: String = cwd_str
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect();
    home.join(".claude").join("projects").join(encoded)
}

fn claude_config_dir_override(config: &Value) -> bool {
    config
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("CLAUDE_CONFIG_DIR"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        || std::env::var_os("CLAUDE_CONFIG_DIR").is_some()
}

fn ensure_exact_claude_session(
    home: Option<&Path>,
    cwd: &Path,
    session_id: &str,
    profile_env: Option<&std::collections::HashMap<String, String>>,
) -> Result<(), ExecutorError> {
    if profile_env
        .and_then(|env| env.get("CLAUDE_CONFIG_DIR"))
        .is_some_and(|value| !value.trim().is_empty())
        || std::env::var_os("CLAUDE_CONFIG_DIR").is_some()
    {
        return Err(ExecutorError::UnsupportedCapability {
            capability: "resume".to_owned(),
            support: executors::CapabilitySupport::Unknown,
        });
    }
    let Some(home) = home else {
        return Err(ExecutorError::Unavailable(
            "cannot verify exact Claude Code HarnessSession without a home directory".to_owned(),
        ));
    };
    if !claude_session_exists(home, cwd, session_id) {
        return Err(ExecutorError::Unavailable(format!(
            "Claude Code HarnessSession {session_id} does not exist for this workspace; resume cannot start a fresh session"
        )));
    }
    Ok(())
}

/// Resolve the actual session id claude-code persisted for this cwd, preferring the
/// `.jsonl` file with the latest mtime ≥ `run_started_at`. Falls back to `captured`
/// if no file matches the run window. Returns `None` only if both inputs are absent.
fn resolve_persisted_session_id(
    sessions_dir: &Path,
    captured: Option<&str>,
    run_started_at: std::time::SystemTime,
) -> Option<String> {
    let entries = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(_) => return captured.map(str::to_owned),
    };
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let modified = match entry.metadata().and_then(|meta| meta.modified()) {
            Ok(modified) => modified,
            Err(_) => continue,
        };
        if modified < run_started_at {
            continue;
        }
        match &best {
            None => best = Some((modified, stem.to_owned())),
            Some((current, _)) if modified > *current => {
                best = Some((modified, stem.to_owned()));
            }
            _ => {}
        }
    }
    best.map(|(_, stem)| stem)
        .or_else(|| captured.map(str::to_owned))
}

/// Returns true if claude-code's persisted session file exists for the given id and cwd.
fn claude_session_exists(home: &Path, cwd: &Path, session_id: &str) -> bool {
    claude_sessions_dir(home, cwd)
        .join(format!("{session_id}.jsonl"))
        .is_file()
}

fn claude_permission_mode(config: &ClaudeCodeConfig) -> Option<&'static str> {
    if config.plan.unwrap_or(false) {
        return Some("plan");
    }

    // claude-code's --permission-mode default prompts the user for every Edit/Write/Bash.
    // Forge runs claude-code non-interactively (-p flag) so those prompts can never be
    // answered, which deadlocks the turn. Forge already sandboxes each agent to its own
    // git worktree and reviews the aggregate diff before merging, so per-tool approval
    // inside the worktree is double-supervision. Map both Auto and Supervised to
    // bypassPermissions until the MCP-based forge_approval prompt tool is wired up.
    match config.permission_policy.as_ref()? {
        PermissionPolicy::Auto | PermissionPolicy::Supervised => Some("bypassPermissions"),
        PermissionPolicy::Plan => Some("plan"),
    }
}

fn truncate_summary(content: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 500;

    if content.chars().count() <= MAX_SUMMARY_CHARS {
        content.to_owned()
    } else {
        content.chars().take(MAX_SUMMARY_CHARS).collect()
    }
}

fn availability_from_home(home: &std::path::Path) -> AvailabilityInfo {
    let claude_config = home.join(".claude.json");

    if claude_config.exists() {
        AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: None,
            config_path: Some(claude_config.to_string_lossy().into_owned()),
        }
    } else {
        let claude_dir = home.join(".claude");
        if claude_dir.exists() {
            AvailabilityInfo {
                status: AvailabilityStatus::Installed,
                authenticated_at: None,
                config_path: Some(claude_dir.to_string_lossy().into_owned()),
            }
        } else {
            AvailabilityInfo {
                status: AvailabilityStatus::NotFound,
                authenticated_at: None,
                config_path: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use executors::CommandOverrides;
    use std::time::Duration;

    #[test]
    fn generic_resume_uses_exact_session_and_start_clears_stale_session() {
        let stale = serde_json::json!({
            "resume_session_id":"old-session",
            "additional_params":["--resume", "old-cli-session", "--continue", "--verbose"]
        });
        let start =
            crate::test_execution_context(executors::HarnessInvocation::Start, stale.clone());
        let start_config = ClaudeCodeAdapter::resolve_config(&start);
        assert!(start_config.resume_session_id.is_none());
        assert_eq!(
            start_config.command_overrides.additional_params,
            Some(vec!["--verbose".to_owned()])
        );

        let resume = crate::test_execution_context(
            executors::HarnessInvocation::Resume {
                external_session_id: "exact-session".to_owned(),
            },
            stale,
        );
        let resume_config = ClaudeCodeAdapter::resolve_config(&resume);
        assert_eq!(
            resume_config.resume_session_id.as_deref(),
            Some("exact-session")
        );
        assert_eq!(
            resume_config.command_overrides.additional_params,
            Some(vec!["--verbose".to_owned()])
        );
    }

    #[test]
    fn missing_claude_session_fails_before_starting_a_new_session() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let error = ensure_exact_claude_session(
            Some(home.path()),
            cwd.path(),
            "missing-exact-session",
            None,
        )
        .expect_err("missing exact session must fail before CLI spawn");
        assert!(matches!(error, ExecutorError::Unavailable(_)));
        assert!(error.to_string().contains("cannot start a fresh session"));

        let mut config = serde_json::json!({});
        config["env"] = serde_json::json!({"CLAUDE_CONFIG_DIR":"/custom/claude"});
        assert_eq!(
            ClaudeCodeAdapter::new().capabilities(&config).resume,
            executors::CapabilitySupport::Unknown
        );
    }

    #[test]
    fn usage_limit_signature_with_epoch_classifies() {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let mut signals = AvailabilitySignals::default();
        signals.classify_error_channel_line(&format!("Claude AI usage limit reached|{epoch}"));

        match signals.into_availability_error(false, None) {
            Some(ExecutorError::UsageExhausted { retry_after, .. }) => {
                let retry = retry_after.expect("epoch converts to relative retry");
                assert!(retry <= Duration::from_secs(3600));
                assert!(retry >= Duration::from_secs(3590));
            }
            other => panic!("expected UsageExhausted, got {other:?}"),
        }
    }

    #[test]
    fn structured_api_rate_limit_error_classifies() {
        let mut signals = AvailabilitySignals::default();
        signals.classify_error_channel_line(
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your per-minute rate limit"}}"#,
        );
        assert!(matches!(
            signals.into_availability_error(false, None),
            Some(ExecutorError::UsageExhausted {
                retry_after: None,
                ..
            })
        ));
    }

    #[test]
    fn auth_failure_classifies_as_unavailable() {
        let mut signals = AvailabilitySignals::default();
        signals.classify_error_channel_line("Invalid API key · Please run /login");
        match signals.into_availability_error(false, None) {
            Some(ExecutorError::Unavailable(reason)) => {
                assert!(reason.to_ascii_lowercase().contains("invalid api key"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn error_result_event_with_limit_text_classifies() {
        let mut signals = AvailabilitySignals::default();
        signals.classify_stdout_event(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"5-hour usage limit reached ∙ resets 3am"}"#,
        );
        // Error result marks the run bad even when the exit code is 0.
        assert!(matches!(
            signals.into_availability_error(true, None),
            Some(ExecutorError::UsageExhausted { .. })
        ));
    }

    #[test]
    fn assistant_events_and_clean_exits_never_classify() {
        let mut signals = AvailabilitySignals::default();
        // Assistant event containing limit-like prose is not a result event.
        signals.classify_stdout_event(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"the API returned: usage limit reached"}]}}"#,
        );
        // Successful result mentioning limits in its text is not an error.
        signals.classify_stdout_event(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"documented the usage limit reached error path"}"#,
        );
        assert!(signals.into_availability_error(true, None).is_none());

        // A stray stderr limit line on a run that still exited 0 with no
        // error result does not classify.
        let mut recovered = AvailabilitySignals::default();
        recovered.classify_error_channel_line("usage limit reached");
        assert!(recovered.into_availability_error(true, None).is_none());
    }

    #[test]
    fn claude_sessions_dir_encodes_slashes_and_dots() {
        let dir = claude_sessions_dir(
            Path::new("/Users/alice"),
            Path::new("/Volumes/work/.cache/proj"),
        );
        assert_eq!(
            dir.to_string_lossy(),
            "/Users/alice/.claude/projects/-Volumes-work--cache-proj"
        );
    }

    #[test]
    fn missing_session_keeps_exact_resume_request() {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        // Pretend the user's HOME is our temp dir by directly probing the helper:
        let cwd_path = cwd.path();
        let session_id = "missing-session";
        assert!(!claude_session_exists(home.path(), cwd_path, session_id));

        let config = ClaudeCodeConfig {
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };
        let cmd = ClaudeCodeAdapter::build_command_inner(&config, Some(session_id), None, false);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        // The adapter preserves exact continuity and lets Claude report that
        // the requested session cannot be opened.
        assert!(args.contains(&"--resume".to_owned()));
        assert!(args.contains(&session_id.to_owned()));
    }

    #[test]
    fn resolve_persisted_session_id_picks_recent_file_in_window() {
        let dir = tempfile::tempdir().expect("dir");
        let stem_old = "00000000-old";
        let stem_new = "11111111-new";
        std::fs::write(dir.path().join(format!("{stem_old}.jsonl")), "").unwrap();
        // Stagger mtimes by sleeping briefly.
        std::thread::sleep(Duration::from_millis(20));
        let started_at = std::time::SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.path().join(format!("{stem_new}.jsonl")), "").unwrap();

        let resolved =
            resolve_persisted_session_id(dir.path(), Some("ignored-from-stream"), started_at);
        assert_eq!(resolved.as_deref(), Some(stem_new));
    }

    #[test]
    fn resolve_persisted_session_id_falls_back_to_captured_when_no_recent_file() {
        let dir = tempfile::tempdir().expect("dir");
        let started_at = std::time::SystemTime::now() + Duration::from_secs(60);
        let resolved = resolve_persisted_session_id(dir.path(), Some("captured-id"), started_at);
        assert_eq!(resolved.as_deref(), Some("captured-id"));
    }

    #[test]
    fn command_builder_uses_installed_claude_and_maps_model_effort_resume() {
        let config = ClaudeCodeConfig {
            model: Some("claude-sonnet-4-6".to_owned()),
            effort: Some("high".to_owned()),
            dangerously_skip_permissions: Some(true),
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };

        let cmd = ClaudeCodeAdapter::build_command(&config, Some("session-123"));
        assert_eq!(cmd.as_std().get_program(), "claude");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "-p",
                "--verbose",
                "--output-format=stream-json",
                "--include-partial-messages",
                "--dangerously-skip-permissions",
                "--model",
                "claude-sonnet-4-6",
                "--effort",
                "high",
                "--resume",
                "session-123"
            ]
        );
    }

    #[test]
    fn project_copilot_command_disables_native_tools_and_restricts_mcp() {
        let cwd = tempfile::tempdir().unwrap();
        let config = ClaudeCodeConfig {
            model: Some("claude-sonnet-4-6".to_owned()),
            effort: Some("high".to_owned()),
            permission_policy: Some(PermissionPolicy::Supervised),
            dangerously_skip_permissions: Some(true),
            plan: Some(true),
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };
        let cmd = ClaudeCodeAdapter::build_command_for_cwd(&config, None, Some(cwd.path()), true);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let has_pair = |flag: &str, value: &str| {
            args.windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value)
        };

        assert!(has_pair("--tools", ""));
        assert!(has_pair("--setting-sources", ""));
        assert!(has_pair("--settings", r#"{"disableAllHooks":true}"#));
        assert!(has_pair("--permission-mode", "dontAsk"));
        assert!(has_pair("--allowedTools", "mcp__forge_project__*"));
        assert!(has_pair(
            "--mcp-config",
            cwd.path().join(".mcp.json").to_str().unwrap()
        ));
        assert!(args.contains(&"--strict-mcp-config".to_owned()));
        assert!(args.contains(&"--disable-slash-commands".to_owned()));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_owned()));
        assert!(!args.contains(&"--bare".to_owned()));
        assert!(args.contains(&"claude-sonnet-4-6".to_owned()));
        assert!(args.contains(&"high".to_owned()));
    }

    #[test]
    fn project_copilot_config_accepts_only_runtime_credentials_and_safe_policy() {
        let safe = ClaudeCodeConfig {
            permission_policy: Some(PermissionPolicy::Supervised),
            command_overrides: CommandOverrides {
                env: Some(HashMap::from([
                    (
                        "FORGE_COPILOT_MCP_TOKEN".to_owned(),
                        "single-use".to_owned(),
                    ),
                    ("OPENAI_API_KEY".to_owned(), "provider-key".to_owned()),
                ])),
                ..CommandOverrides::default()
            },
            ..ClaudeCodeConfig::default()
        };
        validate_project_copilot_config(&safe).expect("trusted runtime environment is allowed");

        let unsafe_env = ClaudeCodeConfig {
            permission_policy: Some(PermissionPolicy::Supervised),
            command_overrides: CommandOverrides {
                env: Some(HashMap::from([
                    (
                        "FORGE_COPILOT_MCP_TOKEN".to_owned(),
                        "single-use".to_owned(),
                    ),
                    ("PATH".to_owned(), "/tmp/agent-bin".to_owned()),
                ])),
                ..CommandOverrides::default()
            },
            ..ClaudeCodeConfig::default()
        };
        assert!(validate_project_copilot_config(&unsafe_env).is_err());

        let unsafe_args = ClaudeCodeConfig {
            permission_policy: Some(PermissionPolicy::Supervised),
            command_overrides: CommandOverrides {
                additional_params: Some(vec!["--tools".to_owned(), "default".to_owned()]),
                env: safe.command_overrides.env.clone(),
                ..CommandOverrides::default()
            },
            ..ClaudeCodeConfig::default()
        };
        assert!(validate_project_copilot_config(&unsafe_args).is_err());
    }

    #[test]
    fn project_copilot_cli_help_must_advertise_mcp_only_controls() {
        let help = [
            "--setting-sources <sources>",
            "--settings <file-or-json>",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--tools <tools...>",
            "Use \"\" to disable all tools",
            "--allowedTools",
            "dontAsk",
        ]
        .join("\n");
        assert!(project_copilot_cli_supports_mcp_only(&help));
        assert!(!project_copilot_cli_supports_mcp_only(
            "--tools <tools...> --strict-mcp-config"
        ));
    }

    #[tokio::test]
    async fn discovery_does_not_fabricate_a_claude_model_catalog() {
        let discovered = ClaudeCodeAdapter::new()
            .discover_options(DiscoverContext { project_path: None })
            .await
            .expect("Claude Code options should be discoverable");

        assert!(discovered.models.is_empty());
        assert!(discovered.warning.is_some());
    }

    #[test]
    fn command_builder_uses_installed_ccr_for_legacy_passthrough() {
        let config = ClaudeCodeConfig {
            claude_code_router: Some(true),
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };

        let cmd = ClaudeCodeAdapter::build_command(&config, None);
        assert_eq!(cmd.as_std().get_program(), "ccr");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.starts_with(&["code".to_owned(), "-p".to_owned()]));
    }

    #[test]
    fn command_builder_maps_permission_policy() {
        for policy in [PermissionPolicy::Auto, PermissionPolicy::Supervised] {
            let config = ClaudeCodeConfig {
                permission_policy: Some(policy.clone()),
                command_overrides: CommandOverrides::default(),
                ..ClaudeCodeConfig::default()
            };

            let cmd = ClaudeCodeAdapter::build_command(&config, None);
            let args: Vec<_> = cmd
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();

            assert!(
                args.windows(2)
                    .any(|window| window == ["--permission-mode", "bypassPermissions"]),
                "{policy:?} should map to bypassPermissions, got {args:?}"
            );
        }
    }

    #[test]
    fn command_builder_maps_plan_policy() {
        let config = ClaudeCodeConfig {
            permission_policy: Some(PermissionPolicy::Plan),
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };

        let cmd = ClaudeCodeAdapter::build_command(&config, None);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(
            args.windows(2)
                .any(|window| window == ["--permission-mode", "plan"])
        );
    }

    #[test]
    fn explicit_claude_plan_mode_is_exposed_as_native_planning_support() {
        let config = ClaudeCodeConfig {
            plan: Some(true),
            command_overrides: CommandOverrides::default(),
            ..ClaudeCodeConfig::default()
        };
        let command = ClaudeCodeAdapter::build_command(&config, None);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|window| window == ["--permission-mode", "plan"])
        );

        let normalized = serde_json::to_value(config).expect("Claude config serializes");
        assert_eq!(
            ClaudeCodeAdapter::new().capabilities(&normalized).planning,
            executors::CapabilitySupport::Native
        );
    }

    #[test]
    fn planning_invocation_selects_claude_native_plan_mode() {
        let ctx = crate::test_execution_context(
            executors::HarnessInvocation::Planning {
                external_session_id: None,
            },
            serde_json::json!({ "permission_policy": "auto" }),
        );
        let config = ClaudeCodeAdapter::resolve_config(&ctx);
        assert_eq!(config.plan, Some(true));

        let command = ClaudeCodeAdapter::build_command(&config, None);
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.windows(2)
                .any(|window| window == ["--permission-mode", "plan"])
        );
    }

    #[test]
    fn availability_reports_installed_from_mock_claude_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();

        let availability = availability_from_home(dir.path());

        assert!(matches!(availability.status, AvailabilityStatus::Installed));
        assert!(
            availability
                .config_path
                .as_deref()
                .unwrap()
                .ends_with(".claude")
        );
    }
}
