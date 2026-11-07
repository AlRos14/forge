use async_trait::async_trait;
use executors::{
    AvailabilityInfo, AvailabilityStatus, DiscoverContext, DiscoveredOptions, ExecutionContext,
    ExecutionOutcome, ExecutionResult, ExecutorError, ExecutorKind, HarnessAdapter, LogKind,
    LogStream, LogWriter, OpencodeConfig, PermissionPolicy, ProcessGroupChild, ProjectCopilotMcp,
};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

const DEFAULT_MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_SUMMARY_CHARS: usize = 500;
const MODEL_DISCOVERY_TIMEOUT_SECONDS: u64 = 10;
const OPENCODE_COPILOT_AGENT: &str = "forge-project-copilot";
const PROJECT_COPILOT_TOKEN_ENV: &str = "FORGE_COPILOT_MCP_TOKEN";

pub struct OpencodeAdapter {
    executions: Arc<Mutex<HashMap<String, RunningExecution>>>,
}

#[derive(Clone)]
struct RunningExecution {
    child: Arc<AsyncMutex<ProcessGroupChild>>,
    cancelled: Arc<AtomicBool>,
}

struct ProjectCopilotServer {
    child: tokio::process::Child,
    url: String,
    password: String,
}

impl Drop for ProjectCopilotServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl OpencodeAdapter {
    pub fn new() -> Self {
        Self {
            executions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn resolve_config(ctx: &ExecutionContext) -> OpencodeConfig {
        let mut config: OpencodeConfig =
            serde_json::from_value(ctx.agent_config.clone()).unwrap_or_default();
        crate::command::clear_session_arguments(
            &mut config.command_overrides,
            &["--session", "--resume"],
            &["--continue"],
        );
        config.resume_session_id = ctx.invocation.external_session_id().map(str::to_owned);
        config
    }

    fn build_command(config: &OpencodeConfig, prompt: &str) -> tokio::process::Command {
        Self::build_command_with_prefix(config, prompt, &[])
    }

    fn build_command_with_prefix(
        config: &OpencodeConfig,
        prompt: &str,
        global_args: &[&str],
    ) -> tokio::process::Command {
        let overrides = &config.command_overrides;

        let merged_overrides = overrides.clone();

        let mut adapter_args: Vec<String> =
            global_args.iter().map(|arg| (*arg).to_owned()).collect();
        adapter_args.extend([
            "run".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ]);
        if let Some(ref model) = config.model {
            adapter_args.push("--model".into());
            adapter_args.push(normalize_opencode_model_id(model));
        }
        if let Some(ref agent) = config.agent {
            adapter_args.push("--agent".into());
            adapter_args.push(agent.clone());
        }
        if let Some(ref variant) = config.variant {
            adapter_args.push("--variant".into());
            adapter_args.push(variant.clone());
        }
        if let Some(ref session_id) = config.resume_session_id {
            adapter_args.push("--session".into());
            adapter_args.push(session_id.clone());
        }
        if should_skip_permissions(config) {
            adapter_args.push("--dangerously-skip-permissions".into());
        }

        let builder = crate::command::CommandBuilder::new("opencode")
            .adapter_args(adapter_args)
            .overrides(&merged_overrides);

        let mut cmd = builder.build();
        cmd.arg(prompt);
        cmd.kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    async fn start_project_copilot_server(
        config: &OpencodeConfig,
        config_dir: &Path,
    ) -> Result<ProjectCopilotServer, ExecutorError> {
        let mut command = tokio::process::Command::new("opencode");
        command
            .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
            .current_dir(config_dir)
            .env("OPENCODE_CONFIG_DIR", config_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(env) = &config.command_overrides.env {
            command.envs(env);
        }
        let mut child = command.spawn().map_err(|error| {
            ExecutorError::Other(format!(
                "failed to start OpenCode Project Copilot server: {error}"
            ))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ExecutorError::Other("failed to capture OpenCode server stdout".to_owned())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ExecutorError::Other("failed to capture OpenCode server stderr".to_owned())
        })?;
        let mut lines = BufReader::new(stdout).lines();
        let mut url = None;
        let mut password = None;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while url.is_none() || password.is_none() {
            let line = tokio::time::timeout_at(deadline, lines.next_line())
                .await
                .map_err(|_| {
                    ExecutorError::Other(
                        "OpenCode Project Copilot server did not become ready".to_owned(),
                    )
                })?
                .map_err(|error| {
                    ExecutorError::Other(format!(
                        "failed to read OpenCode server startup output: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    ExecutorError::Other(
                        "OpenCode Project Copilot server exited before becoming ready".to_owned(),
                    )
                })?;
            if let Some(value) = line.strip_prefix("server listening on ") {
                url = Some(value.trim().to_owned());
            } else if let Some(value) = line.strip_prefix("server password ") {
                password = Some(value.trim().to_owned());
            }
        }
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(_)) = lines.next_line().await {}
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(ProjectCopilotServer {
            child,
            url: url.expect("server URL is set"),
            password: password.expect("server password is set"),
        })
    }

    async fn warm_up_project_copilot_server(
        config: &OpencodeConfig,
        config_dir: &Path,
        server: &ProjectCopilotServer,
    ) -> Result<(), ExecutorError> {
        let mut last_failure = String::new();
        for attempt in 0..3 {
            let mut command = Self::build_project_copilot_command(
                config,
                "Reply with OK.",
                config_dir,
                server.url.as_str(),
            )?;
            command
                .env("OPENCODE_CONFIG_DIR", config_dir)
                .env("OPENCODE_SERVER_PASSWORD", &server.password);
            let output = tokio::time::timeout(Duration::from_secs(60), command.output())
                .await
                .map_err(|_| {
                    ExecutorError::Other("OpenCode Project Copilot warm-up timed out".to_owned())
                })?
                .map_err(|error| {
                    ExecutorError::Other(format!(
                        "OpenCode Project Copilot warm-up failed: {error}"
                    ))
                })?;
            if output.status.success() {
                return Ok(());
            }
            last_failure = format!(
                "status {}: stderr={} stdout={}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim(),
                String::from_utf8_lossy(&output.stdout).trim()
            );
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        Err(ExecutorError::Other(format!(
            "OpenCode Project Copilot warm-up failed against {}: {}",
            server.url, last_failure
        )))
    }

    fn write_project_copilot_policy(config_dir: &Path, policy: &str) -> Result<(), ExecutorError> {
        let policy_path = config_dir.join("opencode.json");
        std::fs::write(&policy_path, policy).map_err(|error| {
            ExecutorError::Other(format!(
                "failed to write OpenCode Project Copilot policy: {error}"
            ))
        })
    }

    fn build_project_copilot_command(
        config: &OpencodeConfig,
        prompt: &str,
        config_dir: &Path,
        server_url: &str,
    ) -> Result<tokio::process::Command, ExecutorError> {
        let mut safe_config = config.clone();
        safe_config.agent = Some(OPENCODE_COPILOT_AGENT.to_owned());
        safe_config.auto_approve = Some(false);
        safe_config.permission_policy = Some(PermissionPolicy::Supervised);
        // OpenCode v2 selects a variant with model#variant; its current CLI no
        // longer advertises the v1 --variant option.
        if let (Some(model), Some(variant)) =
            (safe_config.model.clone(), safe_config.variant.clone())
        {
            safe_config.model = Some(format!("{}#{variant}", normalize_opencode_model_id(&model)));
            safe_config.variant = None;
        }
        // OpenCode v2 keeps a background server whose configuration is not
        // reloaded per invocation. The Project Copilot agent and MCP policy
        // run against a private server with a real config file so
        // `{env:...}` substitutions are resolved for the MCP authorization.
        let mut command =
            Self::build_command_with_prefix(&safe_config, prompt, &["--server", server_url]);
        command.env("OPENCODE_CONFIG_DIR", config_dir);
        Ok(command)
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

impl Default for OpencodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HarnessAdapter for OpencodeAdapter {
    fn supports_project_copilot(&self) -> bool {
        true
    }

    fn kind(&self) -> ExecutorKind {
        ExecutorKind::Opencode
    }

    fn check_availability(&self) -> AvailabilityInfo {
        detect_opencode_availability()
    }

    fn normalize_config(
        &self,
        config: &serde_json::Value,
        overrides: &executors::ExecutionOverrides,
    ) -> Result<serde_json::Value, ExecutorError> {
        let mut normalized =
            executors::normalize_harness_config::<OpencodeConfig>(self.kind(), config, overrides)?;
        let variant = overrides
            .reasoning_effort
            .as_deref()
            .or_else(|| {
                config
                    .get("model_reasoning_effort")
                    .and_then(serde_json::Value::as_str)
            })
            .or_else(|| config.get("effort").and_then(serde_json::Value::as_str))
            .or_else(|| {
                config
                    .get("reasoning_effort")
                    .and_then(serde_json::Value::as_str)
            });
        if let Some(variant) = variant {
            let object = normalized.as_object_mut().ok_or_else(|| {
                ExecutorError::Other("normalized OpenCode config is not an object".to_owned())
            })?;
            object.insert(
                "variant".to_owned(),
                serde_json::Value::String(variant.to_owned()),
            );
        }
        Ok(normalized)
    }

    fn capabilities(&self, _config: &serde_json::Value) -> executors::HarnessCapabilities {
        use executors::CapabilitySupport as S;
        crate::C {
            resume: S::Native,
            cancel: S::Emulated,
            structured_events: S::Native,
            usage_reporting: S::Unsupported,
            account_usage_observation: S::Unsupported,
            model_selection: S::Native,
            reasoning_controls: S::Native,
            approval_policy: S::Native,
            sandbox_controls: S::Unsupported,
            planning: S::Unsupported,
            review_mode: S::Unsupported,
            fork: S::Unsupported,
            steer: S::Unsupported,
            pause_resume: S::Unsupported,
            compaction: S::Unsupported,
            subagents: S::Unsupported,
        }
    }

    fn executable_name(&self) -> Option<String> {
        Some("opencode".to_owned())
    }

    async fn discover_options(
        &self,
        _ctx: DiscoverContext,
    ) -> Result<DiscoveredOptions, ExecutorError> {
        let catalog = discover_opencode_model_catalog().await?;
        Ok(DiscoveredOptions {
            models: catalog.models,
            permission_policies: vec!["auto".into(), "supervised".into()],
            cli_specific: serde_json::json!({
                "model_id_format": "provider/model",
                "model_reasoning_efforts": catalog.variants_by_model,
            }),
            warning: catalog.warning,
        })
    }

    async fn execute(&self, ctx: ExecutionContext) -> Result<ExecutionResult, ExecutorError> {
        let config = Self::resolve_config(&ctx);
        let project_copilot = ctx.role == "project_copilot";
        let project_copilot_mcp = if project_copilot {
            Some(validate_project_copilot_config(&config)?.clone())
        } else {
            None
        };
        let prompt = if let Some(template) = &config.prompt_template {
            format!("{template}\n\n{}", ctx.description)
        } else {
            ctx.description.clone()
        };

        let copilot_runtime_dir =
            if project_copilot {
                Some(tempfile::Builder::new()
                .prefix("forge-opencode-copilot-")
                .tempdir()
                .map_err(|error| {
                    ExecutorError::Other(format!(
                        "failed to create OpenCode Project Copilot runtime directory: {error}"
                    ))
                })?)
            } else {
                None
            };
        if let (Some(mcp), Some(runtime_dir)) =
            (project_copilot_mcp.as_ref(), copilot_runtime_dir.as_ref())
        {
            let policy = opencode_project_copilot_policy(mcp)?;
            Self::write_project_copilot_policy(runtime_dir.path(), &policy)?;
        }
        let copilot_server = if let Some(runtime_dir) = copilot_runtime_dir.as_ref() {
            Some(Self::start_project_copilot_server(&config, runtime_dir.path()).await?)
        } else {
            None
        };
        let mut cmd = if let (Some(runtime_dir), Some(server)) =
            (copilot_runtime_dir.as_ref(), copilot_server.as_ref())
        {
            Self::warm_up_project_copilot_server(&config, runtime_dir.path(), server).await?;
            let mut command = Self::build_project_copilot_command(
                &config,
                &prompt,
                runtime_dir.path(),
                &server.url,
            )?;
            command.env("OPENCODE_SERVER_PASSWORD", &server.password);
            command
        } else {
            Self::build_command(&config, &prompt)
        };
        if let Some(runtime_dir) = &copilot_runtime_dir {
            cmd.current_dir(runtime_dir.path());
        } else {
            cmd.current_dir(&ctx.worktree_path);
        }

        if project_copilot {
            ensure_opencode_project_copilot_cli_supported().await?;
        }

        let mut child = ProcessGroupChild::spawn(&mut cmd)?;

        let stdout = match child.inner().stdout.take() {
            Some(stdout) => stdout,
            None => {
                let cleanup = crate::command::kill_group_and_wait(&mut child).await;
                return Err(ExecutorError::Other(format!(
                    "failed to capture opencode stdout{}",
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
                    "failed to capture opencode stderr{}",
                    cleanup
                        .err()
                        .map(|error| format!("; process-group cleanup failed: {error}"))
                        .unwrap_or_default()
                )));
            }
        };

        let child_arc = Arc::new(AsyncMutex::new(child));
        let mut group_kill_guard = crate::command::GroupKillOnDrop::new(child_arc.clone());
        let cancelled = Arc::new(AtomicBool::new(false));

        // Record the process before the first await. If generation retirement
        // races this setup, its cancellation reaches the child handle directly.
        if let Err(registration_error) = self.insert_execution(
            ctx.execution_id.clone(),
            RunningExecution {
                child: child_arc.clone(),
                cancelled: cancelled.clone(),
            },
        ) {
            let cleanup_result = {
                let mut child = child_arc.lock().await;
                crate::command::kill_group_and_wait(&mut child).await
            };
            if let Err(cleanup_error) = cleanup_result {
                return Err(ExecutorError::Other(format!(
                    "OpenCode process registration failed ({registration_error}); child termination failed ({cleanup_error})"
                )));
            }
            group_kill_guard.disarm();
            return Err(registration_error);
        }

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
                LogKind::User,
                LogStream::Main,
                serde_json::json!({
                    "text": prompt.chars().take(200).collect::<String>(),
                    "source": "forge_prompt",
                    "mode": "cli",
                }),
            )
            .await?;

        let stream_result = stream_run_output(stdout, stderr, &mut writer).await;
        let status_result = {
            let mut child = child_arc.lock().await;
            crate::command::kill_group_and_wait(&mut child).await
        };
        let status = status_result.map_err(|error| {
            ExecutorError::Other(format!(
                "OpenCode process-group termination could not be verified: {error}"
            ))
        })?;
        group_kill_guard.disarm();
        self.remove_execution(&ctx.execution_id)?;

        let stream = stream_result?;
        if let Some(session_id) = &stream.agent_session_id {
            writer
                .write(
                    LogKind::SessionInfo,
                    LogStream::Main,
                    serde_json::json!({
                        "session_id": session_id,
                        "source": "opencode_cli",
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
                        "source": "opencode_cli",
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
                usage: None,
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
                usage: None,
                ..Default::default()
            });
        }

        if !status.success() {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: stream.summary,
                error: Some(opencode_run_error(status, &stream.stderr_tail)),
                usage: None,
                ..Default::default()
            });
        }

        if stream.summary.is_none() {
            return Ok(ExecutionResult {
                status: ExecutionOutcome::Failed,
                after_sha: None,
                agent_session_id: stream.agent_session_id,
                summary: None,
                error: Some("opencode run completed without assistant text".to_owned()),
                usage: None,
                ..Default::default()
            });
        }

        crate::require_exact_resumed_session(
            "OpenCode",
            config.resume_session_id.as_deref(),
            stream.agent_session_id.as_deref(),
        )?;

        let after_sha = if ctx.role == "project_copilot" {
            None
        } else if let Ok(false) = git::is_worktree_clean(Path::new(&ctx.worktree_path)).await {
            let subject = crate::commit::build_commit_subject(Some(&ctx.description), &ctx.task_id);
            crate::commit::commit_worktree_changes_for_role(
                &ctx.role,
                Path::new(&ctx.worktree_path),
                &subject,
            )
            .await
            .unwrap_or(None)
        } else {
            None
        };
        let after_sha = if ctx.role == "project_copilot" {
            None
        } else {
            match after_sha {
                Some(sha) => Some(sha),
                None => git::get_current_sha(Path::new(&ctx.worktree_path))
                    .await
                    .ok(),
            }
        };

        Ok(ExecutionResult {
            status: ExecutionOutcome::Completed,
            after_sha,
            agent_session_id: stream.agent_session_id,
            assistant_output: stream.assistant_output,
            summary: stream.summary,
            error: None,
            usage: None,
            ..Default::default()
        })
    }

    async fn cancel(&self, execution_id: &str) -> Result<(), ExecutorError> {
        let execution = {
            let execs = self
                .executions
                .lock()
                .map_err(|_| ExecutorError::Other("execution map lock poisoned".into()))?;
            execs.get(execution_id).cloned()
        };

        if let Some(exec) = execution {
            exec.cancelled.store(true, Ordering::SeqCst);
            let mut child = exec.child.lock().await;
            crate::command::kill_group_and_wait(&mut child).await?;
        }

        Ok(())
    }
}

#[derive(Default)]
struct OpenCodeModelCatalog {
    models: Vec<String>,
    variants_by_model: HashMap<String, Vec<String>>,
    warning: Option<String>,
}

async fn discover_opencode_model_catalog() -> Result<OpenCodeModelCatalog, ExecutorError> {
    if which::which("opencode").is_err() {
        return Err(ExecutorError::Unavailable(
            "OpenCode CLI is not available on this daemon".to_owned(),
        ));
    }

    let mut api_command = tokio::process::Command::new("opencode");
    api_command
        .args(["--standalone", "api", "get", "/api/model"])
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0");
    if let Some(output) = crate::command::output_with_timeout(
        api_command,
        Duration::from_secs(MODEL_DISCOVERY_TIMEOUT_SECONDS),
    )
    .await
        && output.status.success()
        && let Some(catalog) =
            parse_opencode_model_catalog(&String::from_utf8_lossy(&output.stdout))
        && !catalog.models.is_empty()
    {
        return Ok(catalog);
    }

    let mut command = tokio::process::Command::new("opencode");
    command
        .arg("models")
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0");
    let output = crate::command::output_with_timeout(
        command,
        Duration::from_secs(MODEL_DISCOVERY_TIMEOUT_SECONDS),
    )
    .await
    .ok_or_else(|| {
        ExecutorError::Unavailable("OpenCode model discovery timed out or could not start".into())
    })?;
    if !output.status.success() {
        return Err(ExecutorError::Unavailable(
            "OpenCode could not list models from its configured providers".to_owned(),
        ));
    }

    let models = parse_opencode_models(&String::from_utf8_lossy(&output.stdout));
    if models.is_empty() {
        return Err(ExecutorError::Unavailable(
            "OpenCode could not list models from its configured providers".to_owned(),
        ));
    }

    Ok(OpenCodeModelCatalog {
        models,
        variants_by_model: HashMap::new(),
        warning: Some(
            "The installed OpenCode CLI did not expose model variants; reasoning choices are unavailable for this model list.".to_owned(),
        ),
    })
}

fn parse_opencode_model_catalog(output: &str) -> Option<OpenCodeModelCatalog> {
    let response = serde_json::from_str::<serde_json::Value>(output).ok()?;
    let entries = response.get("data")?.as_array()?;
    let mut catalog = OpenCodeModelCatalog::default();

    for entry in entries {
        let Some(id) = entry
            .get("id")
            .or_else(|| entry.get("modelID"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let provider = entry.get("providerID").and_then(serde_json::Value::as_str);
        let model_id = match provider {
            Some(provider) if !id.starts_with(&format!("{provider}/")) => {
                format!("{provider}/{id}")
            }
            _ => id.to_owned(),
        };
        if model_id.trim().is_empty() || catalog.models.contains(&model_id) {
            continue;
        }

        let variants = entry
            .get("variants")
            .map(parse_opencode_variant_ids)
            .unwrap_or_default();
        if !variants.is_empty() {
            catalog.variants_by_model.insert(model_id.clone(), variants);
        }
        catalog.models.push(model_id);
    }

    Some(catalog)
}

fn parse_opencode_variant_ids(value: &serde_json::Value) -> Vec<String> {
    let variants = match value {
        serde_json::Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| {
                entry
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect(),
        serde_json::Value::Object(entries) => entries.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let mut unique = Vec::new();
    for variant in variants {
        if !variant.trim().is_empty() && !unique.contains(&variant) {
            unique.push(variant);
        }
    }
    unique
}

fn parse_opencode_models(output: &str) -> Vec<String> {
    let mut models = Vec::new();
    for line in output.lines() {
        let Some(id) = line.split_whitespace().next() else {
            continue;
        };
        let id = id.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '_' | '-' | '.' | '/' | ':')
        });
        if !id.contains('/') || id.split('/').any(str::is_empty) || models.iter().any(|m| m == id) {
            continue;
        }
        models.push(id.to_owned());
    }
    models
}

struct RunStreamResult {
    agent_session_id: Option<String>,
    assistant_output: Option<String>,
    summary: Option<String>,
    error: Option<String>,
    stderr_tail: String,
}

async fn stream_run_output(
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    writer: &mut LogWriter,
) -> Result<RunStreamResult, ExecutorError> {
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut agent_session_id = None;
    let mut assistant_output = None;
    let mut summary = None;
    let mut error = None;
    let mut stderr_tail = String::new();

    while !stdout_done || !stderr_done {
        tokio::select! {
            line = stdout_lines.next_line(), if !stdout_done => {
                match line? {
                    Some(line) => {
                        let cleaned = strip_ansi_codes(&line);
                        if let Ok(event) = serde_json::from_str::<serde_json::Value>(&cleaned) {
                            capture_run_event(
                                &event,
                                &mut agent_session_id,
                                &mut assistant_output,
                                &mut summary,
                                &mut error,
                            );
                            writer.write(classify_run_event(&event), LogStream::Main, event).await?;
                        } else {
                            // stdout outside the documented JSON text events can
                            // contain diagnostics or tool output. Keep it in the
                            // execution log, but never present it as assistant text.
                            writer
                                .write(
                                    LogKind::Stdout,
                                    LogStream::Main,
                                    serde_json::json!({ "line": cleaned }),
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
                        let cleaned = strip_ansi_codes(&line);
                        push_tail(&mut stderr_tail, &cleaned, 2000);
                        writer
                            .write(
                                LogKind::Stderr,
                                LogStream::Main,
                                serde_json::json!({ "line": cleaned }),
                            )
                            .await?;
                    }
                    None => stderr_done = true,
                }
            }
        }
    }

    Ok(RunStreamResult {
        agent_session_id,
        assistant_output,
        summary,
        error,
        stderr_tail,
    })
}

fn validate_project_copilot_config(
    config: &OpencodeConfig,
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
    validate_project_copilot_environment(&config.command_overrides.env)?;
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
    validate_project_copilot_descriptor(mcp)?;
    Ok(mcp)
}

fn validate_project_copilot_environment(
    env: &Option<HashMap<String, String>>,
) -> Result<(), ExecutorError> {
    const RUNTIME_ENV_ALLOWLIST: &[&str] = &[
        PROJECT_COPILOT_TOKEN_ENV,
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "XAI_API_KEY",
        "OPENROUTER_API_KEY",
        "OLLAMA_HOST",
    ];
    let Some(env) = env else {
        return Err(ExecutorError::Other(
            "Project Copilot MCP token is missing".to_owned(),
        ));
    };
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
    Ok(())
}

fn validate_project_copilot_descriptor(mcp: &ProjectCopilotMcp) -> Result<(), ExecutorError> {
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
    Ok(())
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

fn opencode_project_copilot_policy(mcp: &ProjectCopilotMcp) -> Result<String, ExecutorError> {
    validate_project_copilot_descriptor(mcp)?;
    let mut permissions = vec![serde_json::json!({
        "action": "*",
        "resource": "*",
        "effect": "deny"
    })];
    for tool in &mcp.enabled_tools {
        permissions.push(serde_json::json!({
            "action": format!("forge_project_{tool}"),
            "resource": "*",
            "effect": "allow"
        }));
    }
    let document = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "default_agent": OPENCODE_COPILOT_AGENT,
        "permissions": permissions.clone(),
        "agents": {
            (OPENCODE_COPILOT_AGENT): {
                "description": "Project management assistant with delegated Forge MCP tools",
                "mode": "primary",
                "hidden": true,
                "permissions": permissions
            }
        },
        "mcp": {
            "servers": {
                "forge_project": {
                    "type": "remote",
                    "url": &mcp.mcp_url,
                    "oauth": false,
                    "headers": {
                        "Authorization": format!("Bearer {{env:{PROJECT_COPILOT_TOKEN_ENV}}}")
                    },
                    "codemode": false
                }
            }
        }
    });
    serde_json::to_string(&document).map_err(|error| {
        ExecutorError::Other(format!(
            "failed to generate OpenCode Project Copilot policy: {error}"
        ))
    })
}

async fn ensure_opencode_project_copilot_cli_supported() -> Result<(), ExecutorError> {
    let output = crate::command::output_with_timeout(
        {
            let mut command = tokio::process::Command::new("opencode");
            command.args(["run", "--help"]);
            command
        },
        Duration::from_secs(5),
    )
    .await
    .filter(|output| output.status.success())
    .ok_or_else(|| {
        ExecutorError::Unavailable(
            "Project Copilot could not verify OpenCode's managed agent and JSON CLI options"
                .to_owned(),
        )
    })?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if opencode_project_copilot_cli_supports_policy(&help) {
        Ok(())
    } else {
        Err(ExecutorError::Unavailable(
            "Project Copilot requires OpenCode CLI support for serve and run --server, --agent, --format, --model, and --session"
                .to_owned(),
        ))
    }
}

fn opencode_project_copilot_cli_supports_policy(help: &str) -> bool {
    ["--server", "--agent", "--format", "--model", "--session"]
        .iter()
        .all(|option| help.contains(option))
}

fn should_skip_permissions(config: &OpencodeConfig) -> bool {
    matches!(
        config.permission_policy.as_ref(),
        Some(PermissionPolicy::Auto)
    ) || config.auto_approve == Some(true)
}

fn normalize_opencode_model_id(model: &str) -> String {
    if model.contains('/') {
        return model.to_owned();
    }

    if model.starts_with("glm-") {
        return format!("zai-coding-plan/{model}");
    }

    if model.starts_with("mimo-") {
        return format!("xiaomi-token-plan-cn/{model}");
    }

    model.to_owned()
}

fn opencode_run_error(status: std::process::ExitStatus, stderr_tail: &str) -> String {
    let mut message = format!("opencode run exited with status {status}");
    let trimmed = stderr_tail.trim();
    if !trimmed.is_empty() {
        message.push_str("\nstderr:\n");
        message.push_str(trimmed);
    }
    message
}

fn classify_run_event(event: &serde_json::Value) -> LogKind {
    let event_type = event_type(event);
    let lower = event_type.to_ascii_lowercase();

    if lower == "error" || lower.contains("error") {
        LogKind::System
    } else if lower.contains("tool") && lower.contains("result") {
        LogKind::ToolResult
    } else if lower.contains("tool") || lower.contains("permission") {
        LogKind::ToolCall
    } else if lower.contains("session") {
        LogKind::SessionInfo
    } else if lower == "text"
        || lower.contains("delta")
        || lower.contains("part")
        || lower.contains("message")
    {
        LogKind::AssistantDelta
    } else {
        LogKind::Stdout
    }
}

fn capture_run_event(
    event: &serde_json::Value,
    agent_session_id: &mut Option<String>,
    assistant_output: &mut Option<String>,
    summary: &mut Option<String>,
    error: &mut Option<String>,
) {
    if agent_session_id.is_none() {
        *agent_session_id = extract_session_id(event);
    }

    if error.is_none() && is_error_event(event) {
        *error = Some(
            extract_error_text(event)
                .unwrap_or_else(|| "opencode emitted an error event".to_owned()),
        );
        return;
    }

    if let Some(text) = extract_assistant_text(event)
        && !text.trim().is_empty()
    {
        *summary = Some(truncate_summary(&text));
        append_assistant_text(assistant_output, &text);
    }
}

/// Extract only user-visible assistant text from OpenCode's run JSON events.
/// Tool results, reasoning parts, user messages, and generic diagnostics never
/// become the Project Copilot response.
fn extract_assistant_text(event: &serde_json::Value) -> Option<String> {
    if has_non_assistant_role(event) {
        return None;
    }
    let kind = event_type(event).to_ascii_lowercase();
    if kind == "error"
        || kind.contains("tool")
        || kind.contains("reason")
        || kind.contains("think")
        || kind.contains("user")
    {
        return None;
    }

    match kind.as_str() {
        "text" => {
            if event.get("part").is_some_and(|part| {
                part.get("type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|part_type| part_type != "text")
            }) {
                return None;
            }
            event
                .get("text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| event.get("part").and_then(extract_visible_text_part))
        }
        "message" | "assistant" => {
            let message = event.get("message").unwrap_or(event);
            if has_non_assistant_role(message) {
                return None;
            }
            extract_legacy_assistant_message(message)
        }
        _ => {
            if kind.contains("tool")
                || kind.contains("reason")
                || kind.contains("think")
                || kind.contains("user")
            {
                return None;
            }
            event
                .get("part")
                .and_then(extract_visible_text_part)
                .or_else(|| event.get("parts").and_then(extract_visible_text_part))
                .or_else(|| {
                    event
                        .get("data")
                        .and_then(|data| data.get("part"))
                        .and_then(extract_visible_text_part)
                })
        }
    }
}

fn has_non_assistant_role(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    ["role", "speaker"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(serde_json::Value::as_str))
        .any(|role| !role.eq_ignore_ascii_case("assistant"))
}

fn extract_legacy_assistant_message(value: &serde_json::Value) -> Option<String> {
    for key in ["text", "content"] {
        if let Some(text) = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty())
        {
            return Some(text.to_owned());
        }
    }
    value
        .get("parts")
        .and_then(extract_visible_text_part)
        .or_else(|| value.get("content").and_then(extract_visible_text_part))
}

fn extract_visible_text_part(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(object) => {
            if has_non_assistant_role(value) {
                return None;
            }
            if let Some(kind) = object.get("type").and_then(serde_json::Value::as_str) {
                if kind != "text" {
                    return None;
                }
                return object
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_owned);
            }
            if let Some(text) = object
                .get("text")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
            {
                return Some(text.to_owned());
            }
            ["part", "parts", "content"]
                .into_iter()
                .filter_map(|key| object.get(key))
                .filter_map(extract_visible_text_part)
                .reduce(|mut output, text| {
                    output.push_str(&text);
                    output
                })
        }
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(extract_visible_text_part)
            .reduce(|mut output, text| {
                output.push_str(&text);
                output
            }),
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn append_assistant_text(current: &mut Option<String>, text: &str) {
    let Some(existing) = current.as_mut() else {
        *current = Some(text.to_owned());
        return;
    };
    if text.starts_with(existing.as_str()) {
        *existing = text.to_owned();
    } else if !existing.ends_with(text) {
        existing.push_str(text);
    }
}

fn is_error_event(event: &serde_json::Value) -> bool {
    event_type(event).eq_ignore_ascii_case("error")
}

fn extract_error_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["error", "data", "cause"] {
                if let Some(text) = map.get(key).and_then(extract_error_text) {
                    return Some(text);
                }
            }
            for key in ["message", "name", "code"] {
                if let Some(text) = map.get(key).and_then(|v| v.as_str())
                    && !text.trim().is_empty()
                {
                    return Some(text.to_owned());
                }
            }
            None
        }
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

fn event_type(event: &serde_json::Value) -> &str {
    event
        .get("type")
        .or_else(|| event.get("event"))
        .or_else(|| event.get("kind"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn extract_session_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in ["sessionID", "sessionId", "session_id"] {
                if let Some(id) = map.get(key).and_then(|v| v.as_str())
                    && !id.trim().is_empty()
                {
                    return Some(id.to_owned());
                }
            }
            for key in ["properties", "data", "session"] {
                if let Some(id) = map.get(key).and_then(extract_session_id) {
                    return Some(id);
                }
            }
            None
        }
        serde_json::Value::Array(values) => values.iter().find_map(extract_session_id),
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

// ---------------------------------------------------------------------------
// Utility / availability detection (unchanged)
// ---------------------------------------------------------------------------

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn opencode_config_dir() -> Option<PathBuf> {
    env_path("XDG_CONFIG_HOME")
        .or_else(dirs::config_dir)
        .map(|dir| dir.join("opencode"))
}

fn opencode_data_dir() -> Option<PathBuf> {
    env_path("XDG_DATA_HOME")
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .map(|dir| dir.join("opencode"))
}

fn opencode_state_dir() -> Option<PathBuf> {
    env_path("XDG_STATE_HOME")
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
        .map(|dir| dir.join("opencode"))
}

fn opencode_config_file_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(path) = env_path("OPENCODE_CONFIG") {
        candidates.push(path);
    }

    if let Some(dir) = opencode_config_dir() {
        candidates.push(dir.join("opencode.json"));
        candidates.push(dir.join("opencode.jsonc"));
    }

    if let Some(dir) = opencode_data_dir() {
        candidates.push(dir.join("opencode.json"));
        candidates.push(dir.join("opencode.jsonc"));
    }

    #[cfg(target_os = "macos")]
    {
        let managed = PathBuf::from("/Library/Application Support/opencode");
        candidates.push(managed.join("opencode.json"));
        candidates.push(managed.join("opencode.jsonc"));
    }

    #[cfg(target_os = "linux")]
    {
        let managed = PathBuf::from("/etc/opencode");
        candidates.push(managed.join("opencode.json"));
        candidates.push(managed.join("opencode.jsonc"));
    }

    #[cfg(windows)]
    if let Some(program_data) = env_path("ProgramData") {
        let managed = program_data.join("opencode");
        candidates.push(managed.join("opencode.json"));
        candidates.push(managed.join("opencode.jsonc"));
    }

    candidates
}

fn first_existing_path(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.exists())
}

fn opencode_auth_path() -> Option<PathBuf> {
    opencode_data_dir().map(|dir| dir.join("auth.json"))
}

fn opencode_database_candidates() -> Vec<PathBuf> {
    if let Some(path) = env_path("OPENCODE_DB") {
        return vec![path];
    }

    opencode_data_dir()
        .map(|data_dir| vec![data_dir.join("opencode.db")])
        .unwrap_or_default()
}

async fn opencode_v2_database_has_active_provider_credential(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .busy_timeout(Duration::from_millis(250));
    let Ok(mut connection) = SqliteConnection::connect_with(&options).await else {
        return false;
    };

    // Query only credential metadata. Never load or log the stored value.
    let result = sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(
             SELECT 1 FROM credential
             WHERE active = 1 AND connector_id IS NULL
         )",
    )
    .fetch_one(&mut connection)
    .await;
    let _ = connection.close().await;
    result.is_ok_and(|has_credential| has_credential != 0)
}

pub async fn detect_availability() -> AvailabilityInfo {
    let mut availability = detect_opencode_availability();
    if !matches!(availability.status, AvailabilityStatus::Installed)
        || which::which("opencode").is_err()
    {
        return availability;
    }

    for path in opencode_database_candidates() {
        if opencode_v2_database_has_active_provider_credential(&path).await {
            availability.status = AvailabilityStatus::Authenticated;
            availability.authenticated_at = path_modified_epoch_secs(&path);
            availability.config_path = Some(path.to_string_lossy().into_owned());
            break;
        }
    }

    availability
}

fn path_modified_epoch_secs(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs().to_string())
}

fn executable_in_path(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&path_var).any(|dir| {
        #[cfg(windows)]
        {
            let pathext = std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    vec![
                        ".COM".to_string(),
                        ".EXE".to_string(),
                        ".BAT".to_string(),
                        ".CMD".to_string(),
                    ]
                });

            let direct = dir.join(name);
            direct.is_file()
                || pathext
                    .iter()
                    .any(|ext| dir.join(format!("{name}{ext}")).is_file())
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let candidate = dir.join(name);
            candidate
                .metadata()
                .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
    })
}

fn detect_opencode_availability() -> AvailabilityInfo {
    if let Some(auth_path) = opencode_auth_path()
        && auth_path.exists()
    {
        return AvailabilityInfo {
            status: AvailabilityStatus::Authenticated,
            authenticated_at: path_modified_epoch_secs(&auth_path),
            config_path: Some(auth_path.to_string_lossy().into_owned()),
        };
    }

    let config_file = first_existing_path(opencode_config_file_candidates());

    let config_dir = opencode_config_dir().filter(|p| p.exists());
    let data_dir = opencode_data_dir().filter(|p| p.exists());
    let state_dir = opencode_state_dir().filter(|p| p.exists());
    let custom_config_dir = env_path("OPENCODE_CONFIG_DIR").filter(|p| p.exists());
    let home_opencode = dirs::home_dir()
        .map(|home| home.join(".opencode"))
        .filter(|p| p.exists());

    let installation_indicator = config_file
        .clone()
        .or(config_dir)
        .or(data_dir)
        .or(state_dir)
        .or(custom_config_dir)
        .or(home_opencode);

    if let Some(path) = installation_indicator {
        return AvailabilityInfo {
            status: AvailabilityStatus::Installed,
            authenticated_at: None,
            config_path: Some(path.to_string_lossy().into_owned()),
        };
    }

    if executable_in_path("opencode") {
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

fn strip_ansi_codes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphabetic() {
                        chars.next();
                        break;
                    }
                    chars.next();
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_resume_uses_exact_session_and_start_clears_stale_session() {
        let stale = serde_json::json!({
            "resume_session_id":"old-session",
            "additional_params":["--session", "old-cli-session", "--continue", "--verbose"]
        });
        let start =
            crate::test_execution_context(executors::HarnessInvocation::Start, stale.clone());
        let start_config = OpencodeAdapter::resolve_config(&start);
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
        let resume_config = OpencodeAdapter::resolve_config(&resume);
        assert_eq!(
            resume_config.resume_session_id.as_deref(),
            Some("exact-session")
        );
        assert_eq!(
            resume_config.command_overrides.additional_params,
            Some(vec!["--verbose".to_owned()])
        );
    }
    use executors::CommandOverrides;

    #[test]
    fn command_builder_uses_run_cli_and_permission_env() {
        let config = OpencodeConfig {
            model: Some("anthropic/claude-sonnet-4-6".to_owned()),
            permission_policy: Some(PermissionPolicy::Auto),
            command_overrides: CommandOverrides::default(),
            ..OpencodeConfig::default()
        };

        let cmd = OpencodeAdapter::build_command(&config, "hello");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(cmd.as_std().get_program(), "opencode");
        assert!(args.contains(&"run".to_owned()));
        assert!(args.windows(2).any(|pair| pair == ["--format", "json"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--model", "anthropic/claude-sonnet-4-6"])
        );
        assert!(args.contains(&"--dangerously-skip-permissions".to_owned()));
        assert_eq!(args.last(), Some(&"hello".to_owned()));

        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(envs.get("OPENCODE_PERMISSION"), None);
    }

    #[test]
    fn project_copilot_command_uses_a_private_server() {
        let config = OpencodeConfig::default();
        let dir = tempfile::tempdir().expect("temp directory creates");
        let policy = r#"{"agents":{}}"#;
        OpencodeAdapter::write_project_copilot_policy(dir.path(), policy).expect("policy writes");
        let cmd = OpencodeAdapter::build_project_copilot_command(
            &config,
            "hello",
            dir.path(),
            "http://127.0.0.1:34567",
        )
        .expect("command builds");
        let args: Vec<_> = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &args[..3],
            &[
                "--server".to_owned(),
                "http://127.0.0.1:34567".to_owned(),
                "run".to_owned()
            ]
        );
        assert!(args.iter().any(|arg| arg == "run"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("opencode.json")).expect("policy file reads"),
            policy
        );
        let envs: HashMap<_, _> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(
            envs.get("OPENCODE_CONFIG_DIR"),
            Some(&dir.path().to_string_lossy().into_owned())
        );
    }

    #[test]
    fn detect_reports_not_found_when_no_indicators_present() {
        let info = detect_opencode_availability();
        match info.status {
            AvailabilityStatus::Authenticated
            | AvailabilityStatus::Installed
            | AvailabilityStatus::NotFound => {}
        }
    }

    #[tokio::test]
    async fn v2_active_provider_credential_is_detected_read_only() {
        let dir = tempfile::tempdir().expect("temp directory creates");
        let path = dir.path().join("opencode.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .expect("test database opens");
        sqlx::query(
            "CREATE TABLE credential (
                 id TEXT PRIMARY KEY,
                 value TEXT NOT NULL,
                 active INTEGER NOT NULL,
                 connector_id TEXT
             )",
        )
        .execute(&mut connection)
        .await
        .expect("credential table creates");
        sqlx::query(
            "INSERT INTO credential (id, value, active, connector_id)
             VALUES ('provider-key', 'test-secret-never-read', 1, NULL)",
        )
        .execute(&mut connection)
        .await
        .expect("provider credential inserts");
        connection.close().await.expect("test database closes");

        assert!(opencode_v2_database_has_active_provider_credential(&path).await);
    }

    #[tokio::test]
    async fn v2_inactive_or_connector_credentials_do_not_authenticate_provider() {
        let dir = tempfile::tempdir().expect("temp directory creates");
        let path = dir.path().join("opencode.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .expect("test database opens");
        sqlx::query(
            "CREATE TABLE credential (
                 id TEXT PRIMARY KEY,
                 value TEXT NOT NULL,
                 active INTEGER NOT NULL,
                 connector_id TEXT
             )",
        )
        .execute(&mut connection)
        .await
        .expect("credential table creates");
        sqlx::query(
            "INSERT INTO credential (id, value, active, connector_id) VALUES
                 ('inactive', 'test-secret-never-read', 0, NULL),
                 ('connector', 'test-secret-never-read', 1, 'mcp-connector')",
        )
        .execute(&mut connection)
        .await
        .expect("non-provider credentials insert");
        connection.close().await.expect("test database closes");

        assert!(!opencode_v2_database_has_active_provider_credential(&path).await);
    }

    #[test]
    fn extract_session_id_reads_opencode_run_event() {
        let event = serde_json::json!({
            "type": "step_start",
            "sessionID": "ses_123",
            "part": { "messageID": "msg_123" }
        });
        assert_eq!(extract_session_id(&event), Some("ses_123".to_owned()));
    }

    #[test]
    fn assistant_output_excludes_reasoning_tools_and_user_messages() {
        let events = [
            serde_json::json!({"type":"reasoning","text":"private chain of thought"}),
            serde_json::json!({"type":"tool_result","text":"internal tool output"}),
            serde_json::json!({"type":"message","role":"user","text":"echoed input"}),
            serde_json::json!({
                "type":"message",
                "message":{"role":"assistant","parts":[
                    {"type":"text","text":"Visible answer"},
                    {"type":"reasoning","text":"hidden reasoning"},
                    {"type":"tool","text":"tool result"}
                ]}
            }),
        ];
        let mut output = None;
        let mut summary = None;
        let mut session = None;
        let mut error = None;

        for event in &events {
            capture_run_event(event, &mut session, &mut output, &mut summary, &mut error);
        }

        assert_eq!(output.as_deref(), Some("Visible answer"));
        assert_eq!(summary.as_deref(), Some("Visible answer"));
        assert_eq!(error, None);
    }

    #[test]
    fn extract_error_text_reads_opencode_error_event() {
        let event = serde_json::json!({
            "type": "error",
            "sessionID": "ses_test",
            "error": {
                "name": "UnknownError",
                "data": { "message": "Model not found: mimo-v2.5-pro/." }
            }
        });
        assert_eq!(
            extract_error_text(&event),
            Some("Model not found: mimo-v2.5-pro/.".to_owned())
        );
    }

    #[test]
    fn normalizes_common_opencode_model_aliases() {
        assert_eq!(
            normalize_opencode_model_id("glm-5.1"),
            "zai-coding-plan/glm-5.1"
        );
        assert_eq!(
            normalize_opencode_model_id("mimo-v2.5"),
            "xiaomi-token-plan-cn/mimo-v2.5"
        );
        assert_eq!(
            normalize_opencode_model_id("openai/gpt-5.2"),
            "openai/gpt-5.2"
        );
    }

    #[tokio::test]
    async fn execute_drives_opencode_run_json_without_permission_env() {
        let dir = tempfile::tempdir().unwrap();
        let fake_opencode = dir.path().join("fake-opencode");
        std::fs::write(
            &fake_opencode,
            r#"#!/bin/sh
if [ "${OPENCODE_PERMISSION+x}" = "x" ]; then
  echo "unexpected OPENCODE_PERMISSION=$OPENCODE_PERMISSION" >&2
  exit 42
fi
printf '%s\n' '{"type":"step_start","sessionID":"ses_test","part":{"type":"step-start","sessionID":"ses_test"}}'
printf '%s\n' 'provider diagnostic should not be returned as assistant text'
printf '%s\n' '{"type":"reasoning","text":"private reasoning"}'
printf '%s\n' '{"type":"tool_result","text":"internal tool output"}'
printf '%s\n' '{"type":"text","sessionID":"ses_test","part":{"type":"text","text":"forge fake ok"}}'
printf '%s\n' '{"type":"step_finish","sessionID":"ses_test","part":{"type":"step-finish","sessionID":"ses_test"}}'
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&fake_opencode).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake_opencode, permissions).unwrap();
        }

        let logs_path = dir.path().join("opencode.jsonl");
        let adapter = OpencodeAdapter::new();
        let result = adapter
            .execute(ExecutionContext {
                invocation: executors::HarnessInvocation::Start,
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                role: "project_copilot".to_owned(),
                task_role_capabilities: vec![],
                worktree_path: dir.path().to_string_lossy().to_string(),
                description: "hello".to_owned(),
                agent_config: serde_json::json!({
                    "permission_policy": "auto",
                    "base_command_override": fake_opencode.to_string_lossy(),
                }),
                logs_path: logs_path.to_string_lossy().to_string(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .unwrap();

        assert_eq!(result.status, ExecutionOutcome::Completed);
        assert_eq!(result.agent_session_id, Some("ses_test".to_owned()));
        assert_eq!(result.summary, Some("forge fake ok".to_owned()));
        assert_eq!(result.assistant_output.as_deref(), Some("forge fake ok"));
        assert_eq!(result.after_sha, None);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn opencode_process_group_is_killed_when_dropped_before_registration() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let leader_pid_file = dir.path().join("opencode-leader.pid");
        let descendant_pid_file = dir.path().join("opencode-descendant.pid");
        let fake_opencode = dir.path().join("fake-opencode");
        std::fs::write(
            &fake_opencode,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 60 &\necho $! > '{}'\nexec sleep 60\n",
                leader_pid_file.display(),
                descendant_pid_file.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_opencode).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_opencode, permissions).unwrap();

        let config = serde_json::from_value(serde_json::json!({
            "base_command_override": fake_opencode.to_string_lossy(),
        }))
        .expect("OpenCode config parses");
        let mut command = OpencodeAdapter::build_command(&config, "test prompt");
        command.current_dir(dir.path());
        let child = ProcessGroupChild::spawn(&mut command).expect("fake OpenCode group spawns");

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if tokio::fs::try_exists(&leader_pid_file)
                    .await
                    .unwrap_or(false)
                    && tokio::fs::try_exists(&descendant_pid_file)
                        .await
                        .unwrap_or(false)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("leader and descendant reach the pre-registration barrier");
        let leader_pid = std::fs::read_to_string(&leader_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        let descendant_pid = std::fs::read_to_string(&descendant_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(pid_is_executable(leader_pid));
        assert!(pid_is_executable(descendant_pid));

        drop(child);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while pid_is_executable(leader_pid) || pid_is_executable(descendant_pid) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("drop kill signal terminates the unregistered OpenCode group");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancel_terminates_opencode_descendant_after_leader_exits() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let leader_pid_file = dir.path().join("opencode-leader.pid");
        let descendant_pid_file = dir.path().join("opencode-descendant.pid");
        let fake_opencode = dir.path().join("fake-opencode-tree");
        std::fs::write(
            &fake_opencode,
            format!(
                "#!/bin/sh\necho $$ > '{}'\nsleep 60 &\necho $! > '{}'\nprintf '%s\\n' '{{\"type\":\"text\",\"sessionID\":\"ses_tree\",\"part\":{{\"type\":\"text\",\"text\":\"tree output\"}}}}'\nexit 0\n",
                leader_pid_file.display(),
                descendant_pid_file.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_opencode).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_opencode, permissions).unwrap();

        let adapter = Arc::new(OpencodeAdapter::new());
        let context = ExecutionContext {
            invocation: executors::HarnessInvocation::Start,
            task_id: "task-tree".to_owned(),
            execution_id: "execution-tree".to_owned(),
            role: "coder".to_owned(),
            task_role_capabilities: vec![],
            worktree_path: dir.path().to_string_lossy().to_string(),
            description: "tree test".to_owned(),
            agent_config: serde_json::json!({
                "base_command_override": fake_opencode.to_string_lossy(),
            }),
            logs_path: dir
                .path()
                .join("opencode-tree.jsonl")
                .to_string_lossy()
                .to_string(),
            heartbeat_interval_seconds: 1,
            max_turns: None,
            log_sender: None,
        };
        let execution_adapter = Arc::clone(&adapter);
        let execution = tokio::spawn(async move { execution_adapter.execute(context).await });

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if tokio::fs::try_exists(&leader_pid_file)
                    .await
                    .unwrap_or(false)
                    && tokio::fs::try_exists(&descendant_pid_file)
                        .await
                        .unwrap_or(false)
                {
                    let leader_pid = std::fs::read_to_string(&leader_pid_file)
                        .unwrap()
                        .trim()
                        .parse::<u32>()
                        .unwrap();
                    if !pid_is_executable(leader_pid) {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("OpenCode leader exits while its descendant holds output open");
        let leader_pid = std::fs::read_to_string(&leader_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        let descendant_pid = std::fs::read_to_string(&descendant_pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        assert!(!pid_is_executable(leader_pid));
        assert!(pid_is_executable(descendant_pid));

        adapter
            .cancel("execution-tree")
            .await
            .expect("adapter cancellation verifies the full process group");
        tokio::time::timeout(std::time::Duration::from_secs(2), execution)
            .await
            .expect("adapter execution finishes after group termination")
            .expect("adapter task joins")
            .expect("adapter returns its execution result");
        assert!(!pid_is_executable(descendant_pid));
    }

    #[cfg(target_os = "linux")]
    fn pid_is_executable(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(end_of_command) = stat.rfind(')') else {
            return false;
        };
        !matches!(stat[end_of_command + 2..].chars().next(), Some('Z' | 'X'))
    }

    #[tokio::test]
    async fn execute_fails_when_opencode_emits_error_event() {
        let dir = tempfile::tempdir().unwrap();
        let fake_opencode = dir.path().join("fake-opencode-error");
        std::fs::write(
            &fake_opencode,
            r#"#!/bin/sh
printf '%s\n' '{"type":"error","sessionID":"ses_test","error":{"data":{"message":"Model not found: bad-model"}}}'
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&fake_opencode).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake_opencode, permissions).unwrap();
        }

        let logs_path = dir.path().join("opencode-error.jsonl");
        let adapter = OpencodeAdapter::new();
        let result = adapter
            .execute(ExecutionContext {
                invocation: executors::HarnessInvocation::Start,
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                role: "coder".to_owned(),
                task_role_capabilities: vec![],
                worktree_path: dir.path().to_string_lossy().to_string(),
                description: "hello".to_owned(),
                agent_config: serde_json::json!({
                    "base_command_override": fake_opencode.to_string_lossy(),
                }),
                logs_path: logs_path.to_string_lossy().to_string(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .unwrap();

        assert_eq!(result.status, ExecutionOutcome::Failed);
        assert_eq!(result.agent_session_id, Some("ses_test".to_owned()));
        assert_eq!(result.error, Some("Model not found: bad-model".to_owned()));
    }

    #[tokio::test]
    async fn execute_fails_when_opencode_exits_without_text() {
        let dir = tempfile::tempdir().unwrap();
        let fake_opencode = dir.path().join("fake-opencode-empty");
        std::fs::write(
            &fake_opencode,
            r#"#!/bin/sh
printf '%s\n' '{"type":"step_start","sessionID":"ses_test","part":{"type":"step-start","sessionID":"ses_test"}}'
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&fake_opencode).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&fake_opencode, permissions).unwrap();
        }

        let logs_path = dir.path().join("opencode-empty.jsonl");
        let adapter = OpencodeAdapter::new();
        let result = adapter
            .execute(ExecutionContext {
                invocation: executors::HarnessInvocation::Start,
                task_id: "task".to_owned(),
                execution_id: "execution".to_owned(),
                role: "coder".to_owned(),
                task_role_capabilities: vec![],
                worktree_path: dir.path().to_string_lossy().to_string(),
                description: "hello".to_owned(),
                agent_config: serde_json::json!({
                    "base_command_override": fake_opencode.to_string_lossy(),
                }),
                logs_path: logs_path.to_string_lossy().to_string(),
                heartbeat_interval_seconds: 1,
                max_turns: None,
                log_sender: None,
            })
            .await
            .unwrap();

        assert_eq!(result.status, ExecutionOutcome::Failed);
        assert_eq!(result.agent_session_id, Some("ses_test".to_owned()));
        assert_eq!(
            result.error,
            Some("opencode run completed without assistant text".to_owned())
        );
    }
}
