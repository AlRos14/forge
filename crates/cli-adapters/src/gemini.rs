use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, DiscoverContext, DiscoveredOptions, ExecutionContext,
    ExecutionOutcome, ExecutionResult, ExecutorError, ExecutorKind, GeminiConfig, HarnessAdapter,
    LogKind, LogStream, LogWriter, PermissionPolicy, ProcessGroupChild, ProjectCopilotMcp,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const PROMPT_SEND_TIMEOUT_SECONDS: u64 = 10;
const FIRST_OUTPUT_TIMEOUT_SECONDS: u64 = 300;
const MAX_SUMMARY_CHARS: usize = 500;
const MODEL_DISCOVERY_TIMEOUT_SECONDS: u64 = 10;
const GEMINI_COPILOT_MCP_SERVER: &str = "forge-project";
const PROJECT_COPILOT_TOKEN_ENV: &str = "FORGE_COPILOT_MCP_TOKEN";

struct GeminiProjectCopilotRuntime {
    root: TempDir,
    system_settings_path: PathBuf,
}

pub struct GeminiAdapter {
    processes: Arc<Mutex<HashMap<String, Arc<AsyncMutex<ProcessGroupChild>>>>>,
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self {
            processes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn resolve_config(ctx: &ExecutionContext) -> GeminiConfig {
        let mut config: GeminiConfig =
            serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default();
        crate::command::clear_session_arguments(
            &mut config.command_overrides,
            &["--resume", "--session"],
            &["--continue"],
        );
        config
    }

    fn build_command(config: &GeminiConfig) -> tokio::process::Command {
        let mut adapter_args = vec!["-p".to_owned(), "--output-format=json".to_owned()];

        if config.yolo.unwrap_or(false) {
            adapter_args.push("--yolo".to_owned());
        } else if let Some(ref policy) = config.permission_policy {
            match policy {
                PermissionPolicy::Auto => {
                    adapter_args.push("--yolo".to_owned());
                }
                PermissionPolicy::Supervised | PermissionPolicy::Plan => {}
            }
        }

        if let Some(ref model) = config.model {
            adapter_args.push("--model".to_owned());
            adapter_args.push(model.clone());
        }

        if let Some(ref sandbox) = config.sandbox {
            adapter_args.push("--sandbox".to_owned());
            adapter_args.push(sandbox.clone());
        }

        if let Some(check_every_n) = config.check_every_n {
            adapter_args.push("--check_every_n".to_owned());
            adapter_args.push(check_every_n.to_string());
        }

        let builder = crate::command::CommandBuilder::new("gemini")
            .adapter_args(adapter_args)
            .overrides(&config.command_overrides);

        let mut cmd = builder.build();
        cmd.kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("NO_COLOR", "1");
        cmd
    }

    fn build_project_copilot_command(config: &GeminiConfig) -> tokio::process::Command {
        let mut safe_config = config.clone();
        safe_config.yolo = Some(false);
        safe_config.permission_policy = Some(PermissionPolicy::Supervised);
        safe_config.sandbox = None;
        safe_config.check_every_n = None;
        let mut command = Self::build_command(&safe_config);
        command.args([
            "--extensions",
            "none",
            "--allowed-mcp-server-names",
            GEMINI_COPILOT_MCP_SERVER,
        ]);
        command
    }

    fn insert_process(
        &self,
        execution_id: String,
        child: Arc<AsyncMutex<ProcessGroupChild>>,
    ) -> Result<(), ExecutorError> {
        self.processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".into()))?
            .insert(execution_id, child);
        Ok(())
    }

    fn remove_process(&self, execution_id: &str) -> Result<(), ExecutorError> {
        self.processes
            .lock()
            .map_err(|_| ExecutorError::Other("process map lock poisoned".into()))?
            .remove(execution_id);
        Ok(())
    }
}

impl Default for GeminiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn validate_project_copilot_config(
    config: &GeminiConfig,
) -> Result<&ProjectCopilotMcp, ExecutorError> {
    if config.command_overrides.base_command_override.is_some()
        || config
            .command_overrides
            .additional_params
            .as_ref()
            .is_some_and(|args| !args.is_empty())
    {
        return Err(ExecutorError::Other(
            "Project Copilot Agent config does not satisfy MCP-only harness policy".to_owned(),
        ));
    }
    const RUNTIME_ENV_ALLOWLIST: &[&str] = &[
        PROJECT_COPILOT_TOKEN_ENV,
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GOOGLE_CLOUD_PROJECT",
        "GOOGLE_CLOUD_LOCATION",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ];
    let env =
        config.command_overrides.env.as_ref().ok_or_else(|| {
            ExecutorError::Other("Project Copilot MCP token is missing".to_owned())
        })?;
    if !env
        .get(PROJECT_COPILOT_TOKEN_ENV)
        .is_some_and(|token| !token.is_empty())
        || env
            .keys()
            .any(|key| !RUNTIME_ENV_ALLOWLIST.contains(&key.as_str()))
    {
        return Err(ExecutorError::Other(
            "Project Copilot runtime environment contains unsupported values".to_owned(),
        ));
    }
    let mcp = config.forge_project_copilot.as_ref().ok_or_else(|| {
        ExecutorError::Other("Project Copilot MCP descriptor is missing".to_owned())
    })?;
    let token = config
        .command_overrides
        .env
        .as_ref()
        .and_then(|env| env.get(PROJECT_COPILOT_TOKEN_ENV))
        .ok_or_else(|| ExecutorError::Other("Project Copilot MCP token is missing".to_owned()))?;
    if mcp.mcp_url.contains(token) {
        return Err(ExecutorError::Other(
            "Project Copilot MCP token must not appear in its URL".to_owned(),
        ));
    }
    if !valid_project_copilot_mcp_url(&mcp.mcp_url)
        || mcp.enabled_tools.iter().any(|tool| {
            tool.is_empty()
                || !tool
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return Err(ExecutorError::Other(
            "Project Copilot MCP descriptor is invalid".to_owned(),
        ));
    }
    Ok(mcp)
}

fn valid_project_copilot_mcp_url(url: &str) -> bool {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https")
        || remainder.is_empty()
        || remainder.contains('?')
        || remainder.contains('#')
        || remainder.chars().any(char::is_whitespace)
    {
        return false;
    }
    let authority = remainder.split('/').next().unwrap_or_default();
    !authority.is_empty() && !authority.contains('@')
}

fn create_gemini_project_copilot_runtime(
    mcp: &ProjectCopilotMcp,
) -> Result<GeminiProjectCopilotRuntime, ExecutorError> {
    let root = tempfile::Builder::new()
        .prefix("forge-gemini-copilot-")
        .tempdir()
        .map_err(|error| {
            ExecutorError::Other(format!(
                "failed to create Gemini Project Copilot runtime directory: {error}"
            ))
        })?;
    let system_settings_path = root.path().join("system-settings.json");
    let settings = serde_json::json!({
        "tools": {"core": []},
        "skills": {"enabled": false},
        "hooksConfig": {"enabled": false},
        "mcp": {"allowed": [GEMINI_COPILOT_MCP_SERVER]},
        "admin": {
            "secureModeEnabled": true,
            "extensions": {"enabled": false},
            "skills": {"enabled": false}
        },
        "mcpServers": {
            (GEMINI_COPILOT_MCP_SERVER): {
                "httpUrl": &mcp.mcp_url,
                "headers": {
                    "Authorization": format!("Bearer ${{{PROJECT_COPILOT_TOKEN_ENV}}}")
                },
                "includeTools": &mcp.enabled_tools,
                "trust": true
            }
        }
    });
    let contents = serde_json::to_vec(&settings).map_err(|error| {
        ExecutorError::Other(format!(
            "failed to encode Gemini Project Copilot settings: {error}"
        ))
    })?;
    fs::write(&system_settings_path, contents).map_err(|error| {
        ExecutorError::Other(format!(
            "failed to write Gemini Project Copilot settings: {error}"
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&system_settings_path, fs::Permissions::from_mode(0o600)).map_err(
            |error| {
                ExecutorError::Other(format!(
                    "failed to protect Gemini Project Copilot settings: {error}"
                ))
            },
        )?;
    }
    Ok(GeminiProjectCopilotRuntime {
        root,
        system_settings_path,
    })
}

async fn ensure_gemini_project_copilot_cli_supported() -> Result<(), ExecutorError> {
    let output = crate::command::output_with_timeout(
        {
            let mut command = tokio::process::Command::new("gemini");
            command.arg("--help");
            command
        },
        Duration::from_secs(5),
    )
    .await
    .filter(|output| output.status.success())
    .ok_or_else(|| {
        ExecutorError::Unavailable(
            "Project Copilot could not verify Gemini CLI MCP and extension options".to_owned(),
        )
    })?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if gemini_project_copilot_cli_supports_policy(&help) {
        Ok(())
    } else {
        Err(ExecutorError::Unavailable(
            "Project Copilot requires Gemini CLI support for --extensions and --allowed-mcp-server-names"
                .to_owned(),
        ))
    }
}

fn gemini_project_copilot_cli_supports_policy(help: &str) -> bool {
    ["--extensions", "--allowed-mcp-server-names"]
        .iter()
        .all(|option| help.contains(option))
}

#[async_trait]
impl HarnessAdapter for GeminiAdapter {
    fn supports_project_copilot(&self) -> bool {
        true
    }
    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Gemini
    }

    fn check_availability(&self) -> AvailabilityInfo {
        detect_gemini_availability()
    }

    fn normalize_config(
        &self,
        config: &serde_json::Value,
        overrides: &executors::ExecutionOverrides,
    ) -> Result<serde_json::Value, ExecutorError> {
        executors::normalize_harness_config::<GeminiConfig>(self.kind(), config, overrides)
    }

    fn capabilities(&self, _config: &serde_json::Value) -> executors::HarnessCapabilities {
        use executors::CapabilitySupport as S;
        crate::C {
            resume: S::Unsupported,
            cancel: S::Emulated,
            structured_events: S::Unknown,
            usage_reporting: S::Unsupported,
            account_usage_observation: S::Unsupported,
            model_selection: S::Native,
            reasoning_controls: S::Unsupported,
            approval_policy: S::Native,
            sandbox_controls: S::Native,
            planning: S::Unsupported,
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
        let isolation = config
            .get("sandbox")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("not_applicable");
        executors::HarnessPolicyInterpretation {
            permission_policy: permission.to_owned(),
            isolation_posture: isolation.to_owned(),
        }
    }

    fn executable_name(&self) -> Option<String> {
        Some("gemini".to_owned())
    }

    async fn discover_options(
        &self,
        _ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError> {
        let (models, warning) = match discover_gemini_models().await {
            Ok(models) => (models, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        Ok(DiscoveredOptions {
            models,
            permission_policies: vec!["auto".into(), "supervised".into()],
            cli_specific: serde_json::json!({}),
            warning,
        })
    }

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let config = Self::resolve_config(&ctx);
        let project_copilot = ctx.role == "project_copilot";
        let copilot_runtime = if project_copilot {
            let mcp = validate_project_copilot_config(&config)?;
            Some(create_gemini_project_copilot_runtime(mcp)?)
        } else {
            None
        };
        if project_copilot {
            ensure_gemini_project_copilot_cli_supported().await?;
        }
        let mut cmd = if project_copilot {
            Self::build_project_copilot_command(&config)
        } else {
            Self::build_command(&config)
        };
        if let Some(runtime) = &copilot_runtime {
            cmd.current_dir(runtime.root.path()).env(
                "GEMINI_CLI_SYSTEM_SETTINGS_PATH",
                &runtime.system_settings_path,
            );
        } else {
            cmd.current_dir(&ctx.worktree_path);
        }

        let mut child = ProcessGroupChild::spawn(&mut cmd)?;

        let stdin = match child.inner().stdin.take() {
            Some(stdin) => stdin,
            None => {
                let cleanup = crate::command::kill_group_and_wait(&mut child).await;
                return Err(ExecutorError::Other(format!(
                    "failed to capture gemini stdin{}",
                    cleanup
                        .err()
                        .map(|error| format!("; process-group cleanup failed: {error}"))
                        .unwrap_or_default()
                )));
            }
        };
        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                let cleanup = crate::command::kill_group_and_wait(&mut child).await;
                return Err(ExecutorError::Other(format!(
                    "failed to capture gemini stdout{}",
                    cleanup
                        .err()
                        .map(|error| format!("; process-group cleanup failed: {error}"))
                        .unwrap_or_default()
                )));
            }
        };
        let stderr = match child.inner().stderr.take() {
            Some(stderr) => stderr,
            None => {
                let cleanup = crate::command::kill_group_and_wait(&mut child).await;
                return Err(ExecutorError::Other(format!(
                    "failed to capture gemini stderr{}",
                    cleanup
                        .err()
                        .map(|error| format!("; process-group cleanup failed: {error}"))
                        .unwrap_or_default()
                )));
            }
        };

        let child_arc = Arc::new(AsyncMutex::new(child));
        let mut group_kill_guard = crate::command::GroupKillOnDrop::new(child_arc.clone());
        if let Err(registration_error) =
            self.insert_process(ctx.execution_id.clone(), child_arc.clone())
        {
            let cleanup_result = {
                let mut child = child_arc.lock().await;
                crate::command::kill_group_and_wait(&mut child).await
            };
            if let Err(cleanup_error) = cleanup_result {
                return Err(ExecutorError::Other(format!(
                    "Gemini process registration failed ({registration_error}); child termination failed ({cleanup_error})"
                )));
            }
            group_kill_guard.disarm();
            return Err(registration_error);
        }

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
                    "type": "gemini_adapter_started",
                    "worktree_path": ctx.worktree_path,
                    "model": ctx.agent_config.get("model").and_then(serde_json::Value::as_str),
                }),
            )
            .await?;

        let prompt = if let Some(template) = &config.prompt_template {
            format!("{template}\n\n{}", ctx.description)
        } else {
            ctx.description.clone()
        };

        let stream_result =
            stream_child_output(&ctx, stdin, stdout, stderr, &prompt, &mut writer).await;

        let status_result = {
            let mut child = child_arc.lock().await;
            crate::command::kill_group_and_wait(&mut child).await
        };
        let status = status_result.map_err(|error| {
            ExecutorError::Other(format!(
                "Gemini process-group termination could not be verified: {error}"
            ))
        })?;
        group_kill_guard.disarm();
        self.remove_process(&ctx.execution_id)?;

        let stream = stream_result?;

        let (outcome, error) = if status.success() {
            (ExecutionOutcome::Completed, None)
        } else {
            (
                ExecutionOutcome::Failed,
                Some(format!("gemini exited with status {status}")),
            )
        };

        let after_sha = if outcome == ExecutionOutcome::Completed {
            let subject = crate::commit::build_commit_subject(Some(&ctx.description), &ctx.task_id);
            match crate::commit::commit_worktree_changes(Path::new(&ctx.worktree_path), &subject)
                .await
            {
                Ok(Some(sha)) => Some(sha),
                Ok(None) => git::get_current_sha(Path::new(&ctx.worktree_path))
                    .await
                    .ok(),
                Err(e) => {
                    return Ok(ExecutionResult {
                        status: ExecutionOutcome::Failed,
                        after_sha: None,
                        agent_session_id: None,
                        summary: stream.summary,
                        error: Some(e.to_string()),
                        usage: None,
                        ..Default::default()
                    });
                }
            }
        } else {
            None
        };

        Ok(ExecutionResult {
            status: outcome,
            after_sha,
            agent_session_id: None,
            summary: stream.summary,
            error,
            usage: None,
            ..Default::default()
        })
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let process = {
            let procs = self
                .processes
                .lock()
                .map_err(|_| ExecutorError::Other("process map lock poisoned".into()))?;
            procs.get(execution_id).cloned()
        };

        if let Some(child_arc) = process {
            let mut child = child_arc.lock().await;
            crate::command::kill_group_and_wait(&mut child).await?;
        }

        Ok(())
    }
}

struct StreamResult {
    summary: Option<String>,
}

async fn stream_child_output(
    _ctx: &ExecutionContext,
    mut stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    prompt: &str,
    writer: &mut LogWriter,
) -> Result<StreamResult, ExecutorError> {
    match tokio::time::timeout(Duration::from_secs(PROMPT_SEND_TIMEOUT_SECONDS), async {
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.shutdown().await
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            return Err(ExecutorError::Other(
                "timed out sending prompt to gemini".to_owned(),
            ));
        }
    }
    drop(stdin);

    writer
        .write(
            LogKind::User,
            LogStream::Main,
            serde_json::json!({
                "text": prompt.chars().take(200).collect::<String>(),
                "source": "forge_prompt",
            }),
        )
        .await?;

    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut summary = None;
    let mut saw_output = false;
    let first_output_timeout =
        tokio::time::sleep(Duration::from_secs(FIRST_OUTPUT_TIMEOUT_SECONDS));
    tokio::pin!(first_output_timeout);

    while !stdout_done || !stderr_done {
        tokio::select! {
            _ = &mut first_output_timeout, if !saw_output => {
                return Err(ExecutorError::Other(
                    format!("gemini produced no output within {FIRST_OUTPUT_TIMEOUT_SECONDS}s"),
                ));
            }
            line = stdout_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(line)) => {
                        saw_output = true;
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                            let kind = json
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let log_kind = match kind {
                                "assistant" | "result" => LogKind::Assistant,
                                "tool_call" | "tool_use" => LogKind::ToolCall,
                                "tool_result" => LogKind::ToolResult,
                                _ => LogKind::Stdout,
                            };

                            if matches!(kind, "assistant" | "result")
                                && let Some(content) = json
                                    .get("content")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| json.get("message").and_then(|v| v.as_str()))
                            {
                                summary = Some(truncate_summary(content));
                            }

                            writer.write(log_kind, LogStream::Main, json).await?;
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
                    Ok(None) => stdout_done = true,
                    Err(e) => return Err(e.into()),
                }
            }
            line = stderr_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(line)) => {
                        saw_output = true;
                        writer
                            .write(
                                LogKind::Stderr,
                                LogStream::Main,
                                serde_json::json!({ "line": line }),
                            )
                            .await?;
                    }
                    Ok(None) => stderr_done = true,
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }

    Ok(StreamResult { summary })
}

fn truncate_summary(content: &str) -> String {
    if content.chars().count() <= MAX_SUMMARY_CHARS {
        content.to_owned()
    } else {
        content.chars().take(MAX_SUMMARY_CHARS).collect()
    }
}

// ---------------------------------------------------------------------------
// Availability detection
// ---------------------------------------------------------------------------

fn detect_gemini_availability() -> AvailabilityInfo {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

    let gemini_config = home.join(".gemini");
    if gemini_config.join("settings.json").exists() {
        return AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: None,
            config_path: Some(gemini_config.to_string_lossy().into_owned()),
        };
    }

    if std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok() {
        return AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: None,
            config_path: None,
        };
    }

    if gemini_config.exists() {
        return AvailabilityInfo {
            status: AvailabilityStatus::Installed,
            authenticated_at: None,
            config_path: Some(gemini_config.to_string_lossy().into_owned()),
        };
    }

    if executable_in_path("gemini") {
        return AvailabilityInfo {
            status: AvailabilityStatus::Installed,
            authenticated_at: None,
            config_path: None,
        };
    }

    AvailabilityInfo {
        status: AvailabilityStatus::NotFound,
        authenticated_at: None,
        config_path: None,
    }
}

fn executable_in_path(name: &str) -> bool {
    which::which(name).is_ok()
}

async fn discover_gemini_models() -> Result<Vec<String>, ExecutorError> {
    if !executable_in_path("gemini") {
        return Err(ExecutorError::Unavailable(
            "Gemini CLI is not available on this daemon. Enter a model ID supported by the installed harness.".to_owned(),
        ));
    }

    // Model listing is available in current Gemini CLI releases. Try both
    // documented spellings so daemon discovery also works across CLI versions.
    for args in [
        vec!["models", "-o", "json"],
        vec!["--list-models", "-o", "json"],
    ] {
        let mut command = tokio::process::Command::new("gemini");
        command.args(args).env("NO_COLOR", "1");
        let Some(output) = crate::command::output_with_timeout(
            command,
            Duration::from_secs(MODEL_DISCOVERY_TIMEOUT_SECONDS),
        )
        .await
        else {
            continue;
        };
        if output.status.success()
            && let Some(models) = parse_gemini_model_catalog(&output.stdout)
            && !models.is_empty()
        {
            return Ok(models);
        }
    }

    Err(ExecutorError::Unavailable(
        "This Gemini CLI does not expose a usable model catalog. Enter a model ID supported by the installed harness.".to_owned(),
    ))
}

fn parse_gemini_model_catalog(output: &[u8]) -> Option<Vec<String>> {
    let value = serde_json::from_slice::<serde_json::Value>(output).ok()?;
    let models = value
        .as_array()
        .or_else(|| value.get("models").and_then(serde_json::Value::as_array))?;

    let mut discovered = Vec::new();
    for model in models {
        let Some(raw_id) = ["modelId", "id", "name"]
            .into_iter()
            .find_map(|key| model.get(key).and_then(serde_json::Value::as_str))
        else {
            continue;
        };
        let id = raw_id
            .trim()
            .strip_prefix("models/")
            .unwrap_or(raw_id.trim());
        if !id.is_empty() && !discovered.iter().any(|existing| existing == id) {
            discovered.push(id.to_owned());
        }
    }

    Some(discovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use executors::CommandOverrides;

    #[test]
    fn fresh_gemini_start_discards_session_scoped_cli_arguments() {
        let ctx = crate::test_execution_context(
            executors::HarnessInvocation::Start,
            serde_json::json!({
                "additional_params":["--resume", "old-session", "--session=stale", "--continue", "--verbose"]
            }),
        );
        let config = GeminiAdapter::resolve_config(&ctx);
        assert_eq!(
            config.command_overrides.additional_params,
            Some(vec!["--verbose".to_owned()])
        );
    }

    #[test]
    fn command_builder_maps_model_and_yolo() {
        let config = GeminiConfig {
            model: Some("gemini-2.5-pro".to_owned()),
            yolo: Some(true),
            command_overrides: CommandOverrides::default(),
            ..GeminiConfig::default()
        };

        let cmd = GeminiAdapter::build_command(&config);
        assert_eq!(cmd.as_std().get_program(), "gemini");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"--yolo".to_owned()));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "gemini-2.5-pro"])
        );
    }

    #[test]
    fn command_builder_maps_permission_policy_auto_to_yolo() {
        let config = GeminiConfig {
            permission_policy: Some(PermissionPolicy::Auto),
            command_overrides: CommandOverrides::default(),
            ..GeminiConfig::default()
        };

        let cmd = GeminiAdapter::build_command(&config);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"--yolo".to_owned()));
    }

    #[test]
    fn command_builder_includes_sandbox_and_check_every_n() {
        let config = GeminiConfig {
            sandbox: Some("docker".to_owned()),
            check_every_n: Some(5),
            command_overrides: CommandOverrides::default(),
            ..GeminiConfig::default()
        };

        let cmd = GeminiAdapter::build_command(&config);
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args.windows(2).any(|pair| pair == ["--sandbox", "docker"]));
        assert!(args.windows(2).any(|pair| pair == ["--check_every_n", "5"]));
    }

    #[test]
    fn command_builder_with_base_override() {
        let config = GeminiConfig {
            command_overrides: CommandOverrides {
                base_command_override: Some("/usr/local/bin/gemini".to_owned()),
                additional_params: Some(vec!["--verbose".to_owned()]),
                env: None,
            },
            ..GeminiConfig::default()
        };

        let cmd = GeminiAdapter::build_command(&config);
        assert_eq!(cmd.as_std().get_program(), "/usr/local/bin/gemini");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"--verbose".to_owned()));
    }
}
